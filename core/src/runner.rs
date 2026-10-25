//! The Claude Code runner: the only module that spawns Claude Code.
//!
//! Every spawn runs in a Profile folder (`profiles/<id>/`, never `~/.claude`;
//! a usage read there with a stale token once blanked the default login) and
//! from a neutral, empty working directory, so no project hooks or
//! `CLAUDE.md` load. Its environment is the app's inherited one minus every
//! `CLAUDE*` and `ANTHROPIC_*` variable, plus the `CLAUDE_CONFIG_DIR` it sets
//! to the Profile folder's stored path, verbatim (the Keychain slot is a hash
//! of that literal string).

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::Path;

use serde_json::Value;

use crate::env::{Environment, RunningProcess, SpawnOutput, SpawnRequest};
use crate::storage::AppPaths;
use crate::usage::{self, UsageReply};

/// The `request_id` of the app's `get_usage` control request.
pub(crate) const USAGE_REQUEST_ID: &str = "accroster-usage";

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

impl<'a> Runner<'a> {
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

    /// `claude auth status --json`; `None` when it can't be run or read.
    /// Kept rare: a short-lived command can start a token refresh and exit
    /// before saving it.
    pub(crate) fn auth_status(&self, config_dir: &Path) -> Option<AuthStatus> {
        let output = self
            .run(config_dir, &["auth", "status", "--json"], None)
            .ok()?;
        let body: Value = serde_json::from_slice(&output.stdout).ok()?;
        let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(AuthStatus {
            logged_in: body.get("loggedIn").and_then(Value::as_bool)?,
            email: text("email"),
            org_id: text("orgId"),
        })
    }

    /// `claude auth logout`: revokes the login and deletes its Keychain item.
    /// `Ok(false)` when Claude Code reported a failure.
    pub(crate) fn sign_out(&self, config_dir: &Path) -> io::Result<bool> {
        Ok(self.run(config_dir, &["auth", "logout"], None)?.succeeded())
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

    fn run(
        &self,
        config_dir: &Path,
        args: &[&str],
        stdin: Option<Vec<u8>>,
    ) -> io::Result<SpawnOutput> {
        let request = self.request(config_dir, args, stdin)?;
        self.env.spawner.run(&request)
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
