//! Helpers shared by the Profile tests: clicking through the menu view model,
//! waiting for a sign-in, and canned Claude Code replies (field names from the
//! research digest; ids and emails made up).

#![allow(dead_code)] // Each test file uses its own subset.

use std::time::{Duration, Instant};

use accroster_core::{Core, Effect, Event, LimitBar, MenuItem, ProfileRow};
use accroster_fake_mac::{FakeMac, FixtureLogin, RecordedCall, Reply, StubCall};
use serde_json::{Value, json};

pub const APP_FOLDER: &str = "Library/Application Support/io.github.jbmanllr.accroster";
pub const PROFILES: &str = "Library/Application Support/io.github.jbmanllr.accroster/profiles";
pub const WAITING: &str = "Waiting for sign-in… Cancel";

/// A work Login, distinct from [`FixtureLogin::perso`].
pub fn work() -> FixtureLogin {
    FixtureLogin::new(
        "33333333-3333-4333-8333-333333333333",
        "44444444-4444-4444-8444-444444444444",
        "me@work.example",
        "Work Inc",
    )
}

/// Clicks the menu item titled `title`, as the UI layer does.
pub fn click(core: &mut Core, title: &str) {
    let command = core
        .view()
        .menu
        .items
        .into_iter()
        .find_map(|item| match item {
            MenuItem::Action { title: t, command } if t == title => Some(command),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the menu has {title}: {:?}", core.view().menu.items));
    core.command(command);
}

pub fn action_titles(core: &Core) -> Vec<String> {
    core.view()
        .menu
        .items
        .into_iter()
        .filter_map(|item| match item {
            MenuItem::Action { title, .. } => Some(title),
            _ => None,
        })
        .collect()
}

pub fn warnings(core: &Core) -> Vec<String> {
    core.view()
        .menu
        .items
        .into_iter()
        .filter_map(|item| match item {
            MenuItem::Warning(text) => Some(text),
            _ => None,
        })
        .collect()
}

pub fn rows(core: &Core) -> Vec<ProfileRow> {
    core.view()
        .menu
        .items
        .into_iter()
        .filter_map(|item| match item {
            MenuItem::Profile(row) => Some(row),
            _ => None,
        })
        .collect()
}

/// Sends ticks until the waiting line is gone (the sign-in process exited and
/// the core noticed) and any usage read it started has landed, failing
/// after a few seconds of real time.
pub fn wait_for_sign_in(core: &mut Core) -> Vec<Effect> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut effects = Vec::new();
    while busy(core) {
        assert!(Instant::now() < deadline, "the sign-in never finished");
        std::thread::sleep(Duration::from_millis(20));
        effects.extend(core.event(Event::Tick));
    }
    effects
}

/// Whether the core is still waiting on a sign-in or a usage read: it then
/// asks for ticks more often than once a second.
fn busy(core: &Core) -> bool {
    action_titles(core).iter().any(|title| title == WAITING)
        || core.tick_interval() < Duration::from_secs(1)
}

/// Ticks until nothing the core started is still running, returning the
/// effects those ticks asked for.
pub fn settle(core: &mut Core) -> Vec<Effect> {
    let mut effects = core.event(Event::Tick);
    effects.extend(wait_for_sign_in(core));
    effects
}

/// Runs `act` on another thread and fails unless it returns within a few
/// seconds: the core never waits on a Claude Code run that doesn't end.
pub fn promptly<T: Send + 'static>(act: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(act());
    });
    received
        .recv_timeout(Duration::from_secs(5))
        .expect("the core waited on a Claude Code run that never ends")
}

/// Sends `command` and fails unless the core answers within a few seconds.
pub fn command_promptly(core: Core, command: accroster_core::Command) -> (Core, Vec<Effect>) {
    promptly(move || {
        let mut core = core;
        let effects = core.command(command);
        (core, effects)
    })
}

/// Sends `event` and fails unless the core answers within a few seconds.
pub fn event_promptly(core: Core, event: Event) -> (Core, Vec<Effect>) {
    promptly(move || {
        let mut core = core;
        let effects = core.event(event);
        (core, effects)
    })
}

/// Waits until the stub has received `count` calls of `kind` (the last of
/// them may still be running), failing after a few seconds.
pub fn wait_for_calls(mac: &FakeMac, kind: StubCall, count: usize) -> Vec<RecordedCall> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let calls: Vec<RecordedCall> = mac
            .claude
            .calls()
            .into_iter()
            .filter(|call| call.kind == kind)
            .collect();
        if calls.len() >= count {
            return calls;
        }
        assert!(
            Instant::now() < deadline,
            "{kind:?} never ran {count} times"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Moves the fake clock on by `by`, then lets the core act on it.
pub fn pass(mac: &FakeMac, core: &mut Core, by: jiff::SignedDuration) -> Vec<Effect> {
    mac.clock.advance(by);
    settle(core)
}

/// How many usage reads (`get_usage` requests) Claude Code has received.
pub fn usage_reads(mac: &FakeMac) -> usize {
    mac.claude
        .calls()
        .iter()
        .filter(|call| call.kind == StubCall::GetUsage)
        .count()
}

/// Clicks Add Profile… and waits for the sign-in to end.
pub fn add_profile(core: &mut Core) -> Vec<Effect> {
    click(core, "Add Profile…");
    wait_for_sign_in(core)
}

/// The first sign-in call, once the stub has recorded it.
pub fn sign_in_call(mac: &FakeMac) -> RecordedCall {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(call) = mac
            .claude
            .calls()
            .into_iter()
            .find(|call| call.kind == StubCall::AuthLogin)
        {
            return call;
        }
        assert!(Instant::now() < deadline, "claude auth login never ran");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The only Profile folder, relative to `~`.
pub fn only_profile_folder(mac: &FakeMac) -> String {
    let folders = mac.list(PROFILES);
    assert_eq!(folders.len(), 1, "{folders:?}");
    format!("{PROFILES}/{}", folders[0])
}

pub fn bar(label: &str, percent: u32) -> LimitBar {
    LimitBar {
        label: label.into(),
        percent,
        reset: false,
    }
}

/// `claude auth login` finishing as `login`: writes its `oauthAccount`.
pub fn sign_in_as(login: &FixtureLogin) -> Reply {
    Reply::ok("Opening browser to sign in…\nLogin successful.\n")
        .writing(".claude.json", login.claude_json().to_string())
}

/// `claude auth status --json` agreeing with `login`.
pub fn auth_status(login: &FixtureLogin) -> Reply {
    Reply::json(&json!({
        "loggedIn": true, "authMethod": "claude.ai", "apiProvider": "firstParty",
        "email": login.email, "orgId": login.organization_uuid,
        "orgName": login.organization_name, "subscriptionType": "max",
    }))
}

pub fn signed_out() -> Reply {
    Reply::ok("Successfully logged out from your Anthropic account.\n")
}

/// A `get_usage` control response carrying `rate_limits`.
pub fn usage_reply(rate_limits: Value) -> Reply {
    Reply::json(&json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": "accroster-usage", "response": {
            "subscription_type": "max",
            "rate_limits_available": true,
            "rate_limits": rate_limits,
            "behaviors": null,
        }},
    }))
}

/// A usage body with `limits[]` (62 / 41 / Fable 92) that disagrees with its
/// legacy `five_hour` / `seven_day` fields, so tests see which one is used.
pub fn limits_body() -> Value {
    json!({
        "five_hour": { "utilization": 10.0, "resets_at": "2026-10-25T16:40:00Z" },
        "seven_day": { "utilization": 20.0, "resets_at": "2026-10-30T09:00:00Z" },
        "limits": [
            { "kind": "session", "group": "session", "percent": 62, "severity": "warning",
              "resets_at": "2026-10-25T16:40:00Z", "scope": null, "is_active": true },
            { "kind": "weekly_all", "group": "weekly", "percent": 41, "severity": "normal",
              "resets_at": "2026-10-30T09:00:00Z", "scope": null, "is_active": true },
            { "kind": "weekly_scoped", "group": "weekly", "percent": 92, "severity": "normal",
              "resets_at": "2026-10-30T09:00:00Z",
              "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null },
              "is_active": true },
        ],
    })
}

/// A Mac whose next Add Profile… signs in as `login` and reads `rate_limits`.
pub fn mac_signing_in_as(login: &FixtureLogin, rate_limits: Value) -> FakeMac {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.claude
        .reply(StubCall::AuthLogin, sign_in_as(login))
        .reply(StubCall::AuthStatus, auth_status(login))
        .reply(StubCall::GetUsage, usage_reply(rate_limits));
    mac
}

/// Makes the only Profile the CLI's Active Profile, as the CLI switcher's
/// pointer records it (the folder's absolute path, verbatim).
pub fn make_cli_active(mac: &FakeMac) {
    let folder = mac.path(only_profile_folder(mac));
    mac.write_file(
        format!("{APP_FOLDER}/bin/cli-active-profile"),
        folder.to_str().expect("UTF-8 path"),
    );
}

/// Clicks the item titled `title` in the hover submenu of the row at `row`,
/// returning the effects the core asked for.
pub fn click_in_row(core: &mut Core, row: usize, title: &str) -> Vec<Effect> {
    let rows = rows(core);
    let command = rows
        .get(row)
        .unwrap_or_else(|| panic!("no row {row}: {rows:?}"))
        .submenu
        .iter()
        .find_map(|item| match item {
            MenuItem::Action { title: t, command } if t == title => Some(command.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("row {row} has {title}: {:?}", rows[row].submenu));
    core.command(command)
}

/// The titles of the clickable items in a row's hover submenu.
pub fn row_actions(row: &ProfileRow) -> Vec<String> {
    row.submenu
        .iter()
        .filter_map(|item| match item {
            MenuItem::Action { title, .. } => Some(title.clone()),
            _ => None,
        })
        .collect()
}

/// The disabled detail lines of a Profile's hover submenu.
pub fn details(row: &ProfileRow) -> Vec<String> {
    row.submenu
        .iter()
        .filter_map(|item| match item {
            MenuItem::Detail(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}
