//! Reopen Claude: after a sync added Sessions to the sidebar Claude is
//! using while it runs, the menu and Settings offer "Reopen Claude". It
//! waits until Claude is idle (the Guided switch's busy guard), then quits
//! Claude and opens it again, so it reads its sidebar afresh. Nothing else
//! ever quits Claude.

use accroster_core::{Command, Core, Effect, Event, MenuItem, SidebarSyncMode, StatusTone};
use accroster_fake_mac::FakeMac;
use serde_json::{Value, json};

/// Made-up Profile folders: Claude uses Perso; Work has a Session Perso's
/// sidebar lacks.
const PERSO: (&str, &str) = (
    "a1111111-1111-4111-8111-111111111111",
    "a2222222-2222-4222-8222-222222222222",
);
const WORK: (&str, &str) = (
    "b1111111-1111-4111-8111-111111111111",
    "b2222222-2222-4222-8222-222222222222",
);

const REOPEN: &str = "Reopen Claude";

/// When the fake desktop children started, and the same instant as Claude
/// Code writes it in `procStart` (always UTC).
const CHILD_STARTED: &str = "2026-10-25T13:00:00Z";
const CHILD_PROC_START: &str = "Sun Oct 25 13:00:00 2026";

fn entry(id: &str, title: &str) -> Value {
    json!({
        "sessionId": format!("local_{id}"),
        "cliSessionId": format!("{id}-cli"),
        "title": title,
        "isArchived": false,
        "createdAt": 1_761_390_000_000_i64,
        "lastActivityAt": 1_761_400_000_000_i64,
        "cwd": "/Users/me/Dev/app",
    })
}

fn put(mac: &FakeMac, profile: (&str, &str), id: &str, title: &str) {
    mac.write_sidebar_entry(profile.0, profile.1, id, &entry(id, title));
}

/// Claude runs, signed in as Perso, with "Fix the build" in its sidebar;
/// Work's sidebar has "From Work", which Perso's lacks. Sidebar sync is
/// on (`mode`).
fn claude_runs_on_perso(mode: SidebarSyncMode) -> (FakeMac, Core) {
    let mac = FakeMac::new();
    put(&mac, WORK, "w", "From Work");
    put(&mac, PERSO, "a", "Fix the build");
    mac.os.set_desktop_app_version(Some("3.6556.2"));
    mac.write_desktop_config(&json!({ "lastKnownAccountUuid": PERSO.0 }));
    mac.os.set_desktop_app_running(true);
    let mut core = mac.launch();
    core.command(Command::SetSidebarSync(mode));
    (mac, core)
}

/// "Sync now", run to its end.
fn sync(core: &mut Core) -> Vec<Effect> {
    let mut effects = core.command(Command::SyncSidebarNow);
    for _ in 0..5 {
        effects.extend(core.event(Event::Tick));
    }
    effects
}

/// A desktop Claude Code child running Session `host` in `status`.
fn desktop_child(mac: &FakeMac, pid: u32, host: &str, status: &str) {
    mac.os
        .start_desktop_child(pid, CHILD_STARTED.parse().unwrap());
    mac.write_live_process(
        ".claude",
        pid,
        &json!({
            "pid": pid, "sessionId": format!("cli-{host}"), "cwd": "/Users/tester/Dev/app",
            "procStart": CHILD_PROC_START, "entrypoint": "claude-desktop",
            "hostSessionId": format!("local_{host}"), "status": status,
        }),
    );
}

/// The menu's status lines, with their tones.
fn statuses(core: &Core) -> Vec<(String, StatusTone)> {
    core.view()
        .menu
        .items
        .into_iter()
        .filter_map(|item| match item {
            MenuItem::Status { text, tone } => Some((text, tone)),
            _ => None,
        })
        .collect()
}

fn status(core: &Core) -> Option<String> {
    statuses(core).into_iter().next().map(|(text, _)| text)
}

/// The actions right after the menu's status lines.
fn status_actions(core: &Core) -> Vec<String> {
    core.view()
        .menu
        .items
        .into_iter()
        .skip_while(|item| !matches!(item, MenuItem::Status { .. }))
        .skip_while(|item| matches!(item, MenuItem::Status { .. }))
        .map_while(|item| match item {
            MenuItem::Action { title, .. } => Some(title),
            _ => None,
        })
        .collect()
}

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

fn settings_reopen(core: &Core) -> Option<Command> {
    core.view().settings.sidebar_sync.reopen
}

fn note(core: &Core) -> Option<String> {
    core.view().settings.sidebar_sync.note
}

fn tick(mac: &FakeMac, core: &mut Core, seconds: i64) -> Vec<Effect> {
    mac.clock.advance(jiff::SignedDuration::from_secs(seconds));
    core.event(Event::Tick)
}

#[test]
fn a_sync_that_added_to_claudes_open_sidebar_offers_reopen_claude_in_the_menu_and_settings() {
    let (_mac, mut core) = claude_runs_on_perso(SidebarSyncMode::Manual);
    assert_eq!(settings_reopen(&core), None, "nothing synced yet");

    sync(&mut core);

    assert_eq!(
        status(&core).as_deref(),
        Some(
            "Synced 2 entries into 2 sidebars · 1 added to Claude's sidebar · Reopen Claude to show them"
        )
    );
    assert_eq!(status_actions(&core), [REOPEN]);
    assert_eq!(
        note(&core).as_deref(),
        Some(
            "Synced 2 entries into 2 sidebars · 1 added to the sidebar Claude is using · \
             Reopen Claude to show them"
        )
    );
    assert!(settings_reopen(&core).is_some());
}

#[test]
fn reopen_claude_with_claude_idle_quits_it_then_opens_it_once_it_has_quit() {
    let (mac, mut core) = claude_runs_on_perso(SidebarSyncMode::Manual);
    sync(&mut core);

    let effects = press(&mut core, REOPEN);

    assert_eq!(effects, [Effect::QuitDesktopApp]);
    assert_eq!(
        statuses(&core),
        [("Reopening Claude…".to_owned(), StatusTone::Working)]
    );
    assert_eq!(status_actions(&core), Vec::<String>::new());
    assert_eq!(settings_reopen(&core), None);
    assert_eq!(note(&core).as_deref(), Some("Reopening Claude…"));
    assert_eq!(tick(&mac, &mut core, 1), [], "Claude is still quitting");

    mac.os.set_desktop_app_running(false);
    let effects = tick(&mac, &mut core, 1);

    assert_eq!(effects, [Effect::BringDesktopAppForward]);
    assert_eq!(status(&core).as_deref(), Some("Reopening Claude…"));

    mac.os.set_desktop_app_running(true);
    let effects = tick(&mac, &mut core, 1);

    let reopened = "Claude reopened · its sidebar now shows the synced Sessions";
    assert_eq!(
        effects,
        [Effect::Notify {
            title: "Claude reopened".into(),
            body: "Its sidebar now shows the synced Sessions".into(),
        }]
    );
    assert_eq!(statuses(&core), [(reopened.to_owned(), StatusTone::Done)]);
    assert_eq!(note(&core).as_deref(), Some(reopened));
    assert_eq!(settings_reopen(&core), None, "nothing left to show");
    assert_eq!(status_actions(&core), Vec::<String>::new());
}

#[test]
fn with_claude_busy_reopen_waits_naming_the_sessions_and_quits_once_they_are_idle() {
    let (mac, mut core) = claude_runs_on_perso(SidebarSyncMode::Manual);
    sync(&mut core);
    desktop_child(&mac, 501, "a", "busy");

    let effects = press(&mut core, REOPEN);

    assert_eq!(effects, [], "Claude is left alone while busy");
    assert_eq!(
        statuses(&core),
        [(
            "Waiting for Claude to finish: “Fix the build”".to_owned(),
            StatusTone::Working
        )]
    );
    assert_eq!(status_actions(&core), ["Cancel"]);
    assert_eq!(
        note(&core).as_deref(),
        Some("Waiting for Claude to finish: “Fix the build”")
    );
    assert_eq!(tick(&mac, &mut core, 1), [], "still busy");

    desktop_child(&mac, 501, "a", "idle");
    assert_eq!(tick(&mac, &mut core, 1), [Effect::QuitDesktopApp]);
    assert_eq!(status(&core).as_deref(), Some("Reopening Claude…"));
}

#[test]
fn cancel_while_waiting_leaves_claude_running_and_reopen_still_offered() {
    let (mac, mut core) = claude_runs_on_perso(SidebarSyncMode::Manual);
    sync(&mut core);
    desktop_child(&mac, 501, "a", "busy");
    press(&mut core, REOPEN);

    assert_eq!(press(&mut core, "Cancel"), []);
    desktop_child(&mac, 501, "a", "idle");

    assert_eq!(tick(&mac, &mut core, 1), [], "nothing quits after Cancel");
    assert_eq!(status_actions(&core), [REOPEN]);
    assert!(settings_reopen(&core).is_some());
}

#[test]
fn when_the_guard_cant_tell_reopen_asks_before_quitting() {
    let (mac, mut core) = claude_runs_on_perso(SidebarSyncMode::Manual);
    sync(&mut core);
    mac.os
        .start_desktop_child(502, CHILD_STARTED.parse().unwrap());

    assert_eq!(press(&mut core, REOPEN), []);

    assert_eq!(
        statuses(&core),
        [(
            "Can't tell whether Claude is busy (a Claude Code process in Claude reports no status)"
                .to_owned(),
            StatusTone::NeedsYou
        )]
    );
    assert_eq!(status_actions(&core), ["Reopen anyway", "Cancel"]);
    assert_eq!(tick(&mac, &mut core, 10), [], "it waits for an answer");

    assert_eq!(press(&mut core, "Reopen anyway"), [Effect::QuitDesktopApp]);
    assert_eq!(status(&core).as_deref(), Some("Reopening Claude…"));
}

#[test]
fn cancel_after_cant_tell_does_nothing_to_claude() {
    let (mac, mut core) = claude_runs_on_perso(SidebarSyncMode::Manual);
    sync(&mut core);
    mac.os
        .start_desktop_child(502, CHILD_STARTED.parse().unwrap());
    press(&mut core, REOPEN);

    assert_eq!(press(&mut core, "Cancel"), []);

    assert_eq!(status_actions(&core), [REOPEN]);
}

#[test]
fn claude_still_running_30_seconds_after_the_quit_is_reported_and_reopen_offered_again() {
    let (mac, mut core) = claude_runs_on_perso(SidebarSyncMode::Manual);
    sync(&mut core);
    press(&mut core, REOPEN);

    assert_eq!(tick(&mac, &mut core, 29), []);
    let effects = tick(&mac, &mut core, 1);

    assert_eq!(
        effects,
        [Effect::Notify {
            title: "Claude didn't quit".into(),
            body: "Try ⌘Q in Claude".into(),
        }]
    );
    let line = "Claude didn't quit · try ⌘Q in Claude";
    assert_eq!(statuses(&core), [(line.to_owned(), StatusTone::Failed)]);
    assert_eq!(note(&core).as_deref(), Some(line));
    assert_eq!(status_actions(&core), [REOPEN]);
    assert_eq!(tick(&mac, &mut core, 60), [], "nothing is opened");
}

#[test]
fn claude_not_running_30_seconds_after_it_was_opened_is_reported() {
    let (mac, mut core) = claude_runs_on_perso(SidebarSyncMode::Manual);
    sync(&mut core);
    press(&mut core, REOPEN);
    mac.os.set_desktop_app_running(false);
    assert_eq!(tick(&mac, &mut core, 1), [Effect::BringDesktopAppForward]);

    assert_eq!(tick(&mac, &mut core, 29), []);
    let effects = tick(&mac, &mut core, 1);

    assert_eq!(
        effects,
        [Effect::Notify {
            title: "Claude didn't open".into(),
            body: "Open Claude from your Applications folder".into(),
        }]
    );
    assert_eq!(
        status(&core).as_deref(),
        Some("Claude didn't open · open it from your Applications folder")
    );
}

#[test]
fn reopen_is_not_offered_when_nothing_was_added_to_claudes_sidebar_or_claude_isnt_running() {
    let (mac, mut core) = claude_runs_on_perso(SidebarSyncMode::Manual);
    put(&mac, PERSO, "w", "From Work");
    sync(&mut core);
    assert_eq!(status_actions(&core), Vec::<String>::new(), "nothing added");
    assert_eq!(settings_reopen(&core), None);

    let (mac, mut core) = claude_runs_on_perso(SidebarSyncMode::Manual);
    mac.os.set_desktop_app_running(false);
    sync(&mut core);
    assert_eq!(
        settings_reopen(&core),
        None,
        "Claude reads it when it opens"
    );
}

#[test]
fn claude_quit_by_the_user_takes_the_offer_away() {
    let (mac, mut core) = claude_runs_on_perso(SidebarSyncMode::Manual);
    sync(&mut core);

    mac.os.set_desktop_app_running(false);
    tick(&mac, &mut core, 1);

    assert_eq!(settings_reopen(&core), None);
    assert_eq!(status_actions(&core), Vec::<String>::new());
}

#[test]
fn automatic_sync_offers_reopen_but_never_quits_or_opens_claude_on_its_own() {
    let (mac, mut core) = claude_runs_on_perso(SidebarSyncMode::Automatic);
    let mut effects = sync(&mut core);
    for _ in 0..40 {
        effects.extend(tick(&mac, &mut core, 5));
    }

    assert!(settings_reopen(&core).is_some());
    assert!(
        !effects.iter().any(|effect| matches!(
            effect,
            Effect::QuitDesktopApp | Effect::BringDesktopAppForward
        )),
        "{effects:?}"
    );
}

fn entry_path(profile: (&str, &str), id: &str) -> String {
    format!(
        "Library/Application Support/Claude/claude-code-sessions/{}/{}/local_{id}.json",
        profile.0, profile.1
    )
}

#[test]
fn automatic_sync_waits_while_claude_quits_and_opens_again() {
    let (mac, mut core) = claude_runs_on_perso(SidebarSyncMode::Automatic);
    sync(&mut core);
    press(&mut core, REOPEN);

    // Claude saves a Session as it quits.
    put(&mac, PERSO, "n", "Saved on quit");
    core.event(Event::SidebarFilesChanged(vec![
        mac.path(entry_path(PERSO, "n")),
    ]));
    tick(&mac, &mut core, 5);
    mac.os.set_desktop_app_running(false);
    tick(&mac, &mut core, 1);
    tick(&mac, &mut core, 5);
    assert!(
        mac.read_json(entry_path(WORK, "n")).is_none(),
        "nothing synced meanwhile"
    );

    mac.os.set_desktop_app_running(true);
    for _ in 0..6 {
        tick(&mac, &mut core, 1);
    }

    assert!(
        mac.read_json(entry_path(WORK, "n")).is_some(),
        "synced once Claude runs"
    );
}
