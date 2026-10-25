//! Banked resets (`cedar_ember`) and Session refills (`juniper_tide`): read
//! from the usage body and shown as lines under a Profile's bars.
//!
//! The app only shows them. It never redeems a Banked reset or uses a Session
//! refill: nothing here, or anywhere else, calls the reset endpoint or drives
//! `/limit-reset`. A grant's `id` is the redemption handle, so it is checked
//! for shape and then dropped, never stored or logged.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::Value;

/// CodexBar's parsing caps: at most this many grants are read, and the count
/// shown never goes above [`MAX_RESETS`].
const MAX_GRANTS: usize = 200;
const MAX_RESETS: u64 = 50;

/// An eligible `cedar_ember` block.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BankedReset {
    grants: Vec<Grant>,
}

#[derive(Debug, Clone, PartialEq)]
struct Grant {
    resets_left: u64,
    starts_at: Option<Timestamp>,
    ends_at: Option<Timestamp>,
    clears: Vec<Cleared>,
    paused: bool,
}

/// A limit a grant clears, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Cleared {
    Session,
    Weekly,
    Opus,
    Sonnet,
}

impl Cleared {
    /// Claude Code's limit types; unknown ones are filtered out, as it does.
    fn from_type(name: &str) -> Option<Cleared> {
        Some(match name {
            "five_hour" => Cleared::Session,
            "seven_day"
            | "seven_day_overage_included"
            | "seven_day_cowork"
            | "seven_day_omelette"
            | "seven_day_oauth_apps" => Cleared::Weekly,
            "seven_day_opus" => Cleared::Opus,
            "seven_day_sonnet" => Cleared::Sonnet,
            _ => return None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            Cleared::Session => "Session",
            Cleared::Weekly => "Weekly",
            Cleared::Opus => "Opus",
            Cleared::Sonnet => "Sonnet",
        }
    }
}

/// A `juniper_tide` block, every field optional.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SessionRefill {
    available: bool,
    next_available_at: Option<Timestamp>,
}

/// Reads `rate_limits.cedar_ember`. `None` when it is null, absent,
/// ineligible or malformed (no boolean `eligible`, or a grant missing its id
/// or `resets_left`, or with an unreadable date): null means unknown, so the
/// row shows nothing rather than zero.
pub(crate) fn banked_reset(rate_limits: &Value) -> Option<BankedReset> {
    let block = rate_limits.get("cedar_ember")?.as_object()?;
    if !block.get("eligible")?.as_bool()? {
        return None;
    }
    let grants = match block.get("grants") {
        None | Some(Value::Null) => Vec::new(),
        Some(grants) => grants
            .as_array()?
            .iter()
            .take(MAX_GRANTS)
            .map(grant)
            .collect::<Option<_>>()?,
    };
    Some(BankedReset { grants })
}

fn grant(value: &Value) -> Option<Grant> {
    let id = value.get("id")?.as_str()?;
    let id_ok = (1..=40).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if !id_ok {
        return None;
    }
    let date = |key: &str| -> Option<Option<Timestamp>> {
        match value.get(key) {
            None | Some(Value::Null) => Some(None),
            Some(text) => text.as_str()?.parse().ok().map(Some),
        }
    };
    Some(Grant {
        resets_left: value.get("resets_left")?.as_u64()?,
        starts_at: date("starts_at")?,
        ends_at: date("ends_at")?,
        clears: value
            .get("clears")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|name| Cleared::from_type(name.as_str()?))
            .collect(),
        paused: value
            .get("paused")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// Reads `rate_limits.juniper_tide`. `None` when it is null, absent, not an
/// object, or explicitly ineligible.
pub(crate) fn session_refill(rate_limits: &Value) -> Option<SessionRefill> {
    let block = rate_limits.get("juniper_tide")?.as_object()?;
    if block.get("eligible").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    Some(SessionRefill {
        available: block.get("available").and_then(Value::as_bool) == Some(true),
        next_available_at: block
            .get("next_available_at")
            .and_then(Value::as_str)
            .and_then(|text| text.parse().ok()),
    })
}

/// The lines a Profile row shows under its bars: the Banked reset, then the
/// Session refill, each only when there is something to say.
pub(crate) fn lines(
    banked: Option<&BankedReset>,
    refill: Option<&SessionRefill>,
    now: Timestamp,
    tz: &TimeZone,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(line) = banked.and_then(|banked| banked_line(banked, now, tz)) {
        lines.push(line);
    }
    if let Some(refill) = refill {
        if refill.available {
            lines.push("Session refill available".to_owned());
        } else if let Some(next) = refill.next_available_at {
            lines.push(format!("Next refill {}", day(next, now, tz)));
        }
    }
    lines
}

/// Counts resets left over grants that aren't paused, have started and
/// haven't expired (CodexBar's rule); `None` when that is zero.
fn banked_line(banked: &BankedReset, now: Timestamp, tz: &TimeZone) -> Option<String> {
    let usable: Vec<&Grant> = banked
        .grants
        .iter()
        .filter(|grant| {
            grant.resets_left > 0
                && !grant.paused
                && grant.starts_at.is_none_or(|start| start <= now)
                && grant.ends_at.is_none_or(|end| end > now)
        })
        .collect();
    let count = usable
        .iter()
        .map(|grant| grant.resets_left)
        .sum::<u64>()
        .min(MAX_RESETS);
    if count == 0 {
        return None;
    }
    let one = count == 1;
    let mut line = format!("{count} Banked reset{}", if one { "" } else { "s" });
    let mut cleared: Vec<Cleared> = usable
        .iter()
        .flat_map(|grant| grant.clears.iter().copied())
        .collect();
    cleared.sort();
    cleared.dedup();
    if !cleared.is_empty() {
        let labels: Vec<&str> = cleared.into_iter().map(Cleared::label).collect();
        line.push_str(if one { " · clears " } else { " · clear " });
        line.push_str(&labels.join(", "));
    }
    if let Some(soonest) = usable.iter().filter_map(|grant| grant.ends_at).min() {
        line.push_str(if one {
            " · expires "
        } else {
            " · soonest expires "
        });
        line.push_str(&day(soonest, now, tz));
    }
    Some(line)
}

/// A day as the menu names it: "today", a weekday within the coming week,
/// else the date ("Nov 3").
pub(crate) fn day(at: Timestamp, now: Timestamp, tz: &TimeZone) -> String {
    let date = at.to_zoned(tz.clone()).date();
    let today = now.to_zoned(tz.clone()).date();
    let days_ahead = today
        .until(date)
        .map(|span| span.get_days())
        .unwrap_or(i32::MAX);
    match days_ahead {
        0 => "today".to_owned(),
        1..=6 => date.strftime("%a").to_string(),
        _ => date.strftime("%b %-d").to_string(),
    }
}
