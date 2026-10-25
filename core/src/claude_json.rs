//! `oauthAccount` in a `.claude.json` (a Profile folder's, or `~/.claude.json`):
//! the one reader of it, shared by the Profile registry, Discovered Profiles
//! and the format checks (ADR 0004).

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use jiff::Timestamp;
use serde_json::Value;

use crate::interface::ProfileKey;

/// The keys of `oauthAccount` the app relies on; the format check reports
/// any that is missing or not text.
const CHECKED_KEYS: [&str; 3] = ["accountUuid", "organizationUuid", "emailAddress"];

/// A Login acting within an Organization, as `oauthAccount` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OauthAccount {
    pub key: ProfileKey,
    /// Empty when missing.
    pub email: String,
    /// Empty when missing.
    pub organization_name: String,
    /// For example `claude_max`.
    pub organization_type: Option<String>,
    /// When Claude Code last refreshed these names.
    pub profile_fetched_at: Option<Timestamp>,
}

/// What a `.claude.json`'s `oauthAccount` holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountRead {
    /// `None` when its account or Organization uuid isn't text.
    pub account: Option<OauthAccount>,
    /// Checked keys that are missing or not text.
    pub bad_keys: BTreeSet<&'static str>,
}

/// The parsed `.claude.json` at `path`; `None` when it is missing or isn't
/// JSON.
pub(crate) fn read(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// `oauthAccount` in a parsed `.claude.json`; `None` when it has none (or
/// a null one).
pub(crate) fn oauth_account(body: &Value) -> Option<AccountRead> {
    let account = body.get("oauthAccount").filter(|value| !value.is_null())?;
    let text = |key: &str| account.get(key).and_then(Value::as_str).map(str::to_owned);
    let bad_keys = CHECKED_KEYS
        .into_iter()
        .filter(|key| text(key).is_none())
        .collect();
    let account = (|| {
        Some(OauthAccount {
            key: ProfileKey {
                account_uuid: text("accountUuid")?,
                organization_uuid: text("organizationUuid")?,
            },
            email: text("emailAddress").unwrap_or_default(),
            organization_name: text("organizationName").unwrap_or_default(),
            organization_type: text("organizationType"),
            profile_fetched_at: account
                .get("profileFetchedAt")
                .and_then(Value::as_i64)
                .and_then(|ms| Timestamp::from_millisecond(ms).ok()),
        })
    })();
    Some(AccountRead { account, bad_keys })
}

/// The account `oauthAccount` names in the `.claude.json` at `path`, if
/// any.
pub(crate) fn read_account(path: &Path) -> Option<OauthAccount> {
    oauth_account(&read(path)?)?.account
}
