//! The Limit monitor: each signed-in Profile's latest Limit reading, when to
//! take the next one, and how honest the shown figures are about their age.
//!
//! Reads go through the [`Runner`] one at a time and never block the core:
//! a read is started on one call and collected on a later [`LimitMonitor::tick`]
//! once Claude Code has exited.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::env::RunningProcess;
use crate::interface::LimitBar;
use crate::limit_offers;
use crate::logging::{Field, Level};
use crate::profiles::Context;
use crate::runner::UsageRead;
use crate::storage::{self, ReadError, VersionedFile};
use crate::usage::{Limits, UsageReply};

/// How often an Active Profile is read.
const ACTIVE_CADENCE: SignedDuration = SignedDuration::from_mins(3);
/// How often any other Profile is read.
const OTHER_CADENCE: SignedDuration = SignedDuration::from_mins(15);
/// The least time between two reads of one Profile.
const MIN_GAP: SignedDuration = SignedDuration::from_secs(60);
/// A read still running after this long is killed and counts as unreadable.
const READ_TIMEOUT: SignedDuration = SignedDuration::from_mins(2);
/// The wait after a Profile's first failed fetch; it doubles with each
/// further failure up to [`BACKOFF_CAP`].
const BACKOFF_START: SignedDuration = SignedDuration::from_mins(5);
const BACKOFF_CAP: SignedDuration = SignedDuration::from_hours(1);
/// How often the core wants a tick while a read runs.
const RUNNING_TICK: Duration = Duration::from_millis(250);

/// A signed-in Profile the monitor reads, as the core sees it right now.
#[derive(Debug, Clone)]
pub(crate) struct Watched {
    pub id: String,
    /// The stored absolute path, used verbatim.
    pub config_dir: PathBuf,
    /// `oauthAccount.accountUuid`, to trust `cachedUsageUtilization` only
    /// when it is this Login's.
    pub account_uuid: String,
    /// The CLI's or the desktop app's Active Profile.
    pub active: bool,
}

impl Watched {
    fn cadence(&self) -> SignedDuration {
        cadence(self.active)
    }
}

fn cadence(active: bool) -> SignedDuration {
    if active {
        ACTIVE_CADENCE
    } else {
        OTHER_CADENCE
    }
}

/// What a finished read gave, handed back to the core.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Completed {
    pub id: String,
    /// `None` when Claude Code gave no readable answer.
    pub reply: Option<UsageReply>,
}

/// What a Profile's row shows about its limits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LimitsView {
    pub bars: Vec<LimitBar>,
    pub dimmed: bool,
    pub note: Option<String>,
    /// Banked reset and Session refill lines.
    pub offers: Vec<String>,
    /// Lines for the hover submenu (as-of time, backoff).
    pub details: Vec<String>,
    /// `subscription_type` from the latest answer (`max`, `pro`, …).
    pub plan: Option<String>,
}

/// The latest usage reads, as the format check sees them.
#[derive(Debug, Default)]
pub(crate) struct UsageSummary {
    /// Profiles whose last read was understood.
    pub read: usize,
    /// Profiles whose last read gave no readable answer.
    pub unreadable: usize,
    /// Limit kinds this app doesn't know, as Claude Code named them.
    pub new_kinds: Vec<String>,
}

/// One Profile's readings and schedule.
#[derive(Debug, Default)]
struct Watch {
    /// The latest answer that carried limits.
    reading: Option<UsageReply>,
    /// When that reading was taken.
    read_at: Option<Timestamp>,
    /// The latest read gave no readable answer.
    unreadable: bool,
    /// When the latest read was started.
    last_spawn: Option<Timestamp>,
    /// Read as soon as allowed (menu opened, woke up).
    refresh_requested: bool,
    /// Set after failed fetches; persisted in `state/backoff.json`.
    backoff: Option<Backoff>,
}

impl Watch {
    /// Whether one of the reading's limits reset after `last` (the latest
    /// read's start) and by `now`: it is read again at once, once.
    fn reset_since(&self, last: Timestamp, now: Timestamp) -> bool {
        self.reading
            .as_ref()
            .and_then(|reading| reading.limits.as_ref())
            .is_some_and(|limits| {
                limits
                    .all()
                    .filter_map(|(_, limit)| limit.resets_at)
                    .any(|at| last < at && at <= now)
            })
    }
}

/// Failed fetches in a row, and when the next read may be tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Backoff {
    failures: u32,
    next_try: Timestamp,
}

impl Backoff {
    /// The backoff after one more failure at `now`.
    fn after(previous: Option<Backoff>, now: Timestamp) -> Backoff {
        let failures = previous.map_or(0, |backoff| backoff.failures) + 1;
        let mut wait = BACKOFF_START;
        for _ in 1..failures {
            wait = (wait * 2).min(BACKOFF_CAP);
        }
        Backoff {
            failures,
            next_try: now + wait,
        }
    }
}

/// `state/backoff.json`: each Profile's backoff, by Profile id.
#[derive(Debug, Default, Serialize, Deserialize)]
struct BackoffFile {
    profiles: BTreeMap<String, Backoff>,
}

impl VersionedFile for BackoffFile {
    const VERSION: u64 = 1;
}

/// The read in flight.
struct Running {
    watched: Watched,
    step: UsageRead,
    process: Box<dyn RunningProcess>,
    spawned_at: Timestamp,
    /// The network went away while it ran: a failure then isn't the
    /// Profile's.
    went_offline: bool,
}

#[derive(Default)]
pub(crate) struct LimitMonitor {
    watches: HashMap<String, Watch>,
    running: Option<Running>,
    asleep: bool,
    locked: bool,
    /// The Mac has no network path.
    offline: bool,
    /// The Profiles watched at the last tick.
    last_watched: HashSet<String>,
}

impl LimitMonitor {
    /// Starts with the backoffs saved by the previous run.
    pub(crate) fn load(ctx: &Context) -> LimitMonitor {
        let file = match storage::read_versioned::<BackoffFile>(&backoff_path(ctx)) {
            Ok(file) => file.unwrap_or_default(),
            Err(ReadError::Io(error)) => {
                let error = error.to_string();
                ctx.log(
                    Level::Error,
                    "backoff record unreadable",
                    &[("error", Field::Text(&error))],
                );
                BackoffFile::default()
            }
            Err(ReadError::Unparsable(reason)) => {
                ctx.log(
                    Level::Warn,
                    "backoff record unparsable",
                    &[("error", Field::Text(&reason))],
                );
                BackoffFile::default()
            }
        };
        let mut monitor = LimitMonitor::default();
        for (id, backoff) in file.profiles {
            monitor.watches.entry(id).or_default().backoff = Some(backoff);
        }
        monitor
    }

    /// Collects the read in flight once it has finished (or timed out).
    pub(crate) fn collect_finished(
        &mut self,
        ctx: &Context,
        watched: &[Watched],
    ) -> Option<Completed> {
        // A Profile watched again (signed in again after its Login expired)
        // is read as soon as allowed.
        let now_watched: HashSet<String> = watched.iter().map(|w| w.id.clone()).collect();
        for id in now_watched.difference(&self.last_watched) {
            if let Some(watch) = self.watches.get_mut(id) {
                watch.refresh_requested = true;
            }
        }
        self.last_watched = now_watched;
        self.collect(ctx)
    }

    /// Starts the next read that is due, unless one is in flight or polling
    /// is paused. `Some` when it couldn't even start (unreadable).
    pub(crate) fn start_due(&mut self, ctx: &Context, watched: &[Watched]) -> Option<Completed> {
        if self.running.is_some() || self.paused() {
            return None;
        }
        let next = self.next_due(ctx.env.clock.now(), watched)?.clone();
        self.start(ctx, next, UsageRead::GetUsage)
    }

    /// The menu opened: Profiles whose reading is over a minute old are
    /// read again as soon as allowed.
    pub(crate) fn menu_opened(&mut self, now: Timestamp, watched: &[Watched]) {
        for w in watched {
            let watch = self.watches.entry(w.id.clone()).or_default();
            let backing_off = watch.backoff.is_some_and(|b| b.next_try > now);
            if !backing_off
                && watch
                    .read_at
                    .is_none_or(|at| now.duration_since(at) > MIN_GAP)
            {
                watch.refresh_requested = true;
            }
        }
    }

    /// Forgets a removed Profile, and its backoff.
    pub(crate) fn forget(&mut self, ctx: &Context, id: &str) {
        let had_backoff = self
            .watches
            .remove(id)
            .is_some_and(|watch| watch.backoff.is_some());
        if had_backoff {
            self.save_backoffs(ctx);
        }
    }

    /// The Mac went to sleep (`true`) or woke (`false`).
    pub(crate) fn set_asleep(&mut self, asleep: bool, watched: &[Watched]) {
        self.asleep = asleep;
        if !asleep {
            self.request_active(watched);
        }
    }

    /// The screen locked (`true`) or unlocked (`false`).
    pub(crate) fn set_locked(&mut self, locked: bool, watched: &[Watched]) {
        self.locked = locked;
        if !locked {
            self.request_active(watched);
        }
    }

    /// Back from sleep or lock, Active Profiles are read first, at once.
    fn request_active(&mut self, watched: &[Watched]) {
        for w in watched.iter().filter(|w| w.active) {
            self.watches
                .entry(w.id.clone())
                .or_default()
                .refresh_requested = true;
        }
    }

    /// The Mac gained or lost its network path.
    pub(crate) fn set_online(&mut self, online: bool) {
        self.offline = !online;
        if let Some(running) = self.running.as_mut().filter(|_| !online) {
            running.went_offline = true;
        }
    }

    /// The line topping the menu while offline.
    /// What the latest usage reads say about the usage format, for
    /// Diagnostics.
    pub(crate) fn usage_summary(&self) -> UsageSummary {
        let mut summary = UsageSummary::default();
        for watch in self.watches.values() {
            if watch.unreadable {
                summary.unreadable += 1;
            } else if watch.reading.is_some() {
                summary.read += 1;
            }
            let kinds = watch
                .reading
                .iter()
                .flat_map(|reading| reading.limits.iter())
                .flat_map(|limits| limits.other.iter().map(|(kind, _)| kind));
            for kind in kinds {
                if !summary.new_kinds.contains(kind) {
                    summary.new_kinds.push(kind.clone());
                }
            }
        }
        summary
    }

    pub(crate) fn offline_line(&self, ctx: &Context, watched: &[Watched]) -> Option<String> {
        if !self.offline {
            return None;
        }
        let oldest = watched
            .iter()
            .filter_map(|w| self.watches.get(&w.id)?.read_at)
            .min();
        Some(match oldest {
            Some(at) => format!("Offline — showing figures from {}", clock_time(ctx, at)),
            None => "Offline".into(),
        })
    }

    /// No reads start while the Mac sleeps, the screen is locked or there is
    /// no network.
    fn paused(&self) -> bool {
        self.asleep || self.locked || self.offline
    }

    /// How soon the core wants the next tick, at most `idle`.
    pub(crate) fn tick_interval(
        &self,
        now: Timestamp,
        watched: &[Watched],
        idle: Duration,
    ) -> Duration {
        if self.running.is_some() {
            return RUNNING_TICK;
        }
        if self.paused() {
            return idle;
        }
        let Some(due) = watched.iter().filter_map(|w| self.due_at(w, now)).min() else {
            return idle;
        };
        let wait = Duration::try_from(due.duration_since(now)).unwrap_or(Duration::ZERO);
        wait.clamp(Duration::from_secs(1), idle)
    }

    /// The latest reading's limits for a Profile, if it has one.
    pub(crate) fn limits(&self, id: &str) -> Option<&Limits> {
        self.watches.get(id)?.reading.as_ref()?.limits.as_ref()
    }

    /// The limits part of a Profile's row.
    pub(crate) fn view(&self, ctx: &Context, id: &str, active: bool) -> LimitsView {
        let Some(watch) = self.watches.get(id) else {
            return LimitsView::default();
        };
        let now = ctx.env.clock.now();
        let mut view = LimitsView::default();
        if let Some(reading) = &watch.reading {
            view.plan = reading.subscription_type.clone();
            view.offers = limit_offers::lines(
                reading.banked_reset.as_ref(),
                reading.session_refill.as_ref(),
                now,
                &ctx.env.clock.time_zone(),
            );
            if let Some(limits) = &reading.limits {
                view.bars.extend(limits.all().map(|(label, limit)| {
                    let reset = limit.resets_at.is_some_and(|at| at <= now);
                    LimitBar {
                        label: label.to_owned(),
                        percent: if reset {
                            0
                        } else {
                            limit.percent.clamp(0.0, 100.0).floor() as u32
                        },
                        reset,
                    }
                }));
            }
        }
        let time = |at: Timestamp| clock_time(ctx, at);
        let stale = watch
            .read_at
            .is_some_and(|at| now.duration_since(at) > cadence(active) * 2);
        if let Some(at) = watch.read_at {
            view.details.push(format!("Limits as of {}", time(at)));
        }
        let backoff = watch
            .backoff
            .map(|backoff| format!("Couldn't refresh · next try {}", time(backoff.next_try)));
        if let Some(line) = &backoff {
            view.details.push(line.clone());
        }
        if watch.unreadable {
            view.note = Some("Couldn't read limits".into());
            view.dimmed = watch.reading.is_some();
        } else if let Some(line) = backoff {
            view.note = Some(line);
            view.dimmed = stale;
        } else if stale && let Some(at) = watch.read_at {
            view.note = Some(format!("as of {}", time(at)));
            view.dimmed = true;
        }
        view
    }

    /// The Profile due soonest whose read can start now.
    fn next_due<'w>(&self, now: Timestamp, watched: &'w [Watched]) -> Option<&'w Watched> {
        watched
            .iter()
            .filter_map(|w| Some((self.due_at(w, now)?, w)))
            .filter(|(due, _)| *due <= now)
            .min_by_key(|(due, w)| (!w.active, *due))
            .map(|(_, w)| w)
    }

    /// When the Profile's next read is due.
    fn due_at(&self, watched: &Watched, now: Timestamp) -> Option<Timestamp> {
        let Some(watch) = self.watches.get(&watched.id) else {
            return Some(now);
        };
        // In backoff, the next try replaces the cadence.
        let mut due = match (watch.backoff, watch.last_spawn) {
            (Some(backoff), _) => backoff.next_try,
            (None, None) => now,
            (None, Some(_)) if watch.refresh_requested => now,
            (None, Some(last)) if watch.reset_since(last, now) => now,
            (None, Some(last)) => last + watched.cadence(),
        };
        if let Some(last) = watch.last_spawn {
            due = due.max(last + MIN_GAP);
        }
        Some(due)
    }

    fn start(&mut self, ctx: &Context, watched: Watched, step: UsageRead) -> Option<Completed> {
        let now = ctx.env.clock.now();
        if step == UsageRead::GetUsage {
            let watch = self.watches.entry(watched.id.clone()).or_default();
            watch.last_spawn = Some(now);
            watch.refresh_requested = false;
        }
        match ctx.runner().start_usage_read(&watched.config_dir, step) {
            Ok(process) => {
                self.running = Some(Running {
                    watched,
                    step,
                    process,
                    spawned_at: now,
                    went_offline: false,
                });
                None
            }
            Err(error) => {
                let error = error.to_string();
                ctx.log(
                    Level::Warn,
                    "usage read not started",
                    &[
                        ("profile", Field::Profile(&watched.id)),
                        ("error", Field::Text(&error)),
                    ],
                );
                Some(self.complete(ctx, &watched, now, None, true))
            }
        }
    }

    /// Checks on the read in flight; finishes it, moves to `/usage`, or
    /// kills it after [`READ_TIMEOUT`].
    fn collect(&mut self, ctx: &Context) -> Option<Completed> {
        let running = self.running.as_mut()?;
        let (output, timed_out) = match running.process.try_wait() {
            Ok(Some(output)) => (Some(output), false),
            Ok(None) => {
                if ctx.env.clock.now().duration_since(running.spawned_at) < READ_TIMEOUT {
                    return None;
                }
                ctx.log(
                    Level::Warn,
                    "usage read timed out",
                    &[("profile", Field::Profile(&running.watched.id))],
                );
                let _ = running.process.kill();
                (None, true)
            }
            Err(_) => (None, false),
        };
        let Running {
            watched,
            step,
            spawned_at,
            went_offline,
            ..
        } = self.running.take().expect("checked above");
        if let Some(reply) = output.and_then(|output| step.parse(&output.stdout)) {
            let online = !went_offline && !self.offline;
            return Some(self.complete(ctx, &watched, spawned_at, Some(reply), online));
        }
        if step == UsageRead::GetUsage && !timed_out {
            return self.start(ctx, watched, UsageRead::SlashUsage);
        }
        ctx.log(
            Level::Warn,
            "limits unreadable",
            &[("profile", Field::Profile(&watched.id))],
        );
        Some(self.complete(ctx, &watched, spawned_at, None, true))
    }

    /// Records a finished read. `online`: the network was up throughout, so
    /// a failed fetch is the Profile's and counts toward its backoff.
    fn complete(
        &mut self,
        ctx: &Context,
        watched: &Watched,
        spawned_at: Timestamp,
        reply: Option<UsageReply>,
        online: bool,
    ) -> Completed {
        let now = ctx.env.clock.now();
        let watch = self.watches.entry(watched.id.clone()).or_default();
        let backoff_before = watch.backoff;
        match &reply {
            Some(answer) if answer.limits.is_some() => {
                watch.read_at = Some(
                    cached_fetch_time(&watched.config_dir, &watched.account_uuid)
                        .unwrap_or(spawned_at),
                );
                watch.reading = Some(answer.clone());
                watch.unreadable = false;
                watch.backoff = None;
            }
            Some(answer)
                if answer.rate_limits_available == Some(true) && answer.rate_limits_null =>
            {
                watch.unreadable = false;
                if online {
                    let backoff = Backoff::after(watch.backoff, now);
                    ctx.log(
                        Level::Warn,
                        "limits not fetched, backing off",
                        &[
                            ("profile", Field::Profile(&watched.id)),
                            ("failures", Field::Number(i64::from(backoff.failures))),
                        ],
                    );
                    watch.backoff = Some(backoff);
                } else {
                    // Not the Profile's fault: try again once back online.
                    watch.refresh_requested = true;
                }
            }
            _ => watch.unreadable = true,
        }
        if watch.backoff != backoff_before {
            self.save_backoffs(ctx);
        }
        Completed {
            id: watched.id.clone(),
            reply,
        }
    }

    fn save_backoffs(&self, ctx: &Context) {
        let file = BackoffFile {
            profiles: self
                .watches
                .iter()
                .filter_map(|(id, watch)| Some((id.clone(), watch.backoff?)))
                .collect(),
        };
        if let Err(error) = storage::write_versioned(&backoff_path(ctx), &file) {
            let error = error.to_string();
            ctx.log(
                Level::Error,
                "backoff record not saved",
                &[("error", Field::Text(&error))],
            );
        }
    }
}

/// `cachedUsageUtilization.fetchedAtMs` in the Profile's `.claude.json`, when
/// Claude Code wrote it for this Login: the time its answer was really
/// fetched (it answers from that cache within 60 s, and from up to an hour
/// old one after an error).
fn cached_fetch_time(config_dir: &Path, account_uuid: &str) -> Option<Timestamp> {
    let bytes = fs::read(config_dir.join(".claude.json")).ok()?;
    let body: Value = serde_json::from_slice(&bytes).ok()?;
    let cache = body.get("cachedUsageUtilization")?;
    if cache.get("accountUuid")?.as_str()? != account_uuid {
        return None;
    }
    Timestamp::from_millisecond(cache.get("fetchedAtMs")?.as_i64()?).ok()
}

/// `14:02` in the Mac's time zone.
fn clock_time(ctx: &Context, at: Timestamp) -> String {
    at.to_zoned(ctx.env.clock.time_zone())
        .strftime("%H:%M")
        .to_string()
}

fn backoff_path(ctx: &Context) -> PathBuf {
    ctx.paths.state().join("backoff.json")
}
