//! The All Sessions window: every Session on the Mac (Transcripts joined with
//! every Profile's Sidebar entries and live-process files), newest first,
//! searchable, with running dots, from a persisted incremental index.

mod common;

use std::fs;
use std::time::{Duration, Instant};

use accroster_core::{
    Command, Core, Effect, Event, OpenChoice, ProfileKey, RunningDot, SessionDetail, SessionFilter,
    SessionRow, SidebarPresence,
};
use accroster_fake_mac::{FakeMac, FixtureLogin};
use common::*;
use serde_json::{Value, json};

const PROJECT: &str = "-Users-tester-Dev-app";

/// Opens All Sessions and lets the index finish, as the driver would by
/// sending ticks.
fn open(core: &mut Core) -> Vec<SessionRow> {
    let effects = core.command(Command::OpenAllSessions);
    assert!(effects.contains(&Effect::ShowAllSessions), "{effects:?}");
    finish(core)
}

fn finish(core: &mut Core) -> Vec<SessionRow> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while core.view().all_sessions.loading {
        assert!(Instant::now() < deadline, "the index finishes");
        core.event(Event::Tick);
    }
    core.view().all_sessions.rows
}

fn titles(rows: &[SessionRow]) -> Vec<&str> {
    rows.iter().map(|row| row.title.as_str()).collect()
}

/// A real prompt, as Claude Code records it.
fn prompt(id: &str, text: &str, at: &str) -> Value {
    json!({
        "type": "user", "message": {"role": "user", "content": text},
        "cwd": "/Users/tester/Dev/app", "gitBranch": "main", "entrypoint": "cli",
        "timestamp": at, "sessionId": id, "uuid": "u1", "parentUuid": null,
        "isSidechain": false, "isMeta": false, "version": "2.1.310", "userType": "external",
    })
}

#[test]
fn a_transcript_is_listed_with_its_first_prompt_project_and_age() {
    let mac = FakeMac::new();
    mac.write_transcript(
        PROJECT,
        "aaaaaaaa-0000-4000-8000-000000000001",
        &[prompt(
            "aaaaaaaa-0000-4000-8000-000000000001",
            "Fix the login bug",
            "2026-10-25T13:57:00.000Z",
        )],
    );
    let mut core = mac.launch();

    let rows = open(&mut core);

    assert_eq!(
        rows,
        vec![SessionRow {
            id: "aaaaaaaa-0000-4000-8000-000000000001".into(),
            title: "“Fix the login bug”".into(),
            subtitle: "app · 5 min ago".into(),
            dot: None,
            deleted: false,
        }]
    );
}

// ----- Fixtures --------------------------------------------------------------------

const ACCOUNT_A: &str = "aaaaaaaa-1111-4111-8111-111111111111";
const ORG_A: &str = "aaaaaaaa-2222-4222-8222-222222222222";
const ACCOUNT_B: &str = "bbbbbbbb-1111-4111-8111-111111111111";
const ORG_B: &str = "bbbbbbbb-2222-4222-8222-222222222222";

const S1: &str = "11111111-0000-4000-8000-000000000001";
const S2: &str = "11111111-0000-4000-8000-000000000002";
const S3: &str = "11111111-0000-4000-8000-000000000003";

/// 2026-10-25T13:00:00Z in epoch ms.
const ONE_HOUR_AGO_MS: i64 = 1_792_933_200_000;

/// A Transcript with one prompt, `minutes_ago` before the fake clock.
fn transcript(mac: &FakeMac, id: &str, text: &str, minutes_ago: i64, extra: &[Value]) {
    let at = mac_time(minutes_ago);
    let mut records = vec![prompt(id, text, &at)];
    records.extend_from_slice(extra);
    mac.write_transcript(PROJECT, id, &records);
}

fn mac_time(minutes_ago: i64) -> String {
    let start: jiff::Timestamp = accroster_fake_mac::START_TIME.parse().unwrap();
    (start - jiff::SignedDuration::from_mins(minutes_ago)).to_string()
}

fn entry(cli_session_id: &str, title: &str, title_source: &str, last_activity_at: i64) -> Value {
    json!({
        "sessionId": format!("local_{cli_session_id}"), "cliSessionId": cli_session_id,
        "title": title, "titleSource": title_source, "isArchived": false,
        "createdAt": last_activity_at, "lastActivityAt": last_activity_at,
        "cwd": "/Users/tester/Dev/app", "bridgeSessionIds": [], "completedTurns": 1,
    })
}

fn desktop_signed_in_as(mac: &FakeMac, account: &str) {
    mac.write_desktop_config(&json!({
        "lastKnownAccountUuid": account,
        "oauth:tokenCache": "secret-never-read",
    }));
}

/// A live-process file for a process started an hour ago.
fn live(mac: &FakeMac, config_dir: &str, pid: u32, session: &str, entrypoint: &str, status: &str) {
    mac.write_live_process(
        config_dir,
        pid,
        &json!({
            "pid": pid, "sessionId": session, "cwd": "/Users/tester/Dev/app",
            "startedAt": ONE_HOUR_AGO_MS, "procStart": "Sun Oct 25 13:00:00 2026",
            "version": "2.1.308", "kind": "interactive", "entrypoint": entrypoint,
            "status": status, "updatedAt": ONE_HOUR_AGO_MS,
        }),
    );
}

/// Starts the process a [`live`] file names.
fn start(mac: &FakeMac, pid: u32) {
    mac.os
        .start_process(pid, 1, "2026-10-25T13:00:00Z".parse().unwrap());
}

fn row<'a>(rows: &'a [SessionRow], id: &str) -> &'a SessionRow {
    rows.iter()
        .find(|row| row.id == id)
        .unwrap_or_else(|| panic!("a row for {id} in {rows:?}"))
}

// ----- Menu and order --------------------------------------------------------------

#[test]
fn all_sessions_in_the_menu_opens_the_window() {
    let mac = FakeMac::new();
    let mut core = mac.launch();

    assert!(action_titles(&core).contains(&"All Sessions…".to_owned()));
    click(&mut core, "All Sessions…");

    assert!(core.view().all_sessions.loading);
    assert!(finish(&mut core).is_empty());
}

#[test]
fn sessions_are_listed_newest_first() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "oldest", 3 * 24 * 60, &[]);
    transcript(&mac, S2, "newest", 2, &[]);
    transcript(&mac, S3, "middle", 90, &[]);
    let mut core = mac.launch();

    let rows = open(&mut core);

    assert_eq!(titles(&rows), ["“newest”", "“middle”", "“oldest”"]);
    let subtitles: Vec<&str> = rows.iter().map(|row| row.subtitle.as_str()).collect();
    assert_eq!(
        subtitles,
        ["app · 2 min ago", "app · 1 h ago", "app · 3 days ago"]
    );
}

// ----- Sources and joins -----------------------------------------------------------

#[test]
fn subagent_transcripts_and_links_are_not_followed() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "the real one", 5, &[]);
    mac.write_file(
        format!(".claude/projects/{PROJECT}/{S1}/subagents/agent-1.jsonl"),
        format!("{}\n", prompt(S2, "a subagent", &mac_time(1))),
    );
    let elsewhere = mac.write_file(
        "elsewhere/linked.jsonl",
        format!("{}\n", prompt(S3, "linked", &mac_time(1))),
    );
    std::os::unix::fs::symlink(
        &elsewhere,
        mac.path(format!(".claude/projects/{PROJECT}/{S3}.jsonl")),
    )
    .unwrap();
    std::os::unix::fs::symlink(mac.path("elsewhere"), mac.path(".claude/projects/-linked"))
        .unwrap();
    let mut core = mac.launch();

    assert_eq!(titles(&open(&mut core)), ["“the real one”"]);
}

#[test]
fn sidebar_entries_from_every_profile_folder_join_their_transcript() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "first prompt", 5, &[]);
    transcript(&mac, S2, "other prompt", 10, &[]);
    mac.write_sidebar_entry(ACCOUNT_A, ORG_A, "x1", &entry(S1, "Named in A", "user", 1));
    mac.write_sidebar_entry(ACCOUNT_B, ORG_B, "x2", &entry(S2, "Named in B", "user", 1));
    let mut core = mac.launch();

    let rows = open(&mut core);

    assert_eq!(rows.len(), 2, "one row per Transcript: {rows:?}");
    assert_eq!(row(&rows, S1).title, "Named in A");
    assert_eq!(row(&rows, S2).title, "Named in B");
}

#[test]
fn scratch_ssh_and_scheduled_task_entries_are_skipped() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "scratch", 5, &[]);
    transcript(&mac, S2, "ssh", 6, &[]);
    transcript(&mac, S3, "scheduled", 7, &[]);
    let mut scratch = entry(S1, "Scratch title", "user", 1);
    scratch["cwd"] = json!(format!(
        "/Users/tester/Library/Application Support/Claude/scratch-workspaces/{ACCOUNT_A}/{ORG_A}/x"
    ));
    let mut ssh = entry(S2, "SSH title", "user", 1);
    ssh["sshConfig"] = json!({"host": "box"});
    let mut scheduled = entry(S3, "Scheduled title", "user", 1);
    scheduled["scheduledTaskId"] = json!("task-1");
    mac.write_sidebar_entry(ACCOUNT_A, ORG_A, "x1", &scratch);
    mac.write_sidebar_entry(ACCOUNT_A, ORG_A, "x2", &ssh);
    mac.write_sidebar_entry(ACCOUNT_A, ORG_A, "x3", &scheduled);
    let mut core = mac.launch();

    assert_eq!(
        titles(&open(&mut core)),
        ["“scratch”", "“ssh”", "“scheduled”"]
    );
}

#[test]
fn an_earlier_transcript_left_by_clear_is_its_own_row() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "before clear", 60, &[]);
    transcript(&mac, S2, "after clear", 5, &[]);
    let mut current = entry(S2, "Desktop title", "user", 1);
    current["priorCliSessionIds"] = json!([S1]);
    mac.write_sidebar_entry(ACCOUNT_A, ORG_A, "x1", &current);
    let mut core = mac.launch();

    assert_eq!(
        titles(&open(&mut core)),
        ["Desktop title", "“before clear”"]
    );
}

#[test]
fn a_live_process_without_a_transcript_adds_no_row() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "only this", 5, &[]);
    live(&mac, ".claude", 4242, S3, "cli", "busy");
    start(&mac, 4242);
    let mut core = mac.launch();

    assert_eq!(titles(&open(&mut core)), ["“only this”"]);
}

// ----- Titles ----------------------------------------------------------------------

#[test]
fn a_user_set_title_beats_automatic_ones() {
    let mac = FakeMac::new();
    let ai = json!({"type": "ai-title", "aiTitle": "AI title", "sessionId": S1});
    transcript(&mac, S1, "prompt", 5, &[ai]);
    mac.write_sidebar_entry(ACCOUNT_A, ORG_A, "x1", &entry(S1, "Auto newest", "auto", 9));
    mac.write_sidebar_entry(ACCOUNT_B, ORG_B, "x1", &entry(S1, "Mine", "user", 1));
    let mut core = mac.launch();

    assert_eq!(titles(&open(&mut core)), ["Mine"]);
}

#[test]
fn the_transcripts_last_custom_title_is_user_set() {
    let mac = FakeMac::new();
    transcript(
        &mac,
        S1,
        "prompt",
        5,
        &[
            json!({"type": "custom-title", "customTitle": "Old name", "sessionId": S1}),
            json!({"type": "custom-title", "customTitle": "Renamed in CLI", "sessionId": S1}),
        ],
    );
    mac.write_sidebar_entry(ACCOUNT_A, ORG_A, "x1", &entry(S1, "Auto title", "auto", 9));
    let mut core = mac.launch();

    assert_eq!(titles(&open(&mut core)), ["Renamed in CLI"]);
}

#[test]
fn among_entries_the_desktops_active_profile_wins_then_the_newest() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "prompt", 5, &[]);
    mac.write_sidebar_entry(ACCOUNT_A, ORG_A, "x1", &entry(S1, "Title in A", "user", 1));
    mac.write_sidebar_entry(ACCOUNT_B, ORG_B, "x1", &entry(S1, "Title in B", "user", 2));
    let mut core = mac.launch();
    assert_eq!(titles(&open(&mut core)), ["Title in B"], "newest wins");

    desktop_signed_in_as(&mac, ACCOUNT_A);
    assert_eq!(
        titles(&open(&mut core)),
        ["Title in A"],
        "the desktop's entry wins"
    );
}

#[test]
fn the_desktops_organization_is_the_folder_it_wrote_last() {
    let mac = FakeMac::new();
    let other_org = "aaaaaaaa-3333-4333-8333-333333333333";
    transcript(&mac, S1, "prompt", 5, &[]);
    transcript(&mac, S2, "other", 6, &[]);
    // The desktop is signed in to account A; it last wrote in `other_org`.
    let older = mac.write_sidebar_entry(ACCOUNT_A, ORG_A, "x1", &entry(S1, "In org A", "user", 1));
    mac.write_sidebar_entry(
        ACCOUNT_A,
        other_org,
        "x2",
        &entry(S2, "Elsewhere", "user", 1),
    );
    mac.write_sidebar_entry(ACCOUNT_B, ORG_B, "x1", &entry(S1, "In B, newer", "user", 2));
    let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(older)
        .unwrap()
        .set_modified(an_hour_ago)
        .unwrap();
    desktop_signed_in_as(&mac, ACCOUNT_A);
    let mut core = mac.launch();

    assert_eq!(
        row(&open(&mut core), S1).title,
        "In B, newer",
        "org A isn't the desktop's Active Profile"
    );
}

#[test]
fn without_titles_the_ai_title_then_the_first_prompt_in_quotes() {
    let mac = FakeMac::new();
    let ai = json!({"type": "ai-title", "aiTitle": "Fixes the build", "sessionId": S1});
    transcript(&mac, S1, "untitled", 5, &[ai]);
    transcript(
        &mac,
        S2,
        "Please refactor the whole authentication module and add tests for it",
        6,
        &[],
    );
    let mut core = mac.launch();

    let rows = open(&mut core);

    assert_eq!(row(&rows, S1).title, "Fixes the build");
    assert_eq!(
        row(&rows, S2).title,
        "“Please refactor the whole authentication module…”"
    );
}

#[test]
fn the_first_prompt_skips_tool_results_meta_and_command_wrappers() {
    let mac = FakeMac::new();
    let at = mac_time(5);
    let mut meta = prompt(S1, "Caveat: meta", &at);
    meta["isMeta"] = json!(true);
    let command = prompt(S1, "<command-name>/clear</command-name>", &at);
    let mut tool = prompt(S1, "tool output", &at);
    tool["toolUseResult"] = json!({});
    let blocks = json!({
        "type": "user",
        "message": {"role": "user", "content": [
            {"type": "text", "text": "Real"}, {"type": "text", "text": "question"},
        ]},
        "cwd": "/Users/tester/Dev/app", "timestamp": at, "sessionId": S1, "isMeta": false,
    });
    mac.write_transcript(PROJECT, S1, &[meta, command, tool, blocks]);
    let mut core = mac.launch();

    assert_eq!(titles(&open(&mut core)), ["“Real question”"]);
}

#[test]
fn the_view_never_writes_claudes_files() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "prompt", 5, &[]);
    mac.write_sidebar_entry(ACCOUNT_A, ORG_A, "x1", &entry(S1, "Title", "user", 1));
    desktop_signed_in_as(&mac, ACCOUNT_A);
    live(&mac, ".claude", 4242, S1, "cli", "idle");
    start(&mac, 4242);
    let before = mac.snapshot();
    let mut core = mac.launch();

    open(&mut core);
    core.command(Command::SearchSessions("tit".into()));

    let changed: Vec<_> = mac
        .snapshot()
        .into_iter()
        .filter(|(path, contents)| before.get(path) != Some(contents))
        .map(|(path, _)| path)
        .filter(|path| !path.starts_with(APP_FOLDER))
        .collect();
    assert!(changed.is_empty(), "wrote {changed:?}");
    let log = mac.read_file(format!("{APP_FOLDER}/logs/app.log")).unwrap();
    assert!(log.contains("session index built"), "{log}");
    assert!(!log.contains("Title") && !log.contains("“"), "{log}");
}

// ----- Dots ------------------------------------------------------------------------

#[test]
fn a_process_outside_the_desktop_shows_green_in_any_status() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "cli idle", 5, &[]);
    transcript(&mac, S2, "in a profile", 6, &[]);
    // A signed-in Profile's folder, where CLI runs under that Profile write.
    mac.write_json(
        format!("{PROFILES}/p1/.claude.json"),
        &FixtureLogin::perso().claude_json(),
    );
    live(&mac, ".claude", 4242, S1, "cli", "idle");
    live(&mac, &format!("{PROFILES}/p1"), 4343, S2, "sdk-ts", "busy");
    start(&mac, 4242);
    start(&mac, 4343);
    let mut core = mac.launch();

    let rows = open(&mut core);

    assert_eq!(row(&rows, S1).dot, Some(RunningDot::Terminal));
    assert_eq!(row(&rows, S2).dot, Some(RunningDot::Terminal));
}

#[test]
fn a_desktop_process_shows_orange_only_while_busy_or_waiting() {
    let mac = FakeMac::new();
    let cases = [
        (1001, S1, "busy"),
        (1002, S2, "idle"),
        (1003, S3, "thinking-hard"),
    ];
    for (pid, id, status) in cases {
        transcript(&mac, id, status, 5, &[]);
        live(&mac, ".claude", pid, id, "claude-desktop", status);
        start(&mac, pid);
    }
    let mut core = mac.launch();

    let rows = open(&mut core);

    assert_eq!(row(&rows, S1).dot, Some(RunningDot::Desktop));
    assert_eq!(row(&rows, S2).dot, None);
    assert_eq!(
        row(&rows, S3).dot,
        Some(RunningDot::Desktop),
        "an unknown status counts as busy"
    );
}

#[test]
fn stale_live_process_files_show_no_dot() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "dead pid", 5, &[]);
    transcript(&mac, S2, "reused pid", 6, &[]);
    live(&mac, ".claude", 5001, S1, "cli", "busy");
    live(&mac, ".claude", 5002, S2, "claude-desktop", "busy");
    // 5001 is gone; 5002 now belongs to a process started at another time.
    mac.os
        .start_process(5002, 1, "2026-10-25T13:30:00Z".parse().unwrap());
    let mut core = mac.launch();

    let rows = open(&mut core);

    assert_eq!(row(&rows, S1).dot, None);
    assert_eq!(row(&rows, S2).dot, None);
}

// ----- Search ----------------------------------------------------------------------

#[test]
fn search_matches_title_first_prompt_project_and_branch() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "Fix the login bug", 5, &[]);
    let mut other = prompt(S2, "Write docs", &mac_time(6));
    other["cwd"] = json!("/Users/tester/Dev/website");
    other["gitBranch"] = json!("feature/landing");
    mac.write_transcript("-Users-tester-Dev-website", S2, &[other]);
    transcript(&mac, S3, "Something", 7, &[]);
    mac.write_sidebar_entry(
        ACCOUNT_A,
        ORG_A,
        "x3",
        &entry(S3, "Release checklist", "user", 1),
    );
    let mut core = mac.launch();
    open(&mut core);

    let search = |core: &mut Core, text: &str| {
        core.command(Command::SearchSessions(text.into()));
        let view = core.view().all_sessions;
        assert_eq!(view.search, text);
        view.rows.into_iter().map(|row| row.id).collect::<Vec<_>>()
    };

    assert_eq!(search(&mut core, "LOGIN"), [S1], "first prompt, any case");
    assert_eq!(search(&mut core, "website"), [S2], "project");
    assert_eq!(search(&mut core, "landing"), [S2], "branch");
    assert_eq!(search(&mut core, "checklist"), [S3], "title");
    assert_eq!(
        search(&mut core, "docs website"),
        [S2],
        "every word matches"
    );
    assert!(search(&mut core, "nothing like this").is_empty());
    assert_eq!(search(&mut core, "").len(), 3);
}

// ----- The index -------------------------------------------------------------------

#[test]
fn the_first_build_fills_the_list_in_batches() {
    let mac = FakeMac::new();
    for n in 0..45 {
        let id = format!("22222222-0000-4000-8000-{n:012}");
        transcript(&mac, &id, &format!("prompt {n}"), n + 1, &[]);
    }
    let mut core = mac.launch();

    core.command(Command::OpenAllSessions);
    let mut seen = vec![core.view().all_sessions.rows.len()];
    while core.view().all_sessions.loading {
        assert_eq!(
            core.tick_interval(),
            Duration::ZERO,
            "the next batch comes at once"
        );
        core.event(Event::Tick);
        seen.push(core.view().all_sessions.rows.len());
    }

    assert_eq!(seen.first(), Some(&0));
    assert_eq!(seen.last(), Some(&45));
    assert!(seen.len() > 3, "filled in over several steps: {seen:?}");
    assert!(
        core.tick_interval() > Duration::from_secs(1),
        "idle once built"
    );
}

#[test]
fn the_index_is_persisted_and_a_relaunch_reads_no_known_bytes_again() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "original", 5, &[]);
    let mut core = mac.launch();
    open(&mut core);
    assert!(
        mac.path(format!("{APP_FOLDER}/index/sessions.json"))
            .is_file()
    );

    // Same size and mtime: a relaunched core trusts what it read before.
    let path = mac.path(format!(".claude/projects/{PROJECT}/{S1}.jsonl"));
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    let body = fs::read_to_string(&path)
        .unwrap()
        .replace("original", "replaced");
    fs::write(&path, body).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    let mut relaunched = mac.launch();

    assert_eq!(titles(&open(&mut relaunched)), ["“original”"]);
}

#[test]
fn a_grown_transcript_is_read_from_its_stored_offset() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "original", 5, &[]);
    let mut core = mac.launch();
    open(&mut core);

    // Bytes before the stored offset are not read again: the edited first
    // prompt stays as read, while the appended title is picked up.
    let path = mac.path(format!(".claude/projects/{PROJECT}/{S1}.jsonl"));
    let mut body = fs::read_to_string(&path)
        .unwrap()
        .replace("original", "replaced");
    body.push_str(&format!(
        "{}\n",
        json!({"type": "custom-title", "customTitle": "Named later", "sessionId": S1})
    ));
    fs::write(&path, body).unwrap();

    assert_eq!(titles(&open(&mut core)), ["Named later"]);
    core.command(Command::SearchSessions("original".into()));
    assert_eq!(core.view().all_sessions.rows.len(), 1, "first prompt kept");
}

#[test]
fn a_partly_written_last_line_is_read_once_complete() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "prompt", 5, &[]);
    let path = mac.path(format!(".claude/projects/{PROJECT}/{S1}.jsonl"));
    let line = json!({"type": "custom-title", "customTitle": "Finished title", "sessionId": S1})
        .to_string();
    let (head, tail) = line.split_at(20);
    let mut body = fs::read_to_string(&path).unwrap();
    body.push_str(head);
    fs::write(&path, &body).unwrap();
    let mut core = mac.launch();
    assert_eq!(titles(&open(&mut core)), ["“prompt”"]);

    body.push_str(tail);
    body.push('\n');
    fs::write(&path, &body).unwrap();

    assert_eq!(titles(&open(&mut core)), ["Finished title"]);
}

#[test]
fn a_deleted_index_is_rebuilt() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "prompt", 5, &[]);
    let mut core = mac.launch();
    open(&mut core);
    let index = mac.path(format!("{APP_FOLDER}/index/sessions.json"));
    fs::remove_file(&index).unwrap();

    let mut relaunched = mac.launch();
    assert_eq!(titles(&open(&mut relaunched)), ["“prompt”"]);
    assert!(index.is_file());
}

#[test]
fn an_unreadable_index_is_rebuilt() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "prompt", 5, &[]);
    mac.write_file(format!("{APP_FOLDER}/index/sessions.json"), "{not json");
    let mut core = mac.launch();

    assert_eq!(titles(&open(&mut core)), ["“prompt”"]);
}

#[test]
fn a_deleted_transcript_leaves_the_list() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "kept", 5, &[]);
    let gone = mac.write_transcript(PROJECT, S2, &[prompt(S2, "gone", &mac_time(6))]);
    let mut core = mac.launch();
    assert_eq!(open(&mut core).len(), 2);

    fs::remove_file(gone).unwrap();

    assert_eq!(titles(&open(&mut core)), ["“kept”"]);
}

// ----- Archive and sidebar filters --------------------------------------------------

fn archived(mut entry: Value) -> Value {
    entry["isArchived"] = json!(true);
    entry
}

fn pair(account: &str, org: &str) -> ProfileKey {
    ProfileKey {
        account_uuid: account.into(),
        organization_uuid: org.into(),
    }
}

fn ids(core: &Core) -> Vec<String> {
    core.view()
        .all_sessions
        .rows
        .into_iter()
        .map(|row| row.id)
        .collect()
}

fn filter(core: &mut Core, filter: SessionFilter) -> Vec<String> {
    core.command(Command::FilterSessions(filter.clone()));
    assert_eq!(core.view().all_sessions.filter, filter);
    ids(core)
}

#[test]
fn a_session_leaves_all_sessions_only_when_every_entry_is_archived() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "archived in A only", 5, &[]);
    transcript(&mac, S2, "archived everywhere", 6, &[]);
    transcript(&mac, S3, "cli only", 7, &[]);
    mac.write_sidebar_entry(
        ACCOUNT_A,
        ORG_A,
        "x1",
        &archived(entry(S1, "One", "user", 1)),
    );
    mac.write_sidebar_entry(ACCOUNT_B, ORG_B, "x1", &entry(S1, "One", "user", 1));
    mac.write_sidebar_entry(
        ACCOUNT_A,
        ORG_A,
        "x2",
        &archived(entry(S2, "Two", "user", 1)),
    );
    mac.write_sidebar_entry(
        ACCOUNT_B,
        ORG_B,
        "x2",
        &archived(entry(S2, "Two", "user", 1)),
    );
    let mut core = mac.launch();
    open(&mut core);

    assert_eq!(ids(&core), [S1, S3]);
    assert_eq!(
        filter(&mut core, SessionFilter::Sidebar(pair(ACCOUNT_A, ORG_A))),
        [S1, S2],
        "a Profile's Sidebar lists its archived entries too"
    );
    let rows = core.view().all_sessions.rows;
    assert_eq!(row(&rows, S2).subtitle, "app · 6 min ago · archived");
    assert_eq!(
        row(&rows, S1).subtitle,
        "app · 5 min ago",
        "not archived everywhere"
    );
}

#[test]
fn the_sidebar_filters_running_project_profile_and_cli_only() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "running cli", 5, &[]);
    let mut web = prompt(S2, "website work", &mac_time(6));
    web["cwd"] = json!("/Users/tester/Dev/website");
    mac.write_transcript("-Users-tester-Dev-website", S2, &[web]);
    transcript(&mac, S3, "in B", 7, &[]);
    mac.write_sidebar_entry(ACCOUNT_B, ORG_B, "x3", &entry(S3, "In B", "user", 1));
    live(&mac, ".claude", 4242, S1, "cli", "idle");
    start(&mac, 4242);
    let mut core = mac.launch();
    open(&mut core);

    assert_eq!(filter(&mut core, SessionFilter::Running), [S1]);
    assert_eq!(
        filter(
            &mut core,
            SessionFilter::Project("/Users/tester/Dev/website".into())
        ),
        [S2]
    );
    assert_eq!(
        filter(
            &mut core,
            SessionFilter::Project("/Users/tester/Dev/app".into())
        ),
        [S1, S3]
    );
    assert_eq!(
        filter(&mut core, SessionFilter::Sidebar(pair(ACCOUNT_B, ORG_B))),
        [S3]
    );
    assert_eq!(filter(&mut core, SessionFilter::CliOnly), [S1, S2]);
    assert_eq!(filter(&mut core, SessionFilter::All), [S1, S2, S3]);
}

#[test]
fn search_narrows_the_filtered_list() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "fix login", 5, &[]);
    transcript(&mac, S2, "fix logout", 6, &[]);
    mac.write_sidebar_entry(ACCOUNT_A, ORG_A, "x2", &entry(S2, "Logout", "auto", 1));
    let mut core = mac.launch();
    open(&mut core);

    core.command(Command::SearchSessions("fix".into()));
    assert_eq!(filter(&mut core, SessionFilter::CliOnly), [S1]);
}

#[test]
fn the_sidebar_lists_projects_and_every_profiles_sidebar() {
    let mac = FakeMac::new();
    let perso = FixtureLogin::perso();
    mac.write_json(format!("{PROFILES}/p1/.claude.json"), &perso.claude_json());
    transcript(&mac, S1, "app", 5, &[]);
    let mut web = prompt(S2, "website", &mac_time(6));
    web["cwd"] = json!("/Users/tester/Dev/website");
    mac.write_transcript("-Users-tester-Dev-website", S2, &[web]);
    // Account B has entries on this Mac but no Profile in AccRoster.
    mac.write_sidebar_entry(ACCOUNT_B, ORG_B, "x1", &entry(S1, "Title", "user", 1));
    let mut core = mac.launch();
    open(&mut core);

    let sidebar = core.view().all_sessions.sidebar;
    type Items<'a> = Vec<(&'a str, &'a SessionFilter)>;
    let groups: Vec<(Option<&str>, Items)> = sidebar
        .iter()
        .map(|group| {
            (
                group.title.as_deref(),
                group
                    .items
                    .iter()
                    .map(|item| (item.title.as_str(), &item.filter))
                    .collect(),
            )
        })
        .collect();
    let perso_pair = pair(&perso.account_uuid, &perso.organization_uuid);
    assert_eq!(
        groups,
        [
            (
                None,
                vec![
                    ("All Sessions", &SessionFilter::All),
                    ("Running", &SessionFilter::Running),
                ]
            ),
            (
                Some("Projects"),
                vec![
                    (
                        "app",
                        &SessionFilter::Project("/Users/tester/Dev/app".into())
                    ),
                    (
                        "website",
                        &SessionFilter::Project("/Users/tester/Dev/website".into())
                    ),
                ]
            ),
            (
                Some("Sidebars"),
                vec![
                    ("me@example.com", &SessionFilter::Sidebar(perso_pair)),
                    (
                        "Unknown Login · 1 Session · last active Jan 1 (not signed in)",
                        &SessionFilter::Sidebar(pair(ACCOUNT_B, ORG_B))
                    ),
                    ("CLI only (no Sidebar entry)", &SessionFilter::CliOnly),
                ]
            ),
        ]
    );
}

// ----- Detail pane and deleted Transcripts -----------------------------------------

fn select(core: &mut Core, id: &str) -> SessionDetail {
    core.command(Command::SelectSession(Some(id.into())));
    core.view()
        .all_sessions
        .detail
        .unwrap_or_else(|| panic!("a detail pane for {id}"))
}

#[test]
fn the_detail_pane_shows_where_a_session_lives() {
    let mac = FakeMac::new();
    let perso = FixtureLogin::perso();
    mac.write_json(format!("{PROFILES}/p1/.claude.json"), &perso.claude_json());
    let folder = mac.path("Dev/app").to_string_lossy().into_owned();
    let mut first = prompt(S1, "Fix the login bug", &mac_time(30));
    first["cwd"] = json!(folder);
    let mut second = prompt(S1, "And add a test", &mac_time(20));
    second["cwd"] = json!(folder);
    // The branch and client shown are the last ones recorded.
    second["gitBranch"] = json!("fix/login");
    second["entrypoint"] = json!("claude-desktop");
    let mut tool = prompt(S1, "tool output", &mac_time(20));
    tool["toolUseResult"] = json!({});
    mac.write_transcript(PROJECT, S1, &[first, tool, second]);
    mac.write_sidebar_entry(
        &perso.account_uuid,
        &perso.organization_uuid,
        "x1",
        &archived(entry(S1, "Login fix", "user", 1)),
    );
    mac.write_sidebar_entry(ACCOUNT_B, ORG_B, "x9", &entry(S2, "Other", "user", 1));
    desktop_signed_in_as(&mac, &perso.account_uuid);
    let mut core = mac.launch();
    open(&mut core);
    // Archived everywhere it has an entry: listed under that Sidebar only.
    let perso_sidebar = pair(&perso.account_uuid, &perso.organization_uuid);
    assert_eq!(
        filter(&mut core, SessionFilter::Sidebar(perso_sidebar)),
        [S1]
    );

    assert_eq!(
        select(&mut core, S1),
        SessionDetail {
            id: S1.into(),
            title: "Login fix".into(),
            folder: Some("~/Dev/app".into()),
            branch: Some("fix/login".into()),
            last_used: "Last used in Claude desktop, 20 min ago".into(),
            turns: Some(2),
            first_prompt: Some("Fix the login bug".into()),
            dot: None,
            sidebars: vec![
                SidebarPresence {
                    profile: "me@example.com".into(),
                    has_entry: true,
                    archived: true,
                    desktop_now: true,
                },
                SidebarPresence {
                    profile: "Unknown Login · 1 Session · last active Jan 1 (not signed in)".into(),
                    has_entry: false,
                    archived: false,
                    desktop_now: false,
                },
            ],
            deleted: None,
            // Claude isn't installed, and `~/Dev/app` was never created.
            open: vec![OpenChoice {
                title: "Resume in Terminal".into(),
                note: Some("Its folder no longer exists".into()),
                command: Command::ResumeInTerminal(S1.into()),
                enabled: false,
            }],
        }
    );
}

#[test]
fn an_entry_whose_transcript_is_gone_is_a_greyed_transcript_deleted_row() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "still here", 5, &[]);
    // Last active three days ago; its Transcript was cleaned up since.
    let three_days_ago = ONE_HOUR_AGO_MS + 3_600_000 - 3 * 24 * 3_600_000;
    mac.write_sidebar_entry(
        ACCOUNT_A,
        ORG_A,
        "x2",
        &entry(S2, "Old migration plan", "user", three_days_ago),
    );
    let mut core = mac.launch();

    let rows = open(&mut core);

    assert_eq!(
        row(&rows, S2),
        &SessionRow {
            id: S2.into(),
            title: "Old migration plan".into(),
            subtitle: "app · 3 days ago · transcript deleted".into(),
            dot: None,
            deleted: true,
        }
    );
    assert!(!row(&rows, S1).deleted);
    let detail = select(&mut core, S2);
    assert_eq!(detail.turns, None);
    assert_eq!(detail.first_prompt, None);
    assert_eq!(detail.last_used, "Last active 3 days ago");
    assert!(
        detail
            .deleted
            .as_deref()
            .is_some_and(|why| why.contains("Transcript was deleted")),
        "{detail:?}"
    );
    assert!(detail.sidebars.iter().any(|sidebar| sidebar.has_entry));
}

#[test]
fn the_detail_pane_follows_the_list() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "cli", 5, &[]);
    transcript(&mac, S2, "in a sidebar", 6, &[]);
    mac.write_sidebar_entry(ACCOUNT_A, ORG_A, "x2", &entry(S2, "Titled", "user", 1));
    let mut core = mac.launch();
    open(&mut core);
    select(&mut core, S2);

    filter(&mut core, SessionFilter::CliOnly);
    assert_eq!(core.view().all_sessions.detail, None, "not in the list");

    filter(&mut core, SessionFilter::All);
    assert_eq!(
        core.view().all_sessions.detail.map(|d| d.id),
        Some(S2.into())
    );
    core.command(Command::SelectSession(None));
    assert_eq!(core.view().all_sessions.detail, None);
}

// ----- Freshness -------------------------------------------------------------------

fn transcript_path(mac: &FakeMac, id: &str) -> std::path::PathBuf {
    mac.path(format!(".claude/projects/{PROJECT}/{id}.jsonl"))
}

fn append(path: &std::path::Path, record: Value) {
    let mut body = fs::read_to_string(path).unwrap();
    body.push_str(&format!("{record}\n"));
    fs::write(path, body).unwrap();
}

fn title_record(id: &str, title: &str) -> Value {
    json!({"type": "custom-title", "customTitle": title, "sessionId": id})
}

/// Rewrites a Transcript in place, keeping its size and mtime, so only a
/// read of its bytes (not a stat) would notice.
fn rewrite_unnoticed(path: &std::path::Path, from: &str, to: &str) {
    assert_eq!(from.len(), to.len());
    let mtime = fs::metadata(path).unwrap().modified().unwrap();
    let body = fs::read_to_string(path).unwrap().replace(from, to);
    fs::write(path, body).unwrap();
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

#[test]
fn the_open_window_watches_claudes_folders_until_it_closes() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.write_json(
        format!("{PROFILES}/p1/.claude.json"),
        &FixtureLogin::perso().claude_json(),
    );
    mac.write_file(format!("{PROFILES}/p1/sessions/.keep"), "");
    mac.write_sidebar_entry(ACCOUNT_A, ORG_A, "x1", &entry(S1, "Title", "user", 1));
    let mut core = mac.launch();

    let watched = vec![
        mac.path(".claude/projects"),
        mac.path(".claude/sessions"),
        mac.path(format!("{PROFILES}/p1/sessions")),
        mac.path("Library/Application Support/Claude/claude-code-sessions"),
    ];
    assert_eq!(
        core.command(Command::OpenAllSessions),
        vec![
            Effect::ShowAllSessions,
            Effect::WatchSessionFiles(watched.clone())
        ]
    );
    assert_eq!(
        core.command(Command::OpenAllSessions),
        vec![Effect::ShowAllSessions],
        "already watching"
    );
    assert_eq!(
        core.command(Command::CloseAllSessions),
        vec![Effect::StopWatchingSessionFiles]
    );
    assert_eq!(
        core.command(Command::OpenAllSessions),
        vec![Effect::ShowAllSessions, Effect::WatchSessionFiles(watched)]
    );
}

#[test]
fn a_changed_transcript_is_reindexed_while_the_window_is_open() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "original", 5, &[]);
    let mut core = mac.launch();
    open(&mut core);

    let path = transcript_path(&mac, S1);
    append(&path, title_record(S1, "Named while open"));
    core.event(Event::SessionFilesChanged(vec![path]));

    assert_eq!(titles(&finish(&mut core)), ["Named while open"]);
}

#[test]
fn only_the_files_fsevents_named_are_reindexed() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "first", 5, &[]);
    transcript(&mac, S2, "second", 6, &[]);
    let mut core = mac.launch();
    open(&mut core);

    let named = transcript_path(&mac, S1);
    append(&named, title_record(S1, "Renamed"));
    // Changed too, but not named by this batch of events.
    append(&transcript_path(&mac, S2), title_record(S2, "Not yet"));
    core.event(Event::SessionFilesChanged(vec![named]));

    assert_eq!(titles(&finish(&mut core)), ["Renamed", "“second”"]);
}

#[test]
fn new_transcripts_entries_and_processes_show_up_while_open() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    transcript(&mac, S1, "existing", 5, &[]);
    let mut core = mac.launch();
    open(&mut core);

    transcript(&mac, S2, "new one", 1, &[]);
    let entry_path = mac.write_sidebar_entry(
        ACCOUNT_A,
        ORG_A,
        "x1",
        &entry(S1, "Named in desktop", "user", 1),
    );
    live(&mac, ".claude", 4242, S2, "cli", "busy");
    start(&mac, 4242);
    core.event(Event::SessionFilesChanged(vec![
        transcript_path(&mac, S2),
        entry_path,
        mac.path(".claude/sessions/4242.json"),
    ]));

    let rows = finish(&mut core);
    assert_eq!(titles(&rows), ["“new one”", "Named in desktop"]);
    assert_eq!(row(&rows, S2).dot, Some(RunningDot::Terminal));
}

#[test]
fn a_transcript_deleted_while_open_leaves_the_list() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "kept", 5, &[]);
    transcript(&mac, S2, "gone", 6, &[]);
    let mut core = mac.launch();
    open(&mut core);

    let gone = transcript_path(&mac, S2);
    fs::remove_file(&gone).unwrap();
    core.event(Event::SessionFilesChanged(vec![gone]));

    assert_eq!(titles(&finish(&mut core)), ["“kept”"]);
}

#[test]
fn a_folder_event_restats_the_whole_folder() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "first", 5, &[]);
    let mut core = mac.launch();
    open(&mut core);

    transcript(&mac, S2, "copied in", 1, &[]);
    core.event(Event::SessionFilesChanged(vec![
        mac.path(format!(".claude/projects/{PROJECT}")),
    ]));

    assert_eq!(titles(&finish(&mut core)), ["“copied in”", "“first”"]);
}

#[test]
fn while_closed_nothing_is_watched_and_a_menu_open_rescans_by_stat() {
    let mac = FakeMac::new();
    transcript(&mac, S1, "original", 5, &[]);
    transcript(&mac, S2, "untouched", 6, &[]);
    let mut core = mac.launch();
    open(&mut core);
    core.command(Command::CloseAllSessions);

    let path = transcript_path(&mac, S1);
    append(&path, title_record(S1, "Named while closed"));
    rewrite_unnoticed(&transcript_path(&mac, S2), "untouched", "rewritten");
    core.event(Event::SessionFilesChanged(vec![path]));
    assert_eq!(
        titles(&core.view().all_sessions.rows),
        ["“original”", "“untouched”"],
        "events after closing are ignored"
    );

    core.event(Event::MenuOpened);
    assert_eq!(
        titles(&finish(&mut core)),
        ["Named while closed", "“untouched”"],
        "only the file whose stat changed is read"
    );
}
