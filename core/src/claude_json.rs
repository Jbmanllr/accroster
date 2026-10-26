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
    pub names: LoginNames,
    /// For example `claude_max`.
    pub organization_type: Option<String>,
    /// When Claude Code last refreshed these names.
    pub profile_fetched_at: Option<Timestamp>,
}

/// The names `oauthAccount` gives a Login and its Organization, and the
/// names the app makes from them (GLOSSARY: Profile name, Short label).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct LoginNames {
    /// Empty when missing.
    pub email: String,
    /// Empty when missing.
    pub organization_name: String,
    /// `fullName`; `None` when missing or empty.
    pub full_name: Option<String>,
    /// `displayName`; `None` when missing or empty.
    pub display_name: Option<String>,
}

impl LoginNames {
    /// The Organization when it says more than the email: not empty and not
    /// Claude's personal default, `<email>'s Organization`.
    fn telling_organization(&self) -> Option<&str> {
        let name = self.organization_name.as_str();
        (!name.is_empty() && name != format!("{}'s Organization", self.email)).then_some(name)
    }

    /// The Profile name: `<fullName> · <email>` (else the display name, else
    /// the email alone), then ` · <Organization>` unless it is the personal
    /// default.
    pub fn name(&self) -> String {
        self.joined(self.telling_organization())
    }

    /// The Profile name with the Organization even when it is the personal
    /// default, for two Profiles that would otherwise share a name.
    pub fn name_with_organization(&self) -> String {
        self.joined(Some(self.organization_name.as_str()))
    }

    fn joined(&self, organization: Option<&str>) -> String {
        let person = self.full_name.as_deref().or(self.display_name.as_deref());
        [person, Some(self.email.as_str()), organization]
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// The Organization is Claude's personal default (`<email>'s
    /// Organization`), which only its owner can be in.
    pub fn personal_organization(&self) -> bool {
        !self.email.is_empty() && self.organization_name == format!("{}'s Organization", self.email)
    }

    /// The Organization as people call it: its name when that tells, else
    /// the person's (`Ada Lovelace's Organization`) for the personal default.
    pub fn organization_label(&self) -> String {
        let person = self.full_name.as_deref().or(self.display_name.as_deref());
        match (self.telling_organization(), person) {
            (Some(organization), _) => organization.to_owned(),
            (None, Some(person)) => format!("{person}'s Organization"),
            (None, None) => self.organization_name.clone(),
        }
    }

    /// `email · Organization`, for the hover line.
    pub fn email_and_organization(&self) -> String {
        format!("{} · {}", self.email, self.organization_name)
    }

    /// The short labels, shortest first: the display name (else the full
    /// name's first word, else the email's local part); it with the
    /// Organization when that tells; it with the email (the email alone in
    /// place of the local part); and that with the Organization.
    pub fn short_labels(&self) -> Vec<String> {
        let first_name = self
            .full_name
            .as_deref()
            .and_then(|name| name.split_whitespace().next());
        let person = self.display_name.as_deref().or(first_name);
        let short = person
            .unwrap_or_else(|| self.email.split('@').next().unwrap_or_default())
            .to_owned();
        let with_email = match person {
            Some(person) => format!("{person} · {}", self.email),
            None => self.email.clone(),
        };
        let mut labels = vec![short.clone()];
        if let Some(organization) = self.telling_organization() {
            labels.push(format!("{short} · {organization}"));
        }
        if !self.organization_name.is_empty() {
            labels.push(with_email.clone());
            labels.push(format!("{with_email} · {}", self.organization_name));
        } else {
            labels.push(with_email);
        }
        labels
    }
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
            names: LoginNames {
                email: text("emailAddress").unwrap_or_default(),
                organization_name: text("organizationName").unwrap_or_default(),
                full_name: text("fullName").filter(|name| !name.trim().is_empty()),
                display_name: text("displayName").filter(|name| !name.trim().is_empty()),
            },
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
