//! The Claude Code runner: the only module that spawns Claude Code.
//!
//! Every spawn runs in a Profile folder (`profiles/<id>/`, never `~/.claude`;
//! a usage read there with a stale token once blanked the default login) and
//! from a neutral, empty working directory, so no project hooks or
//! `CLAUDE.md` load. Its environment is the app's inherited one minus every
//! `CLAUDE*` and `ANTHROPIC_*` variable, plus the `CLAUDE_CONFIG_DIR` it sets
//! to the Profile folder's stored path, verbatim (the Keychain slot is a hash
//! of that literal string).

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use jiff::{SignedDuration, Timestamp};
use serde_json::Value;

use crate::env::{Environment, RunningProcess, SpawnOutput, SpawnRequest};
use crate::storage::AppPaths;
use crate::usage::{self, UsageReply};

/// The `request_id` of the app's `get_usage` control request.
pub(crate) const USAGE_REQUEST_ID: &str = "accroster-usage";

/// `auth status` answers from local files; one still running after this
/// long is killed and says nothing.
pub(crate) const AUTH_STATUS_TIMEOUT: SignedDuration = SignedDuration::from_secs(30);
/// `auth logout` revokes the login over the network first.
pub(crate) const SIGN_OUT_TIMEOUT: SignedDuration = SignedDuration::from_mins(1);
/// `--desktop --resume` only hands the Session over to the desktop app.
pub(crate) const OPEN_IN_DESKTOP_TIMEOUT: SignedDuration = SignedDuration::from_mins(1);

/// Flags every usage read passes after its format flags: no Transcript, no
/// MCP servers, no hooks (ADR 0001).
const USAGE_FLAGS: [&str; 5] = [
    "--verbose",
    "--no-session-persistence",
    "--strict-mcp-config",
    "--settings",
    r#"{"disableAllHooks":true}"#,
];

/// The two ways to read a Profile's usage, tried in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsageRead {
    GetUsage,
    SlashUsage,
}

impl UsageRead {
    /// The answer in a finished read's stdout; `None` when there is none.
    pub(crate) fn parse(self, stdout: &[u8]) -> Option<UsageReply> {
        match self {
            UsageRead::GetUsage => usage::parse_get_usage(stdout, USAGE_REQUEST_ID),
            UsageRead::SlashUsage => usage::parse_slash_usage(stdout),
        }
    }
}

/// Spawns Claude Code in Profile folders.
pub(crate) struct Runner<'a> {
    env: &'a Environment,
    paths: &'a AppPaths,
}

/// What `claude auth status --json` says about a Profile folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthStatus {
    pub logged_in: bool,
    pub email: Option<String>,
    pub org_id: Option<String>,
}

impl AuthStatus {
    /// What a finished `auth status --json` said; `None` when it gave no
    /// readable answer.
    pub(crate) fn parse(finished: &Finished) -> Option<AuthStatus> {
        let Finished::Exited(output) = finished else {
            return None;
        };
        let body: Value = serde_json::from_slice(&output.stdout).ok()?;
        let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(AuthStatus {
            logged_in: body.get("loggedIn").and_then(Value::as_bool)?,
            email: text("email"),
            org_id: text("orgId"),
        })
    }
}

/// A short Claude Code run (`auth status`, `auth logout`,
/// `--desktop --resume`) that the core checks on at each tick instead of
/// waiting for, so the menu keeps updating while it runs. One still running
/// at its deadline is killed.
pub(crate) struct Job {
    process: Box<dyn RunningProcess>,
    deadline: Timestamp,
}

/// How a [`Job`] ended.
#[derive(Debug)]
pub(crate) enum Finished {
    Exited(SpawnOutput),
    /// Killed at its deadline.
    TimedOut,
    /// The process couldn't be waited for.
    Lost,
}

impl Finished {
    /// Claude Code exited with status 0.
    pub(crate) fn succeeded(&self) -> bool {
        matches!(self, Finished::Exited(output) if output.succeeded())
    }

    /// How it ended, as one log word.
    pub(crate) fn word(&self) -> &'static str {
        match self {
            Finished::Exited(output) if output.succeeded() => "succeeded",
            Finished::Exited(_) => "exited_with_error",
            Finished::TimedOut => "timed_out",
            Finished::Lost => "not_started_or_lost",
        }
    }
}

impl Job {
    /// `Some` once the run ended, or once it was killed for running past its
    /// deadline at `now`.
    pub(crate) fn poll(&mut self, now: Timestamp) -> Option<Finished> {
        match self.process.try_wait() {
            Ok(Some(output)) => Some(Finished::Exited(output)),
            Ok(None) if now < self.deadline => None,
            Ok(None) => {
                let _ = self.process.kill();
                Some(Finished::TimedOut)
            }
            Err(_) => Some(Finished::Lost),
        }
    }
}

/// A short run a [`Jobs`] queue can start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JobKind {
    AuthStatus,
    SignOut,
    OpenInDesktop {
        session_id: String,
        /// The Session's folder, when it still exists.
        session_folder: Option<PathBuf>,
    },
}

/// Short Claude Code runs started one at a time, in the order asked, each
/// with a tag saying what to do once it ends. Nothing waits on them: each
/// [`Jobs::poll`] collects what ended and starts what's next.
pub(crate) struct Jobs<T> {
    waiting: VecDeque<Queued<T>>,
    running: Option<(Queued<T>, Job)>,
}

struct Queued<T> {
    config_dir: PathBuf,
    kind: JobKind,
    tag: T,
}

impl<T> Queued<T> {
    fn ended(self, finished: Finished) -> Ended<T> {
        Ended {
            config_dir: self.config_dir,
            tag: self.tag,
            finished,
        }
    }
}

/// A run that ended: its folder, its tag and how it ended.
pub(crate) struct Ended<T> {
    pub config_dir: PathBuf,
    pub tag: T,
    pub finished: Finished,
}

impl<T> Default for Jobs<T> {
    fn default() -> Self {
        Jobs {
            waiting: VecDeque::new(),
            running: None,
        }
    }
}

impl<T> Jobs<T> {
    /// Queues a run in `config_dir`; it starts at the next [`Jobs::poll`].
    pub(crate) fn push(&mut self, config_dir: &Path, kind: JobKind, tag: T) {
        self.waiting.push_back(Queued {
            config_dir: config_dir.to_path_buf(),
            kind,
            tag,
        });
    }

    /// The runs that ended since the last call, after starting the next
    /// waiting one. A run that couldn't start ends as [`Finished::Lost`].
    pub(crate) fn poll(&mut self, runner: &Runner) -> Vec<Ended<T>> {
        let now = runner.env.clock.now();
        let mut ended = Vec::new();
        loop {
            if let Some((_, job)) = &mut self.running {
                let Some(finished) = job.poll(now) else {
                    break;
                };
                let (queued, _) = self.running.take().expect("checked above");
                ended.push(queued.ended(finished));
            }
            let Some(next) = self.waiting.pop_front() else {
                break;
            };
            match runner.start(&next.config_dir, &next.kind) {
                Ok(job) => self.running = Some((next, job)),
                Err(_) => ended.push(next.ended(Finished::Lost)),
            }
        }
        ended
    }

    /// Whether nothing is running or waiting.
    pub(crate) fn is_empty(&self) -> bool {
        self.running.is_none() && self.waiting.is_empty()
    }

    /// Every run running or waiting, as (folder, tag).
    pub(crate) fn queued(&self) -> impl Iterator<Item = (&Path, &T)> {
        self.running
            .iter()
            .map(|(queued, _)| queued)
            .chain(&self.waiting)
            .map(|queued| (queued.config_dir.as_path(), &queued.tag))
    }
}

impl<'a> Runner<'a> {
    /// Starts one short run in `config_dir`.
    fn start(&self, config_dir: &Path, kind: &JobKind) -> io::Result<Job> {
        match kind {
            JobKind::AuthStatus => self.start_auth_status(config_dir),
            JobKind::SignOut => self.start_sign_out(config_dir),
            JobKind::OpenInDesktop {
                session_id,
                session_folder,
            } => self.start_open_in_desktop(config_dir, session_id, session_folder.as_deref()),
        }
    }

    pub(crate) fn new(env: &'a Environment, paths: &'a AppPaths) -> Runner<'a> {
        Runner { env, paths }
    }

    /// Starts `claude auth login --claudeai` (with `--email` when known). It
    /// runs until the user finishes in the browser, so it is cancellable.
    pub(crate) fn start_sign_in(
        &self,
        config_dir: &Path,
        email: Option<&str>,
    ) -> io::Result<Box<dyn RunningProcess>> {
        let mut args = vec!["auth", "login", "--claudeai"];
        if let Some(email) = email {
            args.extend(["--email", email]);
        }
        let request = self.request(config_dir, &args, None)?;
        self.env.spawner.start(&request)
    }

    /// Starts `claude auth status --json`; read its answer with
    /// [`AuthStatus::parse`]. Kept rare: a short-lived command can start a
    /// token refresh and exit before saving it.
    fn start_auth_status(&self, config_dir: &Path) -> io::Result<Job> {
        let request = self.request(config_dir, &["auth", "status", "--json"], None)?;
        self.job(&request, AUTH_STATUS_TIMEOUT)
    }

    /// Starts `claude auth logout`, which revokes the login and deletes its
    /// Keychain item. It signed out when the run [`Finished::succeeded`].
    fn start_sign_out(&self, config_dir: &Path) -> io::Result<Job> {
        let request = self.request(config_dir, &["auth", "logout"], None)?;
        self.job(&request, SIGN_OUT_TIMEOUT)
    }

    /// Starts one step of a usage read: the `get_usage` control request
    /// (its one stdin line, then EOF), or `claude -p "/usage"`, the fallback
    /// when Claude Code no longer answers `get_usage`. Read the finished
    /// output with [`UsageRead::parse`].
    pub(crate) fn start_usage_read(
        &self,
        config_dir: &Path,
        read: UsageRead,
    ) -> io::Result<Box<dyn RunningProcess>> {
        let request = match read {
            UsageRead::GetUsage => {
                let mut args = vec![
                    "-p",
                    "--input-format",
                    "stream-json",
                    "--output-format",
                    "stream-json",
                ];
                args.extend(USAGE_FLAGS);
                let line = serde_json::json!({
                    "type": "control_request",
                    "request_id": USAGE_REQUEST_ID,
                    "request": { "subtype": "get_usage", "skip_behaviors": true },
                });
                let stdin = format!("{line}\n").into_bytes();
                self.request(config_dir, &args, Some(stdin))?
            }
            UsageRead::SlashUsage => {
                let mut args = vec!["-p", "--output-format", "stream-json"];
                args.extend(USAGE_FLAGS);
                args.push("/usage");
                self.request(config_dir, &args, None)?
            }
        };
        let mut process = self.env.spawner.start(&request)?;
        process.close_stdin();
        Ok(process)
    }

    /// Starts `claude --desktop --resume <id>`: Claude Code hands the Session
    /// to the desktop app, which opens it under its own Active Profile. Runs
    /// in the Session's folder when it still exists (the Transcript is
    /// looked up from there).
    fn start_open_in_desktop(
        &self,
        config_dir: &Path,
        session_id: &str,
        session_folder: Option<&Path>,
    ) -> io::Result<Job> {
        let mut request = self.request(config_dir, &["--desktop", "--resume", session_id], None)?;
        if let Some(folder) = session_folder.filter(|folder| folder.is_dir()) {
            request.cwd = Some(folder.to_path_buf());
        }
        self.job(&request, OPEN_IN_DESKTOP_TIMEOUT)
    }

    fn job(&self, request: &SpawnRequest, timeout: SignedDuration) -> io::Result<Job> {
        let mut process = self.env.spawner.start(request)?;
        // Nothing to read: a command that asks for input gets EOF.
        process.close_stdin();
        Ok(Job {
            process,
            deadline: self.env.clock.now() + timeout,
        })
    }

    /// Builds a spawn of `claude` in `config_dir`, refusing any folder that
    /// isn't a Profile folder.
    fn request(
        &self,
        config_dir: &Path,
        args: &[&str],
        stdin: Option<Vec<u8>>,
    ) -> io::Result<SpawnRequest> {
        if config_dir.parent() != Some(self.paths.profiles().as_path())
            || config_dir == self.env.home.join(".claude")
        {
            return Err(io::Error::other("Claude Code only runs in Profile folders"));
        }
        let cwd = self.paths.root().join("spawn-cwd");
        fs::create_dir_all(&cwd)?;
        let mut env: Vec<(OsString, OsString)> = self
            .env
            .inherited_vars
            .iter()
            .filter(|(name, _)| {
                let name = name.to_string_lossy();
                !(name.starts_with("CLAUDE") || name.starts_with("ANTHROPIC_"))
            })
            .cloned()
            .collect();
        env.push((
            "CLAUDE_CONFIG_DIR".into(),
            config_dir.as_os_str().to_owned(),
        ));
        Ok(SpawnRequest {
            program: "claude".into(),
            args: args.iter().map(OsString::from).collect(),
            env,
            cwd: Some(cwd),
            stdin,
        })
    }
}
