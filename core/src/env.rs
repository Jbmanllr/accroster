//! The environment boundary: everything the core touches outside itself.
//!
//! The core never reads `$HOME`, the process environment, the system clock or
//! process tables directly. It receives one [`Environment`] at launch and goes
//! through it for every outside fact or action:
//!
//! - [`Environment::home`]: the home-folder root. Every path the core reads or
//!   writes (`~/.claude`, `~/.claude.json`, `~/.zshrc`, the desktop app's
//!   `~/Library/Application Support/Claude/`, and the app's own folder) is
//!   derived from it. Tests point it at a temporary folder.
//! - [`Environment::inherited_vars`]: the environment variables the app was
//!   started with. Spawns build their environment from these, so the core can
//!   decide what to clear (`CLAUDE*`, `ANTHROPIC_*`) and what to pass through.
//! - [`ProcessSpawner`]: runs an external program. The only spawned program is
//!   Claude Code (`claude`), resolved through the `PATH` of the request's
//!   environment; tests put a stub `claude` first on that `PATH`.
//! - [`Clock`]: the current time.
//! - [`OsFacts`]: facts about the rest of the Mac that are queried, not
//!   pushed: process liveness and parentage, the desktop app's version.
//!   Facts the OS pushes (sleep/wake, screen lock, network path changes, the
//!   desktop app launching or quitting) arrive as [`crate::Event`]s instead.
//!
//! Production uses [`Environment::system`]; tests use the fake Mac in the
//! `accroster-fake-mac` crate. Both go through the same [`SystemSpawner`], so
//! the code that starts real processes is exercised by the tests too.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use jiff::Timestamp;

/// Everything outside the core, handed to [`crate::Core::launch`].
pub struct Environment {
    /// The home-folder root (`~`).
    pub home: PathBuf,
    /// The variables the app process inherited, in their original order.
    pub inherited_vars: Vec<(OsString, OsString)>,
    pub spawner: Box<dyn ProcessSpawner>,
    pub clock: Box<dyn Clock>,
    pub os: Box<dyn OsFacts>,
}

impl Environment {
    /// The real Mac: `$HOME`, the process environment, real processes, the
    /// system clock and live process tables.
    pub fn system() -> io::Result<Environment> {
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
        Ok(Environment {
            home,
            inherited_vars: std::env::vars_os().collect(),
            spawner: Box::new(SystemSpawner),
            clock: Box::new(SystemClock),
            os: Box::new(SystemOsFacts),
        })
    }
}

/// One program run, fully specified. The spawner adds nothing: the child sees
/// exactly `env`, starts in `cwd` and reads exactly `stdin`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnRequest {
    /// A bare name (`claude`) is resolved through the `PATH` in `env`.
    pub program: OsString,
    pub args: Vec<OsString>,
    /// The child's complete environment.
    pub env: Vec<(OsString, OsString)>,
    pub cwd: Option<PathBuf>,
    /// Written to the child's stdin, which is then closed. `None` gives the
    /// child an empty stdin.
    pub stdin: Option<Vec<u8>>,
}

/// What a finished program left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnOutput {
    /// The exit code; `None` when the program was killed by a signal.
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl SpawnOutput {
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Runs external programs to completion.
pub trait ProcessSpawner: Send {
    /// Runs the program and waits for it. `Err` means it could not be started
    /// (for example, not found on `PATH`); a non-zero exit is an `Ok`.
    fn run(&self, request: &SpawnRequest) -> io::Result<SpawnOutput>;
}

/// The current time.
pub trait Clock: Send {
    fn now(&self) -> Timestamp;
}

/// A process as the OS reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessFacts {
    pub parent_pid: u32,
    /// When the process started, to tell a live process from a reused pid.
    pub started_at: Timestamp,
}

/// Facts about the rest of the Mac that the core asks for when it needs them.
pub trait OsFacts: Send {
    /// The process with this pid, or `None` if no such process is alive.
    fn process(&self, pid: u32) -> Option<ProcessFacts>;
    /// `CFBundleShortVersionString` of the installed Claude desktop app, or
    /// `None` when it isn't installed or can't be read.
    fn desktop_app_version(&self) -> Option<String>;
}

/// Starts real processes with [`std::process::Command`].
pub struct SystemSpawner;

impl ProcessSpawner for SystemSpawner {
    fn run(&self, request: &SpawnRequest) -> io::Result<SpawnOutput> {
        let program = resolve_program(&request.program, &request.env)?;
        let mut command = Command::new(program);
        command
            .args(&request.args)
            .env_clear()
            .envs(request.env.iter().map(|(name, value)| (name, value)))
            .stdin(if request.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &request.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn()?;
        if let Some(input) = &request.stdin {
            // Take stdin so it is closed (EOF) once written.
            let mut stdin = child.stdin.take().expect("stdin was piped");
            match stdin.write_all(input) {
                // A child that exits without reading its input is not an error.
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
                other => other?,
            }
        }
        let output = child.wait_with_output()?;
        Ok(SpawnOutput {
            exit_code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

/// `Command::new` searches the parent's `PATH`, not the child's. A bare
/// program name is resolved against the request's own `PATH` instead, so the
/// request alone decides which `claude` runs.
fn resolve_program(program: &std::ffi::OsStr, env: &[(OsString, OsString)]) -> io::Result<PathBuf> {
    let as_path = Path::new(program);
    if as_path.components().count() > 1 {
        return Ok(as_path.to_path_buf());
    }
    let path_var = env
        .iter()
        .rev()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} not found on PATH", program.to_string_lossy()),
            )
        })
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The system clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// Live process tables and the installed desktop app.
pub struct SystemOsFacts;

impl OsFacts for SystemOsFacts {
    fn process(&self, pid: u32) -> Option<ProcessFacts> {
        let pid = i32::try_from(pid).ok()?;
        // SAFETY: `proc_bsdinfo` is plain data; `proc_pidinfo` writes at most
        // `size` bytes into it and returns how many it wrote.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let written = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
        };
        if written != size {
            return None;
        }
        let started_at = Timestamp::new(
            i64::try_from(info.pbi_start_tvsec).ok()?,
            i32::try_from(info.pbi_start_tvusec).ok()? * 1000,
        )
        .ok()?;
        Some(ProcessFacts {
            parent_pid: info.pbi_ppid,
            started_at,
        })
    }

    fn desktop_app_version(&self) -> Option<String> {
        let output = Command::new("/usr/bin/plutil")
            .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
            .arg("/Applications/Claude.app/Contents/Info.plist")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let version = String::from_utf8(output.stdout).ok()?;
        let version = version.trim();
        (!version.is_empty()).then(|| version.to_owned())
    }
}
