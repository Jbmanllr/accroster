//! Parsing Claude Code's usage answers: the `get_usage` control response and,
//! as a fallback, the `usage_report` of `claude -p "/usage"`.
//!
//! Every field is optional. Limits are classified by `limits[].kind`, never by
//! label, falling back to `five_hour` / `seven_day` when `limits` is absent.
//! The API's `severity` is ignored.

use serde_json::Value;

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
}

/// A Profile's limits, utilization in percent (0–100, fractional).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Limits {
    pub session: Option<f64>,
    pub weekly: Option<f64>,
    /// Model-specific weekly limits: display name and utilization.
    pub models: Vec<(String, f64)>,
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
    let utilization = |key: &str| body.get(key)?.get("utilization")?.as_f64();
    let mut limits = Limits {
        session: utilization("five_hour"),
        weekly: utilization("seven_day"),
        models: Vec::new(),
    };
    match body.get("limits").and_then(Value::as_array) {
        Some(entries) => {
            for entry in entries {
                let Some(percent) = entry.get("percent").and_then(Value::as_f64) else {
                    continue;
                };
                match entry.get("kind").and_then(Value::as_str) {
                    Some("session") => limits.session = Some(percent),
                    Some("weekly_all") => limits.weekly = Some(percent),
                    Some("weekly_scoped") => {
                        if let Some(name) = entry
                            .pointer("/scope/model/display_name")
                            .and_then(Value::as_str)
                        {
                            limits.models.push((name.to_owned(), percent));
                        }
                    }
                    _ => {}
                }
            }
        }
        None => {
            for (key, name) in [("seven_day_opus", "Opus"), ("seven_day_sonnet", "Sonnet")] {
                if let Some(percent) = utilization(key) {
                    limits.models.push((name.to_owned(), percent));
                }
            }
        }
    }
    Some(limits)
}
