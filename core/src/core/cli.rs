//! CLI switching as the core runs it (ADR 0002): the switch in Settings,
//! choosing the CLI's Active Profile and moving it off a Profile being
//! removed. The shim, pointer and `.zshrc` block are [`crate::cli_switcher`].

use std::path::{Path, PathBuf};

use super::Core;
use crate::cli_switcher::{self, TurnedOff};
use crate::interface::SafetyFeature;
use crate::logging::{Field, Level};
use crate::monitor::LimitsView;

impl Core {
    /// The CLI's pointer while CLI switching is on.
    pub(super) fn cli_pointer(&self) -> Option<PathBuf> {
        cli_switcher::is_on(&self.paths)
            .then(|| cli_switcher::pointer(&self.paths))
            .flatten()
    }

    /// The CLI's Active Profile as the menu header names it: the pointer's
    /// Profile, or Default (`~/.claude`) without one.
    pub(super) fn cli_profile_name(&self) -> String {
        let Some(pointer) = self.cli_pointer() else {
            return "Default".into();
        };
        self.profiles
            .listed(&self.settings, |_| LimitsView::default())
            .into_iter()
            .find(|profile| profile.config_dir == pointer)
            .map_or_else(
                || "an unknown Profile".into(),
                |profile| profile.short_label,
            )
    }

    /// Points new CLI runs at Profile `next`, or at Default (`~/.claude`,
    /// no pointer) for `None`, before the CLI's Active Profile is removed.
    /// Returns whether that worked; the removal waits for it.
    pub(super) fn point_cli_elsewhere(&mut self, next: Option<&str>) -> bool {
        let result = match next {
            Some(next) => match self.profiles.config_dir(next) {
                Some(config_dir) => cli_switcher::set_pointer(&self.paths, config_dir),
                None => return false,
            },
            None => cli_switcher::clear_pointer(&self.paths),
        };
        match result {
            Ok(()) => {
                self.log(Level::Info, "cli profile moved before removal", &[]);
                true
            }
            Err(error) => {
                let error = error.to_string();
                self.log(
                    Level::Error,
                    "cli profile not moved",
                    &[("error", Field::Text(&error))],
                );
                false
            }
        }
    }

    /// The CLI switching switch: on appends the `.zshrc` block and installs
    /// the shim; off removes them and the pointer, or lists the lines of an
    /// edited block for the user to remove.
    pub(super) fn set_cli_switching(&mut self, on: bool) {
        if on && self.safety.is_paused(SafetyFeature::CliSwitching) {
            self.cli_note =
                Some("CLI switching is paused · see Diagnostics for why and to resume it".into());
            return;
        }
        let now = self.env.clock.now();
        let result = if on {
            cli_switcher::turn_on(&self.paths, &self.env.home, now).map(|()| TurnedOff::Clean)
        } else {
            cli_switcher::turn_off(&self.paths, &self.env.home, now)
        };
        let state = if on { "on" } else { "off" };
        match result {
            Ok(TurnedOff::Clean) => {
                self.cli_note = None;
                self.log(
                    Level::Info,
                    "cli switching changed",
                    &[("state", Field::Word(state))],
                );
            }
            Ok(TurnedOff::EditedBlock(lines)) => {
                self.cli_note = Some(format!(
                    "Your AccRoster block in ~/.zshrc was edited, so it was left in place. Remove these lines yourself:\n{}",
                    lines.join("\n")
                ));
                self.log(
                    Level::Info,
                    "cli switching changed",
                    &[
                        ("state", Field::Word(state)),
                        ("zshrc", Field::Word("edited")),
                    ],
                );
            }
            Err(error) => {
                self.cli_note = Some(format!("Couldn't turn CLI switching {state}: {error}"));
                let error = error.to_string();
                self.log(
                    Level::Error,
                    "cli switching not changed",
                    &[
                        ("state", Field::Word(state)),
                        ("error", Field::Text(&error)),
                    ],
                );
            }
        }
    }

    /// Makes Profile `id` the CLI's Active Profile by rewriting only the
    /// pointer. Ignored while CLI switching is off.
    pub(super) fn use_for_new_cli_runs(&mut self, id: &str) {
        if !cli_switcher::is_on(&self.paths) {
            return;
        }
        // The shared settings may have changed since the menu opened.
        if self.safety.check_settings_env(&self.env, true) {
            self.settle();
        }
        if self.safety.is_paused(SafetyFeature::CliSwitching) {
            self.log(Level::Info, "cli profile not chosen: paused", &[]);
            return;
        }
        let Some(config_dir) = self.profiles.config_dir(id).map(Path::to_path_buf) else {
            return;
        };
        match cli_switcher::set_pointer(&self.paths, &config_dir) {
            Ok(()) => self.log(
                Level::Info,
                "cli profile chosen",
                &[("profile", Field::Profile(id))],
            ),
            Err(error) => {
                let error = error.to_string();
                self.log(
                    Level::Error,
                    "cli profile not chosen",
                    &[
                        ("profile", Field::Profile(id)),
                        ("error", Field::Text(&error)),
                    ],
                );
            }
        }
    }
}
