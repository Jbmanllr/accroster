//! Sidebar sync (ADR 0003): "Sync now" copies every Session's Sidebar entry
//! into every other Profile's desktop sidebar. While Claude runs, its own
//! account's sidebars only gain entries they lack, it never deletes, and it carries
//! title and archive changes over from the copy that changed.

mod common;

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use accroster_core::{
    CheckStatus, Command, Core, Effect, Event, MenuItem, SafetyFeature, SidebarSyncMode,
};
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
    // The run writes one sidebar per tick.
    settle(core);
}

/// The menu's status line, if any.
fn status(core: &Core) -> Option<String> {
    core.view()
        .menu
        .items
        .into_iter()
        .find_map(|item| match item {
            MenuItem::Status { text, .. } => Some(text),
            _ => None,
        })
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
    assert_eq!(note(&core), "Synced 2 entries into 1 sidebar");
}

#[test]
fn a_sync_reports_its_progress_one_sidebar_at_a_time_then_what_it_did() {
    let mac = FakeMac::new();
    put(&mac, PERSO, "s1", &entry("s1", "One"));
    put(&mac, PERSO, "s2", &entry("s2", "Two"));
    empty_folder(&mac, WORK);
    empty_folder(&mac, PERSO_TEAM);
    let mut core = manual(&mac);

    core.command(Command::SyncSidebarNow);

    assert_eq!(note(&core), "Syncing… 0 of 2");
    assert_eq!(status(&core).as_deref(), Some("Syncing… 0 of 2"));
    core.event(Event::Tick);
    assert_eq!(status(&core).as_deref(), Some("Syncing… 1 of 2"));
    core.event(Event::Tick);
    assert_eq!(note(&core), "Synced 4 entries into 2 sidebars");
    assert_eq!(
        status(&core).as_deref(),
        Some("Synced 4 entries into 2 sidebars"),
        "the menu says how it ended too"
    );
    mac.clock.advance(jiff::SignedDuration::from_mins(10));
    core.event(Event::Tick);
    assert_eq!(status(&core), None, "for a while");
    let log = mac
        .read_file(format!("{APP_FOLDER}/logs/app.log"))
        .unwrap_or_default();
    assert!(log.contains("sidebar sync started folders=2"), "{log}");
    assert!(log.contains("sidebar synced entries=4 folders=2"), "{log}");
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
fn a_sync_reads_again_only_the_entries_changed_since_the_last_run() {
    let mac = FakeMac::new();
    put(&mac, PERSO, "s1", &entry("s1", "One"));
    put(&mac, WORK, "s2", &entry("s2", "Two"));
    empty_folder(&mac, PERSO_TEAM);
    let mut core = manual(&mac);

    sync(&mut core);
    sync(&mut core);
    put(&mac, WORK, "s2", &entry("s2", "Renamed"));
    sync(&mut core);

    let log = mac
        .read_file(format!("{APP_FOLDER}/logs/app.log"))
        .unwrap_or_default();
    let parsed: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("sidebar synced "))
        .filter_map(|line| line.split_whitespace().find(|f| f.starts_with("parsed=")))
        .collect();
    assert_eq!(
        parsed,
        ["parsed=2", "parsed=0", "parsed=1"],
        "its own copies aren't read again either: {log}"
    );
    for profile in [PERSO, WORK, PERSO_TEAM] {
        assert_eq!(
            read(&mac, profile, "s2").unwrap()["title"],
            "Renamed",
            "{profile:?}"
        );
    }
}

#[test]
fn the_check_before_a_sync_reads_again_only_the_entries_changed_since_the_last_check() {
    let mac = FakeMac::new();
    put(&mac, PERSO, "s1", &entry("s1", "One"));
    put(&mac, WORK, "s2", &entry("s2", "Two"));
    empty_folder(&mac, PERSO_TEAM);
    let mut core = mac.launch();
    core.command(Command::SetVerboseLogging(true));
    core.command(Command::SetSidebarSync(SidebarSyncMode::Manual));

    sync(&mut core);
    sync(&mut core);
    sync(&mut core);
    let mut changed = entry("s2", "Two");
    changed["brandNewField"] = json!(true);
    put(&mac, WORK, "s2", &changed);
    sync(&mut core);

    let log = mac
        .read_file(format!("{APP_FOLDER}/logs/app.log"))
        .unwrap_or_default();
    let parsed: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("sidebar entries checked "))
        .filter_map(|line| line.split_whitespace().find(|f| f.starts_with("parsed=")))
        .collect();
    assert_eq!(
        parsed,
        // Turning sync on, then each sync: the first run's 4 copies are
        // new files to the check after it.
        ["parsed=0", "parsed=0", "parsed=4", "parsed=0", "parsed=1"],
        "{log}"
    );
    let check = core
        .view()
        .diagnostics
        .checks
        .into_iter()
        .find(|check| check.name == "Sidebar entries")
        .expect("Diagnostics lists Sidebar entries");
    assert_eq!(check.status, CheckStatus::Pass);
    assert_eq!(
        check.detail.as_deref(),
        Some("6 entries read · new fields seen: brandNewField")
    );
}

#[test]
fn ssh_and_no_folder_sessions_are_included_by_default_and_can_be_left_out() {
    let mac = FakeMac::new();
    let mut ssh = entry("ssh", "On the VPS");
    ssh["sshConfig"] = json!({"sshHost": "vps"});
    ssh["sshRemoteProcessId"] = json!(4242);
    ssh["sshReattach"] = json!({"pid": 4242, "token": "bound-to-perso"});
    put(&mac, PERSO, "ssh", &ssh);
    // A "No folder" Session: its working folder is Claude's scratch
    // workspace for Perso, and stays there.
    let workspace = format!(
        "/Users/me/Library/Application Support/Claude/scratch-workspaces/{}/{}/x",
        PERSO.0, PERSO.1
    );
    let mut no_folder = entry("scratch", "Quick question");
    no_folder["cwd"] = json!(workspace);
    no_folder["originCwd"] = json!(workspace);
    put(&mac, PERSO, "scratch", &no_folder);
    empty_folder(&mac, WORK);
    let mut core = manual(&mac);
    assert!(
        core.view()
            .settings
            .sidebar_sync
            .ssh_and_no_folder_sessions
            .on,
        "on by default"
    );

    sync(&mut core);

    let mut expected = without_cleared(ssh.clone());
    expected.as_object_mut().unwrap().remove("sshReattach");
    assert_eq!(read(&mac, WORK, "ssh"), Some(expected));
    assert_eq!(read(&mac, PERSO, "ssh"), Some(ssh), "the source keeps it");
    assert_eq!(
        read(&mac, WORK, "scratch"),
        Some(without_cleared(no_folder)),
        "the copy points at the same scratch workspace"
    );
    assert_eq!(note(&core), "Synced 2 entries into 1 sidebar");

    // Turned off, new SSH and No-folder Sessions are left out, and that is
    // remembered.
    let option = core.view().settings.sidebar_sync.ssh_and_no_folder_sessions;
    core.command(option.command);
    let mut ssh2 = entry("ssh2", "Also on the VPS");
    ssh2["sshConfig"] = json!({"sshHost": "vps"});
    put(&mac, PERSO, "ssh2", &ssh2);
    let mut no_folder2 = entry("scratch2", "Another question");
    no_folder2["cwd"] = json!(format!("{workspace}2"));
    put(&mac, PERSO, "scratch2", &no_folder2);
    sync(&mut core);
    assert_eq!(read(&mac, WORK, "ssh2"), None);
    assert_eq!(read(&mac, WORK, "scratch2"), None);
    assert_eq!(
        note(&core),
        "Every sidebar was already in sync · left out 2 SSH and 2 No-folder Sessions \
         (Include SSH and No-folder Sessions is off)"
    );
    drop(core);
    let core = mac.launch();
    assert!(
        !core
            .view()
            .settings
            .sidebar_sync
            .ssh_and_no_folder_sessions
            .on
    );
}

/// "Include SSH and No-folder Sessions" turned off.
fn leave_out_ssh_and_no_folder(core: &mut Core) {
    let option = core.view().settings.sidebar_sync.ssh_and_no_folder_sessions;
    assert!(option.on);
    core.command(option.command);
}

#[test]
fn the_result_counts_the_sessions_left_out_by_kind_and_says_why() {
    let mac = FakeMac::new();
    put(&mac, PERSO, "s1", &entry("s1", "One"));
    for (id, cwd) in [("scratch1", "a"), ("scratch2", "b")] {
        let mut scratch = entry(id, "Scratch");
        scratch["cwd"] = json!(format!(
            "/Users/me/Library/Application Support/Claude/scratch-workspaces/{cwd}/x"
        ));
        put(&mac, PERSO, id, &scratch);
    }
    let mut ssh = entry("ssh", "Remote");
    ssh["sshConfig"] = json!({"sshHost": "vps"});
    put(&mac, PERSO, "ssh", &ssh);
    // The same SSH Session in a second sidebar counts once.
    put(&mac, WORK, "ssh", &ssh);
    let mut scheduled = entry("task", "Nightly");
    scheduled["scheduledTaskId"] = json!("task-1");
    put(&mac, PERSO, "task", &scheduled);
    let mut core = manual(&mac);
    leave_out_ssh_and_no_folder(&mut core);

    sync(&mut core);

    assert_eq!(
        note(&core),
        "Synced 1 entry into 1 sidebar · left out 1 SSH and 2 No-folder Sessions \
         (Include SSH and No-folder Sessions is off) · left out 1 scheduled-task Session \
         (Claude runs those on one account only)"
    );
    assert_eq!(
        status(&core).as_deref(),
        Some(
            "Synced 1 entry into 1 sidebar · left out 1 SSH, 2 No-folder and 1 scheduled-task Sessions"
        ),
        "the menu's line is shorter"
    );
    let log = mac
        .read_file(format!("{APP_FOLDER}/logs/app.log"))
        .unwrap_or_default();
    assert!(
        log.contains("skipped_ssh=1 skipped_no_folder=2 skipped_scheduled=1"),
        "{log}"
    );
}

#[test]
fn wsl_and_scheduled_task_entries_and_cli_only_sessions_are_not_copied() {
    let mac = FakeMac::new();
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
    assert_eq!(
        note(&core),
        "Every sidebar was already in sync · left out 1 WSL and 1 scheduled-task Sessions \
         (Claude runs those on one account only)"
    );
}

// ----- Where sync writes ---------------------------------------------------------------

#[test]
fn while_claude_runs_its_accounts_sidebars_only_gain_the_sessions_they_lack() {
    let mac = FakeMac::new();
    put(&mac, WORK, "s1", &entry("s1", "From Work"));
    put(&mac, PERSO, "s2", &entry("s2", "Old title"));
    let mut used = entry("s2", "Old title");
    used["lastActivityAt"] = json!(1_761_500_000_000_i64);
    put(&mac, WORK, "s2", &used);
    empty_folder(&mac, PERSO_TEAM);
    desktop_uses(&mac, PERSO);
    mac.os.set_desktop_app_running(true);
    let mut core = manual(&mac);
    let perso_entries_before: Vec<_> = sidebars(&mac)
        .into_iter()
        .filter(|(path, _)| path.starts_with(folder(PERSO)))
        .collect();

    sync(&mut core);

    let added = read(&mac, PERSO, "s1").expect("a Session Claude's sidebar lacked is added");
    assert_eq!(added, without_cleared(entry("s1", "From Work")));
    assert!(read(&mac, PERSO_TEAM, "s1").is_some());
    assert!(read(&mac, PERSO_TEAM, "s2").is_some());
    let perso_entries_after: Vec<_> = sidebars(&mac)
        .into_iter()
        .filter(|(path, _)| path.starts_with(folder(PERSO)))
        .filter(|(path, _)| !path.ends_with("local_s1.json"))
        .collect();
    assert_eq!(
        perso_entries_after, perso_entries_before,
        "entries already in a sidebar Claude uses are never replaced"
    );
    assert_eq!(
        note(&core),
        "Synced 5 entries into 3 sidebars · 4 added to the sidebar Claude is using · \
         Reopen Claude to show them · 1 changed entry there syncs once Claude is quit"
    );
    assert_eq!(
        status(&core).as_deref(),
        Some(
            "Synced 5 entries into 3 sidebars · 4 added to Claude's sidebar · Reopen Claude to show them"
        )
    );

    // AccRoster's own additions are no trace of Claude writing elsewhere.
    mac.clock.advance(seconds(60));
    changed(
        &mut core,
        &[
            mac.path(entry_path(PERSO_TEAM, "s1")),
            mac.path(entry_path(PERSO, "s1")),
        ],
    );
    assert!(!is_paused(&core));

    mac.os.set_desktop_app_running(false);
    sync(&mut core);

    assert!(!is_paused(&core));
    assert_eq!(
        read(&mac, PERSO, "s2").unwrap()["lastActivityAt"],
        1_761_500_000_000_i64
    );
}

#[test]
fn an_entry_claude_creates_while_sync_adds_it_is_kept() {
    let mac = FakeMac::new();
    put(&mac, WORK, "s1", &entry("s1", "From Work"));
    desktop_uses(&mac, PERSO);
    mac.os.set_desktop_app_running(true);
    let mut core = manual(&mac);
    let command = core.view().settings.sidebar_sync.sync_now.unwrap();
    core.command(command);
    // Claude opens the Session and saves its own entry before sync's write.
    let claudes = entry("s1", "Claude's own");
    put(&mac, PERSO, "s1", &claudes);

    settle(&mut core);

    assert_eq!(read(&mac, PERSO, "s1"), Some(claudes));
    assert!(!is_paused(&core));
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
fn a_source_given_another_copys_title_keeps_all_its_own_account_bound_fields() {
    let mac = FakeMac::new();
    let mut ssh = entry("s1", "Old name");
    ssh["sshConfig"] = json!({"sshHost": "vps"});
    ssh["sshReattach"] = json!({"pid": 4242, "token": "bound-to-perso"});
    put(&mac, PERSO, "s1", &ssh);
    empty_folder(&mac, WORK);
    let mut core = manual(&mac);
    sync(&mut core);

    // Renamed in Work; used since in Perso, which stays the source.
    let mut renamed = read(&mac, WORK, "s1").unwrap();
    renamed["title"] = json!("New name");
    put(&mac, WORK, "s1", &renamed);
    ssh["lastActivityAt"] = json!(1_761_500_000_000_i64);
    put(&mac, PERSO, "s1", &ssh);
    sync(&mut core);

    let mut expected = ssh;
    expected["title"] = json!("New name");
    assert_eq!(read(&mac, PERSO, "s1"), Some(expected));
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
fn an_unknown_top_level_field_is_copied_as_is_and_noted_without_pausing_sync() {
    let mac = FakeMac::new();
    mac.os.set_desktop_app_version(Some("3.7000.0"));
    let mut core = manual(&mac);
    let mut newer = entry("s1", "One");
    newer["brandNewField"] = json!({"kept": true});
    put(&mac, PERSO, "s1", &newer);
    empty_folder(&mac, WORK);

    sync(&mut core);

    assert!(!is_paused(&core));
    assert_eq!(read(&mac, WORK, "s1"), Some(without_cleared(newer)));
    let check = core
        .view()
        .diagnostics
        .checks
        .into_iter()
        .find(|check| check.name == "Sidebar entries")
        .unwrap();
    assert_eq!(check.status, CheckStatus::Pass);
    assert_eq!(
        check.detail.as_deref(),
        Some("1 entries read · new fields seen: brandNewField")
    );
}

#[test]
fn a_pause_for_new_entry_fields_from_an_earlier_version_can_be_resumed() {
    let mac = FakeMac::new();
    put(&mac, PERSO, "s1", &entry("s1", "One"));
    empty_folder(&mac, WORK);
    let mut core = manual(&mac);
    drop(core);
    std::fs::create_dir_all(mac.app_support().join("state")).unwrap();
    std::fs::write(
        mac.app_support().join("state/safety.json"),
        json!({ "version": 1, "paused": [
            { "feature": "sidebarSync", "reason": "Claude 3.7000.0 changed Sidebar entries" }
        ] })
        .to_string(),
    )
    .unwrap();
    core = mac.launch();
    assert!(is_paused(&core));

    core.command(resume_button(&core).expect("Resume is offered: new fields no longer block"));
    sync(&mut core);

    assert!(!is_paused(&core));
    assert!(read(&mac, WORK, "s1").is_some());
}

#[test]
fn resuming_takes_the_paused_line_off_the_menu_at_once() {
    let mac = FakeMac::new();
    put(&mac, PERSO, "s1", &entry("s1", "One"));
    empty_folder(&mac, WORK);
    let mut core = manual(&mac);
    drop(core);
    std::fs::create_dir_all(mac.app_support().join("state")).unwrap();
    std::fs::write(
        mac.app_support().join("state/safety.json"),
        json!({ "version": 1, "paused": [
            { "feature": "sidebarSync", "reason": "Claude 3.7000.0 changed Sidebar entries" }
        ] })
        .to_string(),
    )
    .unwrap();
    core = mac.launch();
    sync(&mut core);
    assert!(status(&core).is_some_and(|line| line.contains("paused")));

    core.command(resume_button(&core).expect("Resume is offered"));

    assert!(
        !status(&core).is_some_and(|line| line.contains("paused")),
        "{:?}",
        status(&core)
    );
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
fn a_sidebar_folder_keeps_only_its_last_two_backups() {
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
    assert_eq!(copies(&mac), 2, "pruned as each sync backs the folder up");

    mac.clock.advance(jiff::SignedDuration::from_hours(31 * 24));
    drop(core);
    mac.launch();

    assert_eq!(copies(&mac), 2);
}

// ----- Automatic mode ------------------------------------------------------------------

fn automatic(mac: &FakeMac) -> (Core, Vec<Effect>) {
    let mut core = mac.launch();
    let effects = core.command(Command::SetSidebarSync(SidebarSyncMode::Automatic));
    (core, effects)
}

fn seconds(n: i64) -> jiff::SignedDuration {
    jiff::SignedDuration::from_secs(n)
}

/// What the UI's FSEvents watch reports.
fn changed(core: &mut Core, paths: &[PathBuf]) -> Vec<Effect> {
    core.event(Event::SidebarFilesChanged(paths.to_vec()))
}

#[test]
fn automatic_mode_watches_the_sidebars_and_syncs_a_few_seconds_after_the_last_change() {
    let mac = FakeMac::new();
    desktop_uses(&mac, PERSO);
    empty_folder(&mac, WORK);
    mac.os.set_desktop_app_running(true);

    let (mut core, effects) = automatic(&mac);
    assert!(
        effects.contains(&Effect::WatchSidebarFolders(vec![mac.path(SIDEBAR)])),
        "{effects:?}"
    );
    assert_eq!(
        core.view().settings.sidebar_sync.mode,
        SidebarSyncMode::Automatic
    );

    // Claude writes a Session, then writes it again two seconds later.
    let first = put(&mac, PERSO, "s1", &entry("s1", "Fix login bug"));
    changed(&mut core, &[first]);
    mac.clock.advance(seconds(2));
    let mut later = entry("s1", "Fix login bug");
    later["lastActivityAt"] = json!(1_761_400_500_000_i64);
    let second = put(&mac, PERSO, "s1", &later);
    changed(&mut core, &[second]);

    mac.clock.advance(seconds(2));
    core.event(Event::Tick);
    assert_eq!(read(&mac, WORK, "s1"), None, "still settling");
    assert!(core.tick_interval() <= Duration::from_secs(1));

    mac.clock.advance(seconds(1));
    core.event(Event::Tick);
    assert_eq!(read(&mac, WORK, "s1"), Some(without_cleared(later)));
    assert_eq!(core.tick_interval(), accroster_core::driver::TICK_INTERVAL);
}

#[test]
fn the_watch_starts_once_the_sidebars_exist_resumes_after_relaunch_and_stops_when_sync_is_off() {
    let mac = FakeMac::new();
    let (mut core, effects) = automatic(&mac);
    let watch = Effect::WatchSidebarFolders(vec![mac.path(SIDEBAR)]);
    assert!(!effects.contains(&watch), "no sidebars yet: {effects:?}");

    empty_folder(&mac, WORK);
    assert!(core.event(Event::Tick).contains(&watch));
    assert!(!core.event(Event::Tick).contains(&watch), "asked once");

    drop(core);
    let mut relaunched = mac.launch();
    assert!(relaunched.take_effects().contains(&watch));

    let effects = relaunched.command(Command::SetSidebarSync(SidebarSyncMode::Off));
    assert!(
        effects.contains(&Effect::StopWatchingSidebarFolders),
        "{effects:?}"
    );
}

#[test]
fn copies_sync_wrote_itself_do_not_start_another_run() {
    // Claude is quit, so sync also writes into the desktop's own folder.
    let mac = FakeMac::new();
    put(&mac, WORK, "w1", &entry("w1", "From Work"));
    desktop_uses(&mac, PERSO);
    let (mut core, _) = automatic(&mac);
    let s1 = put(&mac, PERSO, "s1", &entry("s1", "Fix login bug"));
    changed(&mut core, &[s1]);
    mac.clock.advance(seconds(3));
    core.event(Event::Tick);
    // One sidebar per tick: Perso's and Work's.
    core.event(Event::Tick);
    let copy = mac.path(entry_path(PERSO, "w1"));
    assert!(copy.exists());

    changed(&mut core, &[copy]);

    assert_eq!(core.tick_interval(), accroster_core::driver::TICK_INTERVAL);
}

#[test]
fn manual_mode_ignores_changes_until_sync_now() {
    let mac = FakeMac::new();
    desktop_uses(&mac, PERSO);
    empty_folder(&mac, WORK);
    let mut core = manual(&mac);
    let s1 = put(&mac, PERSO, "s1", &entry("s1", "Fix login bug"));

    changed(&mut core, &[s1]);
    mac.clock.advance(seconds(10));
    core.event(Event::Tick);

    assert_eq!(read(&mac, WORK, "s1"), None);
}

// ----- Dormant multi-account mode: writes outside the desktop's folder ------------------

/// Claude runs on Perso; sync is on (Manual) and ran once, so Work's folder
/// holds a copy sync wrote.
fn claude_running_on_perso() -> (FakeMac, Core) {
    let mac = FakeMac::new();
    desktop_uses(&mac, PERSO);
    put(&mac, PERSO, "s1", &entry("s1", "Fix login bug"));
    empty_folder(&mac, WORK);
    mac.os.set_desktop_app_running(true);
    let mut core = manual(&mac);
    sync(&mut core);
    assert!(read(&mac, WORK, "s1").is_some());
    (mac, core)
}

/// Claude itself writes `id` into `profile`'s folder.
fn claude_writes(mac: &FakeMac, profile: (&str, &str), id: &str) -> PathBuf {
    let mut body = entry(id, "Parked");
    body["lastActivityAt"] = json!(1_761_600_000_000_i64);
    put(mac, profile, id, &body)
}

#[test]
fn an_entry_claude_writes_outside_its_folder_while_running_pauses_sync() {
    let (mac, mut core) = claude_running_on_perso();
    mac.clock.advance(seconds(60));
    let parked = claude_writes(&mac, WORK, "p1");

    changed(&mut core, &[parked]);

    assert!(is_paused(&core));
    assert!(
        warnings(&core)
            .contains(&"Claude wrote to a Profile it isn't using · Sidebar sync paused".to_owned()),
        "{:?}",
        warnings(&core)
    );
    let check = core
        .view()
        .diagnostics
        .checks
        .into_iter()
        .find(|check| check.name == "Dormant multi-account mode")
        .unwrap();
    assert_eq!(check.status, CheckStatus::Fail);
    let before = sidebars(&mac);
    sync(&mut core);
    assert_eq!(sidebars(&mac), before, "nothing is written while paused");

    core.command(resume_button(&core).expect("Resume is offered"));
    assert!(!is_paused(&core));
}

#[test]
fn copies_sync_wrote_and_entries_in_claudes_own_folder_are_not_a_pause() {
    let (mac, mut core) = claude_running_on_perso();
    mac.clock.advance(seconds(60));
    let own = claude_writes(&mac, PERSO, "s2");

    changed(&mut core, &[mac.path(entry_path(WORK, "s1")), own]);

    assert!(!is_paused(&core));
}

#[test]
fn writes_outside_claudes_folder_within_30_seconds_of_an_account_switch_are_not_a_pause() {
    let (mac, mut core) = claude_running_on_perso();
    mac.clock.advance(seconds(60));
    // Claude switches to Work: it starts writing Work's folder and flushes
    // Perso's Sessions into Perso's.
    mac.write_desktop_config(&json!({ "lastKnownAccountUuid": WORK.0 }));
    let work_entry = claude_writes(&mac, WORK, "w1");
    changed(&mut core, &[work_entry]);
    mac.clock.advance(seconds(20));
    let flushed = claude_writes(&mac, PERSO, "s1");
    changed(&mut core, &[flushed]);
    assert!(!is_paused(&core), "the switch's own flush");

    mac.clock.advance(seconds(11));
    let parked = claude_writes(&mac, PERSO, "s3");
    changed(&mut core, &[parked]);
    assert!(is_paused(&core), "31 s after the switch");
}

#[test]
fn writes_outside_claudes_folder_within_30_seconds_of_launch_are_not_a_pause() {
    let (mac, core) = claude_running_on_perso();
    drop(core);
    mac.clock.advance(seconds(60));
    // Claude may have switched just before AccRoster started (both open at
    // login) and still be flushing the old account's Sessions.
    let mut core = mac.launch();
    core.take_effects();
    mac.clock.advance(seconds(20));
    let flushed = claude_writes(&mac, WORK, "p1");
    changed(&mut core, &[flushed]);
    assert!(!is_paused(&core), "launch grace");

    mac.clock.advance(seconds(11));
    let parked = claude_writes(&mac, WORK, "p2");
    changed(&mut core, &[parked]);
    assert!(is_paused(&core), "31 s after launch");
}

#[test]
fn a_sign_in_in_main_log_counts_as_a_switch() {
    let (mac, mut core) = claude_running_on_perso();
    mac.clock.advance(seconds(60));
    mac.write_file(
        "Library/Logs/Claude/main.log",
        "[account] Login-state transition (loggedOut: true → false, now)\n",
    );
    core.event(Event::Tick);
    mac.clock.advance(seconds(10));
    let flushed = claude_writes(&mac, WORK, "p1");

    changed(&mut core, &[flushed]);

    assert!(!is_paused(&core));
}

#[test]
fn entries_written_outside_claudes_folder_while_it_is_quit_are_not_a_pause() {
    let (mac, mut core) = claude_running_on_perso();
    mac.os.set_desktop_app_running(false);
    mac.clock.advance(seconds(60));
    let edited = claude_writes(&mac, WORK, "p1");

    changed(&mut core, &[edited]);

    assert!(!is_paused(&core));
}
