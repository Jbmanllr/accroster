//! Sidebar sync as the core runs it (ADR 0003): the mode, "Sync now", the
//! Automatic watch, the dormant multi-account pause, and the sync before a
//! Guided switch. What sync writes is decided in [`crate::sidebar_sync`].

use std::path::PathBuf;

use super::Core;
use crate::desktop::Target;
use crate::interface::{ClaudeVersions, Effect, SafetyFeature, SidebarSyncMode};
use crate::logging::{Field, Level};
use crate::safety::{CheckId, Finding};
use crate::sidebar_sync::{self, Run};

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
        self.check_sidebar_entries();
        self.settle();
    }

    /// "Sync now": checks the Sidebar entries again, then copies every entry
    /// into every other Profile's sidebar (ADR 0003).
    pub(super) fn sync_sidebar(&mut self) {
        if self.settings.sidebar_sync == SidebarSyncMode::Off {
            return;
        }
        self.sidebar_watch.ran();
        self.check_sidebar_entries();
        self.settle();
        if self.safety.is_paused(SafetyFeature::SidebarSync) {
            self.sync_note =
                Some("Sidebar sync is paused · see Diagnostics for why and to resume it".into());
            self.log(Level::Info, "sidebar sync not run: paused", &[]);
            return;
        }
        match sidebar_sync::run(&self.env, &self.paths) {
            Run::Synced(report) => {
                self.safety
                    .record(CheckId::MultiAccountMode, Finding::pass());
                self.log(
                    Level::Info,
                    "sidebar synced",
                    &[
                        ("entries", Field::Number(report.written as i64)),
                        ("folders", Field::Number(report.folders as i64)),
                        ("not_backed_up", Field::Number(report.not_backed_up as i64)),
                    ],
                );
                self.sync_note = Some(sync_note(&report));
            }
            Run::DesktopAccountUnknown => {
                self.log(
                    Level::Info,
                    "sidebar sync not run: desktop account unknown",
                    &[],
                );
                self.sync_note =
                    Some("Couldn't tell which Profile Claude is using · nothing was synced".into());
            }
            Run::UnknownFields(fields) => {
                let detail = format!(
                    "unknown fields: {}",
                    fields.into_iter().collect::<Vec<_>>().join(", ")
                );
                let reason = format!("{} changed Sidebar entries", desktop_name(&self.versions));
                self.safety.record(
                    CheckId::SidebarEntries,
                    Finding::fail(detail).blocking(SafetyFeature::SidebarSync, reason),
                );
                self.paused_sync_note();
            }
            Run::FoldersChanged(count) => {
                let whose = if count > 1 { "sidebars" } else { "sidebar" };
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
    }

    /// A Guided switch the guard let through: syncs the sidebars first when
    /// Sidebar sync is on (so the target's sidebar has every Session when
    /// Claude opens it), then brings Claude forward.
    pub(super) fn guide_desktop_switch(&mut self, cleared: Option<Target>) -> Vec<Effect> {
        let Some(target) = cleared else {
            return Vec::new();
        };
        if self.settings.sidebar_sync != SidebarSyncMode::Off {
            self.sync_sidebar();
        }
        let (desktop, ctx) = self.desktop();
        desktop.guide(&ctx, target)
    }
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
    let mut note = if report.written == 0 {
        "Every sidebar was already in sync".to_owned()
    } else {
        let entries = if report.written == 1 {
            "entry"
        } else {
            "entries"
        };
        let profiles = if report.folders == 1 {
            "Profile"
        } else {
            "Profiles"
        };
        format!(
            "Updated {} Sidebar {entries} in {} {profiles}",
            report.written, report.folders
        )
    };
    if report.left_desktop_account {
        note.push_str(" · the Profile Claude is using syncs once Claude is quit");
    }
    if report.not_backed_up > 0 {
        note.push_str(&format!(
            " · {} couldn't be backed up and were left as they were",
            report.not_backed_up
        ));
    }
    note
}
