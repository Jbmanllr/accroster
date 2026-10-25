//! A fake Mac for driving `accroster-core` end to end through its interface.
//!
//! The spec's one test seam is the core's interface (commands and events in,
//! view models and effects out). A [`FakeMac`] supplies everything on the
//! other side of the core's [`Environment`] boundary:
//!
//! - a temporary **home folder** (its path contains spaces, like the real
//!   `Application Support`), with builders for fixture Claude files: `~/.claude`,
//!   `~/.claude.json`, Transcripts, desktop Sidebar entries, live-process files,
//!   the desktop's `config.json` and `~/.zshrc`;
//! - a **stub `claude`** first on the spawn `PATH`, answering from canned
//!   [`Reply`]s per [`StubCall`] kind and recording every call (args, env,
//!   stdin) — see [`StubClaude`];
//! - a **fake clock** ([`FakeClock`]) the test moves by hand;
//! - **fake OS facts** ([`FakeOs`]): live processes and the desktop app's version;
//! - a **fake Login Items entry** ([`FakeLoginItem`]) the user can turn off in
//!   "System Settings".
//!
//! A good test states a user-visible outcome: what the view shows, which
//! effects came out, which files the core wrote or left alone. Never assert on
//! the core's internal structure.
//!
//! ```no_run
//! use accroster_core::{Command, Effect};
//! use accroster_fake_mac::FakeMac;
//!
//! let mac = FakeMac::new();
//! let mut core = mac.launch();
//! assert_eq!(core.command(Command::OpenSettings), vec![Effect::ShowSettings]);
//! assert!(mac.app_support().join("settings.json").is_file());
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use accroster_core::{
    AppPaths, Clock, Core, Environment, LoginItem, LoginItemStatus, OsFacts, ProcessFacts,
    SystemSpawner,
};
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;

/// The default fake time: Sunday 25 October 2026, 14:02 UTC.
pub const START_TIME: &str = "2026-10-25T14:02:00Z";

/// A temporary Mac. Dropping it deletes everything it created.
pub struct FakeMac {
    _root: tempfile::TempDir,
    home: PathBuf,
    /// The stub `claude` on the spawn `PATH`.
    pub claude: StubClaude,
    pub clock: FakeClock,
    pub os: FakeOs,
    /// The app's entry in System Settings → Login Items.
    pub login_item: FakeLoginItem,
    extra_vars: Mutex<Vec<(OsString, OsString)>>,
}

impl Default for FakeMac {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeMac {
    /// An empty home folder, a stub `claude` with no replies yet, the clock at
    /// [`START_TIME`] and no live processes.
    pub fn new() -> FakeMac {
        let root = tempfile::Builder::new()
            .prefix("accroster fake mac ")
            .tempdir()
            .expect("create the fake Mac's folder");
        let home = root.path().join("Users").join("tester");
        fs::create_dir_all(&home).expect("create the fake home");
        let claude = StubClaude::install(&root.path().join("stub claude"));
        FakeMac {
            _root: root,
            home,
            claude,
            clock: FakeClock::at(START_TIME.parse().expect("valid start time")),
            os: FakeOs::default(),
            login_item: FakeLoginItem::default(),
            extra_vars: Mutex::new(Vec::new()),
        }
    }

    /// The environment boundary pointing at this Mac. Each call gives a fresh
    /// [`Environment`] sharing the same home, stub, clock and OS facts, so a
    /// test can relaunch the core to check what survives a restart.
    pub fn environment(&self) -> Environment {
        let mut inherited_vars: Vec<(OsString, OsString)> = vec![
            ("HOME".into(), self.home.clone().into()),
            ("PATH".into(), self.spawn_path()),
            ("USER".into(), "tester".into()),
            ("SHELL".into(), "/bin/zsh".into()),
        ];
        inherited_vars.extend(self.extra_vars.lock().unwrap().iter().cloned());
        Environment {
            home: self.home.clone(),
            inherited_vars,
            spawner: Box::new(SystemSpawner),
            clock: Box::new(self.clock.clone()),
            os: Box::new(self.os.clone()),
            login_item: Box::new(self.login_item.clone()),
        }
    }

    /// Launches the core on this Mac, panicking if launch fails.
    pub fn launch(&self) -> Core {
        Core::launch(self.environment()).expect("the core launches on the fake Mac")
    }

    /// Adds a variable the app process "inherited" (for example a token the
    /// desktop app injects), as seen by every later [`FakeMac::environment`].
    pub fn inherit_var(&self, name: &str, value: &str) {
        self.extra_vars
            .lock()
            .unwrap()
            .push((name.into(), value.into()));
    }

    /// The `PATH` spawns see: the stub first, then the system folders `sh`
    /// needs. Never the real `~/.local/bin`.
    pub fn spawn_path(&self) -> OsString {
        std::env::join_paths([
            self.claude.bin_dir(),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ])
        .expect("PATH entries contain no colon")
    }

    /// The fake `~`.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// `~/<relative>` on the fake Mac.
    pub fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.home.join(relative)
    }

    /// The app's own folder: `~/Library/Application Support/io.github.jbmanllr.accroster`.
    pub fn app_support(&self) -> PathBuf {
        AppPaths::new(&self.home).root().to_path_buf()
    }

    // ----- Generic fixture files -------------------------------------------------

    /// Writes `~/<relative>`, creating parent folders.
    pub fn write_file(&self, relative: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().expect("a file has a parent"))
            .expect("create fixture folders");
        fs::write(&path, contents).expect("write fixture file");
        path
    }

    /// Writes `~/<relative>` as pretty JSON.
    pub fn write_json(&self, relative: impl AsRef<Path>, value: &Value) -> PathBuf {
        self.write_file(
            relative,
            serde_json::to_vec_pretty(value).expect("serialize fixture"),
        )
    }

    /// Reads `~/<relative>` as text, or `None` if it doesn't exist.
    pub fn read_file(&self, relative: impl AsRef<Path>) -> Option<String> {
        fs::read_to_string(self.path(relative)).ok()
    }

    /// Reads `~/<relative>` as JSON, panicking if it isn't valid JSON.
    pub fn read_json(&self, relative: impl AsRef<Path>) -> Option<Value> {
        self.read_file(relative)
            .map(|text| serde_json::from_str(&text).expect("file is valid JSON"))
    }

    /// File names directly inside `~/<relative>`, sorted; empty if missing.
    pub fn list(&self, relative: impl AsRef<Path>) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.path(relative)) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .map(|entry| {
                entry
                    .expect("readable entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    /// Every file under the home folder with its contents, keyed by path
    /// relative to `~`. Compare two snapshots to prove what changed and what
    /// was left alone.
    pub fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, home: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries {
                let path = entry.expect("readable entry").path();
                let meta = fs::symlink_metadata(&path).expect("stat fixture");
                let relative = path.strip_prefix(home).expect("under home").to_path_buf();
                if meta.is_dir() {
                    walk(&path, home, out);
                } else if meta.file_type().is_symlink() {
                    let target = fs::read_link(&path).expect("read link");
                    out.insert(relative, format!("-> {}", target.display()).into_bytes());
                } else {
                    out.insert(relative, fs::read(&path).expect("read fixture"));
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.home, &self.home, &mut out);
        out
    }

    // ----- Claude fixture files --------------------------------------------------
    //
    // Locations follow the spec; field shapes follow the research digest.
    // Each builder takes the JSON body so tests state the fields they rely on.

    /// A default Claude Code install: `~/.claude/` with `settings.json`,
    /// `projects/`, `sessions/`, and `~/.claude.json` signed in as `login`.
    pub fn with_claude_home(&self, login: &FixtureLogin) -> &Self {
        self.write_json(".claude/settings.json", &serde_json::json!({}));
        fs::create_dir_all(self.path(".claude/projects")).expect("create projects");
        fs::create_dir_all(self.path(".claude/sessions")).expect("create sessions");
        self.write_json(".claude.json", &login.claude_json());
        self
    }

    /// A Transcript: `~/.claude/projects/<project_dir>/<session_id>.jsonl`, one
    /// JSON record per line.
    pub fn write_transcript(
        &self,
        project_dir: &str,
        session_id: &str,
        records: &[Value],
    ) -> PathBuf {
        let mut body = String::new();
        for record in records {
            body.push_str(&serde_json::to_string(record).expect("serialize record"));
            body.push('\n');
        }
        self.write_file(
            format!(".claude/projects/{project_dir}/{session_id}.jsonl"),
            body,
        )
    }

    /// A desktop Sidebar entry: `~/Library/Application Support/Claude/
    /// claude-code-sessions/<account>/<org>/local_<id>.json`.
    pub fn write_sidebar_entry(
        &self,
        account: &str,
        org: &str,
        local_id: &str,
        entry: &Value,
    ) -> PathBuf {
        self.write_json(
            format!("Library/Application Support/Claude/claude-code-sessions/{account}/{org}/local_{local_id}.json"),
            entry,
        )
    }

    /// A live-process file `<config_dir>/sessions/<pid>.json`, where
    /// `config_dir` is relative to `~` (`.claude` or a Profile folder).
    pub fn write_live_process(
        &self,
        config_dir: impl AsRef<Path>,
        pid: u32,
        body: &Value,
    ) -> PathBuf {
        self.write_json(
            config_dir
                .as_ref()
                .join("sessions")
                .join(format!("{pid}.json")),
            body,
        )
    }

    /// The desktop app's `~/Library/Application Support/Claude/config.json`.
    pub fn write_desktop_config(&self, body: &Value) -> PathBuf {
        self.write_json("Library/Application Support/Claude/config.json", body)
    }

    /// `~/.zshrc`.
    pub fn write_zshrc(&self, contents: &str) -> PathBuf {
        self.write_file(".zshrc", contents)
    }

    /// Runs the app's real `claude` shim (`bin/claude` in the app's folder)
    /// with `sh`, as a terminal would after the `.zshrc` block: the app's
    /// `bin/` first on `PATH`, then the stub `claude`, then the system
    /// folders. The environment is exactly `HOME`, that `PATH` and `vars`.
    /// What reached the real binary shows in [`StubClaude::calls`].
    pub fn run_shim(&self, args: &[&str], vars: &[(&str, &str)]) -> std::process::Output {
        let bin = AppPaths::new(&self.home).bin();
        let path = std::env::join_paths([
            bin.clone(),
            self.claude.bin_dir(),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ])
        .expect("PATH entries contain no colon");
        std::process::Command::new(bin.join("claude"))
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", path)
            .envs(vars.iter().copied())
            .stdin(std::process::Stdio::null())
            .output()
            .expect("run the claude shim")
    }
}

/// A Login acting within an Organization, as Claude Code records it in
/// `.claude.json` `oauthAccount`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureLogin {
    pub account_uuid: String,
    pub organization_uuid: String,
    pub email: String,
    pub organization_name: String,
}

impl FixtureLogin {
    pub fn new(
        account_uuid: &str,
        organization_uuid: &str,
        email: &str,
        organization_name: &str,
    ) -> Self {
        FixtureLogin {
            account_uuid: account_uuid.into(),
            organization_uuid: organization_uuid.into(),
            email: email.into(),
            organization_name: organization_name.into(),
        }
    }

    /// A personal Login for tests that need any signed-in identity.
    pub fn perso() -> Self {
        Self::new(
            "11111111-1111-4111-8111-111111111111",
            "22222222-2222-4222-8222-222222222222",
            "me@example.com",
            "me@example.com's Organization",
        )
    }

    /// The `oauthAccount` object.
    pub fn oauth_account(&self) -> Value {
        serde_json::json!({
            "accountUuid": self.account_uuid,
            "organizationUuid": self.organization_uuid,
            "emailAddress": self.email,
            "organizationName": self.organization_name,
        })
    }

    /// A minimal `.claude.json` signed in as this Login.
    pub fn claude_json(&self) -> Value {
        serde_json::json!({
            "hasCompletedOnboarding": true,
            "oauthAccount": self.oauth_account(),
        })
    }
}

// ----- Stub claude -----------------------------------------------------------------

/// The kinds of call the stub tells apart, each with its own reply queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StubCall {
    /// `claude auth login …`
    AuthLogin,
    /// `claude auth status …`
    AuthStatus,
    /// `claude auth logout`
    AuthLogout,
    /// `claude -p --input-format stream-json …` with a `get_usage` control
    /// request as its first stdin line.
    GetUsage,
    /// `claude -p "/usage"`
    SlashUsage,
    /// Anything else.
    Other,
}

impl StubCall {
    fn key(self) -> &'static str {
        match self {
            StubCall::AuthLogin => "auth-login",
            StubCall::AuthStatus => "auth-status",
            StubCall::AuthLogout => "auth-logout",
            StubCall::GetUsage => "get-usage",
            StubCall::SlashUsage => "slash-usage",
            StubCall::Other => "other",
        }
    }

    fn from_key(key: &str) -> StubCall {
        match key {
            "auth-login" => StubCall::AuthLogin,
            "auth-status" => StubCall::AuthStatus,
            "auth-logout" => StubCall::AuthLogout,
            "get-usage" => StubCall::GetUsage,
            "slash-usage" => StubCall::SlashUsage,
            _ => StubCall::Other,
        }
    }
}

/// One canned answer: what the stub prints, writes and exits with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reply {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    /// Files written into the spawn's `$CLAUDE_CONFIG_DIR` before answering,
    /// relative to it (for example a sign-in writing `.claude.json`).
    pub config_dir_files: Vec<(PathBuf, String)>,
    /// Never answer: wait until killed (a sign-in nobody finishes).
    pub hang: bool,
    /// Answer `auth status --json` from the spawn folder's own
    /// `oauthAccount` (logged out when it has none), like Claude Code.
    pub from_folder: bool,
}

impl Reply {
    /// Prints nothing and never exits on its own, like a browser sign-in the
    /// user never finishes.
    pub fn hanging() -> Reply {
        Reply {
            hang: true,
            ..Reply::default()
        }
    }

    /// An honest `claude auth status --json`: reports whatever `oauthAccount`
    /// the spawn's `$CLAUDE_CONFIG_DIR/.claude.json` holds, whichever folder
    /// asks.
    pub fn auth_status_from_folder() -> Reply {
        Reply {
            from_folder: true,
            ..Reply::default()
        }
    }

    /// Exit 0 printing `stdout`.
    pub fn ok(stdout: impl Into<String>) -> Reply {
        Reply {
            stdout: stdout.into(),
            ..Reply::default()
        }
    }

    /// Exit 0 printing `value` as one JSON line.
    pub fn json(value: &Value) -> Reply {
        Reply::ok(format!("{value}\n"))
    }

    /// Exit `exit_code` printing `stderr`.
    pub fn fail(exit_code: i32, stderr: impl Into<String>) -> Reply {
        Reply {
            stderr: stderr.into(),
            exit_code,
            ..Reply::default()
        }
    }

    /// Also write `contents` to `$CLAUDE_CONFIG_DIR/<relative>`.
    pub fn writing(mut self, relative: impl Into<PathBuf>, contents: impl Into<String>) -> Reply {
        self.config_dir_files
            .push((relative.into(), contents.into()));
        self
    }
}

/// A call the stub received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedCall {
    pub kind: StubCall,
    pub args: Vec<String>,
    /// The complete environment the stub ran with.
    pub env: BTreeMap<String, String>,
    /// The stdin line the stub read (only for `--input-format` calls).
    pub stdin: String,
    /// The stub's process id.
    pub pid: u32,
}

impl RecordedCall {
    pub fn var(&self, name: &str) -> Option<&str> {
        self.env.get(name).map(String::as_str)
    }

    /// Whether the stub process for this call is still alive (for example a
    /// hanging sign-in that was never killed).
    pub fn still_running(&self) -> bool {
        std::process::Command::new("/bin/kill")
            .args(["-0", &self.pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

/// The stub `claude` executable and its state folder.
///
/// Replies queue per [`StubCall`]: each call consumes the oldest reply, and
/// the last one keeps answering. A call with no reply exits 97, so a test
/// that forgot a reply fails loudly instead of hanging.
pub struct StubClaude {
    state: PathBuf,
    next_seq: Mutex<HashMap<StubCall, u32>>,
}

impl StubClaude {
    fn install(state: &Path) -> StubClaude {
        let bin = state.join("bin");
        fs::create_dir_all(&bin).expect("create stub bin");
        let script = bin.join("claude");
        fs::write(&script, include_str!("stub_claude.sh")).expect("write stub claude");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .expect("make stub executable");
        StubClaude {
            state: state.to_path_buf(),
            next_seq: Mutex::new(HashMap::new()),
        }
    }

    /// The folder holding the stub `claude`.
    pub fn bin_dir(&self) -> PathBuf {
        self.state.join("bin")
    }

    /// Queues `reply` for the next `kind` call (after any already queued).
    pub fn reply(&self, kind: StubCall, reply: Reply) -> &Self {
        let seq = {
            let mut next = self.next_seq.lock().unwrap();
            let seq = next.entry(kind).or_insert(0);
            *seq += 1;
            *seq
        };
        let dir = self
            .state
            .join("replies")
            .join(kind.key())
            .join(seq.to_string());
        fs::create_dir_all(&dir).expect("create reply folder");
        fs::write(dir.join("stdout"), &reply.stdout).expect("write reply stdout");
        fs::write(dir.join("stderr"), &reply.stderr).expect("write reply stderr");
        fs::write(dir.join("exit_code"), reply.exit_code.to_string()).expect("write exit code");
        if reply.hang {
            fs::write(dir.join("hang"), "").expect("write hang marker");
        }
        if reply.from_folder {
            fs::write(dir.join("from-folder"), "").expect("write from-folder marker");
        }
        for (relative, contents) in &reply.config_dir_files {
            let path = dir.join("files").join(relative);
            fs::create_dir_all(path.parent().expect("has parent")).expect("create reply files");
            fs::write(path, contents).expect("write reply file");
        }
        self
    }

    /// Every call so far, oldest first.
    pub fn calls(&self) -> Vec<RecordedCall> {
        let calls_dir = self.state.join("calls");
        let Ok(entries) = fs::read_dir(&calls_dir) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = entries
            .map(|e| e.expect("readable").path())
            // The pid is recorded last: a call without one is still being
            // recorded by a stub running right now.
            .filter(|dir| dir.join("pid").is_file())
            .collect();
        dirs.sort();
        dirs.iter()
            .map(|dir| {
                let read = |name: &str| fs::read(dir.join(name)).expect("read call record");
                let split = |bytes: Vec<u8>| -> Vec<String> {
                    bytes
                        .split(|b| *b == 0)
                        .filter(|part| !part.is_empty())
                        .map(|part| String::from_utf8_lossy(part).into_owned())
                        .collect()
                };
                let env = split(read("env"))
                    .into_iter()
                    .filter_map(|pair| {
                        pair.split_once('=')
                            .map(|(name, value)| (name.to_owned(), value.to_owned()))
                    })
                    .collect();
                RecordedCall {
                    kind: StubCall::from_key(&String::from_utf8_lossy(&read("key"))),
                    args: split(read("args")),
                    env,
                    stdin: String::from_utf8_lossy(&read("stdin")).into_owned(),
                    pid: String::from_utf8_lossy(&read("pid"))
                        .parse()
                        .expect("the stub records its pid"),
                }
            })
            .collect()
    }
}

// ----- Fake clock and OS facts ----------------------------------------------------

/// A clock that only moves when the test says so. Clones share the time.
#[derive(Debug, Clone)]
pub struct FakeClock(Arc<Mutex<Timestamp>>);

impl FakeClock {
    pub fn at(time: Timestamp) -> FakeClock {
        FakeClock(Arc::new(Mutex::new(time)))
    }

    pub fn set(&self, time: Timestamp) {
        *self.0.lock().unwrap() = time;
    }

    pub fn advance(&self, by: SignedDuration) {
        let mut now = self.0.lock().unwrap();
        *now = now.checked_add(by).expect("time stays in range");
    }
}

/// The fake Mac's time zone is UTC, so local times in fixtures (Claude Code's
/// `procStart`) read the same as the clock.
impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        *self.0.lock().unwrap()
    }

    /// UTC, so shown dates and times (and fixture `procStart`s) don't depend
    /// on the machine running the tests.
    fn time_zone(&self) -> jiff::tz::TimeZone {
        jiff::tz::TimeZone::UTC
    }
}

#[derive(Debug, Default)]
struct FakeOsState {
    processes: HashMap<u32, ProcessFacts>,
    desktop_app_version: Option<String>,
    managed_settings: Option<Vec<u8>>,
    /// Live pids running the desktop app's bundled Claude Code.
    desktop_children: BTreeSet<u32>,
    /// Terminal devices, by pid.
    terminals: HashMap<u32, String>,
    desktop_app_running: bool,
}

/// OS facts the test sets by hand. Clones share the facts.
#[derive(Debug, Clone, Default)]
pub struct FakeOs(Arc<Mutex<FakeOsState>>);

impl FakeOs {
    /// Marks `pid` as a live process.
    pub fn start_process(&self, pid: u32, parent_pid: u32, started_at: Timestamp) {
        self.0.lock().unwrap().processes.insert(
            pid,
            ProcessFacts {
                parent_pid,
                started_at,
            },
        );
    }

    /// Marks `pid` as gone.
    pub fn end_process(&self, pid: u32) {
        let mut state = self.0.lock().unwrap();
        state.processes.remove(&pid);
        state.desktop_children.remove(&pid);
    }

    /// Starts one of the desktop app's Claude Code children (parent: pid 1,
    /// standing in for Claude), which may or may not write a live-process file.
    pub fn start_desktop_child(&self, pid: u32, started_at: Timestamp) {
        self.start_process(pid, 1, started_at);
        self.0.lock().unwrap().desktop_children.insert(pid);
    }

    /// Puts live process `pid` on terminal device `tty` (`ttys004`).
    pub fn set_terminal(&self, pid: u32, tty: &str) {
        self.0.lock().unwrap().terminals.insert(pid, tty.to_owned());
    }

    /// Starts (`true`) or quits (`false`) the desktop app. It starts quit.
    pub fn set_desktop_app_running(&self, running: bool) {
        self.0.lock().unwrap().desktop_app_running = running;
    }

    /// Installs (or with `None`, removes) the desktop app.
    pub fn set_desktop_app_version(&self, version: Option<&str>) {
        self.0.lock().unwrap().desktop_app_version = version.map(str::to_owned);
    }

    /// Writes (or with `None`, removes) Claude Code's managed settings file,
    /// which lives outside the home folder on a real Mac.
    pub fn set_managed_settings(&self, body: Option<&Value>) {
        self.0.lock().unwrap().managed_settings =
            body.map(|body| serde_json::to_vec(body).expect("serialize managed settings"));
    }
}

impl OsFacts for FakeOs {
    fn process(&self, pid: u32) -> Option<ProcessFacts> {
        self.0.lock().unwrap().processes.get(&pid).copied()
    }

    fn desktop_app_version(&self) -> Option<String> {
        self.0.lock().unwrap().desktop_app_version.clone()
    }

    fn managed_settings(&self) -> Option<Vec<u8>> {
        self.0.lock().unwrap().managed_settings.clone()
    }

    fn terminal_of(&self, pid: u32) -> Option<String> {
        let state = self.0.lock().unwrap();
        state.processes.get(&pid)?;
        state.terminals.get(&pid).cloned()
    }

    fn desktop_claude_code_children(&self) -> Vec<u32> {
        self.0
            .lock()
            .unwrap()
            .desktop_children
            .iter()
            .copied()
            .collect()
    }

    fn desktop_app_running(&self) -> bool {
        self.0.lock().unwrap().desktop_app_running
    }
}

// ----- Fake login item -------------------------------------------------------------

#[derive(Debug)]
struct FakeLoginItemState {
    status: LoginItemStatus,
    registrations: usize,
    refusal: Option<String>,
}

/// The app's Login Items entry, behaving like `SMAppService.mainApp`: it
/// starts not registered, `register` enables it, `unregister` removes it,
/// and once the user turns it off in System Settings, registering again
/// leaves it off. Clones share the entry.
#[derive(Debug, Clone)]
pub struct FakeLoginItem(Arc<Mutex<FakeLoginItemState>>);

impl Default for FakeLoginItem {
    fn default() -> Self {
        FakeLoginItem(Arc::new(Mutex::new(FakeLoginItemState {
            status: LoginItemStatus::NotRegistered,
            registrations: 0,
            refusal: None,
        })))
    }
}

impl FakeLoginItem {
    /// Sets the entry's state directly (for example, left over from an
    /// earlier install, or `NotFound` for an app outside its bundle).
    pub fn set_status(&self, status: LoginItemStatus) {
        self.0.lock().unwrap().status = status;
    }

    /// The user turns AccRoster off in System Settings → Login Items.
    pub fn turn_off_in_system_settings(&self) {
        self.set_status(LoginItemStatus::RequiresApproval);
    }

    /// How many times the app asked to be registered.
    pub fn registrations(&self) -> usize {
        self.0.lock().unwrap().registrations
    }

    /// Makes every later `register` fail with this reason.
    pub fn refuse_registration(&self, reason: &str) {
        self.0.lock().unwrap().refusal = Some(reason.to_owned());
    }
}

impl LoginItem for FakeLoginItem {
    fn status(&self) -> LoginItemStatus {
        self.0.lock().unwrap().status
    }

    fn register(&self) -> Result<(), String> {
        let mut state = self.0.lock().unwrap();
        state.registrations += 1;
        if let Some(reason) = &state.refusal {
            return Err(reason.clone());
        }
        match state.status {
            LoginItemStatus::NotFound => Err("The app is not a valid login item.".into()),
            LoginItemStatus::RequiresApproval => Ok(()),
            _ => {
                state.status = LoginItemStatus::Enabled;
                Ok(())
            }
        }
    }

    fn unregister(&self) -> Result<(), String> {
        let mut state = self.0.lock().unwrap();
        match state.status {
            LoginItemStatus::NotFound => Err("The app is not a valid login item.".into()),
            _ => {
                state.status = LoginItemStatus::NotRegistered;
                Ok(())
            }
        }
    }
}
