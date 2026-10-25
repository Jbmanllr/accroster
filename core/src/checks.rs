//! The format checks that read files (ADR 0004): each looks at the fields
//! the app uses in one Claude format and returns a [`Finding`] in field names
//! and counts only, so it can go into "Copy report" as is.
//!
//! These are the self-check's view of the formats. The parsers that use the
//! data (limits, identities, the Session index, Sidebar sync) still check
//! every field they read and degrade per item.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::interface::SafetyFeature;
use crate::safety::{Finding, reportable};

/// `oauthAccount` and `cachedUsageUtilization` in each Profile's
/// `.claude.json`. A malformed `oauthAccount` pauses CLI switching: the app
/// could no longer tell which account a folder is.
pub(crate) fn accounts(folders: &[&Path], claude_code: &str) -> (Finding, Finding) {
    let mut accounts_seen = 0;
    let mut bad_accounts = BTreeSet::new();
    let mut caches_seen = 0;
    let mut bad_caches = BTreeSet::new();
    for folder in folders {
        let Some(body) = fs::read(folder.join(".claude.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        else {
            continue;
        };
        if let Some(account) = body.get("oauthAccount").filter(|v| !v.is_null()) {
            accounts_seen += 1;
            for key in ["accountUuid", "organizationUuid", "emailAddress"] {
                if !account.get(key).is_some_and(Value::is_string) {
                    bad_accounts.insert(key);
                }
            }
        }
        if let Some(cache) = body.get("cachedUsageUtilization").filter(|v| !v.is_null()) {
            caches_seen += 1;
            if !cache.get("fetchedAtMs").is_some_and(Value::is_number) {
                bad_caches.insert("fetchedAtMs");
            }
            if !cache.get("accountUuid").is_some_and(Value::is_string) {
                bad_caches.insert("accountUuid");
            }
        }
    }
    let accounts = if accounts_seen == 0 {
        Finding::never_seen()
    } else if bad_accounts.is_empty() {
        Finding::pass()
    } else {
        Finding::fail(format!("missing or not text: {}", join(&bad_accounts))).blocking(
            SafetyFeature::CliSwitching,
            format!("{claude_code} changed .claude.json accounts"),
        )
    };
    let caches = if caches_seen == 0 {
        Finding::never_seen()
    } else if bad_caches.is_empty() {
        Finding::pass()
    } else {
        Finding::fail(format!("missing or wrong type: {}", join(&bad_caches)))
    };
    (accounts, caches)
}

/// The desktop app's Sidebar folders, as the self-check sees them.
const SIDEBAR_ROOT: &str = "Library/Application Support/Claude/claude-code-sessions";

/// Files and folders an org folder holds besides `local_*.json` and
/// `deleted_*`.
const SIDEBAR_FOLDER_FILES: [&str; 4] = [
    "archived-sessions.idx",
    "scheduled-tasks.json",
    "waiting-input",
    "backlog",
];

/// Every top-level Sidebar entry field the app knows (the 2026-10-25 census
/// plus names read in the desktop app's code). Sidebar sync copies whole
/// entries, so a field outside this list may be bound to an account and
/// stops sync (ADR 0004): a desktop update can add one, and the app learns it
/// in a release. Diagnostics names it so it can be reported.
pub(crate) const SIDEBAR_ENTRY_FIELDS: &[&str] = &[
    "adoptedFromOtherSurface",
    "alwaysAllowedReasons",
    "autoArchiveExempt",
    "autoChosenInApp",
    "autoFixNoticeSent",
    "autoModeServerFallbackPrompt",
    "backgroundTaskSuggestions",
    "branch",
    "bridgeSessionIds",
    "bypassChosenInApp",
    "chromeAllowedDomains",
    "chromePermissionMode",
    "chromeTabGroupId",
    "classifierSummaryEnabled",
    "cliMcpAppServerNames",
    "cliSessionId",
    "completedTurns",
    "createdAt",
    "cuAllowedApps",
    "cuGrantFlags",
    "cuLastScreenshotDims",
    "cuSelectedDisplayId",
    "cutTurnStoppedAt",
    "cwd",
    "devIntentTriggers",
    "devIntents",
    "effort",
    "effortInherited",
    "enabledMcpTools",
    "envScopeId",
    "error",
    "errorAt",
    "forkedFromSessionId",
    "gitAnchors",
    "gitAnchorsFolderRealpath",
    "gitAnchorsLookupOnly",
    "homeTenancy",
    "indexedAt",
    "interruptedAwaitingPerson",
    "interruptedByQuitAt",
    "isArchived",
    "isStarred",
    "lastActivityAt",
    "lastAssistantUuid",
    "lastFocusedAt",
    "lastSpawnRootDetected",
    "lastTurnCutShort",
    "lastTurnEdgeAt",
    "lastTurnReport",
    "latestUserFrameAt",
    "lineageDetached",
    "model",
    "notifySessionId",
    "originCwd",
    "peerMessaged",
    "peerReceipts",
    "pendingSystemReminder",
    "permissionMode",
    "planPath",
    "postTurnSummary",
    "postTurnSummaryFor",
    "previousTitles",
    "priorCliSessionIds",
    "priorErrorMark",
    "promptAppendSnapshot",
    "promptSuggestion",
    "prs",
    "publishedArtifacts",
    "queuedSideSessionNotes",
    "recapAt",
    "remoteControlAutoEligible",
    "remoteControlSpawn",
    "remoteControlUserEnabled",
    "remoteMcpServersConfig",
    "reportFindingsCard",
    "resolvedBackgroundTaskSuggestions",
    "scheduledTaskId",
    "scratchOfferFolder",
    "scratchPromptRecents",
    "sessionId",
    "sessionPermissionUpdates",
    "sessionSettings",
    "setupTools",
    "sideSessionNotes",
    "sideSessionReportOwed",
    "sideSessionStartsSinceUserMessage",
    "sideSessionWakesSinceUserMessage",
    "sourceBranch",
    "spawnSeed",
    "spawnedFrom",
    "spawnedFromEndNotified",
    "sshConfig",
    "sshReattach",
    "sshRemoteProcessId",
    "sshRemoteTranscriptPath",
    "startedFromEnvironmentId",
    "steeredByRemoteClient",
    "subagentsTruncatedFor",
    "terminalClaudeTabOrdinal",
    "title",
    "titleOffers",
    "titleSource",
    "titleTurn",
    "toolSurfaceSnapshot",
    "transcriptUnavailable",
    "turnBoxDeclared",
    "turnBoxMounted",
    "turnWrapUp",
    "unarchivedCliSessionId",
    "worktreeName",
    "worktreePath",
    "writtenBranches",
    "wslConfig",
];

/// Entries over this size are skipped, as the desktop app's loader does.
const MAX_ENTRY_BYTES: u64 = 10 * 1024 * 1024;

/// At most this many Sidebar entries are read per self-check.
const MAX_ENTRIES: usize = 5000;

/// Sidebar entries: the folder layout `<account>/<org>/` with known file
/// names, `sessionId` equal to the file name, and no unknown top-level field.
pub(crate) fn sidebar_entries(home: &Path) -> Finding {
    let root = home.join(SIDEBAR_ROOT);
    let mut read = 0usize;
    let mut unknown_fields = BTreeSet::new();
    let mut mismatched = 0usize;
    let mut unknown_files = 0usize;
    for org in subfolders(&root)
        .iter()
        .flat_map(|account| subfolders(account))
    {
        for entry in fs::read_dir(&org).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_entry = name.starts_with("local_") && name.ends_with(".json");
            if !is_entry {
                if !name.starts_with("deleted_")
                    && !name.starts_with('.')
                    && !SIDEBAR_FOLDER_FILES.contains(&name.as_str())
                {
                    unknown_files += 1;
                }
                continue;
            }
            if read >= MAX_ENTRIES {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.len() == 0 || meta.len() > MAX_ENTRY_BYTES {
                continue;
            }
            let Some(Value::Object(body)) = fs::read(entry.path())
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            else {
                continue;
            };
            read += 1;
            let stem = name.trim_end_matches(".json");
            if body.get("sessionId").and_then(Value::as_str) != Some(stem) {
                mismatched += 1;
            }
            for key in body.keys() {
                if !SIDEBAR_ENTRY_FIELDS.contains(&key.as_str()) {
                    unknown_fields.insert(reportable(key));
                }
            }
        }
    }
    if read == 0 && unknown_files == 0 {
        return Finding::never_seen();
    }
    let mut problems = Vec::new();
    if !unknown_fields.is_empty() {
        problems.push(format!(
            "unknown fields: {}",
            unknown_fields.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    if mismatched > 0 {
        problems.push(format!(
            "sessionId differs from the file name in {mismatched}"
        ));
    }
    if unknown_files > 0 {
        problems.push(format!("{unknown_files} unknown files in Sidebar folders"));
    }
    if problems.is_empty() {
        Finding::pass().with_detail(format!("{read} entries read"))
    } else {
        Finding::fail(problems.join("; "))
    }
}

/// Live-process files `sessions/<pid>.json` in these folders: the fields the
/// busy guard and the running dots read.
pub(crate) fn live_processes(config_dirs: &[PathBuf]) -> Finding {
    let mut read = 0usize;
    let mut bad = BTreeSet::new();
    for dir in config_dirs {
        for entry in fs::read_dir(dir.join("sessions"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".json") else {
                continue;
            };
            if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let Some(body) = fs::read(entry.path())
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            else {
                continue;
            };
            read += 1;
            if !body.get("pid").is_some_and(Value::is_u64) {
                bad.insert("pid");
            }
            for key in ["sessionId", "procStart", "entrypoint", "status"] {
                if !body.get(key).is_some_and(Value::is_string) {
                    bad.insert(key);
                }
            }
        }
    }
    if read == 0 {
        Finding::never_seen()
    } else if bad.is_empty() {
        Finding::pass()
    } else {
        Finding::fail(format!("missing or wrong type: {}", join(&bad)))
    }
}

/// Transcripts sampled per self-check (the newest), and bytes read of each.
const TRANSCRIPT_SAMPLE: usize = 20;
const TRANSCRIPT_BYTES: u64 = 1024 * 1024;

/// Transcript records the Session index reads: `custom-title`, `ai-title`,
/// the first `user` record's content and `entrypoint`, in the newest
/// top-level Transcripts (links not followed, subagents skipped).
pub(crate) fn transcript_records(home: &Path) -> Finding {
    let projects = home.join(".claude/projects");
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = subfolders(&projects)
        .iter()
        .flat_map(|project| fs::read_dir(project).into_iter().flatten().flatten())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    if files.is_empty() {
        return Finding::never_seen();
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let mut bad = BTreeSet::new();
    for (_, path) in files.iter().take(TRANSCRIPT_SAMPLE) {
        let mut text = String::new();
        let Ok(file) = fs::File::open(path) else {
            continue;
        };
        if file
            .take(TRANSCRIPT_BYTES)
            .read_to_string(&mut text)
            .is_err()
        {
            // Cut inside a character: keep what decoded.
            text = String::from_utf8_lossy(text.as_bytes()).into_owned();
        }
        let mut first_user = true;
        // Bad lines (a partly written last one) are tolerated.
        for record in text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        {
            if record.get("entrypoint").is_some_and(|v| !v.is_string()) {
                bad.insert("entrypoint");
            }
            match record.get("type").and_then(Value::as_str) {
                Some("custom-title")
                    if !record.get("customTitle").is_some_and(Value::is_string) =>
                {
                    bad.insert("custom-title.customTitle");
                }
                Some("ai-title") if !record.get("aiTitle").is_some_and(Value::is_string) => {
                    bad.insert("ai-title.aiTitle");
                }
                Some("user") if first_user => {
                    first_user = false;
                    let content = record.pointer("/message/content");
                    if !content.is_some_and(|c| c.is_string() || c.is_array()) {
                        bad.insert("user.message.content");
                    }
                }
                _ => {}
            }
        }
    }
    if bad.is_empty() {
        Finding::pass()
    } else {
        Finding::fail(format!("missing or wrong type: {}", join(&bad)))
    }
}

/// Real subfolders of `dir` (links not followed).
fn subfolders(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

fn join(names: &BTreeSet<&str>) -> String {
    names.iter().copied().collect::<Vec<_>>().join(", ")
}
