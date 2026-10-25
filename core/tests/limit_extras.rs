//! What a Profile row shows besides its known limits: limit kinds the app
//! doesn't know yet, Banked resets, Session refills, and the claude.ai link.

mod common;

use accroster_core::{Effect, MenuItem, ProfileRow};
use accroster_fake_mac::StubCall;
use common::*;
use serde_json::{Value, json};

/// Adds a Profile signed in as `work()` whose usage read answers
/// `rate_limits`, and returns its row.
fn row_for(rate_limits: Value) -> ProfileRow {
    let mac = mac_signing_in_as(&work(), rate_limits);
    let mut core = mac.launch();
    add_profile(&mut core);
    let rows = rows(&core);
    assert_eq!(rows.len(), 1, "{rows:?}");
    rows.into_iter().next().unwrap()
}

#[test]
fn an_unknown_limit_kind_is_an_extra_bar_labelled_with_its_raw_kind() {
    let row = row_for(json!({
        "limits": [
            { "kind": "session", "percent": 62 },
            { "kind": "weekly_all", "percent": 41 },
            { "kind": "daily_cowork", "group": "daily", "percent": 97.5, "scope": null },
        ],
    }));

    assert_eq!(
        ignoring_reset_times(&row.limits),
        [
            bar("Session", 62),
            bar("Weekly", 41),
            bar("daily_cowork", 97)
        ]
    );
}

#[test]
fn a_known_kind_that_disappears_is_not_shown() {
    let row = row_for(json!({
        "limits": [ { "kind": "weekly_all", "percent": 41 } ],
    }));

    assert_eq!(ignoring_reset_times(&row.limits), [bar("Weekly", 41)]);
}

/// A usage body (Session 62%) carrying `extra` next to its limits. The fake
/// clock reads 2026-10-25T14:02Z (a Sunday) in UTC.
fn body_with(extra: Value) -> Value {
    let mut body = json!({ "limits": [ { "kind": "session", "percent": 62 } ] });
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    body
}

fn grant(fields: Value) -> Value {
    let mut grant = json!({
        "id": "grant_abc", "label": "", "resets_total": 1, "resets_left": 1,
        "starts_at": null, "ends_at": null, "clears": ["five_hour"],
        "paused": false, "usable_now": true, "use_requires_limit": true,
        "percent_used": {}, "blocking": [],
    });
    grant
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    grant
}

fn banked(grants: Vec<Value>) -> Value {
    json!({ "cedar_ember": {
        "eligible": true, "ineligible_reason": null, "at_limit": false, "exhausted": [],
        "grants": grants, "next_grant_id": "grant_abc", "weekly_resets_at": null,
        "cooldown_until": null, "event_props": {},
    }})
}

#[test]
fn a_banked_reset_shows_its_count_soonest_expiry_and_the_limits_it_clears() {
    let row = row_for(body_with(banked(vec![
        grant(
            json!({ "id": "a", "resets_left": 2, "ends_at": "2026-11-20T00:00:00Z",
                      "clears": ["seven_day", "seven_day_overage_included"] }),
        ),
        grant(
            json!({ "id": "b", "resets_left": 1, "starts_at": "2026-10-01T00:00:00Z",
                      "ends_at": "2026-11-03T09:00:00Z",
                      "clears": ["five_hour", "seven_day_opus", "brand_new_limit"] }),
        ),
    ])));

    assert_eq!(
        row.offers,
        ["3 Banked resets · clear Session, Weekly, Opus · soonest expires Nov 3"]
    );
    assert_eq!(
        ignoring_reset_times(&row.limits),
        [bar("Session", 62)],
        "bars are unaffected"
    );
}

#[test]
fn paused_unstarted_and_expired_grants_are_not_counted() {
    let row = row_for(body_with(banked(vec![
        grant(json!({ "id": "paused", "resets_left": 5, "paused": true,
                      "ends_at": "2026-10-26T00:00:00Z" })),
        grant(json!({ "id": "later", "resets_left": 5, "starts_at": "2026-10-26T00:00:00Z" })),
        grant(json!({ "id": "gone", "resets_left": 5, "ends_at": "2026-10-25T14:00:00Z" })),
        grant(json!({ "id": "open", "resets_left": 1, "clears": ["seven_day_sonnet"] })),
    ])));

    assert_eq!(row.offers, ["1 Banked reset · clears Sonnet"]);
}

#[test]
fn an_expiry_within_the_week_is_named_by_its_weekday() {
    let row = row_for(body_with(banked(vec![grant(
        json!({ "ends_at": "2026-10-29T10:00:00Z" }),
    )])));

    assert_eq!(
        row.offers,
        ["1 Banked reset · clears Session · expires Thu"]
    );
}

#[test]
fn no_banked_reset_line_when_null_ineligible_or_nothing_usable_never_zero() {
    for extra in [
        json!({ "cedar_ember": null }),
        json!({}),
        json!({ "cedar_ember": { "eligible": false, "ineligible_reason": "tier", "grants": [] } }),
        banked(vec![]),
        banked(vec![grant(json!({ "paused": true }))]),
    ] {
        let row = row_for(body_with(extra.clone()));
        assert_eq!(row.offers, Vec::<String>::new(), "{extra}");
    }
}

#[test]
fn a_malformed_banked_reset_block_is_ignored() {
    let mut no_eligible = banked(vec![grant(json!({}))]);
    no_eligible["cedar_ember"]
        .as_object_mut()
        .unwrap()
        .remove("eligible");
    for extra in [
        no_eligible,
        json!({ "cedar_ember": "yes" }),
        json!({ "cedar_ember": { "eligible": "true", "grants": [] } }),
        banked(vec![grant(json!({ "ends_at": "next tuesday" }))]),
        banked(vec![grant(json!({ "resets_left": -1 }))]),
        banked(vec![grant(json!({ "resets_left": null }))]),
        banked(vec![grant(json!({ "id": "Not A Valid Id!" }))]),
        banked(vec![grant(json!({})), json!("not a grant")]),
    ] {
        let row = row_for(body_with(extra.clone()));
        assert_eq!(row.offers, Vec::<String>::new(), "{extra}");
        assert_eq!(
            ignoring_reset_times(&row.limits),
            [bar("Session", 62)],
            "limits still read: {extra}"
        );
    }
}

fn refill(fields: Value) -> Value {
    let mut block = json!({
        "eligible": true, "ineligible_reason": null, "in_experiment": true, "arm": "reset",
        "available": false, "next_available_at": null,
        "weekly_resets_at": "2026-10-30T09:00:00Z", "resets_per_week": 1,
    });
    block
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    json!({ "juniper_tide": block })
}

#[test]
fn an_available_session_refill_says_so() {
    let row = row_for(body_with(refill(json!({ "available": true }))));

    assert_eq!(row.offers, ["Session refill available"]);
}

#[test]
fn an_unavailable_session_refill_names_the_day_of_the_next_one() {
    let soon = row_for(body_with(refill(
        json!({ "next_available_at": "2026-10-27T08:00:00Z" }),
    )));
    let later = row_for(body_with(refill(
        json!({ "next_available_at": "2026-11-04T08:00:00Z" }),
    )));

    assert_eq!(soon.offers, ["Next refill Tue"]);
    assert_eq!(later.offers, ["Next refill Nov 4"]);
}

#[test]
fn no_session_refill_line_when_null_ineligible_or_unreadable() {
    for extra in [
        json!({ "juniper_tide": null }),
        json!({ "juniper_tide": [] }),
        refill(json!({ "eligible": false, "available": true })),
        refill(json!({ "next_available_at": "soon" })),
        json!({ "juniper_tide": {} }),
    ] {
        let row = row_for(body_with(extra.clone()));
        assert_eq!(row.offers, Vec::<String>::new(), "{extra}");
        assert_eq!(
            ignoring_reset_times(&row.limits),
            [bar("Session", 62)],
            "limits still read: {extra}"
        );
    }
}

#[test]
fn a_banked_reset_and_a_session_refill_show_together() {
    let mut extra = banked(vec![grant(json!({}))]);
    extra.as_object_mut().unwrap().extend(
        refill(json!({ "available": true }))
            .as_object()
            .unwrap()
            .clone(),
    );

    let row = row_for(body_with(extra));

    assert_eq!(
        row.offers,
        [
            "1 Banked reset · clears Session",
            "Session refill available"
        ]
    );
}

#[test]
fn reading_a_banked_reset_or_session_refill_never_uses_one() {
    let mut extra = banked(vec![grant(json!({ "usable_now": true }))]);
    extra.as_object_mut().unwrap().extend(
        refill(json!({ "available": true }))
            .as_object()
            .unwrap()
            .clone(),
    );
    let mac = mac_signing_in_as(&work(), body_with(extra));
    let mut core = mac.launch();
    add_profile(&mut core);
    let row = rows(&core).into_iter().next().unwrap();
    for item in &row.submenu {
        if let MenuItem::Action { command, .. } = item {
            core.command(command.clone());
        }
    }
    core.event(accroster_core::Event::Tick);

    for call in mac.claude.calls() {
        assert!(
            matches!(
                call.kind,
                StubCall::AuthLogin | StubCall::AuthStatus | StubCall::GetUsage
            ),
            "only sign-in and usage reads ran: {call:?}"
        );
        let said = format!("{:?} {}", call.args, call.stdin);
        assert!(
            !said.contains("reset") && !said.contains("cedar") && !said.contains("juniper"),
            "nothing asked Claude Code to redeem or refill: {said}"
        );
    }
}

#[test]
fn every_signed_in_row_offers_limit_resets_on_claude_ai_settings_usage() {
    let mac = mac_signing_in_as(&work(), Value::Null);
    let mut core = mac.launch();
    add_profile(&mut core);

    let row = rows(&core).into_iter().next().expect("a row");
    let command = row
        .submenu
        .into_iter()
        .find_map(|item| match item {
            MenuItem::Action { title, command } if title == "Limit resets → claude.ai" => {
                Some(command)
            }
            _ => None,
        })
        .expect("the row offers Limit resets → claude.ai, even without a reading");

    assert_eq!(
        core.command(command),
        [Effect::OpenUrl("https://claude.ai/settings/usage".into())]
    );
}
