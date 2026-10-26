//! Finding Claude Code (`claude`), the one program the app runs.
//!
//! An app opened from Finder or the Dock inherits launchd's minimal `PATH`
//! (`/usr/bin:/bin:/usr/sbin:/sbin`), which has no `claude`. So when the
//! inherited `PATH` has none, these are tried in order:
//!
//! 1. `~/.local/bin`, `~/.claude/local`, then the system's program folders
//!    ([`crate::OsFacts::program_folders`]: `/opt/homebrew/bin`,
//!    `/usr/local/bin`);
//! 2. where `claude` was last found (`state/claude-path`), so one launch
//!    from a terminal is enough;
//! 3. the user's login-shell `PATH`, asked once per run
//!    (`$SHELL -l -c 'printf …"$PATH"'`), without waiting on it.
//!
//! A candidate is never the desktop app's bundled copy, nor AccRoster's own
//! shim (whatever links to it: its folder is resolved and skipped, as the
//! shim skips itself). Spawns run the found file and get its folder first on
//! their `PATH`, so Claude Code finds `node`, `git` and the rest next to it.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use crate::env::Environment;
use crate::logging::{Field, Level, Logger};
use crate::runner::{Finished, Job, Runner};
use crate::storage::{self, AppPaths};

/// What the menu says while no `claude` can be found.
pub(crate) const MISSING: &str =
    "Can't find Claude Code (claude). Install it, or open AccRoster from a terminal once.";

/// Marks the login shell's `PATH` line in whatever its startup files print.
pub(crate) const PATH_MARKER: &str = "accroster-login-path=";

/// Where the found `claude` is, or why there is none yet.
#[derive(Default)]
pub(crate) struct ClaudeLocator {
    found: Option<PathBuf>,
    shell: ShellLookup,
    /// "claude not found" was logged (once per stretch without one).
    missing_logged: bool,
}

#[derive(Default)]
enum ShellLookup {
    #[default]
    NotAsked,
    Asking(Job),
    /// The login shell's `PATH`, if it gave one.
    Answered(Option<OsString>),
}

/// Where a `claude` was found, for the log.
#[derive(Clone, Copy)]
enum Source {
    Path,
    UsualFolder,
    Remembered,
    LoginShell,
}

impl Source {
    fn word(self) -> &'static str {
        match self {
            Source::Path => "path",
            Source::UsualFolder => "usual_folder",
            Source::Remembered => "remembered",
            Source::LoginShell => "login_shell",
        }
    }
}

impl ClaudeLocator {
    /// The `claude` spawns run, once found.
    pub(crate) fn program(&self) -> Option<&Path> {
        self.found.as_deref()
    }

    /// Still asking the login shell: the core checks back soon.
    pub(crate) fn looking(&self) -> bool {
        self.found.is_none() && matches!(self.shell, ShellLookup::Asking(_))
    }

    /// Every place was searched and none has a `claude`.
    pub(crate) fn missing(&self) -> bool {
        self.found.is_none() && matches!(self.shell, ShellLookup::Answered(_))
    }

    /// Looks again, cheaply: keeps a found `claude` while it is still there,
    /// else searches every place, asking the login shell the first time.
    pub(crate) fn update(
        &mut self,
        env: &Environment,
        paths: &AppPaths,
        logger: &Logger,
        verbose: bool,
    ) {
        if self.found.as_deref().is_some_and(is_executable) {
            return;
        }
        let log = |level, message, fields: &[(&'static str, Field<'_>)]| {
            logger.write(env.clock.now(), verbose, level, message, fields);
        };
        if let ShellLookup::Asking(job) = &mut self.shell {
            let answer = match job.poll(env.clock.now()) {
                None => None,
                Some(Finished::Exited(output)) => Some(login_path(&output.stdout)),
                Some(finished) => {
                    log(
                        Level::Info,
                        "login shell gave no PATH",
                        &[("how", Field::Word(finished.word()))],
                    );
                    Some(None)
                }
            };
            if let Some(answer) = answer {
                self.shell = ShellLookup::Answered(answer);
            }
        }
        let found = search(env, paths, &self.shell);
        if found.is_none() && matches!(self.shell, ShellLookup::NotAsked) {
            self.shell = match Runner::new(env, paths, None).start_login_shell_path() {
                Ok(job) => ShellLookup::Asking(job),
                Err(error) => {
                    let error = error.to_string();
                    log(
                        Level::Info,
                        "login shell not asked",
                        &[("error", Field::Text(&error))],
                    );
                    ShellLookup::Answered(None)
                }
            };
        }
        match found {
            Some((program, source)) => {
                log(
                    Level::Info,
                    "claude found",
                    &[
                        ("path", Field::Path(&program)),
                        ("source", Field::Word(source.word())),
                    ],
                );
                remember(paths, &program);
                self.found = Some(program);
                self.missing_logged = false;
            }
            None => {
                self.found = None;
                if self.missing() && !self.missing_logged {
                    log(Level::Warn, "claude not found", &[]);
                    self.missing_logged = true;
                }
            }
        }
    }
}

/// The first acceptable `claude`, in the order the module documents.
fn search(env: &Environment, paths: &AppPaths, shell: &ShellLookup) -> Option<(PathBuf, Source)> {
    let skip = Skip::new(paths);
    let in_folders = |folders: &mut dyn Iterator<Item = PathBuf>| {
        folders
            .map(|folder| folder.join("claude"))
            .find(|candidate| skip.accepts(candidate))
    };
    let inherited = env
        .inherited_vars
        .iter()
        .rev()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| value.clone());
    if let Some(found) = inherited.and_then(|path| in_folders(&mut std::env::split_paths(&path))) {
        return Some((found, Source::Path));
    }
    let mut usual = [".local/bin", ".claude/local"]
        .into_iter()
        .map(|folder| env.home.join(folder))
        .chain(env.os.program_folders());
    if let Some(found) = in_folders(&mut usual) {
        return Some((found, Source::UsualFolder));
    }
    if let Some(found) = remembered(paths).filter(|path| skip.accepts(path)) {
        return Some((found, Source::Remembered));
    }
    if let ShellLookup::Answered(Some(path)) = shell
        && let Some(found) = in_folders(&mut std::env::split_paths(path))
    {
        return Some((found, Source::LoginShell));
    }
    None
}

/// What no candidate may be.
struct Skip {
    /// AccRoster's shim folder, resolved.
    shim_folder: Option<PathBuf>,
    shim_folder_as_named: PathBuf,
}

impl Skip {
    fn new(paths: &AppPaths) -> Skip {
        Skip {
            shim_folder: fs::canonicalize(paths.bin()).ok(),
            shim_folder_as_named: paths.bin(),
        }
    }

    fn accepts(&self, candidate: &Path) -> bool {
        if !candidate.is_absolute() || !is_executable(candidate) {
            return false;
        }
        if candidate.parent() == Some(self.shim_folder_as_named.as_path()) {
            return false;
        }
        let Ok(resolved) = fs::canonicalize(candidate) else {
            return false;
        };
        if resolved.parent().is_some_and(|folder| {
            Some(folder) == self.shim_folder.as_deref() || folder == self.shim_folder_as_named
        }) {
            return false;
        }
        !is_bundled(&resolved)
    }
}

/// The desktop app's own Claude Code (its Sessions' children), never the
/// Terminal's.
fn is_bundled(resolved: &Path) -> bool {
    let text = resolved.to_string_lossy();
    text.contains("/Library/Application Support/Claude/claude-code/")
        || text.starts_with("/Applications/Claude.app/")
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The `PATH` in the login shell's output: the marked line, whatever its
/// startup files printed around it.
fn login_path(stdout: &[u8]) -> Option<OsString> {
    use std::os::unix::ffi::OsStrExt;
    stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| line.strip_prefix(PATH_MARKER.as_bytes()))
        .next_back()
        .filter(|path| !path.is_empty())
        .map(|path| OsStr::from_bytes(path).to_owned())
}

fn remembered_file(paths: &AppPaths) -> PathBuf {
    paths.state().join("claude-path")
}

fn remembered(paths: &AppPaths) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let bytes = fs::read(remembered_file(paths)).ok()?;
    Some(PathBuf::from(OsString::from_vec(bytes)))
}

fn remember(paths: &AppPaths, program: &Path) {
    use std::os::unix::ffi::OsStrExt;
    if remembered(paths).as_deref() != Some(program) {
        // Only a convenience for the next launch from Finder.
        let _ = storage::write_atomically(&remembered_file(paths), program.as_os_str().as_bytes());
    }
}
