//! The menu, assembled from named parts: the status lines, the header, the
//! Profile rows, the Discovered rows and the fixed items. [`Core::view`]
//! puts them in order.
//!
//! Each client's Active Profile is read once for the menu and once for a
//! Limit monitor step ([`ActiveProfiles`]) and passed down, so the parts of
//! one menu agree on it. The All Sessions detail and Sidebar sync's watch
//! still read the desktop's account themselves.

use std::path::PathBuf;

use super::{Core, RECENT_SESSIONS, most_headroom};
use crate::claude_code;
use crate::claude_files::ClaudePaths;
use crate::cli_switcher;
use crate::context::Context;
use crate::desktop_account::DesktopAccount;
use crate::interface::{Badge, Command, HiddenDiscovered, MOST_HEADROOM, MenuItem};
use crate::monitor::Watched;
use crate::profiles::{Listed, SignedInProfile};

/// Each client's Active Profile, as its files say now.
pub(super) struct ActiveProfiles {
    /// The folder the CLI's pointer names; `None` without a pointer.
    pub cli: Option<PathBuf>,
    /// The desktop app's account and Organization.
    pub desktop: Option<DesktopAccount>,
}

impl Core {
    /// Reads each client's Active Profile: the CLI's pointer and the
    /// desktop app's account.
    pub(super) fn active_profiles(&self) -> ActiveProfiles {
        ActiveProfiles {
            cli: cli_switcher::pointer(&self.paths),
            desktop: self.desktop_account(),
        }
    }

    /// The desktop's account and Organization.
    pub(super) fn desktop_account(&self) -> Option<DesktopAccount> {
        self.newest_entries.desktop_account(
            &ClaudePaths::new(&self.env.home),
            &self.profiles.sidebar_kinds(),
        )
    }

    /// The signed-in Profiles the Limit monitor reads, with which are a
    /// client's Active Profile: the CLI's (its pointer) or the desktop app's.
    pub(super) fn watched(&self, active: &ActiveProfiles) -> Vec<Watched> {
        let desktop_active = active.desktop.as_ref().and_then(|account| {
            let signed_in = self.profiles.signed_in();
            account
                .profile(&signed_in)
                .map(|profile| profile.id.clone())
        });
        let mut watched = self.profiles.watched(|id, config_dir| {
            active.cli.as_deref() == Some(config_dir) || desktop_active.as_deref() == Some(id)
        });
        // Two Claude Code runs never share a folder.
        watched.retain(|w| !self.openings.queued().any(|(dir, _)| dir == w.config_dir));
        watched
    }

    /// The lines above everything else: a Guided switch's status, else a
    /// sync's, then what is wrong (offline, warnings, a Safety pause).
    pub(super) fn status_lines(&self, ctx: &Context, watched: &[Watched]) -> Vec<MenuItem> {
        let mut items = self.desktop.menu_items(self.sync_progress());
        items.extend(self.sync_menu_items());
        items.extend(
            self.monitor
                .offline_line(ctx, watched)
                .into_iter()
                .chain(self.warnings.iter().cloned())
                .chain(
                    self.claude
                        .missing()
                        .then(|| claude_code::MISSING.to_owned()),
                )
                .chain(self.safety.warning_lines())
                .chain(self.profiles.notice().map(str::to_owned))
                .map(MenuItem::Warning),
        );
        items
    }

    /// The Profiles in the user's order, each row with its limits, the
    /// "most headroom" mark and its links' state. What each client adds to a
    /// row comes after: [`cli_switcher::present`], [`Core::default_login_part`]
    /// and [`Core::add_desktop_items`].
    pub(super) fn listed_profiles(&self, ctx: &Context, watched: &[Watched]) -> Vec<Listed<'_>> {
        let mut listed = self.profiles.listed(&self.settings, |id| {
            let active = watched.iter().any(|w| w.id == id && w.active);
            self.monitor.view(ctx, id, active)
        });
        if let Some(headroom) = most_headroom(&listed, watched) {
            for listed in listed.iter_mut() {
                listed.row.marker = (listed.id == headroom.id).then(|| MOST_HEADROOM.to_owned());
            }
        }
        for profile in &mut listed {
            self.annotate_links(profile.id, &mut profile.row);
        }
        listed
    }

    /// Without CLI switching the CLI uses `~/.claude`, named by its Login:
    /// marks the row of the Profile that Login is, and returns the header's
    /// words for it. `None` when `~/.claude` holds no Login.
    pub(super) fn default_login_part(
        &self,
        listed: &mut [Listed<'_>],
        signed_in: &[SignedInProfile],
    ) -> Option<String> {
        let (pair, names) = self
            .discovered
            .iter()
            .find_map(|evidence| Some((&evidence.pair, evidence.default_login()?)))?;
        let listed = signed_in
            .iter()
            .find(|profile| profile.key == *pair)
            .and_then(|profile| listed.iter_mut().find(|listed| listed.id == profile.id));
        let name = match listed {
            Some(listed) => {
                listed.row.default_login = true;
                listed.short_label.clone()
            }
            None => names.name(),
        };
        Some(format!("CLI: {name} (default login)"))
    }

    /// Adds the `Desktop` badge to the desktop app's Active Profile, and
    /// "Switch desktop app to this Profile…" to the others when the desktop
    /// app is installed.
    pub(super) fn add_desktop_items(
        &self,
        listed: &mut [Listed<'_>],
        desktop_profile: Option<&SignedInProfile>,
        signed_in: &[SignedInProfile],
    ) {
        let desktop_installed = self.env.os.desktop_app_version().is_some();
        for listed in listed.iter_mut() {
            let Some(profile) = signed_in.iter().find(|profile| profile.id == listed.id) else {
                continue;
            };
            let row = &mut listed.row;
            if desktop_profile.is_some_and(|desktop| desktop.id == profile.id) {
                row.badges.push(Badge::Desktop);
            } else if desktop_installed {
                row.submenu.push(MenuItem::Action {
                    title: "Switch desktop app to this Profile…".into(),
                    command: Command::SwitchDesktopApp(profile.id.clone()),
                });
            }
        }
    }

    /// The Discovered Profiles' rows for the menu, and the hidden ones for
    /// Settings.
    pub(super) fn discovered_parts(&self) -> (Vec<MenuItem>, Vec<HiddenDiscovered>) {
        let shown = self.discovered_rows();
        let tz = self.env.clock.time_zone();
        let rows = shown
            .iter()
            .filter(|(_, choice)| !choice.is_some_and(|c| c.hidden))
            .map(|(evidence, choice)| {
                MenuItem::Discovered(evidence.row(choice.and_then(|c| c.nickname.as_deref()), &tz))
            })
            .collect();
        let hidden = shown
            .iter()
            .filter(|(_, choice)| choice.is_some_and(|c| c.hidden))
            .map(|(evidence, choice)| HiddenDiscovered {
                title: evidence.hidden_title(choice.and_then(|c| c.nickname.as_deref()), &tz),
                restore: Command::UnhideDiscovered(evidence.pair.clone()),
            })
            .collect();
        (rows, hidden)
    }

    /// The menu's last items, the same whatever the Profiles are: Recent
    /// Sessions, All Sessions…, Add Profile… (or the sign-in under way),
    /// Settings…, Diagnostics…, About and Quit.
    pub(super) fn fixed_menu_items(&self) -> Vec<MenuItem> {
        let mut items = Vec::new();
        let recent: Vec<MenuItem> = self
            .sessions
            .recent(RECENT_SESSIONS)
            .into_iter()
            .map(|(id, title)| MenuItem::Action {
                title,
                command: Command::OpenSession(id),
            })
            .collect();
        if !recent.is_empty() {
            items.push(MenuItem::Submenu {
                title: "Recent Sessions".into(),
                items: recent,
            });
        }
        items.push(MenuItem::Action {
            title: "All Sessions…".into(),
            command: Command::OpenAllSessions,
        });
        items.push(if self.profiles.signing_in() {
            MenuItem::Action {
                title: "Waiting for sign-in… Cancel".into(),
                command: Command::CancelSignIn,
            }
        } else {
            MenuItem::Action {
                title: "Add Profile…".into(),
                command: Command::AddProfile,
            }
        });
        items.extend([
            MenuItem::Action {
                title: "Settings…".into(),
                command: Command::OpenSettings,
            },
            MenuItem::Action {
                title: "Diagnostics…".into(),
                command: Command::OpenDiagnostics,
            },
            MenuItem::Action {
                title: "About".into(),
                command: Command::OpenAbout,
            },
            MenuItem::Separator,
            MenuItem::Action {
                title: "Quit".into(),
                command: Command::Quit,
            },
        ]);
        items
    }
}

/// The header line: each client's Active Profile by its Short label, the
/// CLI's first. `cli_part` is CLI switching's, else the Default login's;
/// without either the CLI reads "Default", said only when the desktop's part
/// is.
pub(super) fn header_line(
    cli_part: Option<String>,
    desktop: Option<&DesktopAccount>,
    desktop_profile: Option<&SignedInProfile>,
    listed: &[Listed<'_>],
) -> Option<String> {
    let desktop_part = desktop.map(|_| {
        let name = desktop_profile
            .and_then(|desktop| listed.iter().find(|listed| listed.id == desktop.id))
            .map_or("another Profile", |listed| listed.short_label.as_str());
        format!("Desktop: {name}")
    });
    let cli_part = cli_part.or_else(|| desktop_part.as_ref().map(|_| "CLI: Default".to_owned()));
    let header: Vec<String> = cli_part.into_iter().chain(desktop_part).collect();
    (!header.is_empty()).then(|| header.join("    "))
}
