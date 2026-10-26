//! Opening a Session from the core: who could open it (the CLI, the
//! desktop app, the signed-in Profiles), double-click, Resume in Terminal
//! and Open in Claude. The rules are in [`crate::opening`].

use std::path::{Path, PathBuf};

use super::Core;
use crate::desktop::{self, DesktopPaths, Target};
use crate::interface::{Command, Effect};
use crate::logging::{Field, Level};
use crate::opening::{self, Clients, DefaultOpen, DesktopClient, InClaude, NamedProfile};
use crate::runner::{Ended, JobKind, Runner};
use crate::sessions::SessionFacts;

impl Core {
    /// Who could open a Session right now: the CLI's and the desktop's
    /// Active Profiles, and the signed-in Profiles.
    pub(super) fn clients(&self) -> Clients {
        let signed_in = self.profiles.signed_in();
        let profiles: Vec<NamedProfile> = self
            .profiles
            .ordered_ids(&self.settings.profile_order)
            .into_iter()
            .filter_map(|id| {
                let profile = signed_in.iter().find(|profile| profile.id == id)?;
                Some(NamedProfile {
                    name: (self.settings.nicknames.get(&id).cloned())
                        .unwrap_or_else(|| profile.name.clone()),
                    pair: profile.key.clone(),
                    id,
                })
            })
            .collect();
        let desktop = self.env.os.desktop_app_version().map(|_| {
            let account = desktop::desktop_account(&DesktopPaths::new(&self.env.home));
            let pair = account
                .as_ref()
                .and_then(|account| match account.profile(&signed_in) {
                    Some(profile) => Some(profile.key.clone()),
                    None => account.key(),
                });
            let name = pair.as_ref().and_then(|pair| {
                self.sidebar_profiles()
                    .into_iter()
                    .find(|sidebar| sidebar.pair == *pair)
                    .map(|sidebar| sidebar.name)
            });
            DesktopClient { pair, name }
        });
        let desktop_turn_running = desktop.is_some()
            && matches!(
                desktop::guard(&self.env, &DesktopPaths::new(&self.env.home)),
                desktop::Guard::Busy(_)
            );
        Clients {
            cli: self.cli_profile_name(),
            desktop,
            profiles,
            desktop_turn_running,
            can_run_claude_code: self.claude_code_folder().is_some(),
        }
    }

    /// Where `claude --desktop --resume` runs (any subscription login will
    /// do, never `~/.claude`): the CLI's Active Profile, else the desktop's,
    /// else the first signed-in Profile.
    pub(super) fn claude_code_folder(&self) -> Option<PathBuf> {
        let ordered = self.profiles.ordered_ids(&self.settings.profile_order);
        let folders: Vec<(&str, &Path)> = ordered
            .iter()
            .filter_map(|id| Some((id.as_str(), self.profiles.config_dir(id)?)))
            .collect();
        let cli = self
            .cli_pointer()
            .filter(|pointer| folders.iter().any(|(_, dir)| *dir == pointer.as_path()));
        let desktop = || {
            let account = desktop::desktop_account(&DesktopPaths::new(&self.env.home))?;
            let profile = account.profile(&self.profiles.signed_in())?.id.clone();
            let (_, dir) = folders.iter().find(|(id, _)| *id == profile)?;
            Some(dir.to_path_buf())
        };
        let first = || folders.first().map(|(_, dir)| dir.to_path_buf());
        cli.or_else(desktop).or_else(first)
    }

    /// A double-click or a Recent Sessions item: the client the Session was
    /// last used in. When Claude's Active Profile has no entry for it, the
    /// Session is shown in All Sessions, where the choices are spelled out.
    pub(super) fn open_session(&mut self, id: String) -> Vec<Effect> {
        let Some(facts) = self.sessions.facts(&id) else {
            return Vec::new();
        };
        match opening::default_open(&facts, &self.clients()) {
            None => Vec::new(),
            Some(DefaultOpen::Terminal) => self.open_in_terminal(&id),
            Some(DefaultOpen::Claude) => self.open_in_claude(&id),
            Some(DefaultOpen::Choose) => {
                let effects = self.handle(Command::OpenAllSessions);
                self.sessions.set_filter(Default::default());
                self.sessions.set_search(String::new());
                self.sessions.select(Some(id));
                effects
            }
        }
    }

    /// Resume in Terminal, or Show in Terminal when it already runs there.
    pub(super) fn open_in_terminal(&mut self, id: &str) -> Vec<Effect> {
        let Some(effect) = self
            .sessions
            .facts(id)
            .and_then(|facts| opening::terminal(&facts, &*self.env.os))
        else {
            return Vec::new();
        };
        let how = match effect {
            Effect::ShowInTerminal { .. } => "show_in_terminal",
            _ => "resume_in_terminal",
        };
        self.log_opened(id, how);
        vec![effect]
    }

    /// Open in Claude: its entry in the desktop's Active Profile, else
    /// `claude --desktop --resume`.
    pub(super) fn open_in_claude(&mut self, id: &str) -> Vec<Effect> {
        let Some(facts) = self.sessions.facts(id) else {
            return Vec::new();
        };
        let clients = self.clients();
        match opening::in_claude(&facts, &clients) {
            None => Vec::new(),
            Some(InClaude::Entry(entry)) => {
                self.log_opened(id, "claude_entry");
                vec![Effect::OpenUrl(opening::continue_url(&entry))]
            }
            Some(InClaude::Resume) => {
                if opening::resume_blocked(&facts, &clients).is_some() {
                    return Vec::new();
                }
                self.resume_in_claude(&facts)
            }
        }
    }

    /// Starts `claude --desktop --resume <id>` in a Profile folder; its end
    /// is collected by [`Core::poll_openings`].
    pub(super) fn resume_in_claude(&mut self, facts: &SessionFacts) -> Vec<Effect> {
        let Some(folder) = self.claude_code_folder() else {
            return Vec::new();
        };
        let kind = JobKind::OpenInDesktop {
            session_id: facts.id.clone(),
            session_folder: facts.cwd.clone(),
        };
        self.openings.push(&folder, kind, facts.id.clone());
        Vec::new()
    }

    /// Collects the `--desktop --resume` runs that ended; a failed or
    /// timed-out hand-over is notified.
    pub(super) fn poll_openings(&mut self) -> Vec<Effect> {
        let ended = self
            .openings
            .poll(&Runner::new(&self.env, &self.paths, self.claude.program()));
        let mut effects = Vec::new();
        for Ended {
            tag: id, finished, ..
        } in ended
        {
            if finished.succeeded() {
                self.log_opened(&id, "claude_resume");
                continue;
            }
            self.log(
                Level::Warn,
                "session not opened in claude",
                &[
                    ("session", Field::SessionId(&id)),
                    ("error", Field::Word(finished.word())),
                ],
            );
            effects.push(Effect::Notify {
                title: "Couldn't open the Session in Claude".into(),
                body: "Claude Code couldn't hand it over. Check that Claude is signed in, then try again."
                    .into(),
            });
        }
        effects
    }

    /// "Switch Claude to <Profile>, then open…": a Guided switch that opens
    /// the Profile's entry once confirmed.
    pub(super) fn switch_claude_then_open(
        &mut self,
        profile_id: &str,
        session: &str,
    ) -> Vec<Effect> {
        let Some(facts) = self.sessions.facts(session) else {
            return Vec::new();
        };
        let Some(profile) = self
            .profiles
            .signed_in()
            .into_iter()
            .find(|profile| profile.id == profile_id)
        else {
            return Vec::new();
        };
        let pair = profile.key.clone();
        let Some(entry) = opening::switch_entry(&facts, &pair) else {
            return Vec::new();
        };
        let target = Target {
            then_open: Some(entry.entry_id.clone()),
            ..Target::of(&profile)
        };
        let (desktop, ctx) = self.desktop();
        let cleared = desktop.start(&ctx, target);
        self.guide_desktop_switch(cleared)
    }

    pub(super) fn log_opened(&self, id: &str, how: &'static str) {
        self.log(
            Level::Info,
            "session opened",
            &[("session", Field::SessionId(id)), ("how", Field::Word(how))],
        );
    }
}
