//! Parsing Claude Code's usage answers: the `get_usage` control response and,
//! as a fallback, the `usage_report` of `claude -p "/usage"`.
//!
//! Every field is optional. Limits are classified by `limits[].kind`, never by
//! label, falling back to `five_hour` / `seven_day` when `limits` is absent.
//! The API's `severity` is ignored.

use jiff::Timestamp;
use serde_json::Value;

use crate::limit_offers::{self, BankedReset, SessionRefill};

/// A usage answer, as far as it could be read.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UsageReply {
    /// `subscription_type` (`max`, `pro`, …).
    pub subscription_type: Option<String>,
    /// `false` for an API-key login or a login without the `user:profile`
    /// scope.
    pub rate_limits_available: Option<bool>,
    /// `None` when `rate_limits` was null or isn't a usage body.
    pub limits: Option<Limits>,
    /// Whether `rate_limits` was null (or absent): with
    /// `rate_limits_available: true` that is a failed fetch, not an
    /// unreadable answer.
    pub rate_limits_null: bool,
    /// `rate_limits.cedar_ember`, when present, eligible and well formed.
    pub banked_reset: Option<BankedReset>,
    /// `rate_limits.juniper_tide`, when reported.
    pub session_refill: Option<SessionRefill>,
}

/// A Profile's limits.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Limits {
    pub session: Option<Limit>,
    pub weekly: Option<Limit>,
    /// Model-specific weekly limits, by display name.
    pub models: Vec<(String, Limit)>,
    /// Limit kinds this app doesn't know yet, by raw `kind`.
    pub other: Vec<(String, Limit)>,
}

impl Limits {
    /// Every limit with its label, in display order: Session, Weekly, each
    /// model-specific weekly limit, then each unknown kind. Anything that
    /// looks for the tightest limit goes through this, so unknown kinds count.
    pub fn all(&self) -> impl Iterator<Item = (&str, &Limit)> {
        let session = self.session.as_ref().map(|limit| ("Session", limit));
        let weekly = self.weekly.as_ref().map(|limit| ("Weekly", limit));
        session.into_iter().chain(weekly).chain(
            self.models
                .iter()
                .chain(&self.other)
                .map(|(label, limit)| (label.as_str(), limit)),
        )
    }
}

/// One limit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Limit {
    /// Utilization in percent (0–100, fractional).
    pub percent: f64,
    /// When the window resets, when known.
    pub resets_at: Option<Timestamp>,
}

/// Reads the `control_response` answering `request_id` from stream-json
/// stdout. `None` when there is no successful answer, which means Claude Code
/// doesn't (or no longer does) answer `get_usage`.
pub(crate) fn parse_get_usage(stdout: &[u8], request_id: &str) -> Option<UsageReply> {
    json_lines(stdout).find_map(|line| {
        if line.get("type")?.as_str()? != "control_response" {
            return None;
        }
        let response = line.get("response")?;
        if response.get("request_id")?.as_str()? != request_id
            || response.get("subtype")?.as_str()? != "success"
        {
            return None;
        }
        Some(reply_from_body(response.get("response")?))
    })
}

/// Reads the `usage_report` an assistant message carries in `claude -p
/// "/usage"` stream-json output. Its `rate_limits` holds only `limits` and
/// `extra_usage`.
pub(crate) fn parse_slash_usage(stdout: &[u8]) -> Option<UsageReply> {
    json_lines(stdout).find_map(|line| {
        let report = line
            .get("usage_report")
            .or_else(|| line.get("message")?.get("usage_report"))?;
        report.is_object().then(|| reply_from_body(report))
    })
}

fn json_lines(stdout: &[u8]) -> impl Iterator<Item = Value> + '_ {
    stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
}

fn reply_from_body(body: &Value) -> UsageReply {
    UsageReply {
        subscription_type: body
            .get("subscription_type")
            .and_then(Value::as_str)
            .map(str::to_owned),
        rate_limits_available: body.get("rate_limits_available").and_then(Value::as_bool),
        limits: body.get("rate_limits").and_then(parse_rate_limits),
        rate_limits_null: body.get("rate_limits").is_none_or(Value::is_null),
        banked_reset: body.get("rate_limits").and_then(limit_offers::banked_reset),
        session_refill: body
            .get("rate_limits")
            .and_then(limit_offers::session_refill),
    }
}

/// Keys whose presence makes an object a usage body (Claude Code's own check).
const USAGE_BODY_KEYS: [&str; 9] = [
    "five_hour",
    "seven_day",
    "seven_day_oauth_apps",
    "seven_day_opus",
    "seven_day_sonnet",
    "cinder_cove",
    "wattle_ember",
    "extra_usage",
    "limits",
];

fn parse_rate_limits(body: &Value) -> Option<Limits> {
    let object = body.as_object()?;
    if !USAGE_BODY_KEYS.iter().any(|key| object.contains_key(*key)) {
        return None;
    }
    let utilization = |key: &str| {
        let window = body.get(key)?;
        Some(Limit {
            percent: window.get("utilization")?.as_f64()?,
            resets_at: resets_at(window),
        })
    };
    let mut limits = Limits {
        session: utilization("five_hour"),
        weekly: utilization("seven_day"),
        models: Vec::new(),
        other: Vec::new(),
    };
    match body.get("limits").and_then(Value::as_array) {
        Some(entries) => {
            for entry in entries {
                let Some(percent) = entry.get("percent").and_then(Value::as_f64) else {
                    continue;
                };
                let limit = Limit {
                    percent,
                    resets_at: resets_at(entry),
                };
                match entry.get("kind").and_then(Value::as_str) {
                    Some("session") => limits.session = Some(limit),
                    Some("weekly_all") => limits.weekly = Some(limit),
                    Some("weekly_scoped") => {
                        if let Some(name) = entry
                            .pointer("/scope/model/display_name")
                            .and_then(Value::as_str)
                        {
                            limits.models.push((name.to_owned(), limit));
                        }
                    }
                    Some(kind) => limits.other.push((kind.to_owned(), limit)),
                    None => {}
                }
            }
        }
        None => {
            for (key, name) in [("seven_day_opus", "Opus"), ("seven_day_sonnet", "Sonnet")] {
                if let Some(limit) = utilization(key) {
                    limits.models.push((name.to_owned(), limit));
                }
            }
        }
    }
    Some(limits)
}

/// An ISO 8601 `resets_at`; `None` when absent or unreadable.
fn resets_at(window: &Value) -> Option<Timestamp> {
    window.get("resets_at")?.as_str()?.parse().ok()
}
