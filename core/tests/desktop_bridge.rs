//! The Desktop bridge: which Profile the Claude desktop app uses, the busy
//! guard, and the Guided switch that walks the user through signing out and
//! in. AccRoster never quits Claude.

mod common;

use std::time::{Duration, SystemTime};

use accroster_core::{
    ClaudeVersions, Command, Core, Effect, Event, MenuItem, ProfileRow, SidebarSyncMode,
};
use accroster_fake_mac::{FakeMac, FixtureLogin, StubCall};
use common::*;
use serde_json::json;

/// When the fake desktop children started, and the same instant as Claude
/// Code writes it in `procStart` (`ps -o lstart`, in the fake Mac's UTC).
const CHILD_STARTED: &str = "2026-10-25T13:00:00Z";
const CHILD_PROC_START: &str = "Sun Oct 25 13:00:00 2026";

/// A Mac with Claude installed and two signed-in Profiles, perso then work.
fn mac_with_two_profiles() -> (FakeMac, Core) {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.os.set_desktop_app_version(Some("3.6556.2"));
    for login in [FixtureLogin::perso(), work()] {
        mac.claude
            .reply(StubCall::AuthLogin, sign_in_as(&login))
            .reply(StubCall::AuthStatus, auth_status(&login));
    }
    mac.claude
        .reply(StubCall::GetUsage, usage_reply(limits_body()));
    let mut core = mac.launch();
    add_profile(&mut core);
    add_profile(&mut core);
    assert_eq!(rows(&core).len(), 2, "{:?}", core.view().menu.items);
    (mac, core)
}

/// The desktop's `config.json`, with a token cache next to the one key the
/// bridge reads (it must never surface).
fn desktop_signed_in_as(mac: &FakeMac, login: &FixtureLogin) {
    mac.write_desktop_config(&json!({
        "lastKnownAccountUuid": login.account_uuid,
        "oauth:tokenCacheV2": "sk-ant-oat01-secret-token-value",
    }));
}

/// A Sidebar entry written `age` ago in `login`'s folder.
fn sidebar_entry(mac: &FakeMac, login: &FixtureLogin, id: &str, title: &str, age: Duration) {
    let path = mac.write_sidebar_entry(
        &login.account_uuid,
        &login.organization_uuid,
        id,
        &json!({ "sessionId": format!("local_{id}"), "cliSessionId": id, "title": title }),
    );
    set_age(&path, age);
}

fn set_age(path: &std::path::Path, age: Duration) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - age)
        .unwrap();
}

fn header(core: &Core) -> Option<String> {
    core.view()
        .menu
        .items
        .into_iter()
        .find_map(|item| match item {
            MenuItem::Detail(text) => Some(text),
            _ => None,
        })
}

fn row<'a>(rows: &'a [ProfileRow], title: &str) -> &'a ProfileRow {
    rows.iter()
        .find(|row| row.title == title)
        .unwrap_or_else(|| panic!("a row titled {title}: {rows:?}"))
}

const PERSO: &str = "me@example.com · me@example.com's Organization";
const WORK: &str = "me@work.example · Work Inc";

#[test]
fn the_desktop_badge_and_header_follow_the_account_and_the_folder_claude_writes() {
    let (mac, core) = mac_with_two_profiles();
    assert_eq!(header(&core), None, "no desktop config, no Desktop part");

    desktop_signed_in_as(&mac, &work());
    sidebar_entry(&mac, &work(), "a", "Fix the build", Duration::from_secs(60));
    assert_eq!(
        header(&core).as_deref(),
        Some("CLI: Default    Desktop: me")
    );
    let rows = rows(&core);
    assert_eq!(row(&rows, WORK).badges, ["Desktop"]);
    assert!(row(&rows, PERSO).badges.is_empty());
}

/// Names the Profile in the row titled `title` `nickname`.
fn nickname(core: &mut Core, title: &str, nickname: &str) {
    let effects = click_in_row(core, title, "Rename…");
    let [Effect::AskNickname { target, .. }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    core.command(Command::SetNickname {
        target: target.clone(),
        nickname: Some(nickname.into()),
    });
}

#[test]
fn the_header_names_each_clients_active_profile_by_its_short_name() {
    let (mac, mut core) = mac_with_two_profiles();
    nickname(&mut core, PERSO, "Perso");
    nickname(&mut core, WORK, "Work");
    desktop_signed_in_as(&mac, &work());
    sidebar_entry(&mac, &work(), "a", "Fix the build", Duration::from_secs(60));
    assert_eq!(
        header(&core).as_deref(),
        Some("CLI: Default    Desktop: Work"),
        "the CLI part shows with CLI switching off"
    );

    core.command(Command::SetCliSwitching(true));
    let perso = rows(&core)[0].command.clone().expect("rows switch the CLI");
    core.command(perso);
    assert_eq!(
        header(&core).as_deref(),
        Some("CLI: Perso    Desktop: Work")
    );

    let work_row = rows(&core)[1].command.clone().expect("rows switch the CLI");
    core.command(work_row);
    assert_eq!(header(&core).as_deref(), Some("CLI: Work    Desktop: Work"));
    assert_eq!(row(&rows(&core), "Work").badges, ["CLI", "Desktop"]);
    assert!(row(&rows(&core), "Perso").badges.is_empty());
}

#[test]
fn the_same_login_in_two_organizations_is_told_apart_by_the_folder_written_last() {
    let (mac, core) = mac_with_two_profiles();
    let other_org = FixtureLogin::new(
        &work().account_uuid,
        "55555555-5555-4555-8555-555555555555",
        "me@work.example",
        "Side Org",
    );
    desktop_signed_in_as(&mac, &work());
    sidebar_entry(&mac, &work(), "a", "Old", Duration::from_secs(3600));
    sidebar_entry(&mac, &other_org, "b", "New", Duration::from_secs(10));
    assert_eq!(
        header(&core).as_deref(),
        Some("CLI: Default    Desktop: another Profile"),
        "the newest folder belongs to no Profile"
    );
    assert!(rows(&core).iter().all(|row| row.badges.is_empty()));

    sidebar_entry(&mac, &work(), "c", "Newer", Duration::from_secs(1));
    assert_eq!(
        header(&core).as_deref(),
        Some("CLI: Default    Desktop: me")
    );
}

#[test]
fn the_desktop_config_is_read_for_its_account_only_and_never_logged() {
    let (mac, core) = mac_with_two_profiles();
    desktop_signed_in_as(&mac, &FixtureLogin::perso());
    let _ = core.view();
    let log = mac
        .read_file(format!("{APP_FOLDER}/logs/app.log"))
        .unwrap_or_default();
    assert!(!log.contains("sk-ant"), "{log}");
    let view = format!("{:?}", core.view());
    assert!(!view.contains("sk-ant"), "{view}");
    assert_eq!(
        header(&core).as_deref(),
        Some("CLI: Default    Desktop: me")
    );
}

// ----- The busy guard --------------------------------------------------------------

/// A running desktop Claude Code child, pid `pid`, with its live-process file
/// in `~/.claude/sessions/` for the Sidebar entry `local_<host>`.
fn desktop_child(mac: &FakeMac, pid: u32, host: &str, status: &str) {
    mac.os
        .start_desktop_child(pid, CHILD_STARTED.parse().unwrap());
    live_file(mac, pid, host, status, "claude-desktop", CHILD_PROC_START);
}

fn live_file(
    mac: &FakeMac,
    pid: u32,
    host: &str,
    status: &str,
    entrypoint: &str,
    proc_start: &str,
) {
    mac.write_live_process(
        ".claude",
        pid,
        &json!({
            "pid": pid, "sessionId": format!("cli-{host}"), "cwd": "/Users/tester/Dev/app",
            "startedAt": 1_761_397_200_000_i64, "procStart": proc_start,
            "version": "2.1.308", "kind": "interactive", "entrypoint": entrypoint,
            "hostSessionId": format!("local_{host}"), "status": status,
            "statusUpdatedAt": 1_761_397_300_000_i64, "updatedAt": 1_761_397_300_000_i64,
        }),
    );
}

/// Clicks `item` in the hover submenu of the row titled `row_title`.
fn click_in_row(core: &mut Core, row_title: &str, item: &str) -> Vec<Effect> {
    let rows = rows(core);
    let submenu = &row(&rows, row_title).submenu;
    let command = submenu
        .iter()
        .find_map(|entry| match entry {
            MenuItem::Action { title, command } if title == item => Some(command.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{row_title} has {item}: {submenu:?}"));
    core.command(command)
}

/// Clicks the top-level menu item titled `title` and returns its effects.
fn press(core: &mut Core, title: &str) -> Vec<Effect> {
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
    core.command(command)
}

const SWITCH: &str = "Switch desktop app to this Profile…";

/// Claude signed in as perso, with Sidebar entries "Fix the build" (a) and
/// "Write docs" (b).
fn desktop_on_perso() -> (FakeMac, Core) {
    let (mac, core) = mac_with_two_profiles();
    let perso = FixtureLogin::perso();
    desktop_signed_in_as(&mac, &perso);
    sidebar_entry(&mac, &perso, "a", "Fix the build", Duration::from_secs(120));
    sidebar_entry(&mac, &perso, "b", "Write docs", Duration::from_secs(60));
    (mac, core)
}

fn brought_forward(effects: &[Effect]) -> bool {
    effects.contains(&Effect::BringDesktopAppForward)
}

fn has_switch(row: &ProfileRow) -> bool {
    row.submenu
        .iter()
        .any(|item| matches!(item, MenuItem::Action { title, .. } if title == SWITCH))
}

#[test]
fn a_busy_desktop_session_blocks_the_switch_and_is_named() {
    let (mac, mut core) = desktop_on_perso();
    desktop_child(&mac, 501, "a", "busy");
    desktop_child(&mac, 502, "b", "idle");

    let effects = click_in_row(&mut core, WORK, SWITCH);

    assert!(!brought_forward(&effects), "{effects:?}");
    assert_eq!(
        warnings(&core),
        [format!(
            "Claude is running “Fix the build” · switch to {WORK} not started"
        )]
    );
    assert!(action_titles(&core).contains(&"Notify me when idle".to_string()));
}

#[test]
fn a_waiting_desktop_session_blocks_the_switch_too() {
    let (mac, mut core) = desktop_on_perso();
    desktop_child(&mac, 501, "a", "waiting");
    desktop_child(&mac, 502, "b", "busy");

    let effects = click_in_row(&mut core, WORK, SWITCH);

    assert!(!brought_forward(&effects), "{effects:?}");
    assert_eq!(
        warnings(&core),
        [format!(
            "Claude is running “Fix the build”, “Write docs” · switch to {WORK} not started"
        )]
    );
}

#[test]
fn a_proc_start_with_a_space_padded_day_is_recognised() {
    let (mac, mut core) = desktop_on_perso();
    mac.os
        .start_desktop_child(501, "2026-10-04T13:00:00Z".parse().unwrap());
    live_file(
        &mac,
        501,
        "a",
        "busy",
        "claude-desktop",
        "Sun Oct  4 13:00:00 2026",
    );

    let effects = click_in_row(&mut core, WORK, SWITCH);

    assert!(!brought_forward(&effects), "{effects:?}");
    assert_eq!(
        warnings(&core),
        [format!(
            "Claude is running “Fix the build” · switch to {WORK} not started"
        )]
    );
}

#[test]
fn an_unknown_status_counts_as_busy() {
    let (mac, mut core) = desktop_on_perso();
    desktop_child(&mac, 501, "b", "thinking-hard");

    let effects = click_in_row(&mut core, WORK, SWITCH);

    assert!(!brought_forward(&effects), "{effects:?}");
    assert_eq!(
        warnings(&core),
        [format!(
            "Claude is running “Write docs” · switch to {WORK} not started"
        )]
    );
}

#[test]
fn idle_desktop_sessions_let_the_switch_go_ahead() {
    let (mac, mut core) = desktop_on_perso();
    desktop_child(&mac, 501, "a", "idle");
    // Not the desktop's: a busy Terminal run, a dead pid and a reused pid.
    live_file(&mac, 600, "x", "busy", "cli", CHILD_PROC_START);
    mac.os.start_process(600, 1, CHILD_STARTED.parse().unwrap());
    live_file(&mac, 601, "y", "busy", "claude-desktop", CHILD_PROC_START);
    live_file(
        &mac,
        602,
        "z",
        "busy",
        "claude-desktop",
        "Sat Oct 24 09:00:00 2026",
    );
    mac.os.start_process(602, 1, CHILD_STARTED.parse().unwrap());
    mac.write_file(".claude/sessions/501.key", "not JSON");

    let effects = click_in_row(&mut core, WORK, SWITCH);

    assert!(brought_forward(&effects), "{effects:?}");
    assert!(
        effects.contains(&Effect::Notify {
            title: format!("Switch Claude to {WORK}"),
            body: format!("Sign out, then sign in as {WORK}"),
        }),
        "{effects:?}"
    );
    assert_eq!(
        warnings(&core),
        [format!("In Claude: Sign out, then sign in as {WORK}")]
    );
    assert!(action_titles(&core).contains(&"Cancel switch".to_string()));
}

#[test]
fn a_desktop_child_without_a_file_means_cant_tell_and_asks_first() {
    let (mac, mut core) = desktop_on_perso();
    desktop_child(&mac, 501, "a", "idle");
    mac.os
        .start_desktop_child(503, CHILD_STARTED.parse().unwrap());

    let effects = click_in_row(&mut core, WORK, SWITCH);

    assert!(!brought_forward(&effects), "{effects:?}");
    assert_eq!(
        warnings(&core),
        ["Can't tell whether Claude is running a turn · switching ends every running turn"]
    );
    let effects = press(&mut core, "Switch anyway");
    assert!(brought_forward(&effects), "{effects:?}");
}

#[test]
fn cant_tell_can_be_cancelled() {
    let (mac, mut core) = desktop_on_perso();
    mac.os
        .start_desktop_child(503, CHILD_STARTED.parse().unwrap());
    click_in_row(&mut core, WORK, SWITCH);

    let effects = press(&mut core, "Cancel switch");

    assert!(effects.is_empty(), "{effects:?}");
    assert!(warnings(&core).is_empty(), "{:?}", warnings(&core));
}

#[test]
fn notify_me_when_idle_fires_once_when_the_last_turn_ends() {
    let (mac, mut core) = desktop_on_perso();
    desktop_child(&mac, 501, "a", "busy");
    click_in_row(&mut core, WORK, SWITCH);

    assert!(press(&mut core, "Notify me when idle").is_empty());
    assert!(core.event(Event::Tick).is_empty(), "still busy");
    assert_eq!(
        warnings(&core),
        ["Claude is running “Fix the build” · AccRoster will tell you when it's idle"]
    );

    live_file(&mac, 501, "a", "idle", "claude-desktop", CHILD_PROC_START);
    let effects = core.event(Event::Tick);
    assert_eq!(
        effects,
        [Effect::Notify {
            title: "Claude is idle".into(),
            body: format!("You can switch Claude to {WORK} now"),
        }]
    );
    assert!(core.event(Event::Tick).is_empty(), "only once");
    assert!(warnings(&core).is_empty(), "{:?}", warnings(&core));
}

#[test]
fn the_desktop_profile_offers_no_switch_to_itself_and_no_claude_offers_none() {
    let (mac, core) = desktop_on_perso();
    let current = rows(&core);
    assert!(!has_switch(row(&current, PERSO)));
    assert!(has_switch(row(&current, WORK)));

    mac.os.set_desktop_app_version(None);
    assert!(rows(&core).iter().all(|row| !has_switch(row)));
}

// ----- Versions --------------------------------------------------------------------

#[test]
fn claude_versions_are_read_from_files_without_spawning() {
    let mac = FakeMac::new();
    let mut core = mac.launch();
    assert_eq!(core.view().diagnostics.versions, ClaudeVersions::default());

    mac.write_file(".local/share/claude/versions/2.1.310", "binary");
    std::fs::create_dir_all(mac.path(".local/bin")).unwrap();
    std::os::unix::fs::symlink(
        mac.path(".local/share/claude/versions/2.1.310"),
        mac.path(".local/bin/claude"),
    )
    .unwrap();
    for version in ["2.1.308", "2.1.304", "2.1.1000"] {
        mac.write_file(
            format!(
                "Library/Application Support/Claude/claude-code/{version}/abc123/claude.app/Contents/MacOS/claude"
            ),
            "binary",
        );
    }
    mac.write_file(
        "Library/Application Support/Claude/claude-code/.DS_Store",
        "",
    );
    mac.os.set_desktop_app_version(Some("3.6556.2"));

    // Noticed when the menu opens.
    core.event(Event::MenuOpened);

    assert_eq!(
        core.view().diagnostics.versions,
        ClaudeVersions {
            claude_code: Some("2.1.310".into()),
            desktop_claude_code: vec!["2.1.304".into(), "2.1.308".into(), "2.1.1000".into()],
            desktop: Some("3.6556.2".into()),
        }
    );
    assert!(mac.claude.calls().is_empty());
}

// ----- Confirming the switch -------------------------------------------------------

const MAIN_LOG: &str = "Library/Logs/Claude/main.log";

/// Claude on perso, idle, with an older `main.log`; the user has just clicked
/// Switch for work.
fn switch_started() -> (FakeMac, Core, Vec<Effect>) {
    let (mac, mut core) = desktop_on_perso();
    mac.write_file(
        MAIN_LOG,
        "[account] Login-state transition (loggedOut: true → false, earlier)\n",
    );
    let effects = click_in_row(&mut core, WORK, SWITCH);
    assert!(brought_forward(&effects), "{effects:?}");
    (mac, core, effects)
}

fn append_main_log(mac: &FakeMac, line: &str) {
    let mut log = mac.read_file(MAIN_LOG).unwrap_or_default();
    log.push_str(line);
    mac.write_file(MAIN_LOG, log);
}

#[test]
fn with_sidebar_sync_on_the_switch_first_syncs_into_the_target_profile() {
    let (mac, mut core) = desktop_on_perso();
    mac.os.set_desktop_app_running(true);
    let work_folder = format!(
        "Library/Application Support/Claude/claude-code-sessions/{}/{}",
        work().account_uuid,
        work().organization_uuid
    );
    // Claude was signed in as Work once, so Work has a sidebar.
    std::fs::create_dir_all(mac.path(&work_folder)).unwrap();
    core.command(Command::SetSidebarSync(SidebarSyncMode::Manual));

    let effects = click_in_row(&mut core, WORK, SWITCH);

    assert!(brought_forward(&effects), "{effects:?}");
    let copy: Option<serde_json::Value> = mac.read_json(format!("{work_folder}/local_a.json"));
    assert_eq!(copy.unwrap()["title"], "Fix the build");
    // The copies sync just wrote are not Claude signing in to Work.
    core.event(Event::Tick);
    assert_eq!(
        warnings(&core),
        [format!("In Claude: Sign out, then sign in as {WORK}")]
    );
}

#[test]
fn automatic_sync_waits_while_a_switch_waits_so_its_copies_confirm_nothing() {
    let (mac, mut core) = desktop_on_perso();
    mac.os.set_desktop_app_running(true);
    let work_folder = format!(
        "Library/Application Support/Claude/claude-code-sessions/{}/{}",
        work().account_uuid,
        work().organization_uuid
    );
    std::fs::create_dir_all(mac.path(&work_folder)).unwrap();
    core.command(Command::SetSidebarSync(SidebarSyncMode::Automatic));
    click_in_row(&mut core, WORK, SWITCH);

    // Signed out, Claude keeps saving Perso's Sessions.
    let perso = FixtureLogin::perso();
    sidebar_entry(&mac, &perso, "c", "New while signing out", Duration::ZERO);
    let written = mac.path(format!(
        "Library/Application Support/Claude/claude-code-sessions/{}/{}/local_c.json",
        perso.account_uuid, perso.organization_uuid
    ));
    core.event(Event::SidebarFilesChanged(vec![written]));
    mac.clock.advance(jiff::SignedDuration::from_secs(5));
    core.event(Event::Tick);

    assert!(!mac.path(format!("{work_folder}/local_c.json")).exists());
    assert_eq!(
        warnings(&core),
        [format!("In Claude: Sign out, then sign in as {WORK}")]
    );
}

#[test]
fn the_switch_is_confirmed_from_the_folder_claude_writes() {
    let (mac, mut core, _) = switch_started();
    // Signed out: Claude keeps saving the old account's Sessions.
    sidebar_entry(
        &mac,
        &FixtureLogin::perso(),
        "a",
        "Fix the build",
        Duration::ZERO,
    );
    assert!(core.event(Event::Tick).is_empty());
    assert_eq!(
        warnings(&core),
        [format!("In Claude: Sign out, then sign in as {WORK}")]
    );

    desktop_signed_in_as(&mac, &work());
    sidebar_entry(&mac, &work(), "n", "New one", Duration::ZERO);
    let effects = core.event(Event::Tick);

    assert_eq!(
        effects,
        [Effect::Notify {
            title: "Claude switched".into(),
            body: format!("Claude now uses {WORK}"),
        }]
    );
    assert_eq!(warnings(&core), [format!("Claude now uses {WORK}")]);
    assert_eq!(
        header(&core).as_deref(),
        Some("CLI: Default    Desktop: me")
    );
    assert_eq!(row(&rows(&core), WORK).badges, ["Desktop"]);
}

#[test]
fn a_folder_of_another_account_is_flagged_as_a_mismatch() {
    let (mac, mut core, _) = switch_started();
    let stranger = FixtureLogin::new(
        "66666666-6666-4666-8666-666666666666",
        "77777777-7777-4777-8777-777777777777",
        "other@example.com",
        "Other",
    );
    sidebar_entry(&mac, &stranger, "s", "Elsewhere", Duration::ZERO);

    let effects = core.event(Event::Tick);

    assert_eq!(
        effects,
        [Effect::Notify {
            title: "Claude signed in to another Profile".into(),
            body: format!("Claude signed in as another Profile, not {WORK}"),
        }]
    );
    assert_eq!(
        warnings(&core),
        [format!("Claude signed in as another Profile, not {WORK}")]
    );
}

#[test]
fn main_log_confirms_when_no_folder_was_written_yet() {
    let (mac, mut core, _) = switch_started();
    desktop_signed_in_as(&mac, &work());
    assert!(core.event(Event::Tick).is_empty(), "no sign-in line yet");

    append_main_log(
        &mac,
        "[account] Login-state transition (loggedOut: true → false, …)\n",
    );
    core.event(Event::Tick);

    assert_eq!(warnings(&core), [format!("Claude now uses {WORK}")]);
}

#[test]
fn main_log_flags_signing_back_in_as_the_old_account() {
    let (mac, mut core, _) = switch_started();
    append_main_log(
        &mac,
        "[account] Login-state transition (loggedOut: false → true, …)\n\
         [account] Login-state transition (loggedOut: true → false, …)\n",
    );

    core.event(Event::Tick);

    assert_eq!(
        warnings(&core),
        [format!("Claude signed in as {PERSO}, not {WORK}")]
    );
}

#[test]
fn an_unconfirmed_switch_gives_up_after_half_an_hour_and_can_be_cancelled_before() {
    let (mac, mut core, _) = switch_started();
    mac.clock.advance(jiff::SignedDuration::from_mins(29));
    core.event(Event::Tick);
    assert_eq!(
        warnings(&core),
        [format!("In Claude: Sign out, then sign in as {WORK}")]
    );
    mac.clock.advance(jiff::SignedDuration::from_mins(1));
    core.event(Event::Tick);
    assert_eq!(
        warnings(&core),
        [format!(
            "Couldn't confirm Claude switched to {WORK} · check its account menu"
        )]
    );

    click_in_row(&mut core, WORK, SWITCH);
    press(&mut core, "Cancel switch");
    assert!(warnings(&core).is_empty(), "{:?}", warnings(&core));
}

#[test]
fn no_path_quits_relaunches_or_spawns_anything_against_claude() {
    let (mac, mut core) = desktop_on_perso();
    let spawns_before = mac.claude.calls().len();
    let mut effects = Vec::new();
    mac.os
        .start_desktop_child(503, CHILD_STARTED.parse().unwrap());
    effects.extend(click_in_row(&mut core, WORK, SWITCH));
    effects.extend(press(&mut core, "Switch anyway"));
    desktop_signed_in_as(&mac, &work());
    sidebar_entry(&mac, &work(), "n", "New one", Duration::ZERO);
    effects.extend(core.event(Event::Tick));
    desktop_child(&mac, 504, "a", "busy");
    effects.extend(click_in_row(&mut core, PERSO, SWITCH));
    effects.extend(press(&mut core, "Notify me when idle"));
    mac.os.end_process(504);
    effects.extend(core.event(Event::Tick));

    assert!(
        effects.iter().all(|effect| matches!(
            effect,
            Effect::BringDesktopAppForward | Effect::Notify { .. }
        )),
        "{effects:?}"
    );
    assert_eq!(mac.claude.calls().len(), spawns_before, "nothing spawned");
}
