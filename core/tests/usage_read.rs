//! The usage read after a sign-in: one `get_usage` control request through
//! Claude Code, parsed field by field, with `/usage` as the fallback.

mod common;

use accroster_fake_mac::{Reply, StubCall};
use common::*;
use serde_json::{Value, json};

/// Adds a Profile signed in as `work()` whose usage read answers `reply`, and
/// returns its row's bars and note.
fn read_with(reply: Reply) -> (Vec<accroster_core::LimitBar>, Option<String>) {
    let (bars, note, _) = read_with_mac(reply, |_| {});
    (bars, note)
}

fn read_with_mac(
    reply: Reply,
    setup: impl FnOnce(&accroster_fake_mac::FakeMac),
) -> (
    Vec<accroster_core::LimitBar>,
    Option<String>,
    accroster_fake_mac::FakeMac,
) {
    let mac = accroster_fake_mac::FakeMac::new();
    mac.with_claude_home(&accroster_fake_mac::FixtureLogin::perso());
    mac.claude
        .reply(StubCall::AuthLogin, sign_in_as(&work()))
        .reply(StubCall::AuthStatus, auth_status(&work()))
        .reply(StubCall::GetUsage, reply);
    setup(&mac);
    let mut core = mac.launch();
    add_profile(&mut core);
    let rows = rows(&core);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = rows.into_iter().next().unwrap();
    (row.limits, row.note, mac)
}

#[test]
fn the_usage_read_runs_claude_code_with_the_spec_flags_and_one_get_usage_request() {
    let (_, _, mac) = read_with_mac(usage_reply(limits_body()), |_| {});

    let call = mac
        .claude
        .calls()
        .into_iter()
        .find(|call| call.kind == StubCall::GetUsage)
        .expect("a usage read ran");
    assert_eq!(
        call.args,
        [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--no-session-persistence",
            "--strict-mcp-config",
            "--settings",
            r#"{"disableAllHooks":true}"#,
        ]
    );
    let request: Value = serde_json::from_str(&call.stdin).expect("one JSON line");
    assert_eq!(
        request,
        json!({
            "type": "control_request",
            "request_id": "accroster-usage",
            "request": { "subtype": "get_usage", "skip_behaviors": true },
        })
    );
}

#[test]
fn without_a_limits_list_the_legacy_five_hour_and_seven_day_fields_are_read() {
    let (bars, note) = read_with(usage_reply(json!({
        "five_hour": { "utilization": 62.7, "resets_at": "2026-10-25T16:40:00Z" },
        "seven_day": { "utilization": 41.0, "resets_at": "2026-10-30T09:00:00Z" },
        "seven_day_opus": { "utilization": 12.5, "resets_at": "2026-10-30T09:00:00Z" },
        "seven_day_sonnet": null,
        "extra_usage": { "is_enabled": false },
    })));

    assert_eq!(
        bars,
        [bar("Session", 62), bar("Weekly", 41), bar("Opus", 12)]
    );
    assert_eq!(note, None);
}

#[test]
fn every_field_is_optional() {
    let (bars, note) = read_with(usage_reply(json!({
        "limits": [
            { "kind": "weekly_all" },
            { "kind": "session", "percent": 30 },
            { "kind": "weekly_scoped", "percent": 5 },
            { "percent": 99 },
        ],
    })));

    assert_eq!(
        bars,
        [bar("Session", 30)],
        "entries missing what a bar needs are skipped"
    );
    assert_eq!(note, None);
}

#[test]
fn null_rate_limits_show_couldnt_read_limits() {
    let (bars, note) = read_with(usage_reply(Value::Null));

    assert!(bars.is_empty());
    assert_eq!(note.as_deref(), Some("Couldn't read limits"));
}

#[test]
fn a_body_that_is_not_a_usage_answer_shows_couldnt_read_limits() {
    let (bars, note) = read_with(usage_reply(json!({ "something": "else" })));

    assert!(bars.is_empty());
    assert_eq!(note.as_deref(), Some("Couldn't read limits"));
}

#[test]
fn slash_usage_is_the_fallback_when_get_usage_is_unavailable() {
    let (bars, note, mac) = read_with_mac(
        Reply::json(&json!({
            "type": "control_response",
            "response": { "subtype": "error", "request_id": "accroster-usage",
                          "error": "Unsupported control request subtype: get_usage" },
        })),
        |mac| {
            mac.claude.reply(
                StubCall::SlashUsage,
                Reply::json(&json!({
                    "type": "assistant",
                    "message": { "role": "assistant", "content": [] },
                    "usage_report": {
                        "rate_limits": {
                            "limits": [
                                { "kind": "session", "percent": 18 },
                                { "kind": "weekly_all", "percent": 77 },
                            ],
                            "extra_usage": null,
                        },
                    },
                })),
            );
        },
    );

    assert_eq!(bars, [bar("Session", 18), bar("Weekly", 77)]);
    assert_eq!(note, None);
    let call = mac
        .claude
        .calls()
        .into_iter()
        .find(|call| call.kind == StubCall::SlashUsage)
        .expect("/usage ran");
    assert_eq!(
        call.args,
        [
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--no-session-persistence",
            "--strict-mcp-config",
            "--settings",
            r#"{"disableAllHooks":true}"#,
            "/usage",
        ]
    );
}

#[test]
fn when_neither_read_answers_the_row_says_couldnt_read_limits() {
    let (bars, note, _) = read_with_mac(Reply::fail(1, "error: unknown option\n"), |mac| {
        mac.claude
            .reply(StubCall::SlashUsage, Reply::ok("not json at all\n"));
    });

    assert!(bars.is_empty());
    assert_eq!(note.as_deref(), Some("Couldn't read limits"));
}
