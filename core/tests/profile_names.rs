//! How a Profile is named: its row title (the Profile name), its short label
//! (menu bar, header) and which Login the CLI uses without CLI switching.

mod common;

use accroster_core::{Command, Core, MenuItem};
use accroster_fake_mac::{FakeMac, FixtureLogin, StubCall};
use common::*;
use serde_json::json;

/// A personal Login: its Organization is Claude's default
/// `<email>'s Organization`.
fn lucie() -> FixtureLogin {
    FixtureLogin::new(
        "55555555-5555-4555-8555-555555555555",
        "66666666-6666-4666-8666-666666666666",
        "lucie@example.com",
        "lucie@example.com's Organization",
    )
    .named(Some("Lucie Martin"), Some("Lucie"))
}

/// A Mac with one Profile signed in as `login`; `~/.claude.json` is
/// signed in as [`FixtureLogin::perso`].
fn one_profile(login: &FixtureLogin) -> (FakeMac, Core) {
    let mac = mac_signing_in_as(login, limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);
    (mac, core)
}

fn titles(core: &Core) -> Vec<String> {
    rows(core).into_iter().map(|row| row.title).collect()
}

/// The menu header's CLI part.
fn cli_header(core: &Core) -> Option<String> {
    core.view()
        .menu
        .items
        .into_iter()
        .find_map(|item| match item {
            MenuItem::Detail(text) => text
                .split("    ")
                .find(|part| part.starts_with("CLI: "))
                .map(str::to_owned),
            _ => None,
        })
}

/// The short label the header gives the only Profile once it is the CLI's
/// Active Profile.
fn short_label(mac: &FakeMac, core: &mut Core) -> String {
    make_cli_active(mac);
    core.command(Command::SetCliSwitching(true));
    let header = cli_header(core).expect("a CLI part");
    header.trim_start_matches("CLI: ").to_owned()
}

#[test]
fn a_personal_profile_is_named_by_its_full_name_then_its_email() {
    let (_mac, core) = one_profile(&lucie());

    assert_eq!(titles(&core), ["Lucie Martin · lucie@example.com"]);
    assert_eq!(
        details(&rows(&core)[0])[0],
        "lucie@example.com · lucie@example.com's Organization · Max",
        "the hover line keeps the Organization"
    );
}

#[test]
fn without_a_full_name_the_display_name_then_the_email_alone_names_it() {
    let display_only = lucie().named(None, Some("Lucie"));
    let (_mac, core) = one_profile(&display_only);
    assert_eq!(titles(&core), ["Lucie · lucie@example.com"]);

    let unnamed = lucie().named(Some(""), None);
    let (_mac, core) = one_profile(&unnamed);
    assert_eq!(titles(&core), ["lucie@example.com"]);
}

#[test]
fn an_organization_other_than_the_personal_default_is_shown() {
    let team = FixtureLogin::new(
        &lucie().account_uuid,
        "77777777-7777-4777-8777-777777777777",
        "lucie@example.com",
        "Research Lab",
    )
    .named(Some("Lucie Martin"), Some("Lucie"));
    let (_mac, core) = one_profile(&team);

    assert_eq!(
        titles(&core),
        ["Lucie Martin · lucie@example.com · Research Lab"]
    );
}

#[test]
fn the_short_label_is_the_display_name_else_the_first_word_of_the_full_name() {
    let (mac, mut core) = one_profile(&lucie());
    assert_eq!(short_label(&mac, &mut core), "Lucie");

    let full_only = lucie().named(Some("Lucie Martin"), None);
    let (mac, mut core) = one_profile(&full_only);
    assert_eq!(short_label(&mac, &mut core), "Lucie");

    let unnamed = lucie().named(None, None);
    let (mac, mut core) = one_profile(&unnamed);
    assert_eq!(short_label(&mac, &mut core), "lucie");
}

#[test]
fn without_cli_switching_the_header_names_the_default_folders_login() {
    let mac = mac_signing_in_as(&lucie(), limits_body());
    mac.with_claude_home(&lucie());
    let mut core = mac.launch();
    assert_eq!(
        cli_header(&core).as_deref(),
        Some("CLI: Lucie Martin · lucie@example.com (default login)"),
        "not yet a Profile: named in full"
    );

    add_profile(&mut core);

    assert_eq!(
        cli_header(&core).as_deref(),
        Some("CLI: Lucie (default login)")
    );
}

#[test]
fn without_cli_switching_the_default_logins_row_says_so() {
    let mac = mac_signing_in_as(&work(), limits_body());
    mac.claude
        .reply(StubCall::AuthLogin, sign_in_as(&FixtureLogin::perso()))
        .reply(StubCall::AuthStatus, auth_status(&FixtureLogin::perso()))
        .reply(StubCall::GetUsage, usage_reply(limits_body()));
    let mut core = mac.launch();
    add_profile(&mut core);
    add_profile(&mut core);

    let notes: Vec<bool> = rows(&core).iter().map(|row| row.default_login).collect();
    assert_eq!(notes, [false, true], "~/.claude.json is signed in as perso");

    core.command(Command::SetCliSwitching(true));

    assert!(
        rows(&core).iter().all(|row| !row.default_login),
        "with CLI switching on the CLI badge says which Profile the CLI uses"
    );
}

#[test]
fn without_a_default_login_the_header_keeps_saying_default() {
    let mac = FakeMac::new();
    mac.write_json(".claude/settings.json", &json!({}));
    mac.write_json(".claude.json", &json!({ "hasCompletedOnboarding": true }));
    mac.os.set_desktop_app_version(Some("3.6556.2"));
    mac.write_desktop_config(&json!({ "lastKnownAccountUuid": work().account_uuid }));
    mac.write_sidebar_entry(
        &work().account_uuid,
        &work().organization_uuid,
        "a",
        &json!({ "sessionId": "local_a", "cliSessionId": "a", "title": "Fix the build" }),
    );
    let core = mac.launch();

    assert_eq!(cli_header(&core).as_deref(), Some("CLI: Default"));
}

#[test]
fn a_discovered_login_is_named_like_a_profile() {
    let mac = FakeMac::new();
    mac.with_claude_home(&lucie());
    let core = mac.launch();

    let discovered: Vec<String> = core
        .view()
        .menu
        .items
        .into_iter()
        .filter_map(|item| match item {
            MenuItem::Discovered(row) => Some(row.title),
            _ => None,
        })
        .collect();
    assert_eq!(discovered, ["Sign in to Lucie Martin · lucie@example.com…"]);
}
