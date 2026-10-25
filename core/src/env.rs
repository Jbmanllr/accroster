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
//! - [`ProcessSpawner`]: starts an external program that the core polls on
//!   later ticks and kills at its deadline; nothing waits on one. The only spawned
//!   program is Claude Code (`claude`), resolved through the `PATH` of the
//!   request's environment; tests put a stub `claude` first on that `PATH`.
//! - [`Clock`]: the current time.
//! - [`LoginItem`]: the app's launch-at-login registration
//!   (`SMAppService.mainApp`), which the user can also change in System
//!   Settings → Login Items.
//! - [`OsFacts`]: facts about the rest of the Mac that are queried, not
//!   pushed: process liveness and parentage, the desktop app's version and
//!   its running Claude Code children.
//!   Facts the OS pushes (sleep/wake, screen lock, network path changes, the
//!   desktop app launching or quitting) arrive as [`crate::Event`]s instead.
//!
//! Production uses [`Environment::system`]; tests use the fake Mac in the
//! `accroster-fake-mac` crate. Both go through the same [`SystemSpawner`], so
//! the code that starts real processes is exercised by the tests too.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};

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
    pub login_item: Box<dyn LoginItem>,
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
            login_item: Box::new(SystemLoginItem),
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

/// Runs external programs.
pub trait ProcessSpawner: Send {
    /// Starts the program and returns at once. Nothing on the core thread
    /// waits for a program: the core polls it on later ticks and kills it
    /// at its deadline. The child's stdin receives `request.stdin`, if any,
    /// and then stays open until it is closed, the child exits or is killed.
    /// `Err` means it could not be started (for example, not found on
    /// `PATH`).
    fn start(&self, request: &SpawnRequest) -> io::Result<Box<dyn RunningProcess>>;
}

/// A program started by [`ProcessSpawner::start`]. Dropping it kills the
/// program if it is still running.
pub trait RunningProcess: Send {
    /// `Some` once the program has exited, with everything it printed.
    fn try_wait(&mut self) -> io::Result<Option<SpawnOutput>>;
    /// Kills the program (if still running) and waits for it to go.
    fn kill(&mut self) -> io::Result<SpawnOutput>;
    /// Closes the program's stdin (EOF), for a program that should finish
    /// once it has read its input.
    fn close_stdin(&mut self);
}

/// The current time.
pub trait Clock: Send {
    fn now(&self) -> Timestamp;

    /// The Mac's own time zone: dates and times are shown in it, and local
    /// times other programs write (Claude Code's `procStart`) are read in it.
    fn time_zone(&self) -> jiff::tz::TimeZone {
        jiff::tz::TimeZone::system()
    }
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
    /// The bytes of Claude Code's managed settings file
    /// (`/Library/Application Support/ClaudeCode/managed-settings.json`), or
    /// `None` when there is none or it can't be read.
    fn managed_settings(&self) -> Option<Vec<u8>>;
    /// The pids of running Claude Code processes bundled with the desktop app
    /// (`…/Application Support/Claude/claude-code/<ver>/…/claude`): its
    /// Sessions' children.
    fn desktop_claude_code_children(&self) -> Vec<u32>;
    /// The name of the terminal device process `pid` runs on (`ttys004`),
    /// or `None` when it has none or isn't alive.
    fn terminal_of(&self, _pid: u32) -> Option<String> {
        None
    }
    /// Whether the Claude desktop app (`/Applications/Claude.app`) is
    /// running. When the process table can't be read, it counts as running:
    /// that is the case in which fewer files get written.
    fn desktop_app_running(&self) -> bool;
}

/// Where Claude Code reads managed (administrator) settings.
const MANAGED_SETTINGS: &str = "/Library/Application Support/ClaudeCode/managed-settings.json";

/// Starts real processes with [`std::process::Command`].
pub struct SystemSpawner;

impl ProcessSpawner for SystemSpawner {
    fn start(&self, request: &SpawnRequest) -> io::Result<Box<dyn RunningProcess>> {
        let mut command = command_for(request)?;
        command.stdin(Stdio::piped());
        let mut child = command.spawn()?;
        let mut stdin = child.stdin.take().expect("stdin was piped");
        if let Some(input) = &request.stdin {
            match stdin.write_all(input) {
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
                other => other?,
            }
        }
        // Drain stdout and stderr on their own threads, so a chatty child
        // never blocks on a full pipe while nobody is waiting on it.
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());
        Ok(Box::new(SystemProcess {
            child,
            stdin: Some(stdin),
            stdout: Some(stdout),
            stderr: Some(stderr),
        }))
    }
}

/// What every spawn shares: the program, its args, exactly the request's
/// environment, its cwd, and piped stdout and stderr.
fn command_for(request: &SpawnRequest) -> io::Result<Command> {
    let program = resolve_program(&request.program, &request.env)?;
    let mut command = Command::new(program);
    command
        .args(&request.args)
        .env_clear()
        .envs(request.env.iter().map(|(name, value)| (name, value)))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = &request.cwd {
        command.current_dir(cwd);
    }
    Ok(command)
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

/// A child started by [`SystemSpawner::start`].
struct SystemProcess {
    child: Child,
    /// Held open until the child exits.
    stdin: Option<ChildStdin>,
    /// `None` once the child has been reaped.
    stdout: Option<JoinHandle<Vec<u8>>>,
    stderr: Option<JoinHandle<Vec<u8>>>,
}

impl SystemProcess {
    fn finish(&mut self, status: ExitStatus) -> SpawnOutput {
        self.stdin = None;
        let collect = |handle: Option<JoinHandle<Vec<u8>>>| {
            handle
                .map(|handle| handle.join().unwrap_or_default())
                .unwrap_or_default()
        };
        SpawnOutput {
            exit_code: status.code(),
            stdout: collect(self.stdout.take()),
            stderr: collect(self.stderr.take()),
        }
    }
}

impl RunningProcess for SystemProcess {
    fn try_wait(&mut self) -> io::Result<Option<SpawnOutput>> {
        Ok(self.child.try_wait()?.map(|status| self.finish(status)))
    }

    fn kill(&mut self) -> io::Result<SpawnOutput> {
        // `Child::kill` on an already-reaped child is a no-op `Ok`.
        self.child.kill()?;
        let status = self.child.wait()?;
        Ok(self.finish(status))
    }

    fn close_stdin(&mut self) {
        self.stdin = None;
    }
}

impl Drop for SystemProcess {
    fn drop(&mut self) {
        if self.stdout.is_some() {
            let _ = self.kill();
        }
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

    /// Read in-process from the bundle's `Info.plist`; nothing is spawned.
    fn desktop_app_version(&self) -> Option<String> {
        use objc2_foundation::{NSBundle, NSString};
        let bundle = NSBundle::bundleWithPath(&NSString::from_str("/Applications/Claude.app"))?;
        let value =
            bundle.objectForInfoDictionaryKey(&NSString::from_str("CFBundleShortVersionString"))?;
        let version = value.downcast::<NSString>().ok()?.to_string();
        let version = version.trim();
        (!version.is_empty()).then(|| version.to_owned())
    }

    fn managed_settings(&self) -> Option<Vec<u8>> {
        std::fs::read(MANAGED_SETTINGS).ok()
    }

    fn terminal_of(&self, pid: u32) -> Option<String> {
        let pid = i32::try_from(pid).ok()?;
        // SAFETY: as in `process`: plain data, at most `size` bytes written.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let written = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
        };
        // No controlling terminal: `e_tdev` is NODEV (all bits set).
        if written != size || info.e_tdev == u32::MAX {
            return None;
        }
        // SAFETY: `devname` returns a pointer to a static buffer, or null.
        let name = unsafe { libc::devname(info.e_tdev as libc::dev_t, libc::S_IFCHR) };
        if name.is_null() {
            return None;
        }
        let name = unsafe { std::ffi::CStr::from_ptr(name) };
        let name = name.to_str().ok()?;
        (!name.is_empty() && name != "??").then(|| name.to_owned())
    }

    fn desktop_claude_code_children(&self) -> Vec<u32> {
        const BUNDLED: &str = "/Library/Application Support/Claude/claude-code/";
        let Some(processes) = process_paths() else {
            return Vec::new();
        };
        processes
            .into_iter()
            .filter(|(_, path)| path.contains(BUNDLED) && path.ends_with("/claude"))
            .map(|(pid, _)| pid)
            .collect()
    }

    fn desktop_app_running(&self) -> bool {
        const DESKTOP_APP: &str = "/Applications/Claude.app/Contents/MacOS/Claude";
        process_paths()
            .is_none_or(|processes| processes.iter().any(|(_, path)| path == DESKTOP_APP))
    }
}

/// Every live process with its executable's path, or `None` when the process
/// table can't be read.
fn process_paths() -> Option<Vec<(u32, String)>> {
    // SAFETY: with a null buffer `proc_listallpids` returns the number of
    // pids; with a buffer it writes at most `size` bytes of pids.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let count = usize::try_from(count).ok().filter(|count| *count > 0)?;
    // Room for processes started in between.
    let mut pids = vec![0 as libc::pid_t; count + 64];
    let size = i32::try_from(pids.len() * std::mem::size_of::<libc::pid_t>()).unwrap_or(0);
    let written = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), size) };
    pids.truncate(usize::try_from(written).ok().filter(|n| *n > 0)?);
    let processes = pids
        .into_iter()
        .filter(|&pid| pid > 0)
        .filter_map(|pid| {
            let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
            // SAFETY: `proc_pidpath` writes at most `buffer.len()` bytes and
            // returns the path's length.
            let len =
                unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
            let len = usize::try_from(len).ok()?;
            let path = String::from_utf8_lossy(&buffer[..len.min(buffer.len())]).into_owned();
            Some((u32::try_from(pid).ok()?, path))
        })
        .collect();
    Some(processes)
}

/// Where the app stands in System Settings → Login Items, as
/// `SMAppService.status` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginItemStatus {
    /// Not registered: the app doesn't open at login.
    NotRegistered,
    /// Registered and allowed: the app opens at login.
    Enabled,
    /// Registered, but the user turned it off in System Settings → Login
    /// Items. Only the user can turn it back on, there.
    RequiresApproval,
    /// macOS can't find the app as a login item (for example, it isn't
    /// running from its `.app` bundle).
    NotFound,
}

/// The app's own launch-at-login registration.
pub trait LoginItem: Send {
    fn status(&self) -> LoginItemStatus;
    /// Registers the app to open at login. `Err` carries macOS's reason.
    fn register(&self) -> Result<(), String>;
    /// Stops the app opening at login. `Err` carries macOS's reason.
    fn unregister(&self) -> Result<(), String>;
}

/// `SMAppService.mainApp`.
pub struct SystemLoginItem;

impl LoginItem for SystemLoginItem {
    fn status(&self) -> LoginItemStatus {
        use objc2_service_management::{SMAppService, SMAppServiceStatus};
        // SAFETY: `mainAppService` and `status` take no arguments; SMAppService
        // has no main-thread requirement.
        let status = unsafe { SMAppService::mainAppService().status() };
        match status {
            SMAppServiceStatus::Enabled => LoginItemStatus::Enabled,
            SMAppServiceStatus::RequiresApproval => LoginItemStatus::RequiresApproval,
            SMAppServiceStatus::NotFound => LoginItemStatus::NotFound,
            _ => LoginItemStatus::NotRegistered,
        }
    }

    fn register(&self) -> Result<(), String> {
        use objc2_service_management::SMAppService;
        // SAFETY: as in `status`; objc2 handles the error out-parameter.
        unsafe { SMAppService::mainAppService().registerAndReturnError() }
            .map_err(|error| error.localizedDescription().to_string())
    }

    fn unregister(&self) -> Result<(), String> {
        use objc2_service_management::SMAppService;
        // SAFETY: as in `register`.
        unsafe { SMAppService::mainAppService().unregisterAndReturnError() }
            .map_err(|error| error.localizedDescription().to_string())
    }
}
