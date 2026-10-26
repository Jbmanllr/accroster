//! Sidebar sync as the core runs it (ADR 0003): the mode, "Sync now", the
//! Automatic watch, the dormant multi-account pause, and the sync before a
//! Guided switch. What sync writes is decided in [`crate::sidebar_sync`];
//! a run writes one sidebar per tick, so the menu and Settings show its
//! progress ("Syncing… 3 of 10") and then what it did.

use std::path::PathBuf;

use jiff::{SignedDuration, Timestamp};

use super::Core;
use crate::desktop::{Confirmed, Target};
use crate::interface::{
    ClaudeVersions, Command, Effect, MenuItem, SafetyFeature, SidebarSyncMode, StatusTone,
};
use crate::logging::{Field, Level};
use crate::safety::{CheckId, Finding};
use crate::sidebar_sync::{self, Begun, LeftOut, Run, SyncRun};

/// How long the menu shows how the last sync ended.
const RESULT_SHOWN_FOR: SignedDuration = SignedDuration::from_mins(5);

/// A sync under way.
pub(super) struct ActiveSync {
    run: Box<SyncRun>,
}

/// How the last sync ended, for the menu.
pub(super) struct SyncResult {
    text: String,
    tone: StatusTone,
    at: Timestamp,
}

impl Core {
    /// The Sidebar sync mode in Settings. Turning sync on checks the Sidebar
    /// entries first, so a changed format pauses it before any write.
    pub(super) fn set_sidebar_sync(&mut self, mode: SidebarSyncMode) {
        let mut updated = self.settings.clone();
        updated.sidebar_sync = mode;
        if !self.save_settings(updated) {
            return;
        }
        self.sync_note = None;
        self.sync_result = None;
        self.log(
            Level::Info,
            "sidebar sync mode",
            &[(
                "mode",
                Field::Word(match mode {
                    SidebarSyncMode::Off => "off",
                    SidebarSyncMode::Manual => "manual",
                    SidebarSyncMode::Automatic => "automatic",
                }),
            )],
        );
        if mode == SidebarSyncMode::Off && self.sync.take().is_some() {
            self.log(Level::Info, "sidebar sync stopped: turned off", &[]);
        }
        self.check_sidebar_entries();
        self.settle();
    }

    /// "Sync now" (or Automatic mode, or a Guided switch): checks the
    /// Sidebar entries again, plans the run and starts it (ADR 0003).
    /// Returns how it ended when it ended at once.
    pub(super) fn sync_sidebar(&mut self) -> Option<Run> {
        if self.settings.sidebar_sync == SidebarSyncMode::Off || self.sync.is_some() {
            return None;
        }
        self.sidebar_watch.ran();
        self.check_sidebar_entries();
        self.settle();
        if self.safety.is_paused(SafetyFeature::SidebarSync) {
            self.sync_note =
                Some("Sidebar sync is paused · see Diagnostics for why and to resume it".into());
            self.log(Level::Info, "sidebar sync not run: paused", &[]);
            self.show_result(StatusTone::Failed);
            return Some(Run::Paused);
        }
        match sidebar_sync::begin(
            &self.env,
            &self.paths,
            !self.settings.leave_out_ssh_and_no_folder_sessions,
            &mut self.sidebar_entries,
        ) {
            Begun::Ended(run) => {
                self.sync_ended(&run);
                Some(run)
            }
            Begun::Running(run) => {
                let (_, total) = run.progress();
                self.log(
                    Level::Info,
                    "sidebar sync started",
                    &[("folders", Field::Number(total as i64))],
                );
                self.sync = Some(ActiveSync { run });
                self.sync_result = None;
                None
            }
        }
    }

    /// Writes the next sidebar of the run under way; returns how the run
    /// ended once it did.
    pub(super) fn step_sync(&mut self) -> Option<Run> {
        let active = self.sync.as_mut()?;
        let ended = active
            .run
            .step(&self.env, &self.paths, &mut self.sidebar_entries)?;
        self.sync = None;
        self.sync_ended(&ended);
        Some(ended)
    }

    /// The run's progress (sidebars written, of how many), while one runs.
    pub(super) fn sync_progress(&self) -> Option<(usize, usize)> {
        self.sync.as_ref().map(|active| active.run.progress())
    }

    /// Settings' line: the progress while a run is under way, else what the
    /// last one did.
    pub(super) fn sync_settings_note(&self) -> Option<String> {
        if let Some(progress) = self.sync_progress() {
            return Some(syncing(progress));
        }
        match self.reopen.status() {
            Some((text, _)) => Some(text),
            None => self.sync_note.clone(),
        }
    }

    /// Whether "Reopen Claude" is offered: not while a Guided switch has
    /// Claude's attention.
    pub(super) fn reopen_offered(&self) -> bool {
        self.reopen.offered() && !self.desktop.active()
    }

    /// The menu's sync status line, while a run is under way (unless a
    /// Guided switch shows its own) and for a while after it ended, with
    /// "Reopen Claude" while it is offered; a reopen under way shows its
    /// own line instead of the result.
    pub(super) fn sync_menu_items(&self) -> Vec<MenuItem> {
        if self.desktop.sync_target().is_some() {
            return Vec::new();
        }
        if let Some(progress) = self.sync_progress() {
            return vec![MenuItem::Status {
                text: syncing(progress),
                tone: StatusTone::Working,
            }];
        }
        if self.reopen.active() {
            return self.reopen.menu_items();
        }
        let now = self.env.clock.now();
        let mut items: Vec<MenuItem> = self
            .sync_result
            .iter()
            .filter(|result| now.duration_since(result.at) < RESULT_SHOWN_FOR)
            .map(|result| MenuItem::Status {
                text: result.text.clone(),
                tone: result.tone,
            })
            .collect();
        if !items.is_empty() && self.reopen_offered() {
            items.push(MenuItem::Action {
                title: "Reopen Claude".into(),
                command: Command::ReopenDesktopApp,
            });
        }
        items
    }

    /// Follows a Reopen Claude under way; its outcome becomes the sync's
    /// result line, in the menu and Settings.
    pub(super) fn poll_reopen(&mut self) -> Vec<Effect> {
        let (reopen, ctx) = self.reopen();
        let (effects, outcome) = reopen.poll(&ctx);
        if let Some((text, tone)) = outcome {
            self.sync_note = Some(text);
            self.show_result(tone);
        }
        effects
    }

    fn show_result(&mut self, tone: StatusTone) {
        self.sync_result = self.sync_note.clone().map(|text| SyncResult {
            text,
            tone,
            at: self.env.clock.now(),
        });
    }

    /// Records how a run ended: the log, Settings' note, the menu's line and
    /// any Safety pause.
    fn sync_ended(&mut self, run: &Run) {
        match run {
            Run::Synced(report) => {
                self.safety
                    .record(CheckId::MultiAccountMode, Finding::pass());
                let mut fields = vec![
                    ("entries", Field::Number(report.written as i64)),
                    ("folders", Field::Number(report.folders as i64)),
                    ("not_backed_up", Field::Number(report.not_backed_up as i64)),
                    ("parsed", Field::Number(report.parsed as i64)),
                ];
                for (why, count) in &report.left_out {
                    fields.push((why.log_key(), Field::Number(*count as i64)));
                }
                self.log(Level::Info, "sidebar synced", &fields);
                if report.added_to_desktop > 0 {
                    self.reopen.offer();
                }
                self.sync_note = Some(sync_note(report));
                self.sync_result = Some(SyncResult {
                    text: sync_menu_line(report),
                    tone: StatusTone::Done,
                    at: self.env.clock.now(),
                });
            }
            Run::Paused => {}
            Run::DesktopAccountUnknown => {
                self.log(
                    Level::Info,
                    "sidebar sync not run: desktop account unknown",
                    &[],
                );
                self.sync_note =
                    Some("Couldn't tell which Profile Claude is using · nothing was synced".into());
                self.show_result(StatusTone::Failed);
            }
            Run::FoldersChanged(count) => {
                let whose = if *count > 1 { "sidebars" } else { "sidebar" };
                let detail =
                    format!("{count} {whose} Claude wasn't using changed since the last sync");
                self.pause_for_multi_account(detail);
                self.paused_sync_note();
            }
        }
    }

    /// The dormant multi-account mode's trace: a Safety pause of Sidebar
    /// sync. It stays until Resume, which accepts the sidebars as they are;
    /// the finding itself no longer blocks, so Resume shows at once.
    pub(super) fn pause_for_multi_account(&mut self, detail: String) {
        self.safety.record(
            CheckId::MultiAccountMode,
            Finding::fail(detail.clone()).blocking(
                SafetyFeature::SidebarSync,
                "Claude wrote to a Profile it isn't using",
            ),
        );
        self.settle();
        self.safety
            .record(CheckId::MultiAccountMode, Finding::fail(detail));
    }

    /// The sidebars' watch reported changes: Automatic mode schedules a
    /// run, and an entry Claude wrote outside its Active Profile folder
    /// pauses sync.
    pub(super) fn sidebar_files_changed(&mut self, changed: &[PathBuf]) {
        let mode = self.settings.sidebar_sync;
        let paused = self.safety.is_paused(SafetyFeature::SidebarSync);
        if mode == SidebarSyncMode::Off {
            return;
        }
        let automatic = mode == SidebarSyncMode::Automatic && !paused;
        let elsewhere =
            self.sidebar_watch
                .files_changed(&self.env, &self.paths, changed, automatic);
        if elsewhere == 0 || paused {
            return;
        }
        let entries = if elsewhere == 1 { "entry" } else { "entries" };
        self.pause_for_multi_account(format!(
            "Claude wrote {elsewhere} Sidebar {entries} outside the Profile it's using"
        ));
        self.log(
            Level::Warn,
            "sidebar sync paused: claude wrote another profile",
            &[("entries", Field::Number(elsewhere as i64))],
        );
        self.sync_note =
            Some("Sidebar sync is paused · Claude wrote to a Profile it isn't using".into());
    }

    pub(super) fn paused_sync_note(&mut self) {
        self.settle();
        self.log(Level::Warn, "sidebar sync stopped before writing", &[]);
        self.sync_note =
            Some("Sidebar sync is paused · nothing was written · see Diagnostics".into());
        self.show_result(StatusTone::Failed);
    }

    /// A Guided switch the guard let through: syncs the sidebars first when
    /// Sidebar sync is on (so the target's sidebar has every Session when
    /// Claude opens it), then brings Claude forward. The sync runs over the
    /// next ticks; [`Core::continue_desktop_switch`] picks up when it ends.
    pub(super) fn guide_desktop_switch(&mut self, cleared: Option<Target>) -> Vec<Effect> {
        let Some(target) = cleared else {
            return Vec::new();
        };
        if self.settings.sidebar_sync == SidebarSyncMode::Off {
            let (desktop, ctx) = self.desktop();
            return desktop.guide(&ctx, target);
        }
        let (desktop, ctx) = self.desktop();
        desktop.syncing(&ctx, target);
        match (self.sync.is_some(), self.sync_sidebar()) {
            // A run already under way (Sync now, Automatic) serves.
            (true, _) | (false, None) => Vec::new(),
            (false, Some(ended)) => self.continue_desktop_switch(&ended),
        }
    }

    /// "Switch anyway": after "can't tell", the switch syncs (when on) then
    /// guides; after a stopped sync, it guides without syncing.
    pub(super) fn confirm_desktop_switch(&mut self) -> Vec<Effect> {
        let (desktop, ctx) = self.desktop();
        match desktop.confirm(&ctx) {
            Some(Confirmed::Unsure(target)) => self.guide_desktop_switch(Some(target)),
            Some(Confirmed::SyncStopped(target)) => {
                let (desktop, ctx) = self.desktop();
                desktop.guide(&ctx, target)
            }
            None => Vec::new(),
        }
    }

    /// A sync ended while a Guided switch waited on it: guide, or say why
    /// the switch stopped.
    pub(super) fn continue_desktop_switch(&mut self, ended: &Run) -> Vec<Effect> {
        let Some(target) = self.desktop.sync_target().cloned() else {
            return Vec::new();
        };
        let reason = match ended {
            Run::Synced(_) => {
                let (desktop, ctx) = self.desktop();
                return desktop.guide(&ctx, target);
            }
            Run::Paused => self
                .safety
                .paused_reason(SafetyFeature::SidebarSync)
                .unwrap_or_else(|| "it is paused".into()),
            Run::DesktopAccountUnknown => "couldn't tell which Profile Claude is using".into(),
            Run::FoldersChanged(_) => "Claude wrote to a Profile it isn't using".into(),
        };
        let (desktop, ctx) = self.desktop();
        desktop.sync_stopped(&ctx, reason)
    }
}

/// "Syncing… 3 of 10".
fn syncing((done, total): (usize, usize)) -> String {
    format!("Syncing… {done} of {total}")
}

/// "Claude 3.6556.2", or "Claude" when the desktop app's version is unknown.
pub(super) fn desktop_name(versions: &ClaudeVersions) -> String {
    match &versions.desktop {
        Some(version) => format!("Claude {version}"),
        None => "Claude".to_owned(),
    }
}

/// Settings' line after a sync.
fn sync_note(report: &sidebar_sync::Report) -> String {
    sync_line(report, true)
}

/// The menu's shorter line: what was written and left out, without the
/// explanations Settings gives.
fn sync_menu_line(report: &sidebar_sync::Report) -> String {
    sync_line(report, false)
}

fn sync_line(report: &sidebar_sync::Report, explained: bool) -> String {
    let plural = |count: usize, one: &str, many: &str| {
        format!(
            "{} {}",
            thousands(count),
            if count == 1 { one } else { many }
        )
    };
    let mut note = if report.written == 0 {
        "Every sidebar was already in sync".to_owned()
    } else {
        format!(
            "Synced {} into {}",
            plural(report.written, "entry", "entries"),
            plural(report.folders, "sidebar", "sidebars")
        )
    };
    if report.added_to_desktop > 0 {
        let added = thousands(report.added_to_desktop);
        note.push_str(&if explained {
            format!(" · {added} added to the sidebar Claude is using · Reopen Claude to show them")
        } else {
            format!(" · {added} added to Claude's sidebar · Reopen Claude to show them")
        });
    }
    if report.held_for_desktop > 0 && explained {
        note.push_str(&format!(
            " · {} there syncs once Claude is quit",
            plural(report.held_for_desktop, "changed entry", "changed entries")
        ));
    }
    if report.not_backed_up > 0 {
        note.push_str(&format!(
            " · {} couldn't be backed up and were left as they were",
            report.not_backed_up
        ));
    }
    if !explained {
        if !report.left_out.is_empty() {
            note.push_str(&left_out_part(report.left_out.iter(), None));
        }
        return note;
    }
    // Settings says why, one part per reason.
    let mut reasons: Vec<&'static str> = report.left_out.keys().map(|kind| kind.why()).collect();
    reasons.dedup();
    for reason in reasons {
        let kinds = report
            .left_out
            .iter()
            .filter(|(kind, _)| kind.why() == reason);
        note.push_str(&left_out_part(kinds, Some(reason)));
    }
    note
}

/// " · left out 17 SSH and 21 No-folder Sessions", then `(why)` when given.
fn left_out_part<'a>(
    left_out: impl Iterator<Item = (&'a LeftOut, &'a usize)>,
    why: Option<&str>,
) -> String {
    let mut total = 0;
    let parts: Vec<String> = left_out
        .map(|(kind, count)| {
            total += count;
            format!("{} {}", thousands(*count), kind.name())
        })
        .collect();
    let listed = match parts.as_slice() {
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        [] => String::new(),
    };
    let sessions = if total == 1 { "Session" } else { "Sessions" };
    let why = why.map_or(String::new(), |why| format!(" ({why})"));
    format!(" · left out {listed} {sessions}{why}")
}

/// `5014` as "5,014".
fn thousands(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}
