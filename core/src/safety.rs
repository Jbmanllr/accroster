//! Format checks and Safety pauses (ADR 0004).
//!
//! Each check looks at one format the app relies on and yields a [`Finding`]:
//! pass, fail or never seen, a detail in field names, and the write feature
//! it blocks, if any. [`Safety`] keeps this run's findings and the persisted
//! pauses in `state/safety.json`:
//!
//! - A blocked feature that isn't paused yet gets a Safety pause: it is
//!   recorded before anything else happens, one notification is queued, and
//!   a menu warning line shows while it lasts (across relaunches).
//! - A pause never ends on its own. Once no check blocks the feature,
//!   Diagnostics offers "Resume <feature>"; only that click ends it, after
//!   the feature's checks ran again.
//!
//! Reads never pause anything: they degrade per item where they're parsed.

use std::collections::BTreeMap;
use std::fs;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::env::Environment;
use crate::interface::{
    CheckRow, CheckStatus, ClaudeVersions, Command, DiagnosticsView, Effect, PausedFeature,
    SafetyFeature,
};
use crate::storage::{self, AppPaths, VersionedFile};

/// The formats the app checks, in Diagnostics order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CheckId {
    UsageResponse,
    OauthAccount,
    CachedUsage,
    ConfigDirHonoured,
    SettingsEnv,
    TranscriptLinks,
    OtherLinks,
    SidebarEntries,
    LiveProcesses,
    TranscriptRecords,
    MultiAccountMode,
}

impl CheckId {
    pub(crate) const ALL: [CheckId; 11] = [
        CheckId::UsageResponse,
        CheckId::OauthAccount,
        CheckId::CachedUsage,
        CheckId::ConfigDirHonoured,
        CheckId::SettingsEnv,
        CheckId::TranscriptLinks,
        CheckId::OtherLinks,
        CheckId::SidebarEntries,
        CheckId::LiveProcesses,
        CheckId::TranscriptRecords,
        CheckId::MultiAccountMode,
    ];

    fn name(self) -> &'static str {
        match self {
            CheckId::UsageResponse => "get_usage response",
            CheckId::OauthAccount => ".claude.json oauthAccount",
            CheckId::CachedUsage => ".claude.json cachedUsageUtilization",
            CheckId::ConfigDirHonoured => "Claude Code honours Profile folders",
            CheckId::SettingsEnv => "Settings env",
            CheckId::TranscriptLinks => "Shared-Transcript links",
            CheckId::OtherLinks => "Other Profile links",
            CheckId::SidebarEntries => "Sidebar entries",
            CheckId::LiveProcesses => "Live-process files",
            CheckId::TranscriptRecords => "Transcript records",
            CheckId::MultiAccountMode => "Dormant multi-account mode",
        }
    }
}

/// What one check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub status: CheckStatus,
    /// Field names, counts and versions only: it goes into "Copy report".
    pub detail: Option<String>,
    /// The write features this result stops, and why (a phrase for the
    /// menu line, like "settings.json sets CLAUDE_CONFIG_DIR").
    pub blocks: Vec<(SafetyFeature, String)>,
}

impl Finding {
    pub(crate) fn pass() -> Finding {
        Finding {
            status: CheckStatus::Pass,
            detail: None,
            blocks: Vec::new(),
        }
    }

    pub(crate) fn never_seen() -> Finding {
        Finding {
            status: CheckStatus::NeverSeen,
            detail: None,
            blocks: Vec::new(),
        }
    }

    pub(crate) fn fail(detail: impl Into<String>) -> Finding {
        Finding {
            status: CheckStatus::Fail,
            detail: Some(detail.into()),
            blocks: Vec::new(),
        }
    }

    pub(crate) fn with_detail(mut self, detail: impl Into<String>) -> Finding {
        self.detail = Some(detail.into());
        self
    }

    pub(crate) fn blocking(mut self, feature: SafetyFeature, reason: impl Into<String>) -> Finding {
        self.blocks.push((feature, reason.into()));
        self
    }
}

fn feature_from_key(key: &str) -> Option<SafetyFeature> {
    SafetyFeature::ALL
        .into_iter()
        .find(|feature| feature.key() == key)
}

/// `state/safety.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SafetyFile {
    #[serde(default)]
    paused: Vec<PauseRecord>,
    /// The versions the last full self-check ran against.
    #[serde(default)]
    checked_versions: Option<VersionsRecord>,
    /// Settings `env` variables already warned about, as `<source>:<name>`.
    #[serde(default)]
    warned_env: Vec<String>,
    /// The last `auth status` cross-check, which only runs when Claude Code's
    /// version changes (a short-lived `claude` can burn a refresh token).
    #[serde(default)]
    config_dir_check: Option<StoredFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PauseRecord {
    feature: String,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionsRecord {
    claude_code: Option<String>,
    desktop_claude_code: Vec<String>,
    desktop: Option<String>,
}

impl From<&ClaudeVersions> for VersionsRecord {
    fn from(versions: &ClaudeVersions) -> Self {
        VersionsRecord {
            claude_code: versions.claude_code.clone(),
            desktop_claude_code: versions.desktop_claude_code.clone(),
            desktop: versions.desktop.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredFinding {
    passed: bool,
    detail: Option<String>,
}

impl VersionedFile for SafetyFile {
    const VERSION: u64 = 1;
}

/// The checks' results and the Safety pauses.
pub(crate) struct Safety {
    path: std::path::PathBuf,
    file: SafetyFile,
    findings: BTreeMap<CheckId, Finding>,
    /// One-time warning lines for this run (settings `env` variables).
    warnings: Vec<String>,
    /// The shared `settings.json`'s (mtime, size) when its `env` was last
    /// checked.
    settings_stamp: Option<(SystemTime, u64)>,
    /// Effects to hand to the UI with the next update (a pause noticed at
    /// launch, before any input).
    pending: Vec<Effect>,
}

impl Safety {
    /// Loads the persisted pauses. An unreadable file keeps no pause it
    /// can't read; the next self-check finds the cause again.
    pub(crate) fn load(paths: &AppPaths) -> Safety {
        let path = paths.state().join("safety.json");
        let file = storage::read_versioned::<SafetyFile>(&path)
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut findings = BTreeMap::new();
        if let Some(stored) = &file.config_dir_check {
            findings.insert(
                CheckId::ConfigDirHonoured,
                Finding {
                    status: if stored.passed {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Fail
                    },
                    detail: stored.detail.clone(),
                    blocks: Vec::new(),
                },
            );
        }
        Safety {
            path,
            file,
            findings,
            warnings: Vec::new(),
            settings_stamp: None,
            pending: Vec::new(),
        }
    }

    /// Records a check's result. Call [`Safety::settle`] after a batch.
    pub(crate) fn record(&mut self, check: CheckId, finding: Finding) {
        if check == CheckId::ConfigDirHonoured {
            self.file.config_dir_check = Some(StoredFinding {
                passed: finding.status != CheckStatus::Fail,
                detail: finding.detail.clone(),
            });
        }
        self.findings.insert(check, finding);
    }

    /// Pauses every feature a current finding blocks and that isn't paused
    /// yet, queueing one notification each, then saves. Returns the features
    /// newly paused.
    pub(crate) fn settle(&mut self) -> Vec<SafetyFeature> {
        let mut paused = Vec::new();
        for (feature, reason) in self.blockers() {
            if self.is_paused(feature) || paused.contains(&feature) {
                continue;
            }
            self.file.paused.push(PauseRecord {
                feature: feature.key().to_owned(),
                reason: reason.clone(),
            });
            self.pending.push(Effect::Notify {
                title: format!("{} paused", feature.name()),
                body: format!(
                    "{reason}. Nothing was written. Resume it in Diagnostics once this is fixed."
                ),
            });
            paused.push(feature);
        }
        self.save();
        paused
    }

    /// Every (feature, reason) a current finding blocks.
    fn blockers(&self) -> Vec<(SafetyFeature, String)> {
        self.findings
            .values()
            .flat_map(|finding| finding.blocks.iter().cloned())
            .collect()
    }

    pub(crate) fn is_blocked(&self, feature: SafetyFeature) -> bool {
        self.blockers()
            .iter()
            .any(|(blocked, _)| *blocked == feature)
    }

    /// Every feature under a Safety pause.
    pub(crate) fn paused(&self) -> Vec<SafetyFeature> {
        self.file
            .paused
            .iter()
            .filter_map(|record| feature_from_key(&record.feature))
            .collect()
    }

    pub(crate) fn is_paused(&self, feature: SafetyFeature) -> bool {
        self.file
            .paused
            .iter()
            .any(|record| feature_from_key(&record.feature) == Some(feature))
    }

    /// Why `feature` is under a Safety pause, if it is.
    pub(crate) fn paused_reason(&self, feature: SafetyFeature) -> Option<String> {
        self.file
            .paused
            .iter()
            .find(|record| feature_from_key(&record.feature) == Some(feature))
            .map(|record| record.reason.clone())
    }

    /// Ends `feature`'s pause if no check blocks it any more (the caller ran
    /// its checks again first). Returns whether it resumed.
    pub(crate) fn resume(&mut self, feature: SafetyFeature) -> bool {
        if !self.is_paused(feature) || self.is_blocked(feature) {
            return false;
        }
        self.file
            .paused
            .retain(|record| feature_from_key(&record.feature) != Some(feature));
        self.save();
        true
    }

    /// The effects queued since the last call.
    pub(crate) fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.pending)
    }

    /// Menu warning lines: each pause, then this run's one-time warnings.
    pub(crate) fn warning_lines(&self) -> Vec<String> {
        self.file
            .paused
            .iter()
            .filter_map(|record| {
                let feature = feature_from_key(&record.feature)?;
                Some(format!("{} · {} paused", record.reason, feature.name()))
            })
            .chain(self.warnings.iter().cloned())
            .collect()
    }

    /// Whether the versions differ from those of the last full self-check
    /// (or none ran yet).
    pub(crate) fn versions_changed(&self, versions: &ClaudeVersions) -> bool {
        self.file.checked_versions.as_ref() != Some(&VersionsRecord::from(versions))
    }

    pub(crate) fn mark_checked(&mut self, versions: &ClaudeVersions) {
        self.file.checked_versions = Some(VersionsRecord::from(versions));
        self.save();
    }

    /// Checks the `env` blocks of the shared and managed settings, unless
    /// `only_if_changed` and the shared file's mtime and size are as last
    /// time. Returns whether it checked.
    pub(crate) fn check_settings_env(&mut self, env: &Environment, only_if_changed: bool) -> bool {
        let shared = env.home.join(".claude/settings.json");
        let stamp = fs::metadata(&shared)
            .ok()
            .and_then(|meta| Some((meta.modified().ok()?, meta.len())));
        if only_if_changed && stamp == self.settings_stamp {
            return false;
        }
        self.settings_stamp = stamp;
        let sources = [
            (EnvSource::Shared, fs::read(&shared).ok()),
            (EnvSource::Managed, env.os.managed_settings()),
        ];
        let mut pausing = Vec::new();
        let mut noted = Vec::new();
        let mut seen = Vec::new();
        for (source, bytes) in sources {
            let Some(vars) = bytes.as_deref().and_then(env_names) else {
                continue;
            };
            let blocked: Vec<&str> = PAUSING_VARS
                .iter()
                .copied()
                .filter(|name| vars.iter().any(|(var, _)| var == name))
                .collect();
            if !blocked.is_empty() {
                pausing.push(format!("{} {}", source.sets(), blocked.join(", ")));
            }
            let warned: Vec<&str> = WARNED_VARS
                .iter()
                .copied()
                .filter(|name| vars.iter().any(|(var, _)| var == name))
                .collect();
            let scrub = vars
                .iter()
                .any(|(var, value)| var == SCRUB_VAR && is_on(value));
            let mut new_lines = Vec::new();
            let fresh: Vec<&str> = warned
                .iter()
                .copied()
                .filter(|name| !self.was_warned(source, name))
                .collect();
            if !fresh.is_empty() {
                new_lines.push(format!(
                    "{} {} · those Sessions don't use the Profile's subscription",
                    source.sets(),
                    fresh.join(", ")
                ));
            }
            if scrub && !self.was_warned(source, SCRUB_VAR) {
                new_lines.push(format!(
                    "{} {SCRUB_VAR}=1 · a claude started inside a Session follows the CLI's Active Profile",
                    source.sets()
                ));
            }
            for name in warned.iter().copied().chain(scrub.then_some(SCRUB_VAR)) {
                seen.push(source.key(name));
                noted.push(format!("{} {name}", source.sets()));
            }
            self.warnings.extend(new_lines);
        }
        self.file.warned_env = seen;
        let finding = if pausing.is_empty() {
            let finding = Finding::pass();
            if noted.is_empty() {
                finding
            } else {
                finding.with_detail(noted.join("; "))
            }
        } else {
            let reason = pausing.join("; ");
            Finding::fail(reason.clone()).blocking(SafetyFeature::CliSwitching, reason)
        };
        self.record(CheckId::SettingsEnv, finding);
        true
    }

    fn was_warned(&self, source: EnvSource, name: &str) -> bool {
        self.file.warned_env.contains(&source.key(name))
    }

    /// The Diagnostics window's content.
    /// `usage` is the usage format's finding, which the latest Limit readings
    /// decide.
    pub(crate) fn diagnostics(&self, versions: ClaudeVersions, usage: Finding) -> DiagnosticsView {
        let checks: Vec<CheckRow> = CheckId::ALL
            .iter()
            .map(|check| {
                let finding = if *check == CheckId::UsageResponse {
                    Some(&usage)
                } else {
                    self.findings.get(check)
                };
                CheckRow {
                    name: check.name().to_owned(),
                    status: finding.map_or(CheckStatus::NeverSeen, |f| f.status),
                    detail: finding.and_then(|f| f.detail.clone()),
                }
            })
            .collect();
        let paused: Vec<PausedFeature> = self
            .file
            .paused
            .iter()
            .filter_map(|record| {
                let feature = feature_from_key(&record.feature)?;
                Some(PausedFeature {
                    feature,
                    title: format!("{} paused", feature.name()),
                    reason: record.reason.clone(),
                    resume: (!self.is_blocked(feature)).then_some(Command::ResumeFeature(feature)),
                })
            })
            .collect();
        let report = report(&checks, &versions, &paused);
        DiagnosticsView {
            checks,
            versions,
            paused,
            report,
        }
    }

    fn save(&self) {
        // A lost write only means a pause may need finding again at the next
        // self-check; nothing is written to Claude's files meanwhile.
        let _ = storage::write_versioned(&self.path, &self.file);
    }
}

/// "Copy report": names, results and versions; details are field names and
/// counts by construction.
fn report(checks: &[CheckRow], versions: &ClaudeVersions, paused: &[PausedFeature]) -> String {
    let unknown = || "unknown".to_owned();
    let mut lines = vec![
        format!("AccRoster {} diagnostics", env!("CARGO_PKG_VERSION")),
        format!(
            "Claude Code: {}",
            versions.claude_code.clone().unwrap_or_else(unknown)
        ),
        format!(
            "Desktop's Claude Code: {}",
            if versions.desktop_claude_code.is_empty() {
                unknown()
            } else {
                versions.desktop_claude_code.join(", ")
            }
        ),
        format!(
            "Claude desktop app: {}",
            versions.desktop.clone().unwrap_or_else(unknown)
        ),
    ];
    for paused in paused {
        lines.push(format!("{}: {}", paused.title, paused.reason));
    }
    lines.push("Checks:".to_owned());
    for check in checks {
        let status = match check.status {
            CheckStatus::Pass => "pass",
            CheckStatus::Fail => "FAIL",
            CheckStatus::NeverSeen => "never seen",
        };
        match &check.detail {
            Some(detail) => lines.push(format!("  {status}: {} ({detail})", check.name)),
            None => lines.push(format!("  {status}: {}", check.name)),
        }
    }
    lines.join("\n") + "\n"
}

/// Variables in a settings `env` that the shim can't clear and that would
/// put another folder or one account behind every Profile.
const PAUSING_VARS: [&str; 3] = [
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
];

/// Deliberate setups that only get a one-time warning: an API key or another
/// provider instead of the Profile's subscription.
const WARNED_VARS: [&str; 7] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "CLAUDE_CODE_USE_MANTLE",
];

/// Removes `CLAUDE_CONFIG_DIR` from Bash-tool children, so a nested `claude`
/// follows the pointer instead of its Session's Profile.
const SCRUB_VAR: &str = "CLAUDE_CODE_SUBPROCESS_ENV_SCRUB";

#[derive(Debug, Clone, Copy)]
enum EnvSource {
    Shared,
    Managed,
}

impl EnvSource {
    fn sets(self) -> &'static str {
        match self {
            EnvSource::Shared => "settings.json sets",
            EnvSource::Managed => "Managed settings set",
        }
    }

    fn key(self, name: &str) -> String {
        match self {
            EnvSource::Shared => format!("shared:{name}"),
            EnvSource::Managed => format!("managed:{name}"),
        }
    }
}

/// The `env` block's names with their values as text, or `None` when the
/// file isn't JSON (Claude Code can't apply it either).
fn env_names(bytes: &[u8]) -> Option<Vec<(String, String)>> {
    let body: Value = serde_json::from_slice(bytes).ok()?;
    let env = body.get("env")?.as_object()?;
    Some(
        env.iter()
            .map(|(name, value)| {
                let text = match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                (name.clone(), text)
            })
            .collect(),
    )
}

fn is_on(value: &str) -> bool {
    matches!(value.trim(), "1" | "true")
}

/// "Claude Code 2.1.310", or "Claude Code" when its version is unknown.
pub(crate) fn claude_code_name(versions: &ClaudeVersions) -> String {
    match &versions.claude_code {
        Some(version) => format!("Claude Code {version}"),
        None => "Claude Code".to_owned(),
    }
}

/// A key or file name as it may appear in a report: ASCII identifier
/// characters, at most 64; anything else is withheld.
pub(crate) fn reportable(name: &str) -> String {
    if !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        name.to_owned()
    } else {
        "<unusual name>".to_owned()
    }
}
