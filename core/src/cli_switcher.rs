//! The CLI switcher (ADR 0002): the `claude` shim in the app's `bin/`, put
//! first on `PATH` by a marked block at the end of `~/.zshrc`, and the
//! pointer `bin/cli-active-profile` naming the CLI's Active Profile folder.
//! A switch rewrites only the pointer; running Sessions read their
//! environment once at launch, so no switch reaches them.
//!
//! What makes a Profile folder usable by that `claude` (its links back to
//! `~/.claude` and its `.claude.json` seed) is [`crate::profile_folder`].

use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use jiff::Timestamp;

use crate::backups;
use crate::interface::{Badge, Command, MenuItem};
use crate::profiles::Listed;
use crate::shell;
use crate::storage::{self, AppPaths};

/// The variables the shim unsets on every launch, even when an inherited
/// `CLAUDE_CONFIG_DIR` wins: each would put another identity or credential
/// under the Profile's name (ADR 0002).
const CLEARED_VARS: [&str; 9] = [
    "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    "CLAUDE_CODE_ACCOUNT_UUID",
    "CLAUDE_CODE_USER_EMAIL",
    "CLAUDE_CODE_ORGANIZATION_UUID",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    "CLAUDE_CODE_OAUTH_SCOPES",
    "CLAUDE_CODE_SUBSCRIPTION_TYPE",
    "CLAUDE_CODE_RATE_LIMIT_TIER",
];

const SHIM: &str = "claude";
const POINTER: &str = "cli-active-profile";
const BLOCK_START: &str = "# >>> accroster >>>";
const BLOCK_END: &str = "# <<< accroster <<<";

/// Whether CLI switching is on: the shim is installed.
pub(crate) fn is_on(paths: &AppPaths) -> bool {
    paths.bin().join(SHIM).is_file()
}

/// The `PATH` line of the `.zshrc` block, exactly as written.
pub(crate) fn path_line(paths: &AppPaths) -> String {
    format!("export PATH={}:\"$PATH\"", shell::quote_path(&paths.bin()))
}

/// The line "Copy shell override" copies: one terminal on `config_dir`
/// without changing the CLI's Active Profile.
pub(crate) fn shell_override(config_dir: &Path) -> String {
    format!("export CLAUDE_CONFIG_DIR={}", shell::quote_path(config_dir))
}

/// The folder the pointer names, byte for byte, or `None` without a pointer.
pub(crate) fn pointer(paths: &AppPaths) -> Option<PathBuf> {
    fs::read(paths.bin().join(POINTER))
        .ok()
        .map(|bytes| PathBuf::from(OsString::from_vec(bytes)))
}

/// Makes `config_dir` the CLI's Active Profile: rewrites the pointer, and
/// nothing else, atomically, with the absolute path verbatim.
pub(crate) fn set_pointer(paths: &AppPaths, config_dir: &Path) -> io::Result<()> {
    storage::write_atomically(
        &paths.bin().join(POINTER),
        config_dir.as_os_str().as_bytes(),
    )
}

/// Turns the CLI's Active Profile's "Remove…" into a submenu asking which
/// Profile new CLI runs use next: each other signed-in Profile, or Default.
fn ask_next_cli_on_removal(listed: &mut [Listed<'_>], active: usize) {
    let id = listed[active].id.to_owned();
    let mut choices: Vec<MenuItem> = listed
        .iter()
        .enumerate()
        .filter(|(index, profile)| *index != active && profile.signed_in)
        .map(|(_, profile)| MenuItem::Action {
            title: profile.row.title.clone(),
            command: Command::AskRemoveProfileWithNextCli {
                id: id.clone(),
                next: Some(profile.id.to_owned()),
            },
        })
        .collect();
    choices.insert(0, MenuItem::Detail("Then use for new CLI runs:".into()));
    choices.push(MenuItem::Action {
        title: "Default (~/.claude)".into(),
        command: Command::AskRemoveProfileWithNextCli {
            id: id.clone(),
            next: None,
        },
    });
    for item in &mut listed[active].row.submenu {
        if matches!(
            item,
            MenuItem::Action {
                command: Command::AskRemoveProfile(_),
                ..
            }
        ) {
            *item = MenuItem::Submenu {
                title: "Remove…".into(),
                items: choices,
            };
            return;
        }
    }
}

/// Back to Default: deletes the pointer, so new CLI runs use `~/.claude`.
pub(crate) fn clear_pointer(paths: &AppPaths) -> io::Result<()> {
    remove_if_present(&paths.bin().join(POINTER))
}

/// Deletes a file; one already gone is done.
fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// What CLI switching adds to the menu.
pub(crate) struct CliMenu {
    /// The menu bar label (`Perso 62%`), when CLI switching is on.
    pub label: Option<String>,
    /// The `CLI: Perso` part of the menu header, when CLI switching is on.
    pub header: Option<String>,
}

/// What stops the CLI's Active Profile from changing for now.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hold {
    Free,
    /// The self-check's `auth status` answers are still coming in.
    Checking,
    /// A Safety pause.
    Paused,
}

/// Adds CLI switching to the listed Profiles' rows (the `CLI` badge, the row
/// click, "Use for new CLI runs" and "Copy shell override"), and says what
/// the menu bar and the header show. Under a `hold`, rows don't switch and
/// the menu bar shows `?`; while checking, the header says so.
pub(crate) fn present(paths: &AppPaths, listed: &mut [Listed<'_>], hold: Hold) -> CliMenu {
    let paused = hold != Hold::Free;
    let on = is_on(paths);
    let pointer = if on { pointer(paths) } else { None };
    let mut active = None;
    for profile in listed.iter_mut() {
        let is_active = pointer.as_deref() == Some(profile.config_dir);
        let row = &mut profile.row;
        let mut items = Vec::new();
        // A signed-out Profile offers nothing to run `claude` with.
        if on && !paused && profile.signed_in {
            let command = Command::UseForNewCliRuns(profile.id.to_owned());
            row.command = Some(command.clone());
            items.push(MenuItem::Check {
                title: "Use for new CLI runs".into(),
                checked: is_active,
                command,
            });
        }
        if profile.signed_in {
            items.push(MenuItem::Action {
                title: "Copy shell override".into(),
                command: Command::CopyShellOverride(profile.id.to_owned()),
            });
        }
        row.submenu.splice(0..0, items);
        if is_active {
            row.badges.insert(0, Badge::Cli);
            let tightest = row.tightest_limit();
            active = Some((profile.short_label.clone(), tightest));
        }
    }
    if let Some(index) = listed
        .iter()
        .position(|profile| pointer.as_deref() == Some(profile.config_dir))
    {
        ask_next_cli_on_removal(listed, index);
    }
    if !on {
        return CliMenu {
            label: None,
            header: None,
        };
    }
    let (name, label) = match (&pointer, active) {
        (None, _) => ("Default".to_owned(), "Default".to_owned()),
        (Some(_), Some((name, Some(tightest)))) => (name.clone(), format!("{name} {tightest}%")),
        (Some(_), Some((name, None))) => (name.clone(), name),
        // The pointer names a folder no listed Profile has.
        (Some(_), None) => ("unknown Profile".to_owned(), "?".to_owned()),
    };
    let label = if paused { "?".to_owned() } else { label };
    let checking = if hold == Hold::Checking {
        " · checking…"
    } else {
        ""
    };
    CliMenu {
        label: Some(label),
        header: Some(format!("CLI: {name}{checking}")),
    }
}

/// Turns CLI switching on: backs up `~/.zshrc`, appends the marked block at
/// its end (so it comes after any `~/.local/bin` prepend), then installs the
/// shim. A `.zshrc` that already has a block keeps it. A `.zshrc` that is a
/// link is written through the link. `~/.local/bin/claude` is never touched.
pub(crate) fn turn_on(paths: &AppPaths, home: &Path, now: Timestamp) -> io::Result<()> {
    let zshrc = home.join(".zshrc");
    let contents = read_text(&zshrc)?;
    let has_block = contents
        .as_deref()
        .is_some_and(|text| text.contains(BLOCK_START));
    if !has_block {
        let mut updated = contents.clone().unwrap_or_default();
        if !updated.is_empty() {
            if !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push('\n');
        }
        updated.push_str(&block(paths));
        if contents.is_some() {
            backups::back_up(&paths.backups(), &zshrc, now)?;
        }
        storage::write_atomically(&write_target(&zshrc)?, updated.as_bytes())?;
    }
    install_shim(paths)
}

/// What turning CLI switching off left for the user to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnedOff {
    Clean,
    /// The block was edited, so it was left in `.zshrc`: these are its lines.
    EditedBlock(Vec<String>),
}

/// Turns CLI switching off: removes the block from `~/.zshrc` byte-exact
/// (after a backup), or reports its lines if it was edited; then removes the
/// shim and the pointer. Profiles stay.
pub(crate) fn turn_off(paths: &AppPaths, home: &Path, now: Timestamp) -> io::Result<TurnedOff> {
    let zshrc = home.join(".zshrc");
    let mut outcome = TurnedOff::Clean;
    if let Some(contents) = read_text(&zshrc)? {
        let block = block(paths);
        if let Some(at) = contents.find(&block) {
            // Also drop the blank line turning on put before the block.
            let start = if contents[..at].ends_with("\n\n") {
                at - 1
            } else {
                at
            };
            let updated = format!("{}{}", &contents[..start], &contents[at + block.len()..]);
            backups::back_up(&paths.backups(), &zshrc, now)?;
            storage::write_atomically(&write_target(&zshrc)?, updated.as_bytes())?;
        } else if let Some(lines) = edited_block(&contents) {
            outcome = TurnedOff::EditedBlock(lines);
        }
    }
    remove_if_present(&paths.bin().join(SHIM))?;
    clear_pointer(paths)?;
    Ok(outcome)
}

/// Rewrites the shim if it is installed but differs from this version's.
/// Returns whether it was rewritten.
pub(crate) fn heal_shim(paths: &AppPaths) -> io::Result<bool> {
    if !is_on(paths) || fs::read(paths.bin().join(SHIM))? == shim_script(paths).as_bytes() {
        return Ok(false);
    }
    install_shim(paths)?;
    Ok(true)
}

fn install_shim(paths: &AppPaths) -> io::Result<()> {
    let shim = paths.bin().join(SHIM);
    storage::write_atomically(&shim, shim_script(paths).as_bytes())?;
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))
}

/// The marked block, with its trailing newline.
fn block(paths: &AppPaths) -> String {
    format!(
        "{BLOCK_START}\n\
         # AccRoster's claude shim for CLI switching. Turn CLI switching off in AccRoster to remove this block.\n\
         {}\n\
         {BLOCK_END}\n",
        path_line(paths)
    )
}

/// The lines from the first start marker to the next end marker, when both
/// are there.
fn edited_block(contents: &str) -> Option<Vec<String>> {
    let lines: Vec<&str> = contents.lines().collect();
    let start = lines.iter().position(|line| line.trim() == BLOCK_START)?;
    let end = start
        + lines[start..]
            .iter()
            .position(|line| line.trim() == BLOCK_END)?;
    Some(
        lines[start..=end]
            .iter()
            .map(|line| (*line).to_owned())
            .collect(),
    )
}

/// The `claude` shim: a POSIX `sh` script (ADR 0002).
///
/// 1. Unsets [`CLEARED_VARS`], silently, on every launch.
/// 2. Keeps an inherited, non-empty `CLAUDE_CONFIG_DIR` (the per-shell
///    override, and what keeps a `claude` started inside a Session on that
///    Session's Profile). An empty one counts as unset.
/// 3. Otherwise reads the pointer: no pointer passes through untouched; a
///    pointer naming a missing folder is an error, never `~/.claude`.
/// 4. Walks `PATH`, skipping its own folder and empty entries, and `exec`s
///    the first other `claude`.
fn shim_script(paths: &AppPaths) -> String {
    let bin = shell::quote_path(&paths.bin());
    let cleared = CLEARED_VARS.join(" ");
    format!(
        r#"#!/bin/sh
# AccRoster's claude shim (CLI switching). AccRoster rewrites this file;
# turn CLI switching off in AccRoster to remove it. To bypass it, run the
# real claude directly (for example ~/.local/bin/claude).
unset {cleared}
bin={bin}
if [ -z "${{CLAUDE_CONFIG_DIR:-}}" ] && [ -f "$bin/{POINTER}" ]; then
  dir=$(cat "$bin/{POINTER}")
  if [ ! -d "$dir" ]; then
    echo "accroster: the CLI's Active Profile folder is missing: $dir" >&2
    echo "accroster: choose another Profile in AccRoster, or run the real claude directly." >&2
    exit 1
  fi
  CLAUDE_CONFIG_DIR=$dir
  export CLAUDE_CONFIG_DIR
fi
set -f
IFS=:
for d in $PATH; do
  case "$d" in
    ''|"$bin"|"$bin/") continue ;;
  esac
  if [ -f "$d/claude" ] && [ -x "$d/claude" ]; then
    unset IFS
    exec "$d/claude" "$@"
  fi
done
echo "accroster: no claude found on PATH besides AccRoster's shim." >&2
exit 127
"#
    )
}

/// A text file's contents, or `None` if it doesn't exist.
fn read_text(path: &Path) -> io::Result<Option<String>> {
    match fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| io::Error::other(format!("{} is not UTF-8", path.display()))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Where writing `path` lands: through a link to its target, so a `.zshrc`
/// kept in a dotfiles folder stays linked.
fn write_target(path: &Path) -> io::Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => fs::canonicalize(path),
        _ => Ok(path.to_path_buf()),
    }
}
