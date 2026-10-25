//! The self-check (ADR 0004): which format checks run when, the `auth status`
//! cross-check, settling Safety pauses and resuming them, and the
//! Diagnostics window. The checks themselves are in [`crate::checks`].

use std::path::Path;

use super::Core;
use super::sidebar::desktop_name;
use crate::checks;
use crate::cli_switcher;
use crate::desktop;
use crate::interface::{CheckStatus, MenuItem, SafetyFeature, SidebarSyncMode};
use crate::logging::{Field, Level};
use crate::profiles::CrossCheckAnswer;
use crate::safety::{self, CheckId, Finding};
use crate::sidebar_sync;

/// What the last link check found in one Profile folder.
pub(super) struct ProfileLinks {
    id: String,
    /// Links whose place holds something else.
    replaced: Vec<&'static str>,
    /// Whether every shared-Transcript link is in place.
    transcripts_shared: bool,
}
/// When a self-check runs Claude Code's `auth status` in every Profile.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AuthCheck {
    /// Only when the Claude versions changed since the last self-check:
    /// a short-lived `claude` can start a token refresh and exit before
    /// saving it, so the cross-check stays rare.
    IfVersionsChanged,
    Always,
}

impl Core {
    /// The Diagnostics window: every check, the versions, the pauses.
    pub(super) fn diagnostics(&self) -> crate::interface::DiagnosticsView {
        let usage = self.monitor.usage_summary();
        let mut finding = if usage.unreadable > 0 {
            Finding::fail(format!(
                "{} of {} last answers couldn't be read",
                usage.unreadable,
                usage.unreadable + usage.read
            ))
        } else if usage.read > 0 {
            Finding::pass()
        } else {
            Finding::never_seen()
        };
        if !usage.new_kinds.is_empty() {
            let kinds: Vec<String> = usage
                .new_kinds
                .iter()
                .map(|kind| safety::reportable(kind))
                .collect();
            let line = format!("New limit kind seen: {}", kinds.join(", "));
            finding.detail = Some(match finding.detail.take() {
                Some(detail) => format!("{detail}; {line}"),
                None => line,
            });
        }
        self.safety.diagnostics(self.versions.clone(), finding)
    }

    /// "Transcripts not shared" under a Profile whose shared-Transcript links
    /// aren't in place, and a detail line per other replaced link.
    pub(super) fn annotate_links(&self, id: &str, row: &mut crate::interface::ProfileRow) {
        let Some(links) = self.links.iter().find(|links| links.id == id) else {
            return;
        };
        if !links.transcripts_shared {
            row.note = Some(match row.note.take() {
                Some(note) => format!("{note} · Transcripts not shared"),
                None => "Transcripts not shared".to_owned(),
            });
        }
        for name in links
            .replaced
            .iter()
            .filter(|name| !cli_switcher::TRANSCRIPT_LINKS.contains(name))
        {
            row.submenu.push(MenuItem::Detail(format!(
                "{name} link replaced · not shared"
            )));
        }
    }

    /// Runs the full self-check when a Claude version changed since the last
    /// one. Returns whether it ran.
    pub(super) fn self_check_if_versions_changed(&mut self) -> bool {
        if !self.safety.versions_changed(&desktop::versions(&self.env)) {
            return false;
        }
        self.log(Level::Info, "claude versions changed", &[]);
        self.self_check(AuthCheck::IfVersionsChanged);
        true
    }

    /// The full self-check: settings `env`, accounts, links, Sidebar entries,
    /// live-process files and Transcript records, plus `auth status` per
    /// `auth` (recorded once its answers are in); then pauses whatever a
    /// failed check blocks.
    pub(super) fn self_check(&mut self, auth: AuthCheck) {
        let versions = desktop::versions(&self.env);
        let run_auth = auth == AuthCheck::Always || self.safety.versions_changed(&versions);
        self.versions = versions;
        self.safety.check_settings_env(&self.env, false);
        self.check_accounts();
        self.check_links();
        if run_auth {
            self.check_config_dir_honoured();
        }
        let home = self.env.home.clone();
        self.check_sidebar_entries();
        let mut session_dirs = vec![home.join(".claude")];
        session_dirs.extend(
            self.profiles
                .folders()
                .into_iter()
                .map(|(_, dir)| dir.to_path_buf()),
        );
        self.safety.record(
            CheckId::LiveProcesses,
            checks::live_processes(&session_dirs),
        );
        self.safety.record(
            CheckId::TranscriptRecords,
            checks::transcript_records(&home),
        );
        self.safety.mark_checked(&self.versions);
        self.settle();
    }

    /// Pauses whatever a current finding blocks, and logs each new pause.
    pub(super) fn settle(&mut self) {
        for feature in self.safety.settle() {
            self.log(
                Level::Warn,
                "safety pause",
                &[("feature", Field::Word(feature.key()))],
            );
        }
    }

    pub(super) fn check_accounts(&mut self) {
        let folders: Vec<&Path> = self
            .profiles
            .folders()
            .into_iter()
            .map(|(_, dir)| dir)
            .collect();
        let (accounts, caches) =
            checks::accounts(&folders, &safety::claude_code_name(&self.versions));
        self.safety.record(CheckId::OauthAccount, accounts);
        self.safety.record(CheckId::CachedUsage, caches);
    }

    /// Checks every Profile folder's links: a replaced shared-Transcript link
    /// pauses Profile links; other replaced links are only reported.
    pub(super) fn check_links(&mut self) {
        let claude_home = self.env.home.join(".claude");
        self.links = self
            .profiles
            .folders()
            .into_iter()
            .map(|(id, dir)| ProfileLinks {
                id: id.to_owned(),
                replaced: cli_switcher::replaced_links(dir, &claude_home),
                transcripts_shared: cli_switcher::TRANSCRIPT_LINKS.iter().all(|name| {
                    std::fs::read_link(dir.join(name)).ok() == Some(claude_home.join(name))
                }),
            })
            .collect();
        if self.links.is_empty() {
            return;
        }
        let summary = |transcript: bool| -> Option<String> {
            let mut names: Vec<&str> = Vec::new();
            let mut profiles = 0;
            for links in &self.links {
                let hit: Vec<&str> = links
                    .replaced
                    .iter()
                    .copied()
                    .filter(|name| cli_switcher::TRANSCRIPT_LINKS.contains(name) == transcript)
                    .collect();
                if !hit.is_empty() {
                    profiles += 1;
                }
                for name in hit {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }
            (profiles > 0).then(|| {
                let plural = if names.len() > 1 { "links" } else { "link" };
                let whose = if profiles > 1 { "Profiles" } else { "Profile" };
                format!(
                    "{} {plural} replaced in {profiles} {whose}",
                    names.join(", ")
                )
            })
        };
        let transcripts = match summary(true) {
            Some(text) => Finding::fail(text.clone()).blocking(SafetyFeature::Links, text),
            None => Finding::pass(),
        };
        let others = match summary(false) {
            Some(text) => Finding::fail(text),
            None => Finding::pass(),
        };
        self.safety.record(CheckId::TranscriptLinks, transcripts);
        self.safety.record(CheckId::OtherLinks, others);
    }

    /// Queues `claude auth status --json` in every signed-in Profile, to
    /// compare with each folder's `oauthAccount` once the answers are in
    /// ([`Core::cross_checked`]). Returns whether anything was queued.
    pub(super) fn check_config_dir_honoured(&mut self) -> bool {
        let (profiles, ctx) = self.profiles();
        profiles.start_cross_check(&ctx)
    }

    /// Every cross-check answer is in: records what they say.
    pub(super) fn cross_checked(&mut self, answers: &[CrossCheckAnswer]) {
        if let Some(finding) = checks::config_dir_honoured(&self.versions, answers) {
            self.safety.record(CheckId::ConfigDirHonoured, finding);
        }
        self.settle();
        if let Some(feature) = self.resume_waiting.take() {
            self.finish_resume(feature);
        }
    }

    /// "Resume <feature>": runs the feature's checks again and ends its
    /// pause if none fails. CLI switching's include the `auth status`
    /// cross-check, so it resumes once that is in.
    pub(super) fn resume(&mut self, feature: SafetyFeature) {
        match feature {
            SafetyFeature::CliSwitching => {
                self.versions = desktop::versions(&self.env);
                self.safety.check_settings_env(&self.env, false);
                self.check_accounts();
                if self.check_config_dir_honoured() {
                    self.resume_waiting = Some(feature);
                    return;
                }
            }
            SafetyFeature::Links => self.check_links(),
            SafetyFeature::Seeding => self.check_accounts(),
            SafetyFeature::SidebarSync => self.check_sidebar_entries(),
        }
        self.finish_resume(feature);
    }

    pub(super) fn finish_resume(&mut self, feature: SafetyFeature) {
        self.settle();
        if !self.safety.resume(feature) {
            self.log(
                Level::Info,
                "safety pause kept",
                &[("feature", Field::Word(feature.key()))],
            );
            return;
        }
        self.log(
            Level::Info,
            "safety pause resumed",
            &[("feature", Field::Word(feature.key()))],
        );
        if feature == SafetyFeature::Links {
            let (profiles, ctx) = self.profiles();
            profiles.heal_links(&ctx);
            self.check_links();
        }
        if feature == SafetyFeature::SidebarSync {
            // The user accepts the sidebars as they are now.
            sidebar_sync::accept_current(&self.env, &self.paths);
            self.sync_note = None;
        }
    }

    /// The Sidebar entries check. A failure pauses Sidebar sync only while
    /// sync is on: with it off, nothing would be written anyway.
    pub(super) fn check_sidebar_entries(&mut self) {
        let mut finding = checks::sidebar_entries(&self.env.home);
        if finding.status == CheckStatus::Fail && self.settings.sidebar_sync != SidebarSyncMode::Off
        {
            let reason = format!("{} changed Sidebar entries", desktop_name(&self.versions));
            finding = finding.blocking(SafetyFeature::SidebarSync, reason);
        }
        self.safety.record(CheckId::SidebarEntries, finding);
    }
}
