//! Discovered Profiles: accounts the Mac already has evidence of (desktop
//! Sidebar folders, today's `~/.claude.json` login), shown greyed until signed
//! in, matched by their exact account and Organization.

mod common;

use accroster_core::{Command, Core, DiscoveredRow, Effect, Event, MenuItem};
use accroster_fake_mac::{FakeMac, FixtureLogin};
use common::*;
use serde_json::json;

const WORK_ACCOUNT: &str = "55555555-5555-4555-8555-555555555555";
const WORK_ORG: &str = "66666666-6666-4666-8666-666666666666";

/// 2026-10-24T09:30:00Z and 2026-10-23T09:30:00Z, in epoch ms (the fake
/// Mac shows times in UTC).
const OCT_24: i64 = 1_761_298_200_000;
const OCT_23: i64 = OCT_24 - 86_400_000;

fn discovered(core: &Core) -> Vec<DiscoveredRow> {
    core.view()
        .menu
        .items
        .into_iter()
        .filter_map(|item| match item {
            MenuItem::Discovered(row) => Some(row),
            _ => None,
        })
        .collect()
}

fn titles(core: &Core) -> Vec<String> {
    discovered(core).into_iter().map(|row| row.title).collect()
}

/// Sends the command of the submenu item titled `title` in `row`.
fn click_in(core: &mut Core, row: &DiscoveredRow, title: &str) -> Vec<Effect> {
    let command = row
        .submenu
        .iter()
        .find_map(|item| match item {
            MenuItem::Action { title: t, command } if t == title => Some(command.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{title} in {:?}", row.submenu));
    core.command(command)
}

fn details(row: &DiscoveredRow) -> Vec<String> {
    row.submenu
        .iter()
        .filter_map(|item| match item {
            MenuItem::Detail(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn entry(last_activity_ms: i64) -> serde_json::Value {
    json!({
        "sessionId": "local_x", "cliSessionId": "3c", "title": "Fix login bug",
        "isArchived": false, "createdAt": last_activity_ms - 60_000,
        "lastActivityAt": last_activity_ms, "cwd": "/Users/me/Dev/app",
    })
}

/// A Mac with no `~/.claude.json` login, so only Sidebar folders count.
fn mac_without_cli_login() -> FakeMac {
    let mac = FakeMac::new();
    mac.write_json(".claude/settings.json", &json!({}));
    mac
}

#[test]
fn a_sidebar_folder_with_entries_is_an_unknown_login_with_its_sessions() {
    let mac = mac_without_cli_login();
    mac.write_sidebar_entry(WORK_ACCOUNT, WORK_ORG, "a", &entry(OCT_23));
    mac.write_sidebar_entry(WORK_ACCOUNT, WORK_ORG, "b", &entry(OCT_24));
    mac.write_desktop_config(&json!({ "lastKnownAccountUuid": WORK_ACCOUNT }));

    let core = mac.launch();

    let rows = discovered(&core);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].title, "Sign in to Unknown Login…");
    assert_eq!(
        rows[0].subtitle.as_deref(),
        Some("2 Sessions · last active Oct 24 · Desktop's last account")
    );
    assert_eq!(
        details(&rows[0]),
        ["Account 55555555 · Organization 66666666"],
        "uuids only in hover, first 8 characters"
    );
    assert!(!rows[0].title.contains("5555"));
    assert!(
        rows[0]
            .submenu
            .iter()
            .all(|item| !matches!(item, MenuItem::Profile(_)))
    );
}

#[test]
fn todays_cli_login_is_named_by_its_email_and_organization() {
    let mac = FakeMac::new();
    let mut claude_json = FixtureLogin::perso().claude_json();
    claude_json["oauthAccount"]["profileFetchedAt"] = json!(OCT_24);
    mac.with_claude_home(&FixtureLogin::perso());
    mac.write_json(".claude.json", &claude_json);

    let core = mac.launch();

    let rows = discovered(&core);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        rows[0].title,
        "Sign in to me@example.com · me@example.com's Organization…"
    );
    assert_eq!(rows[0].subtitle, None);
    assert_eq!(
        details(&rows[0]),
        [
            "as of Oct 24, 09:30".to_string(),
            "Account 11111111 · Organization 22222222".to_string()
        ]
    );
}

#[test]
fn one_pair_found_in_both_sources_is_one_row() {
    let mac = FakeMac::new();
    let perso = FixtureLogin::perso();
    mac.with_claude_home(&perso);
    mac.write_sidebar_entry(
        &perso.account_uuid,
        &perso.organization_uuid,
        "a",
        &entry(OCT_24),
    );
    mac.write_sidebar_entry(WORK_ACCOUNT, WORK_ORG, "b", &entry(OCT_23));
    // The same account in a second Organization is a second row.
    mac.write_sidebar_entry(&perso.account_uuid, WORK_ORG, "c", &entry(OCT_23));

    let core = mac.launch();

    assert_eq!(
        titles(&core),
        [
            "Sign in to me@example.com · me@example.com's Organization…",
            "Sign in to Unknown Login…",
            "Sign in to Unknown Login…",
        ]
    );
}

#[test]
fn nothing_but_sidebar_entries_and_the_cli_login_counts_as_evidence() {
    let mac = mac_without_cli_login();
    let folder = format!(
        "Library/Application Support/Claude/claude-code-sessions/{WORK_ACCOUNT}/{WORK_ORG}"
    );
    let other = FixtureLogin::new(WORK_ACCOUNT, WORK_ORG, "w@example.com", "W");
    // An org folder with no readable Sidebar entry.
    mac.write_file(format!("{folder}/deleted_3c"), "x");
    mac.write_json(
        format!("{folder}/archived-sessions.idx"),
        &json!({"v": 1, "archived": ["local_a"]}),
    );
    mac.write_json(format!("{folder}/scheduled-tasks.json"), &json!({}));
    mac.write_file(format!("{folder}/local_empty.json"), "");
    mac.write_file(format!("{folder}/local_bad.json"), "{not json");
    mac.write_json(format!("{folder}/backlog/local_x.json"), &entry(OCT_24));
    std::fs::create_dir_all(mac.path(format!(
        "Library/Application Support/Claude/claude-code-sessions/77777777-7777-4777-8777-777777777777/{WORK_ORG}"
    )))
    .unwrap();
    // Logins outside `~/.claude.json`, and id-only desktop facts.
    mac.write_json(".claude/.claude.json", &other.claude_json());
    mac.write_json(".claude.json.backup", &other.claude_json());
    mac.write_json(".claude.json", &json!({"hasCompletedOnboarding": true}));
    mac.write_desktop_config(&json!({
        "lastKnownAccountUuid": WORK_ACCOUNT,
        "bootSideSessions": { "accountUuid": WORK_ACCOUNT },
    }));
    mac.write_json(
        "Library/Application Support/Claude/remote-control-state.json",
        &json!({ "identities": { format!("{WORK_ORG}:{WORK_ACCOUNT}"): {} } }),
    );

    let core = mac.launch();

    assert!(discovered(&core).is_empty(), "{:?}", discovered(&core));
}

#[test]
fn signing_in_to_a_discovered_profile_passes_its_email_and_replaces_its_row() {
    let perso = FixtureLogin::perso();
    let mac = mac_signing_in_as(&perso, limits_body());
    let mut core = mac.launch();
    let row = discovered(&core).remove(0);

    core.command(row.command.clone());
    wait_for_sign_in(&mut core);

    assert_eq!(
        sign_in_call(&mac).args,
        ["auth", "login", "--claudeai", "--email", "me@example.com"]
    );
    assert!(discovered(&core).is_empty(), "{:?}", discovered(&core));
    assert_eq!(
        profile_titles(&core),
        ["me@example.com · me@example.com's Organization"]
    );
    assert!(warnings(&core).is_empty(), "{:?}", warnings(&core));
}

#[test]
fn an_unknown_login_signs_in_without_an_email_and_its_exact_pair_matches() {
    let work = FixtureLogin::new(WORK_ACCOUNT, WORK_ORG, "me@work.example", "Work Inc");
    let mac = mac_signing_in_as(&work, limits_body());
    mac.write_json(".claude.json", &json!({"hasCompletedOnboarding": true}));
    mac.write_sidebar_entry(WORK_ACCOUNT, WORK_ORG, "a", &entry(OCT_24));
    let mut core = mac.launch();
    let row = discovered(&core).remove(0);
    assert_eq!(row.title, "Sign in to Unknown Login…");

    core.command(row.command.clone());
    wait_for_sign_in(&mut core);

    assert_eq!(sign_in_call(&mac).args, ["auth", "login", "--claudeai"]);
    assert!(discovered(&core).is_empty(), "the exact pair matched");
    assert_eq!(profile_titles(&core), ["me@work.example · Work Inc"]);
    assert!(warnings(&core).is_empty(), "{:?}", warnings(&core));
}

#[test]
fn signing_in_as_another_account_keeps_it_and_leaves_the_discovered_profile() {
    let mac = mac_signing_in_as(&work(), limits_body());
    mac.write_sidebar_entry(WORK_ACCOUNT, WORK_ORG, "a", &entry(OCT_24));
    let mut core = mac.launch();
    let unknown = discovered(&core)
        .into_iter()
        .find(|row| row.title == "Sign in to Unknown Login…")
        .unwrap();

    core.command(unknown.command.clone());
    wait_for_sign_in(&mut core);

    assert_eq!(
        profile_titles(&core),
        ["me@work.example · Work Inc"],
        "the new Profile is kept"
    );
    assert_eq!(
        titles(&core),
        [
            "Sign in to me@example.com · me@example.com's Organization…",
            "Sign in to Unknown Login…",
        ],
        "both Discovered Profiles stay greyed"
    );
    assert_eq!(
        warnings(&core),
        [
            "Signed in as me@work.example · Work Inc, not Unknown Login (account 55555555) · to add it, pick that account and Organization in the browser"
        ]
    );
}

#[test]
fn a_hidden_discovered_profile_is_remembered_and_restorable_from_settings() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.write_sidebar_entry(WORK_ACCOUNT, WORK_ORG, "a", &entry(OCT_24));
    let sidebar_before = sidebar_files(&mac);
    let mut core = mac.launch();
    let unknown = discovered(&core).remove(1);

    click_in(&mut core, &unknown, "Hide");

    assert_eq!(
        titles(&core),
        ["Sign in to me@example.com · me@example.com's Organization…"]
    );
    drop(core);
    let mut core = mac.launch();
    assert_eq!(titles(&core).len(), 1, "Hide is remembered");
    let hidden = core.view().settings.hidden_discovered;
    assert_eq!(hidden.len(), 1, "Settings > Hidden (1)…");
    assert_eq!(hidden[0].title, "Unknown Login (account 55555555)");
    assert_eq!(
        sidebar_files(&mac),
        sidebar_before,
        "Sidebar entries untouched"
    );

    core.command(hidden[0].restore.clone());

    assert_eq!(titles(&core).len(), 2);
    assert!(core.view().settings.hidden_discovered.is_empty());
}

#[test]
fn a_nickname_names_the_discovered_profile_and_persists() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    let mut core = mac.launch();
    let row = discovered(&core).remove(0);

    let effects = click_in(&mut core, &row, "Rename…");
    let [
        Effect::AskNickname {
            name,
            current,
            target,
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(name, "me@example.com · me@example.com's Organization");
    assert_eq!(current, &None);
    core.command(Command::SetNickname {
        target: target.clone(),
        nickname: Some(" Perso ".into()),
    });

    let named = ["Sign in to Perso (me@example.com · me@example.com's Organization)…"];
    assert_eq!(titles(&core), named);
    drop(core);
    let mut core = mac.launch();
    assert_eq!(titles(&core), named);

    core.command(Command::SetNickname {
        target: target.clone(),
        nickname: Some("  ".into()),
    });
    assert_eq!(
        titles(&core),
        ["Sign in to me@example.com · me@example.com's Organization…"]
    );
}

#[test]
fn opening_the_menu_rescans_new_and_emptied_pairs() {
    let mac = mac_without_cli_login();
    let gone = mac.write_sidebar_entry(WORK_ACCOUNT, WORK_ORG, "a", &entry(OCT_24));
    let mut core = mac.launch();
    assert_eq!(titles(&core).len(), 1);

    std::fs::remove_file(gone).unwrap();
    mac.write_sidebar_entry(
        WORK_ACCOUNT,
        "88888888-8888-4888-8888-888888888888",
        "b",
        &entry(OCT_23),
    );
    assert_eq!(
        details(&discovered(&core)[0]),
        ["Account 55555555 · Organization 66666666"],
        "nothing changes until the menu opens"
    );
    core.event(Event::MenuOpened);

    let rows = discovered(&core);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        details(&rows[0]),
        ["Account 55555555 · Organization 88888888"]
    );
    assert!(warnings(&core).is_empty(), "new pairs appear silently");
}

#[test]
fn discovery_never_shows_limits_or_runs_claude() {
    let mac = mac_without_cli_login();
    mac.write_sidebar_entry(WORK_ACCOUNT, WORK_ORG, "a", &entry(OCT_24));
    let mut core = mac.launch();
    core.event(Event::MenuOpened);

    assert!(rows(&core).is_empty());
    assert!(mac.claude.calls().is_empty(), "{:?}", mac.claude.calls());
}

fn profile_titles(core: &Core) -> Vec<String> {
    rows(core).into_iter().map(|row| row.title).collect()
}

fn sidebar_files(mac: &FakeMac) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    mac.snapshot()
        .into_iter()
        .filter(|(path, _)| path.to_string_lossy().contains("claude-code-sessions"))
        .collect()
}
