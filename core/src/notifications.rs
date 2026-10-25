//! Limit notifications: each threshold fires once per limit per window
//! (keyed by the limit's `resets_at`), the CLI's Active Profile's warning
//! suggests the Profile with the most headroom, and an optional notification
//! says when a limit that warned resets.
//!
//! What already fired lives in `state/notifications.json`, so a relaunch
//! never repeats a warning within its window.

use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::interface::{Effect, NotificationAction};
use crate::limit_offers;
use crate::logging::{Field, Level};
use crate::profiles::Context;
use crate::storage::{self, ReadError, VersionedFile};
use crate::usage::{Limit, Limits};

/// A signed-in, watched Profile, as the notifier sees it.
pub(crate) struct Subject<'a> {
    pub id: &'a str,
    /// The Short label notifications use: the nickname, else the person's
    /// first name.
    pub name: &'a str,
    /// The latest reading's limits, if any.
    pub limits: Option<&'a Limits>,
    /// The row's tightest limit (whole percent), for the headroom
    /// comparison.
    pub tightest: Option<u32>,
    /// Notifications for it are muted.
    pub muted: bool,
    /// It is the CLI's Active Profile.
    pub cli_active: bool,
}

/// The non-stale Profile with the lowest tightest limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Headroom {
    pub id: String,
    pub name: String,
    pub tightest: u32,
}

/// What the user chose in Settings.
pub(crate) struct Choices<'a> {
    pub thresholds: &'a [u32],
    pub reset_notification: bool,
}

/// One threshold that fired for one limit in one window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fired {
    profile: String,
    /// The limit's label ("Session", "Weekly", a model or a raw kind).
    limit: String,
    /// The window, by its reset time. `None` when Claude Code didn't say:
    /// the threshold then re-arms once the limit drops below it.
    resets_at: Option<Timestamp>,
    threshold: u32,
}

/// `state/notifications.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct FiredFile {
    fired: Vec<Fired>,
}

impl VersionedFile for FiredFile {
    const VERSION: u64 = 1;
}

#[derive(Debug, Default)]
pub(crate) struct Notifier {
    fired: Vec<Fired>,
}

impl Notifier {
    /// Starts with what fired before the relaunch.
    pub(crate) fn load(ctx: &Context) -> Notifier {
        let fired = match storage::read_versioned::<FiredFile>(&fired_path(ctx)) {
            Ok(file) => file.unwrap_or_default().fired,
            Err(ReadError::Io(error)) => {
                let error = error.to_string();
                ctx.log(
                    Level::Error,
                    "notification record unreadable",
                    &[("error", Field::Text(&error))],
                );
                Vec::new()
            }
            Err(ReadError::Unparsable(reason)) => {
                ctx.log(
                    Level::Warn,
                    "notification record unparsable",
                    &[("error", Field::Text(&reason))],
                );
                Vec::new()
            }
        };
        Notifier { fired }
    }

    /// The notifications due now: resets of limits that warned in their
    /// window, then each limit that crossed a threshold not yet fired.
    pub(crate) fn check(
        &mut self,
        ctx: &Context,
        subjects: &[Subject],
        headroom: Option<&Headroom>,
        choices: &Choices,
    ) -> Vec<Effect> {
        let now = ctx.env.clock.now();
        let before = self.fired.clone();
        let mut effects = self.resets(subjects, choices, now);
        for subject in subjects {
            let Some(limits) = subject.limits else {
                continue;
            };
            // A threshold without a known window re-arms below it.
            self.fired.retain(|fired| {
                fired.profile != subject.id
                    || fired.resets_at.is_some()
                    || limits.all().any(|(label, limit)| {
                        label == fired.limit && percent(limit) >= fired.threshold
                    })
            });
            for (label, limit) in limits.all() {
                // A passed reset leaves an old figure: it no longer counts.
                if limit.resets_at.is_some_and(|at| at <= now) {
                    continue;
                }
                let percent = percent(limit);
                let new: Vec<u32> = choices
                    .thresholds
                    .iter()
                    .copied()
                    .filter(|threshold| percent >= *threshold)
                    .filter(|threshold| !self.has_fired(subject.id, label, limit, *threshold))
                    .collect();
                if new.is_empty() {
                    continue;
                }
                self.fired.extend(new.iter().map(|threshold| Fired {
                    profile: subject.id.to_owned(),
                    limit: label.to_owned(),
                    resets_at: limit.resets_at,
                    threshold: *threshold,
                }));
                if subject.muted {
                    continue;
                }
                ctx.log(
                    Level::Info,
                    "limit threshold crossed",
                    &[
                        ("profile", Field::Profile(subject.id)),
                        ("percent", Field::Number(i64::from(percent))),
                    ],
                );
                effects.push(warning(ctx, subject, limits, label, limit, headroom, now));
            }
        }
        if self.fired != before {
            self.save(ctx);
        }
        effects
    }

    /// Drops the windows that ended, saying so for each limit that warned
    /// when the reset notification is on.
    fn resets(&mut self, subjects: &[Subject], choices: &Choices, now: Timestamp) -> Vec<Effect> {
        let (ended, kept): (Vec<Fired>, Vec<Fired>) = self
            .fired
            .drain(..)
            .partition(|fired| fired.resets_at.is_some_and(|at| at <= now));
        self.fired = kept;
        if !choices.reset_notification {
            return Vec::new();
        }
        let mut said: Vec<&Fired> = Vec::new();
        let mut effects = Vec::new();
        for fired in &ended {
            if said.iter().any(|earlier| {
                earlier.profile == fired.profile
                    && earlier.limit == fired.limit
                    && same_window(earlier.resets_at, fired.resets_at)
            }) {
                continue;
            }
            said.push(fired);
            let Some(subject) = subjects
                .iter()
                .find(|subject| subject.id == fired.profile && !subject.muted)
            else {
                continue;
            };
            let name = subject.limits.map_or_else(
                || format!("{} limit", fired.limit),
                |limits| limit_name(limits, &fired.limit),
            );
            effects.push(Effect::Notify {
                title: format!("{}: {name} reset", subject.name),
                body: String::new(),
            });
        }
        effects
    }

    fn has_fired(&self, id: &str, label: &str, limit: &Limit, threshold: u32) -> bool {
        self.fired.iter().any(|fired| {
            fired.profile == id
                && fired.limit == label
                && same_window(fired.resets_at, limit.resets_at)
                && fired.threshold == threshold
        })
    }

    /// Forgets a removed Profile.
    pub(crate) fn forget(&mut self, ctx: &Context, id: &str) {
        let count = self.fired.len();
        self.fired.retain(|fired| fired.profile != id);
        if self.fired.len() != count {
            self.save(ctx);
        }
    }

    fn save(&self, ctx: &Context) {
        let file = FiredFile {
            fired: self.fired.clone(),
        };
        if let Err(error) = storage::write_versioned(&fired_path(ctx), &file) {
            let error = error.to_string();
            ctx.log(
                Level::Error,
                "notification record not saved",
                &[("error", Field::Text(&error))],
            );
        }
    }
}

/// The non-stale Profile with the lowest tightest limit, first in the
/// user's order on a tie, among `(id, name, tightest)` candidates. `None`
/// with fewer than two: one Profile has nowhere to switch to.
pub(crate) fn most_headroom<'a>(
    candidates: impl IntoIterator<Item = (&'a str, &'a str, u32)>,
) -> Option<Headroom> {
    let candidates: Vec<_> = candidates.into_iter().collect();
    if candidates.len() < 2 {
        return None;
    }
    let mut best = candidates[0];
    for candidate in &candidates[1..] {
        if candidate.2 < best.2 {
            best = *candidate;
        }
    }
    Some(Headroom {
        id: best.0.to_owned(),
        name: best.1.to_owned(),
        tightest: best.2,
    })
}

/// "Perso: Session limit 95% · resets 16:40", plus the most-headroom
/// suggestion and Switch CLI for the CLI's Active Profile.
/// Whether two readings' reset times name the same window. Claude Code
/// reports one reset with sub-second differences from read to read, while
/// real windows are hours apart, so a few minutes' slack keeps them one.
fn same_window(a: Option<Timestamp>, b: Option<Timestamp>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.duration_since(b).abs() <= jiff::SignedDuration::from_mins(5),
        (a, b) => a == b,
    }
}

fn warning(
    ctx: &Context,
    subject: &Subject,
    limits: &Limits,
    label: &str,
    limit: &Limit,
    headroom: Option<&Headroom>,
    now: Timestamp,
) -> Effect {
    let mut title = format!(
        "{}: {} {}%",
        subject.name,
        limit_name(limits, label),
        percent(limit)
    );
    if let Some(at) = limit.resets_at {
        title.push_str(" · resets ");
        title.push_str(&reset_time(ctx, at, now));
    }
    let better = headroom.filter(|headroom| {
        subject.cli_active
            && headroom.id != subject.id
            && subject
                .tightest
                .is_some_and(|tightest| headroom.tightest < tightest)
    });
    match better {
        Some(headroom) => Effect::NotifyWithAction {
            title,
            body: format!(
                "{} has the most headroom ({}%)",
                headroom.name, headroom.tightest
            ),
            action: "Switch CLI".to_owned(),
            on_click: NotificationAction::SwitchCli {
                profile: headroom.id.clone(),
            },
        },
        None => Effect::Notify {
            title,
            body: String::new(),
        },
    }
}

/// "Session limit", "Weekly limit", "Opus weekly limit", or "<kind> limit"
/// for a kind this app doesn't know.
fn limit_name(limits: &Limits, label: &str) -> String {
    if limits.models.iter().any(|(name, _)| name == label) {
        format!("{label} weekly limit")
    } else {
        format!("{label} limit")
    }
}

/// `16:40` today, else the day ("Thu", "Nov 3"), in the Mac's time zone.
fn reset_time(ctx: &Context, at: Timestamp, now: Timestamp) -> String {
    let tz = ctx.env.clock.time_zone();
    let day = limit_offers::day(at, now, &tz);
    if day == "today" {
        at.to_zoned(tz).strftime("%H:%M").to_string()
    } else {
        day
    }
}

/// Whole percent, rounded down like the bars.
fn percent(limit: &Limit) -> u32 {
    limit.percent.clamp(0.0, 100.0).floor() as u32
}

fn fired_path(ctx: &Context) -> PathBuf {
    ctx.paths.state().join("notifications.json")
}
