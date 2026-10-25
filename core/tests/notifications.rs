//! Limit notifications: thresholds that fire once per limit per window, the
//! most-headroom suggestion with its Switch CLI action, mutes and the
//! optional reset notification, all on the fake clock.

mod common;

use accroster_core::{Command, Core, Effect, Event, MenuItem};
use std::path::PathBuf;

use accroster_fake_mac::{FakeMac, FixtureLogin, Reply, StubCall};
use common::*;
use jiff::SignedDuration;
use serde_json::{Value, json};

/// A usage body with a Session limit (resetting at `session_resets`) and a
/// Weekly limit at these percentages.
fn body(session: u32, session_resets: &str, weekly: u32) -> Value {
    json!({
        "limits": [
            { "kind": "session", "percent": session, "resets_at": session_resets },
            { "kind": "weekly_all", "percent": weekly, "resets_at": "2026-10-30T09:00:00Z" },
        ],
    })
}

const SESSION_RESETS: &str = "2026-10-25T16:40:00Z";

/// The notifications among `effects`, as `(title, body)`.
fn notices(effects: &[Effect]) -> Vec<(String, String)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Notify { title, body } => Some((title.clone(), body.clone())),
            Effect::NotifyWithAction { title, body, .. } => Some((title.clone(), body.clone())),
            _ => None,
        })
        .collect()
}

fn titles(effects: &[Effect]) -> Vec<String> {
    notices(effects)
        .into_iter()
        .map(|(title, _)| title)
        .collect()
}

#[test]
fn a_limit_crossing_80_percent_notifies_with_its_reset_time() {
    let mac = mac_signing_in_as(&work(), body(85, SESSION_RESETS, 41));
    let mut core = mac.launch();

    let effects = add_profile(&mut core);

    assert_eq!(
        notices(&effects),
        [(
            "me: Session limit 85% · resets 16:40".to_owned(),
            String::new()
        )]
    );
}

/// Queues the next usage reads' answers, in order (the last one repeats).
fn then_reads(mac: &FakeMac, bodies: &[Value]) {
    for body in bodies {
        mac.claude
            .reply(StubCall::GetUsage, usage_reply(body.clone()));
    }
}

/// The time the next non-active read is due.
const NEXT_READ: SignedDuration = SignedDuration::from_mins(15);

/// Passes `by` on the fake clock and returns the effects of the ticks.
fn after(mac: &FakeMac, core: &mut Core, by: SignedDuration) -> Vec<Effect> {
    mac.clock.advance(by);
    let mut effects = core.event(Event::Tick);
    effects.extend(wait_for_sign_in(core));
    effects
}

#[test]
fn each_threshold_fires_once_per_limit_per_window_even_across_relaunches() {
    let mac = mac_signing_in_as(&work(), body(85, SESSION_RESETS, 41));
    then_reads(
        &mac,
        &[
            body(88, SESSION_RESETS, 41),
            body(96, SESSION_RESETS, 41),
            body(97, SESSION_RESETS, 41),
            body(85, "2026-10-25T21:40:00Z", 41),
        ],
    );
    let mut core = mac.launch();
    assert_eq!(titles(&add_profile(&mut core)).len(), 1);

    let effects = after(&mac, &mut core, NEXT_READ);
    assert_eq!(titles(&effects), Vec::<String>::new(), "80% already fired");

    let effects = after(&mac, &mut core, NEXT_READ);
    assert_eq!(titles(&effects), ["me: Session limit 96% · resets 16:40"]);

    drop(core);
    let mut core = mac.launch();
    let effects = after(&mac, &mut core, NEXT_READ);
    assert_eq!(usage_reads(&mac), 4);
    assert_eq!(
        titles(&effects),
        Vec::<String>::new(),
        "remembered in state/"
    );

    // The next window warns again.
    let effects = after(&mac, &mut core, NEXT_READ);
    assert_eq!(titles(&effects), ["me: Session limit 85% · resets 21:40"]);
}

#[test]
fn crossing_both_thresholds_at_once_notifies_once() {
    let mac = mac_signing_in_as(&work(), body(99, SESSION_RESETS, 41));
    then_reads(&mac, &[body(99, SESSION_RESETS, 41)]);
    let mut core = mac.launch();

    assert_eq!(
        titles(&add_profile(&mut core)),
        ["me: Session limit 99% · resets 16:40"]
    );
    assert_eq!(
        titles(&after(&mac, &mut core, NEXT_READ)),
        Vec::<String>::new()
    );
}

#[test]
fn weekly_model_specific_and_unknown_limits_warn_too() {
    let mac = mac_signing_in_as(
        &work(),
        json!({
            "limits": [
                { "kind": "session", "percent": 10, "resets_at": SESSION_RESETS },
                { "kind": "weekly_all", "percent": 90, "resets_at": "2026-10-30T09:00:00Z" },
                { "kind": "weekly_scoped", "percent": 81, "resets_at": "2026-10-30T09:00:00Z",
                  "scope": { "model": { "display_name": "Fable" } } },
                { "kind": "monthly_new", "percent": 99, "resets_at": "2026-11-01T00:00:00Z" },
            ],
        }),
    );
    let mut core = mac.launch();

    assert_eq!(
        titles(&add_profile(&mut core)),
        [
            "me: Weekly limit 90% · resets Fri",
            "me: Fable weekly limit 81% · resets Fri",
            "me: monthly_new limit 99% · resets Nov 1",
        ]
    );
}

#[test]
fn thresholds_default_to_80_and_95_and_are_editable_in_settings() {
    let mac = mac_signing_in_as(&work(), body(60, SESSION_RESETS, 41));
    let mut core = mac.launch();
    assert_eq!(core.view().settings.notifications.thresholds, [80, 95]);

    core.command(Command::SetThresholds(vec![90, 50, 0, 50, 101]));
    assert_eq!(core.view().settings.notifications.thresholds, [50, 90]);
    drop(core);
    let mut core = mac.launch();
    assert_eq!(
        core.view().settings.notifications.thresholds,
        [50, 90],
        "kept in settings.json"
    );

    assert_eq!(
        titles(&add_profile(&mut core)),
        ["me: Session limit 60% · resets 16:40"]
    );
}

fn mute_item(core: &Core) -> (bool, Command) {
    rows(core)[0]
        .submenu
        .iter()
        .find_map(|item| match item {
            MenuItem::Check {
                title,
                checked,
                command,
            } if title == "Mute notifications" => Some((*checked, command.clone())),
            _ => None,
        })
        .expect("a Mute notifications item")
}

#[test]
fn a_muted_profile_stays_silent() {
    let mac = mac_signing_in_as(&work(), body(50, SESSION_RESETS, 41));
    then_reads(
        &mac,
        &[body(85, SESSION_RESETS, 41), body(96, SESSION_RESETS, 41)],
    );
    let mut core = mac.launch();
    add_profile(&mut core);

    let (checked, mute) = mute_item(&core);
    assert!(!checked);
    core.command(mute);
    assert!(mute_item(&core).0, "checked once muted");

    assert_eq!(
        titles(&after(&mac, &mut core, NEXT_READ)),
        Vec::<String>::new()
    );

    let (_, unmute) = mute_item(&core);
    core.command(unmute);
    assert_eq!(
        titles(&after(&mac, &mut core, NEXT_READ)),
        ["me: Session limit 96% · resets 16:40"],
        "what fired while muted stays quiet"
    );
}

#[test]
fn the_reset_notification_is_off_by_default() {
    let mac = mac_signing_in_as(&work(), body(85, SESSION_RESETS, 41));
    let mut core = mac.launch();
    assert!(!core.view().settings.notifications.reset_notification);
    add_profile(&mut core);

    mac.clock.set("2026-10-25T16:41:00Z".parse().unwrap());
    let mut effects = core.event(Event::Tick);
    effects.extend(wait_for_sign_in(&mut core));
    assert_eq!(titles(&effects), Vec::<String>::new());
}

#[test]
fn the_reset_notification_fires_once_when_a_warned_limit_resets() {
    let mac = mac_signing_in_as(&work(), body(85, SESSION_RESETS, 41));
    let mut core = mac.launch();
    core.command(Command::SetResetNotification(true));
    assert!(core.view().settings.notifications.reset_notification);
    add_profile(&mut core);

    // The re-read after the reset still answers with the old window.
    mac.clock.set("2026-10-25T16:41:00Z".parse().unwrap());
    let mut effects = core.event(Event::Tick);
    effects.extend(wait_for_sign_in(&mut core));
    assert_eq!(titles(&effects), ["me: Session limit reset"]);

    assert_eq!(
        titles(&after(&mac, &mut core, NEXT_READ)),
        Vec::<String>::new()
    );
}

/// A third Login.
fn team() -> FixtureLogin {
    FixtureLogin::new(
        "55555555-5555-4555-8555-555555555555",
        "66666666-6666-4666-8666-666666666666",
        "me@team.example",
        "Team Co",
    )
}

/// A Mac with Profiles "Perso" (work Login, read as `perso`) then "Pro"
/// (team Login, read as `pro`); later reads answer `then`, in order.
fn mac_with_perso_and_pro(perso: Value, pro: Value, then: &[Value]) -> (FakeMac, Core) {
    let mac = mac_signing_in_as(&work(), perso);
    mac.claude
        .reply(StubCall::AuthLogin, sign_in_as(&team()))
        .reply(StubCall::AuthStatus, auth_status(&team()))
        .reply(StubCall::GetUsage, usage_reply(pro));
    then_reads(&mac, then);
    let mut core = mac.launch();
    add_profile(&mut core);
    add_profile(&mut core);
    for (row, name) in [(0, "Perso"), (1, "Pro")] {
        let Some(Effect::AskNickname { target, .. }) =
            click_in_row(&mut core, row, "Rename…").pop()
        else {
            panic!("Rename… asks for a nickname");
        };
        core.command(Command::SetNickname {
            target,
            nickname: Some(name.into()),
        });
    }
    (mac, core)
}

/// The Profile folder signed in as `login`.
fn folder_of(mac: &FakeMac, login: &FixtureLogin) -> PathBuf {
    mac.list(PROFILES)
        .into_iter()
        .map(|id| mac.path(format!("{PROFILES}/{id}")))
        .find(|folder| {
            std::fs::read_to_string(folder.join(".claude.json"))
                .is_ok_and(|text| text.contains(&login.email))
        })
        .expect("a Profile folder for this Login")
}

/// Turns CLI switching on and points it at `login`'s Profile.
fn cli_uses(mac: &FakeMac, core: &mut Core, login: &FixtureLogin) {
    core.command(Command::SetCliSwitching(true));
    mac.write_file(
        format!("{APP_FOLDER}/bin/cli-active-profile"),
        folder_of(mac, login).to_str().expect("UTF-8 path"),
    );
}

fn pointer(mac: &FakeMac) -> String {
    mac.read_file(format!("{APP_FOLDER}/bin/cli-active-profile"))
        .expect("a pointer")
}

/// The CLI's Active Profile is read every 3 minutes.
const NEXT_ACTIVE_READ: SignedDuration = SignedDuration::from_mins(3);

#[test]
fn the_clis_active_profile_suggests_the_most_headroom_and_switches_only_on_click() {
    let (mac, mut core) = mac_with_perso_and_pro(
        body(50, SESSION_RESETS, 41),
        body(12, SESSION_RESETS, 5),
        &[body(96, SESSION_RESETS, 41)],
    );
    cli_uses(&mac, &mut core, &work());
    let perso = pointer(&mac);

    let effects = after(&mac, &mut core, NEXT_ACTIVE_READ);
    let [
        Effect::NotifyWithAction {
            title,
            body,
            action,
            command,
        },
    ] = effects.as_slice()
    else {
        panic!("one notification with an action: {effects:?}");
    };
    assert_eq!(title, "Perso: Session limit 96% · resets 16:40");
    assert_eq!(body, "Pro has the most headroom (12%)");
    assert_eq!(action, "Switch CLI");
    assert_eq!(pointer(&mac), perso, "nothing switches on its own");

    core.command(command.clone());
    assert_eq!(
        PathBuf::from(pointer(&mac)),
        folder_of(&mac, &team()),
        "Switch CLI rewrites the pointer"
    );
}

#[test]
fn no_suggestion_when_no_other_profile_has_more_headroom() {
    let (mac, mut core) = mac_with_perso_and_pro(
        body(50, SESSION_RESETS, 41),
        body(97, SESSION_RESETS, 5),
        &[body(85, SESSION_RESETS, 41)],
    );
    cli_uses(&mac, &mut core, &work());

    let effects = after(&mac, &mut core, NEXT_ACTIVE_READ);
    assert_eq!(
        notices(&effects),
        [(
            "Perso: Session limit 85% · resets 16:40".to_owned(),
            String::new()
        )]
    );
}

#[test]
fn a_profile_that_is_not_the_clis_gets_no_suggestion() {
    let mac = mac_signing_in_as(&work(), body(12, SESSION_RESETS, 5));
    let mut core = mac.launch();
    add_profile(&mut core);
    cli_uses(&mac, &mut core, &work());

    mac.claude
        .reply(StubCall::AuthLogin, sign_in_as(&team()))
        .reply(StubCall::AuthStatus, auth_status(&team()))
        .reply(
            StubCall::GetUsage,
            usage_reply(body(85, SESSION_RESETS, 41)),
        );
    let effects = add_profile(&mut core);

    assert_eq!(
        notices(&effects),
        [(
            "me: Session limit 85% · resets 16:40".to_owned(),
            String::new()
        )]
    );
}

fn markers(core: &Core) -> Vec<bool> {
    rows(core).iter().map(|row| row.most_headroom).collect()
}

#[test]
fn the_most_headroom_marker_counts_unknown_limit_kinds() {
    let (_mac, core) = mac_with_perso_and_pro(
        json!({ "limits": [
            { "kind": "session", "percent": 30, "resets_at": SESSION_RESETS },
            { "kind": "monthly_new", "percent": 90, "resets_at": "2026-11-01T00:00:00Z" },
        ]}),
        body(50, SESSION_RESETS, 41),
        &[],
    );

    assert_eq!(markers(&core), [false, true]);
}

#[test]
fn a_single_profile_has_no_most_headroom_marker() {
    let mac = mac_signing_in_as(&work(), body(10, SESSION_RESETS, 5));
    let mut core = mac.launch();
    add_profile(&mut core);

    assert_eq!(markers(&core), [false]);
}

#[test]
fn a_stale_profile_never_gets_the_marker() {
    let (mac, mut core) = mac_with_perso_and_pro(
        body(50, SESSION_RESETS, 41),
        body(12, SESSION_RESETS, 5),
        &[],
    );
    assert_eq!(markers(&core), [false, true]);

    // Pro's reads stop answering: its 12% is dimmed, Perso's stays fresh.
    mac.claude
        .reply(StubCall::GetUsage, Reply::fail(1, "no answer"))
        .reply(StubCall::SlashUsage, Reply::fail(1, "no answer"));
    after(&mac, &mut core, NEXT_READ);
    assert!(rows(&core)[1].dimmed, "{:?}", rows(&core)[1]);
    assert_eq!(markers(&core), [false, false], "one fresh Profile left");
}
