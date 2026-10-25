//! Sidebar sync (ADR 0003): "Sync now" copies every Session's Sidebar entry
//! into every other Profile's desktop sidebar. While Claude runs it writes
//! only to Profiles Claude isn't using, it never deletes, and it carries
//! title and archive changes over from the copy that changed.

mod common;

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use accroster_core::{CheckStatus, Command, Core, SafetyFeature, SidebarSyncMode};
use accroster_fake_mac::FakeMac;
use common::*;
use serde_json::{Value, json};

const SIDEBAR: &str = "Library/Application Support/Claude/claude-code-sessions";

/// Made-up Profile folders: Perso, Work, and a second Organization of
/// Perso's account.
const PERSO: (&str, &str) = (
    "a1111111-1111-4111-8111-111111111111",
    "a2222222-2222-4222-8222-222222222222",
);
const WORK: (&str, &str) = (
    "b1111111-1111-4111-8111-111111111111",
    "b2222222-2222-4222-8222-222222222222",
);
const PERSO_TEAM: (&str, &str) = (
    "a1111111-1111-4111-8111-111111111111",
    "a3333333-3333-4333-8333-333333333333",
);

fn folder(profile: (&str, &str)) -> String {
    format!("{SIDEBAR}/{}/{}", profile.0, profile.1)
}

fn entry_path(profile: (&str, &str), id: &str) -> String {
    format!("{}/local_{id}.json", folder(profile))
}

/// A Sidebar entry as the desktop app writes it (field names from the
/// research digest; values made up).
fn entry(id: &str, title: &str) -> Value {
    json!({
        "sessionId": format!("local_{id}"),
        "cliSessionId": format!("{id}-cli"),
        "title": title,
        "titleSource": "auto",
        "isArchived": false,
        "createdAt": 1_761_390_000_000_i64,
        "lastActivityAt": 1_761_400_000_000_i64,
        "lastFocusedAt": 1_761_400_000_000_i64,
        "cwd": "/Users/me/Dev/app",
        "originCwd": "/Users/me/Dev/app",
        "model": "claude-opus",
        "permissionMode": "default",
        "bridgeSessionIds": ["bridge-1"],
        "remoteMcpServersConfig": [{"uuid": "u", "name": "n", "url": "https://mcp.example", "tools": []}],
        "enabledMcpTools": {"n": ["search"]},
        "toolSurfaceSnapshot": {"tools": ["Bash"]},
        "completedTurns": 3,
    })
}

fn put(mac: &FakeMac, profile: (&str, &str), id: &str, body: &Value) -> PathBuf {
    mac.write_sidebar_entry(profile.0, profile.1, id, body)
}

fn read(mac: &FakeMac, profile: (&str, &str), id: &str) -> Option<Value> {
    mac.read_json(entry_path(profile, id))
}

/// Creates an empty Profile folder (the desktop app makes one per Profile
/// it was signed in as).
fn empty_folder(mac: &FakeMac, profile: (&str, &str)) {
    std::fs::create_dir_all(mac.path(folder(profile))).unwrap();
}

fn set_age(path: &std::path::Path, age: Duration) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - age)
        .unwrap();
}

/// The desktop app signed in as `profile`'s account, its folder written
/// last.
fn desktop_uses(mac: &FakeMac, profile: (&str, &str)) {
    mac.os.set_desktop_app_version(Some("3.6556.2"));
    mac.write_desktop_config(&json!({ "lastKnownAccountUuid": profile.0 }));
    let marker = put(
        mac,
        profile,
        "desktop-marker",
        &entry("desktop-marker", "Marker"),
    );
    set_age(&marker, Duration::ZERO);
}

fn manual(mac: &FakeMac) -> Core {
    let mut core = mac.launch();
    core.command(Command::SetSidebarSync(SidebarSyncMode::Manual));
    core
}

fn sync(core: &mut Core) {
    let command = core
        .view()
        .settings
        .sidebar_sync
        .sync_now
        .expect("Sync now is offered");
    core.command(command);
}

fn note(core: &Core) -> String {
    core.view()
        .settings
        .sidebar_sync
        .note
        .expect("Settings says what sync did")
}

fn without_cleared(mut body: Value) -> Value {
    let object = body.as_object_mut().unwrap();
    object.insert("bridgeSessionIds".into(), json!([]));
    object.insert("remoteMcpServersConfig".into(), json!([]));
    object.insert("enabledMcpTools".into(), json!({}));
    body
}

fn is_paused(core: &Core) -> bool {
    core.view()
        .diagnostics
        .paused
        .iter()
        .any(|paused| paused.feature == SafetyFeature::SidebarSync)
}

fn resume_button(core: &Core) -> Option<Command> {
    core.view()
        .diagnostics
        .paused
        .into_iter()
        .find(|paused| paused.feature == SafetyFeature::SidebarSync)
        .and_then(|paused| paused.resume)
}

/// Every file in every sidebar folder, as text.
fn sidebars(mac: &FakeMac) -> Vec<(PathBuf, String)> {
    mac.snapshot()
        .into_iter()
        .filter(|(path, _)| path.starts_with(SIDEBAR))
        .map(|(path, bytes)| (path, String::from_utf8(bytes).unwrap()))
        .collect()
}

// ----- Settings ------------------------------------------------------------------------

#[test]
fn sidebar_sync_is_off_until_set_to_manual_which_offers_sync_now_and_is_remembered() {
    let mac = FakeMac::new();
    let mut core = mac.launch();
    assert_eq!(core.view().settings.sidebar_sync.mode, SidebarSyncMode::Off);
    assert_eq!(core.view().settings.sidebar_sync.sync_now, None);

    core.command(Command::SetSidebarSync(SidebarSyncMode::Manual));

    assert_eq!(
        core.view().settings.sidebar_sync.mode,
        SidebarSyncMode::Manual
    );
    assert_eq!(
        core.view().settings.sidebar_sync.sync_now,
        Some(Command::SyncSidebarNow)
    );
    let relaunched = mac.launch();
    assert_eq!(
        relaunched.view().settings.sidebar_sync.mode,
        SidebarSyncMode::Manual
    );
}

#[test]
fn with_sync_off_sync_now_writes_nothing() {
    let mac = FakeMac::new();
    put(&mac, PERSO, "s1", &entry("s1", "Fix login bug"));
    empty_folder(&mac, WORK);
    let mut core = mac.launch();
    let before = sidebars(&mac);

    core.command(Command::SyncSidebarNow);

    assert_eq!(sidebars(&mac), before);
}

// ----- Copying -------------------------------------------------------------------------

#[test]
fn a_copy_keeps_the_file_name_and_every_field_but_clears_the_connector_and_bridge_fields() {
    let mac = FakeMac::new();
    let original = entry("s1", "Fix login bug");
    put(&mac, PERSO, "s1", &original);
    let mut lean = entry("s2", "Lean entry");
    for field in [
        "bridgeSessionIds",
        "remoteMcpServersConfig",
        "enabledMcpTools",
    ] {
        lean.as_object_mut().unwrap().remove(field);
    }
    put(&mac, PERSO, "s2", &lean);
    empty_folder(&mac, WORK);
    let mut core = manual(&mac);

    sync(&mut core);

    assert_eq!(
        read(&mac, WORK, "s1"),
        Some(without_cleared(original.clone()))
    );
    assert_eq!(
        read(&mac, WORK, "s2"),
        Some(lean),
        "fields absent from the entry stay absent"
    );
    assert_eq!(
        read(&mac, PERSO, "s1"),
        Some(original),
        "the source is left as it was"
    );
    assert_eq!(note(&core), "Updated 2 Sidebar entries in 1 Profile");
}

#[test]
fn every_profile_gets_every_session_and_a_second_sync_has_nothing_to_do() {
    let mac = FakeMac::new();
    put(&mac, PERSO, "s1", &entry("s1", "One"));
    put(&mac, WORK, "s2", &entry("s2", "Two"));
    empty_folder(&mac, PERSO_TEAM);
    let mut core = manual(&mac);

    sync(&mut core);

    for profile in [PERSO, WORK, PERSO_TEAM] {
        assert!(read(&mac, profile, "s1").is_some(), "{profile:?}");
        assert!(read(&mac, profile, "s2").is_some(), "{profile:?}");
    }
    let after_first = sidebars(&mac);
    sync(&mut core);
    assert_eq!(sidebars(&mac), after_first);
    assert_eq!(note(&core), "Every sidebar was already in sync");
}

#[test]
fn scratch_ssh_wsl_and_scheduled_task_entries_and_cli_only_sessions_are_not_copied() {
    let mac = FakeMac::new();
    let mut scratch = entry("scratch", "Scratch");
    scratch["cwd"] = json!(format!(
        "/Users/me/Library/Application Support/Claude/scratch-workspaces/{}/{}/x",
        PERSO.0, PERSO.1
    ));
    put(&mac, PERSO, "scratch", &scratch);
    let mut ssh = entry("ssh", "Remote");
    ssh["sshConfig"] = json!({"host": "box"});
    put(&mac, PERSO, "ssh", &ssh);
    let mut wsl = entry("wsl", "WSL");
    wsl["wslConfig"] = json!({"distro": "x"});
    put(&mac, PERSO, "wsl", &wsl);
    let mut scheduled = entry("task", "Nightly");
    scheduled["scheduledTaskId"] = json!("task-1");
    put(&mac, PERSO, "task", &scheduled);
    mac.write_json(
        format!("{}/scheduled-tasks.json", folder(PERSO)),
        &json!([]),
    );
    mac.write_transcript("-Users-me-Dev-app", "cli-only", &[json!({"type": "user"})]);
    empty_folder(&mac, WORK);
    let mut core = manual(&mac);

    sync(&mut core);

    assert!(
        mac.list(folder(WORK)).is_empty(),
        "{:?}",
        mac.list(folder(WORK))
    );
    assert_eq!(
        mac.list(SIDEBAR),
        [PERSO.0, WORK.0],
        "no Profile folder is created"
    );
}

// ----- Where sync writes ---------------------------------------------------------------

#[test]
fn while_claude_runs_no_write_lands_in_any_folder_of_the_desktops_account() {
    let mac = FakeMac::new();
    put(&mac, WORK, "s1", &entry("s1", "From Work"));
    empty_folder(&mac, PERSO_TEAM);
    desktop_uses(&mac, PERSO);
    mac.os.set_desktop_app_running(true);
    let mut core = manual(&mac);
    let perso_before: Vec<_> = sidebars(&mac)
        .into_iter()
        .filter(|(path, _)| path.starts_with(format!("{SIDEBAR}/{}", PERSO.0)))
        .collect();

    sync(&mut core);

    let perso_after: Vec<_> = sidebars(&mac)
        .into_iter()
        .filter(|(path, _)| path.starts_with(format!("{SIDEBAR}/{}", PERSO.0)))
        .collect();
    assert_eq!(perso_after, perso_before);
    assert!(
        read(&mac, WORK, "desktop-marker").is_some(),
        "the other Profile still gets Perso's Sessions"
    );
    assert_eq!(
        note(&core),
        "Updated 1 Sidebar entry in 1 Profile · the Profile Claude is using syncs once Claude is quit"
    );

    mac.os.set_desktop_app_running(false);
    sync(&mut core);

    assert!(read(&mac, PERSO, "s1").is_some());
    assert!(read(&mac, PERSO_TEAM, "s1").is_some());
}

#[test]
fn while_claude_runs_on_an_account_it_cant_read_nothing_is_written() {
    let mac = FakeMac::new();
    put(&mac, PERSO, "s1", &entry("s1", "One"));
    empty_folder(&mac, WORK);
    mac.os.set_desktop_app_running(true);
    let mut core = manual(&mac);
    let before = sidebars(&mac);

    sync(&mut core);

    assert_eq!(sidebars(&mac), before);
    assert_eq!(
        note(&core),
        "Couldn't tell which Profile Claude is using · nothing was synced"
    );
}

#[test]
fn every_target_folder_is_backed_up_before_sync_writes_into_it() {
    let mac = FakeMac::new();
    put(&mac, PERSO, "s1", &entry("s1", "One"));
    let kept = entry("w1", "Work's own");
    put(&mac, WORK, "w1", &kept);
    let mut core = manual(&mac);

    sync(&mut core);

    let backups = format!("{APP_FOLDER}/backups");
    let work_group = mac
        .list(&backups)
        .into_iter()
        .find(|name| name.starts_with(&format!("{}-", WORK.1)))
        .expect("Work's folder was backed up");
    let copies: Vec<String> = mac
        .list(format!("{backups}/{work_group}"))
        .into_iter()
        .filter(|name| name.ends_with(".bak"))
        .collect();
    assert_eq!(copies.len(), 1);
    let copy = format!("{backups}/{work_group}/{}", copies[0]);
    assert_eq!(
        mac.list(&copy),
        ["local_w1.json"],
        "the folder as it was before"
    );
    assert_eq!(mac.read_json(format!("{copy}/local_w1.json")), Some(kept));
}

// ----- Deletes -------------------------------------------------------------------------

#[test]
fn a_tombstone_stops_re_adding_to_that_profile_and_no_delete_is_propagated() {
    let mac = FakeMac::new();
    put(&mac, PERSO, "s1", &entry("s1", "One"));
    empty_folder(&mac, WORK);
    empty_folder(&mac, PERSO_TEAM);
    let mut core = manual(&mac);
    sync(&mut core);
    // Deleted in Work: the desktop app removes the entry and leaves a
    // tombstone named after the Transcript.
    std::fs::remove_file(mac.path(entry_path(WORK, "s1"))).unwrap();
    mac.write_file(format!("{}/deleted_s1-cli", folder(WORK)), "{\"v\":1}");

    sync(&mut core);

    assert_eq!(read(&mac, WORK, "s1"), None, "not added back");
    assert!(read(&mac, PERSO, "s1").is_some(), "not deleted elsewhere");
    assert!(read(&mac, PERSO_TEAM, "s1").is_some());
    assert!(
        mac.list(folder(WORK))
            .contains(&"deleted_s1-cli".to_owned())
    );
}

// ----- Which copy wins -----------------------------------------------------------------

#[test]
fn a_rename_or_an_archive_in_any_copy_carries_over_to_the_others() {
    let mac = FakeMac::new();
    put(&mac, PERSO, "s1", &entry("s1", "Old name"));
    empty_folder(&mac, WORK);
    let mut core = manual(&mac);
    sync(&mut core);

    let mut renamed = read(&mac, WORK, "s1").unwrap();
    renamed["title"] = json!("New name");
    renamed["titleSource"] = json!("user");
    renamed["previousTitles"] = json!(["Old name"]);
    put(&mac, WORK, "s1", &renamed);
    let mut archived = read(&mac, PERSO, "s1").unwrap();
    archived["isArchived"] = json!(true);
    put(&mac, PERSO, "s1", &archived);
    sync(&mut core);

    for profile in [PERSO, WORK] {
        let copy = read(&mac, profile, "s1").unwrap();
        assert_eq!(copy["title"], "New name", "{profile:?}");
        assert_eq!(copy["titleSource"], "user", "{profile:?}");
        assert_eq!(copy["previousTitles"], json!(["Old name"]), "{profile:?}");
        assert_eq!(copy["isArchived"], true, "{profile:?}");
    }
    assert_eq!(
        read(&mac, PERSO, "s1").unwrap()["bridgeSessionIds"],
        json!(["bridge-1"]),
        "the source keeps its own connectors"
    );
}

#[test]
fn when_two_copies_were_renamed_the_desktops_active_profile_wins() {
    let mac = FakeMac::new();
    put(&mac, PERSO, "s1", &entry("s1", "Old name"));
    put(&mac, WORK, "s1", &entry("s1", "Old name"));
    desktop_uses(&mac, WORK);
    let mut core = manual(&mac);
    sync(&mut core);

    // Renamed in Work, then Claude switched to Perso and it was renamed
    // there too, earlier on the clock than Work's file.
    let work_copy = put(&mac, WORK, "s1", &entry("s1", "Work's name"));
    desktop_uses(&mac, PERSO);
    let perso_copy = put(&mac, PERSO, "s1", &entry("s1", "Perso's name"));
    set_age(&perso_copy, Duration::from_secs(600));
    set_age(&work_copy, Duration::from_secs(60));
    sync(&mut core);

    assert_eq!(read(&mac, WORK, "s1").unwrap()["title"], "Perso's name");
    assert_eq!(read(&mac, PERSO, "s1").unwrap()["title"], "Perso's name");
}

#[test]
fn when_two_copies_were_renamed_and_neither_is_the_desktops_the_newest_file_wins() {
    let mac = FakeMac::new();
    put(&mac, PERSO, "s1", &entry("s1", "Old name"));
    put(&mac, WORK, "s1", &entry("s1", "Old name"));
    let mut core = manual(&mac);
    sync(&mut core);

    let work_copy = put(&mac, WORK, "s1", &entry("s1", "Work's name"));
    let perso_copy = put(&mac, PERSO, "s1", &entry("s1", "Perso's name"));
    set_age(&perso_copy, Duration::from_secs(600));
    set_age(&work_copy, Duration::from_secs(60));
    sync(&mut core);

    assert_eq!(read(&mac, PERSO, "s1").unwrap()["title"], "Work's name");
    assert_eq!(read(&mac, WORK, "s1").unwrap()["title"], "Work's name");
}

#[test]
fn the_copy_with_the_newest_activity_carries_the_current_transcript() {
    let mac = FakeMac::new();
    put(&mac, PERSO, "s1", &entry("s1", "One"));
    empty_folder(&mac, WORK);
    let mut core = manual(&mac);
    sync(&mut core);

    // Resumed and /clear-ed in Work: the Transcript id moved on.
    let mut continued = read(&mac, WORK, "s1").unwrap();
    continued["cliSessionId"] = json!("s1-cli-after-clear");
    continued["priorCliSessionIds"] = json!(["s1-cli"]);
    continued["lastActivityAt"] = json!(1_761_500_000_000_i64);
    continued["bridgeSessionIds"] = json!(["work-bridge"]);
    put(&mac, WORK, "s1", &continued);
    sync(&mut core);

    let perso = read(&mac, PERSO, "s1").unwrap();
    assert_eq!(perso["cliSessionId"], "s1-cli-after-clear");
    assert_eq!(perso["priorCliSessionIds"], json!(["s1-cli"]));
    assert_eq!(
        perso["bridgeSessionIds"],
        json!([]),
        "Work's bridge isn't copied"
    );
    assert_eq!(
        read(&mac, WORK, "s1").unwrap()["bridgeSessionIds"],
        json!(["work-bridge"])
    );
}

// ----- Safety pauses -------------------------------------------------------------------

#[test]
fn an_unknown_top_level_field_pauses_sync_before_any_write_until_resumed() {
    let mac = FakeMac::new();
    mac.os.set_desktop_app_version(Some("3.7000.0"));
    let mut core = manual(&mac);
    let mut newer = entry("s1", "One");
    newer["accountBoundThing"] = json!("x");
    put(&mac, PERSO, "s1", &newer);
    empty_folder(&mac, WORK);
    let before = sidebars(&mac);

    sync(&mut core);

    assert_eq!(sidebars(&mac), before, "nothing was written");
    assert!(is_paused(&core));
    assert!(
        warnings(&core)
            .contains(&"Claude 3.7000.0 changed Sidebar entries · Sidebar sync paused".to_owned()),
        "{:?}",
        warnings(&core)
    );
    let check = core
        .view()
        .diagnostics
        .checks
        .into_iter()
        .find(|check| check.name == "Sidebar entries")
        .unwrap();
    assert_eq!(check.status, CheckStatus::Fail);
    assert!(check.detail.unwrap().contains("accountBoundThing"));
    assert_eq!(resume_button(&core), None, "not while the field is there");

    put(&mac, PERSO, "s1", &entry("s1", "One"));
    sync(&mut core);
    assert_eq!(
        sidebars(&mac).len(),
        before.len(),
        "still paused: Resume is a click"
    );

    core.command(Command::OpenDiagnostics);
    core.command(resume_button(&core).expect("Resume once the check passes"));
    sync(&mut core);
    assert!(!is_paused(&core));
    assert!(read(&mac, WORK, "s1").is_some());
}

#[test]
fn with_sync_off_an_unknown_field_pauses_nothing() {
    let mac = FakeMac::new();
    let mut newer = entry("s1", "One");
    newer["accountBoundThing"] = json!("x");
    put(&mac, PERSO, "s1", &newer);

    let core = mac.launch();

    assert!(!is_paused(&core));
}

#[test]
fn a_folder_claude_wasnt_using_that_changed_since_the_last_sync_pauses_sync() {
    let mac = FakeMac::new();
    put(&mac, WORK, "s1", &entry("s1", "One"));
    desktop_uses(&mac, PERSO);
    mac.os.set_desktop_app_running(true);
    let mut core = manual(&mac);
    sync(&mut core);
    // Claude, still on Perso, writes into Work's folder: the dormant
    // multi-account mode.
    let mut touched = read(&mac, WORK, "s1").unwrap();
    touched["lastFocusedAt"] = json!(1_761_450_000_000_i64);
    put(&mac, WORK, "s1", &touched);
    empty_folder(&mac, PERSO_TEAM);
    mac.os.set_desktop_app_running(false);
    let before = sidebars(&mac);

    sync(&mut core);

    assert_eq!(sidebars(&mac), before, "nothing was written");
    assert!(is_paused(&core));
    assert!(
        warnings(&core)
            .contains(&"Claude wrote to a Profile it isn't using · Sidebar sync paused".to_owned()),
        "{:?}",
        warnings(&core)
    );

    core.command(resume_button(&core).expect("Resume accepts the folders as they are"));
    sync(&mut core);
    assert!(!is_paused(&core));
    assert!(read(&mac, PERSO_TEAM, "s1").is_some());
}

#[test]
fn a_folder_changed_after_an_account_switch_is_not_a_pause() {
    let mac = FakeMac::new();
    put(&mac, WORK, "s1", &entry("s1", "One"));
    desktop_uses(&mac, PERSO);
    let mut core = manual(&mac);
    sync(&mut core);
    // The user switched Claude to Work and kept working there.
    mac.write_desktop_config(&json!({ "lastKnownAccountUuid": WORK.0 }));
    let mut used = read(&mac, WORK, "s1").unwrap();
    used["lastActivityAt"] = json!(1_761_500_000_000_i64);
    put(&mac, WORK, "s1", &used);

    sync(&mut core);

    assert!(!is_paused(&core));
    assert_eq!(
        read(&mac, PERSO, "s1").unwrap()["lastActivityAt"],
        1_761_500_000_000_i64
    );
}

#[test]
fn old_folder_backups_are_pruned_at_launch_keeping_the_last_five() {
    let mac = FakeMac::new();
    empty_folder(&mac, WORK);
    let mut core = manual(&mac);
    for round in 0..7 {
        put(&mac, PERSO, "s1", &entry("s1", &format!("Name {round}")));
        mac.clock.advance(jiff::SignedDuration::from_hours(1));
        sync(&mut core);
    }
    let backups = format!("{APP_FOLDER}/backups");
    let group = mac
        .list(&backups)
        .into_iter()
        .find(|name| name.starts_with(&format!("{}-", WORK.1)))
        .unwrap();
    let copies = |mac: &FakeMac| {
        mac.list(format!("{backups}/{group}"))
            .into_iter()
            .filter(|name| name.ends_with(".bak"))
            .count()
    };
    assert_eq!(copies(&mac), 7);

    mac.clock.advance(jiff::SignedDuration::from_hours(31 * 24));
    drop(core);
    mac.launch();

    assert_eq!(copies(&mac), 5);
}
