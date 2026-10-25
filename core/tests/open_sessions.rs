//! Opening a Session: the All Sessions Open section, double-click and the
//! menu's Recent Sessions. Each choice says which Profile pays and what
//! carries over before the click.

mod common;

use std::time::{Duration, Instant};

use accroster_core::{
    Command, Core, DiscoveredPair, Effect, Event, MenuItem, NicknameTarget, OpenChoice,
    SessionDetail, SessionFilter,
};
use accroster_fake_mac::{FakeMac, FixtureLogin, Reply, StubCall};
use common::*;
use serde_json::json;

const PROJECT: &str = "-fake-home-Dev-app";
const S1: &str = "11111111-0000-4000-8000-000000000001";
const S2: &str = "11111111-0000-4000-8000-000000000002";

/// A Mac with two signed-in Profiles nicknamed Perso and Work, the desktop
/// app installed, and a project folder `~/Dev/app`.
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
    std::fs::create_dir_all(mac.path("Dev/app")).unwrap();
    let mut core = mac.launch();
    add_profile(&mut core);
    add_profile(&mut core);
    for (index, nickname) in ["Perso", "Work"].into_iter().enumerate() {
        let id = profile_id(&core, index);
        core.command(Command::SetNickname {
            target: NicknameTarget::Profile(id),
            nickname: Some(nickname.into()),
        });
    }
    (mac, core)
}

/// The id of the Profile in row `index`, from its Copy shell override item.
fn profile_id(core: &Core, index: usize) -> String {
    rows(core)[index]
        .submenu
        .iter()
        .find_map(|item| match item {
            MenuItem::Action {
                command: Command::CopyShellOverride(id),
                ..
            } => Some(id.clone()),
            _ => None,
        })
        .expect("a signed-in row")
}

/// Turns CLI switching on and makes the Profile in row `index` the CLI's
/// Active Profile.
fn cli_uses(core: &mut Core, index: usize) {
    core.command(Command::SetCliSwitching(true));
    let id = profile_id(core, index);
    core.command(Command::UseForNewCliRuns(id));
}

fn app_folder(mac: &FakeMac) -> String {
    mac.path("Dev/app").to_str().unwrap().to_owned()
}

/// A Transcript with one prompt, last used in `entrypoint`, 5 minutes ago.
fn transcript(mac: &FakeMac, id: &str, text: &str, entrypoint: &str) {
    transcript_at(mac, id, text, entrypoint, "2026-10-25T13:57:00.000Z");
}

fn transcript_at(mac: &FakeMac, id: &str, text: &str, entrypoint: &str, at: &str) {
    mac.write_transcript(
        PROJECT,
        id,
        &[json!({
            "type": "user", "message": {"role": "user", "content": text},
            "cwd": app_folder(mac), "gitBranch": "main", "entrypoint": entrypoint,
            "timestamp": at, "sessionId": id, "uuid": "u1", "parentUuid": null,
            "isSidechain": false, "isMeta": false, "version": "2.1.310", "userType": "external",
        })],
    );
}

/// `login`'s Sidebar entry `local_<local_id>` for Transcript `cli_session_id`.
fn entry(
    mac: &FakeMac,
    login: &FixtureLogin,
    local_id: &str,
    cli_session_id: &str,
    archived: bool,
) {
    mac.write_sidebar_entry(
        &login.account_uuid,
        &login.organization_uuid,
        local_id,
        &json!({
            "sessionId": format!("local_{local_id}"), "cliSessionId": cli_session_id,
            "title": "Fix the build", "titleSource": "user", "isArchived": archived,
            "lastActivityAt": 1_761_400_000_000_i64, "cwd": app_folder(mac),
        }),
    );
}

/// The desktop app signed in as `login` (its folder is the one written last).
fn desktop_signed_in_as(mac: &FakeMac, login: &FixtureLogin) {
    mac.write_desktop_config(&json!({ "lastKnownAccountUuid": login.account_uuid }));
}

/// Opens All Sessions, lets the index finish and selects Session `id`.
fn select(core: &mut Core, id: &str) -> SessionDetail {
    core.command(Command::OpenAllSessions);
    let deadline = Instant::now() + Duration::from_secs(10);
    while core.view().all_sessions.loading {
        assert!(Instant::now() < deadline, "the index finishes");
        core.event(Event::Tick);
    }
    core.command(Command::SelectSession(Some(id.into())));
    core.view()
        .all_sessions
        .detail
        .unwrap_or_else(|| panic!("{id} is listed: {:?}", core.view().all_sessions.rows))
}

fn titles(open: &[OpenChoice]) -> Vec<&str> {
    open.iter().map(|choice| choice.title.as_str()).collect()
}

fn choice<'a>(open: &'a [OpenChoice], title: &str) -> &'a OpenChoice {
    open.iter()
        .find(|choice| choice.title == title)
        .unwrap_or_else(|| panic!("an Open choice {title}: {open:?}"))
}

/// Clicks the Open choice titled `title` in Session `id`'s detail pane.
fn press(core: &mut Core, id: &str, title: &str) -> Vec<Effect> {
    let detail = select(core, id);
    let choice = choice(&detail.open, title);
    assert!(choice.enabled, "{choice:?}");
    core.command(choice.command.clone())
}

fn resume_line(mac: &FakeMac, id: &str) -> String {
    format!("cd '{}' && claude --resume '{id}'", app_folder(mac))
}

// ----- Resume in Terminal ------------------------------------------------------------

#[test]
fn resume_in_terminal_names_the_cli_profile_and_resumes_in_the_sessions_folder() {
    let (mac, mut core) = mac_with_two_profiles();
    cli_uses(&mut core, 1);
    transcript(&mac, S1, "Fix the login bug", "cli");

    let detail = select(&mut core, S1);
    assert_eq!(
        detail.open[0],
        OpenChoice {
            title: "Resume in Terminal".into(),
            note: Some("New turns count against Work (CLI)".into()),
            command: Command::ResumeInTerminal(S1.into()),
            enabled: true,
        }
    );

    let effects = core.command(Command::ResumeInTerminal(S1.into()));
    assert_eq!(effects, [Effect::RunInTerminal(resume_line(&mac, S1))]);
    assert!(
        resume_line(&mac, S1).contains("fake mac "),
        "the folder has spaces and stays one quoted word"
    );
}

#[test]
fn without_cli_switching_new_turns_count_against_default() {
    let (mac, mut core) = mac_with_two_profiles();
    transcript(&mac, S1, "Fix the login bug", "cli");

    let detail = select(&mut core, S1);

    assert_eq!(
        choice(&detail.open, "Resume in Terminal").note.as_deref(),
        Some("New turns count against Default (CLI)")
    );
}

/// Session `id` running in a Terminal `claude` (pid `pid`), whose live file
/// is in `~/.claude/sessions`.
fn running_in_terminal(mac: &FakeMac, id: &str, pid: u32) {
    mac.os
        .start_process(pid, 1, "2026-10-25T13:00:00Z".parse().unwrap());
    mac.write_live_process(
        ".claude",
        pid,
        &json!({
            "pid": pid, "sessionId": id, "cwd": app_folder(mac),
            "procStart": "Sun Oct 25 13:00:00 2026", "kind": "interactive",
            "entrypoint": "cli", "status": "idle",
        }),
    );
}

#[test]
fn a_session_running_in_terminal_reads_show_in_terminal_and_brings_its_window_forward() {
    let (mac, mut core) = mac_with_two_profiles();
    cli_uses(&mut core, 0);
    transcript(&mac, S1, "Fix the login bug", "cli");
    running_in_terminal(&mac, S1, 4242);
    mac.os.set_terminal(4242, "ttys004");

    let detail = select(&mut core, S1);
    assert_eq!(
        choice(&detail.open, "Show in Terminal").note.as_deref(),
        Some("New turns count against Perso (CLI)")
    );
    assert!(!titles(&detail.open).contains(&"Resume in Terminal"));

    let effects = press(&mut core, S1, "Show in Terminal");
    assert_eq!(
        effects,
        [Effect::ShowInTerminal {
            tty: Some("ttys004".into())
        }],
        "no second copy is started"
    );
}

#[test]
fn a_session_whose_folder_is_gone_cant_be_resumed() {
    let (mac, mut core) = mac_with_two_profiles();
    transcript(&mac, S1, "Fix the login bug", "cli");
    std::fs::remove_dir(mac.path("Dev/app")).unwrap();

    let detail = select(&mut core, S1);

    let resume = choice(&detail.open, "Resume in Terminal");
    assert!(!resume.enabled);
    assert_eq!(resume.note.as_deref(), Some("Its folder no longer exists"));
    assert!(
        core.command(Command::ResumeInTerminal(S1.into()))
            .is_empty()
    );
}

// ----- Open in Claude ----------------------------------------------------------------

/// The snapshot's files outside the app's own folder.
fn claude_files(mac: &FakeMac) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    mac.snapshot()
        .into_iter()
        .filter(|(path, _)| {
            !path
                .to_string_lossy()
                .contains("io.github.jbmanllr.accroster")
        })
        .collect()
}

#[test]
fn an_entry_in_the_desktops_profile_opens_in_place_and_writes_nothing() {
    let (mac, mut core) = mac_with_two_profiles();
    desktop_signed_in_as(&mac, &work());
    entry(&mac, &work(), "w1", S1, false);
    entry(&mac, &FixtureLogin::perso(), "p1", S1, false);
    transcript(&mac, S1, "Fix the login bug", "claude-desktop");

    let detail = select(&mut core, S1);
    assert_eq!(
        titles(&detail.open),
        ["Resume in Terminal", "Open in Claude"],
        "no switch is offered when Claude already has it"
    );
    assert_eq!(
        choice(&detail.open, "Open in Claude").note.as_deref(),
        Some("Opens its entry in Work's sidebar")
    );

    let before = claude_files(&mac);
    let calls = mac.claude.calls().len();
    let effects = press(&mut core, S1, "Open in Claude");

    assert_eq!(
        effects,
        [Effect::OpenUrl(
            "claude://code/continue?session=local_w1".into()
        )]
    );
    assert_eq!(mac.claude.calls().len(), calls, "nothing is spawned");
    assert_eq!(claude_files(&mac), before, "no Claude file changes");
}

#[test]
fn without_an_entry_there_it_opens_as_a_new_entry_or_switches_to_an_owner() {
    let (mac, mut core) = mac_with_two_profiles();
    desktop_signed_in_as(&mac, &work());
    entry(&mac, &work(), "other", S2, false);
    entry(&mac, &FixtureLogin::perso(), "p1", S1, false);
    transcript(&mac, S1, "Fix the login bug", "claude-desktop");
    transcript(&mac, S2, "Other work", "claude-desktop");

    let detail = select(&mut core, S1);

    assert_eq!(
        detail.open[1..],
        [
            OpenChoice {
                title: "Open in Claude as Work".into(),
                note: Some(
                    "Adds a new entry to Work's sidebar; title and archive state don't carry over"
                        .into()
                ),
                command: Command::OpenInClaude(S1.into()),
                enabled: true,
            },
            OpenChoice {
                title: "Switch Claude to Perso, then open…".into(),
                note: None,
                command: Command::SwitchClaudeThenOpen {
                    profile: profile_id(&core, 0),
                    session: S1.into(),
                },
                enabled: true,
            },
        ]
    );
}

#[test]
fn open_in_claude_as_runs_claude_desktop_resume_in_the_cli_profiles_folder() {
    let (mac, mut core) = mac_with_two_profiles();
    cli_uses(&mut core, 1);
    desktop_signed_in_as(&mac, &FixtureLogin::perso());
    entry(&mac, &FixtureLogin::perso(), "other", S2, false);
    transcript(&mac, S1, "Fix the login bug", "cli");
    transcript(&mac, S2, "Other work", "claude-desktop");
    mac.claude.reply(
        StubCall::Other,
        Reply::ok(format!("Opening session {S1} in Claude Desktop\n")),
    );
    let before = claude_files(&mac);

    let effects = press(&mut core, S1, "Open in Claude as Perso");

    assert!(effects.is_empty(), "{effects:?}");
    let call = mac
        .claude
        .calls()
        .into_iter()
        .find(|call| call.kind == StubCall::Other)
        .expect("claude ran");
    assert_eq!(call.args, ["--desktop", "--resume", S1]);
    let work_folder = mac.path(format!("{PROFILES}/{}", profile_id(&core, 1)));
    assert_eq!(
        call.var("CLAUDE_CONFIG_DIR"),
        work_folder.to_str(),
        "the CLI's Active Profile's folder, never ~/.claude"
    );
    assert_eq!(
        claude_files(&mac),
        before,
        "Claude adds the entry itself; AccRoster writes none"
    );
}

#[test]
fn a_failed_hand_over_is_reported() {
    let (mac, mut core) = mac_with_two_profiles();
    desktop_signed_in_as(&mac, &FixtureLogin::perso());
    entry(&mac, &FixtureLogin::perso(), "other", S2, false);
    transcript(&mac, S1, "Fix the login bug", "cli");
    transcript(&mac, S2, "Other work", "claude-desktop");
    mac.claude
        .reply(StubCall::Other, Reply::fail(1, "Not signed in\n"));

    let effects = press(&mut core, S1, "Open in Claude as Perso");

    assert_eq!(
        effects,
        [Effect::Notify {
            title: "Couldn't open the Session in Claude".into(),
            body:
                "Claude Code couldn't hand it over. Check that Claude is signed in, then try again."
                    .into(),
        }]
    );
}

#[test]
fn a_session_running_in_terminal_cant_be_moved_to_claude() {
    let (mac, mut core) = mac_with_two_profiles();
    desktop_signed_in_as(&mac, &FixtureLogin::perso());
    entry(&mac, &FixtureLogin::perso(), "other", S2, false);
    transcript(&mac, S1, "Fix the login bug", "cli");
    transcript(&mac, S2, "Other work", "claude-desktop");
    running_in_terminal(&mac, S1, 4242);

    let detail = select(&mut core, S1);

    let open = choice(&detail.open, "Open in Claude as Perso");
    assert!(!open.enabled);
    assert_eq!(
        open.note.as_deref(),
        Some("It runs in Terminal, and Claude Code won't move a running Session")
    );
    let calls = mac.claude.calls().len();
    assert!(core.command(Command::OpenInClaude(S1.into())).is_empty());
    assert_eq!(mac.claude.calls().len(), calls);
}

#[test]
fn an_archived_entry_in_claudes_profile_doesnt_count_as_one() {
    // claude://code/continue skips archived entries: it would do nothing.
    let (mac, mut core) = mac_with_two_profiles();
    desktop_signed_in_as(&mac, &work());
    entry(&mac, &work(), "w1", S1, true);
    entry(&mac, &work(), S2, S2, true);
    transcript(&mac, S1, "Fix the login bug", "claude-desktop");
    transcript(&mac, S2, "Other work", "claude-desktop");
    core.command(Command::FilterSessions(SessionFilter::Sidebar(
        DiscoveredPair {
            account_uuid: work().account_uuid,
            organization_uuid: work().organization_uuid,
        },
    )));

    let detail = select(&mut core, S1);
    assert_eq!(
        choice(&detail.open, "Open in Claude as Work")
            .note
            .as_deref(),
        Some("Adds a new entry to Work's sidebar; title and archive state don't carry over")
    );
    let detail = select(&mut core, S2);
    assert_eq!(
        choice(&detail.open, "Open in Claude as Work")
            .note
            .as_deref(),
        Some("Unarchives its entry in Work's sidebar"),
        "claude --desktop --resume reopens the archived local_<id> it made"
    );
}

#[test]
fn a_profile_whose_entry_is_archived_gets_no_switch() {
    let (mac, mut core) = mac_with_two_profiles();
    desktop_signed_in_as(&mac, &work());
    entry(&mac, &work(), "other", S2, false);
    entry(&mac, &FixtureLogin::perso(), "p1", S1, true);
    entry(&mac, &FixtureLogin::perso(), "p2", S1, false);
    transcript(&mac, S1, "Fix the login bug", "claude-desktop");
    transcript(&mac, S2, "Other work", "claude-desktop");
    let detail = select(&mut core, S1);
    assert!(choice(&detail.open, "Switch Claude to Perso, then open…").enabled);

    // Archived in Perso too: the link couldn't open it after the switch.
    entry(&mac, &FixtureLogin::perso(), "p2", S1, true);
    core.command(Command::FilterSessions(SessionFilter::Sidebar(
        DiscoveredPair {
            account_uuid: FixtureLogin::perso().account_uuid,
            organization_uuid: FixtureLogin::perso().organization_uuid,
        },
    )));
    let detail = select(&mut core, S1);

    assert!(!titles(&detail.open).contains(&"Switch Claude to Perso, then open…"));
}

#[test]
fn claude_not_installed_offers_the_terminal_only() {
    let (mac, mut core) = mac_with_two_profiles();
    mac.os.set_desktop_app_version(None);
    desktop_signed_in_as(&mac, &work());
    entry(&mac, &work(), "w1", S1, false);
    transcript(&mac, S1, "Fix the login bug", "claude-desktop");

    let detail = select(&mut core, S1);

    assert_eq!(titles(&detail.open), ["Resume in Terminal"]);
    assert_eq!(
        core.command(Command::OpenSession(S1.into())),
        [Effect::RunInTerminal(resume_line(&mac, S1))],
        "double-click falls back to the Terminal"
    );
}

#[test]
fn a_deleted_transcript_offers_nothing_to_open() {
    let (mac, mut core) = mac_with_two_profiles();
    desktop_signed_in_as(&mac, &work());
    entry(&mac, &work(), "w1", S1, false);

    let detail = select(&mut core, S1);

    assert!(detail.deleted.is_some());
    assert!(detail.open.is_empty(), "{:?}", detail.open);
    assert!(core.command(Command::OpenSession(S1.into())).is_empty());
    assert!(core.command(Command::OpenInClaude(S1.into())).is_empty());
    assert!(
        core.command(Command::ResumeInTerminal(S1.into()))
            .is_empty()
    );
}

// ----- Switch Claude, then open ------------------------------------------------------

#[test]
fn switch_then_open_runs_the_guided_switch_and_opens_the_entry_once_confirmed() {
    let (mac, mut core) = mac_with_two_profiles();
    desktop_signed_in_as(&mac, &work());
    entry(&mac, &work(), "other", S2, false);
    entry(&mac, &FixtureLogin::perso(), "p1", S1, false);
    transcript(&mac, S1, "Fix the login bug", "claude-desktop");
    transcript(&mac, S2, "Other work", "claude-desktop");

    let effects = press(&mut core, S1, "Switch Claude to Perso, then open…");
    assert!(
        effects.contains(&Effect::BringDesktopAppForward),
        "{effects:?}"
    );
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::OpenUrl(_))),
        "nothing opens before the switch"
    );

    // The user signs out and in as Perso; Claude writes Perso's folder.
    desktop_signed_in_as(&mac, &FixtureLogin::perso());
    entry(&mac, &FixtureLogin::perso(), "p9", S2, false);
    let effects = core.event(Event::Tick);

    assert!(
        effects.contains(&Effect::OpenUrl(
            "claude://code/continue?session=local_p1".into()
        )),
        "{effects:?}"
    );
}

#[test]
fn switching_is_disabled_while_a_desktop_turn_runs() {
    let (mac, mut core) = mac_with_two_profiles();
    desktop_signed_in_as(&mac, &work());
    entry(&mac, &work(), "other", S2, false);
    entry(&mac, &FixtureLogin::perso(), "p1", S1, false);
    transcript(&mac, S1, "Fix the login bug", "claude-desktop");
    transcript(&mac, S2, "Other work", "claude-desktop");
    mac.os
        .start_desktop_child(5151, "2026-10-25T13:00:00Z".parse().unwrap());
    mac.write_live_process(
        ".claude",
        5151,
        &json!({
            "pid": 5151, "sessionId": S2, "procStart": "Sun Oct 25 13:00:00 2026",
            "entrypoint": "claude-desktop", "status": "busy", "hostSessionId": "local_other",
        }),
    );

    let detail = select(&mut core, S1);

    let switch = choice(&detail.open, "Switch Claude to Perso, then open…");
    assert!(!switch.enabled);
    assert_eq!(
        switch.note.as_deref(),
        Some("A desktop turn is running; switching now would end it")
    );
    assert!(
        choice(&detail.open, "Open in Claude as Work").enabled,
        "opening as a new entry doesn't switch"
    );
}

// ----- Double-click and Recent Sessions ----------------------------------------------

#[test]
fn double_click_opens_in_the_client_last_used() {
    let (mac, mut core) = mac_with_two_profiles();
    desktop_signed_in_as(&mac, &work());
    entry(&mac, &work(), "w2", S2, false);
    transcript(&mac, S1, "In the CLI", "cli");
    transcript(&mac, S2, "In Claude", "claude-desktop");
    select(&mut core, S1);

    assert_eq!(
        core.command(Command::OpenSession(S1.into())),
        [Effect::RunInTerminal(resume_line(&mac, S1))]
    );
    assert_eq!(
        core.command(Command::OpenSession(S2.into())),
        [Effect::OpenUrl(
            "claude://code/continue?session=local_w2".into()
        )]
    );
}

#[test]
fn double_click_without_an_entry_in_claudes_profile_shows_the_choices_instead() {
    let (mac, mut core) = mac_with_two_profiles();
    desktop_signed_in_as(&mac, &work());
    entry(&mac, &work(), "other", S2, false);
    entry(&mac, &FixtureLogin::perso(), "p1", S1, false);
    transcript(&mac, S1, "Fix the login bug", "claude-desktop");
    transcript(&mac, S2, "Other work", "claude-desktop");
    select(&mut core, S2);
    core.command(Command::CloseAllSessions);
    let calls = mac.claude.calls().len();

    let effects = core.command(Command::OpenSession(S1.into()));

    assert!(effects.contains(&Effect::ShowAllSessions), "{effects:?}");
    assert_eq!(mac.claude.calls().len(), calls, "nothing was opened");
    let detail = core.view().all_sessions.detail.expect("it is selected");
    assert_eq!(detail.id, S1);
    assert!(titles(&detail.open).contains(&"Open in Claude as Work"));
}

fn recent_sessions(core: &Core) -> Vec<(String, Command)> {
    core.view()
        .menu
        .items
        .into_iter()
        .find_map(|item| match item {
            MenuItem::Submenu { title, items } if title == "Recent Sessions" => Some(items),
            _ => None,
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| match item {
            MenuItem::Action { title, command } => Some((title, command)),
            _ => None,
        })
        .collect()
}

#[test]
fn recent_sessions_lists_the_newest_openable_sessions_from_launch() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    std::fs::create_dir_all(mac.path("Dev/app")).unwrap();
    transcript_at(&mac, S1, "Older one", "cli", "2026-10-25T12:00:00.000Z");
    transcript_at(&mac, S2, "Newer one", "cli", "2026-10-25T13:00:00.000Z");
    // Archived in its only Sidebar, and one whose Transcript is gone.
    let s3 = "11111111-0000-4000-8000-000000000003";
    transcript_at(&mac, s3, "Archived one", "cli", "2026-10-25T13:30:00.000Z");
    entry(&mac, &work(), "a3", s3, true);
    entry(
        &mac,
        &work(),
        "gone",
        "11111111-0000-4000-8000-000000000004",
        false,
    );
    let mut core = mac.launch();
    core.event(Event::Tick);

    let recent = recent_sessions(&core);

    assert_eq!(
        recent,
        [
            (
                "“Newer one” · app".to_owned(),
                Command::OpenSession(S2.into())
            ),
            (
                "“Older one” · app".to_owned(),
                Command::OpenSession(S1.into())
            ),
        ]
    );
    let menu = core.view().menu.items;
    let position = |wanted: &str| {
        menu.iter().position(|item| match item {
            MenuItem::Submenu { title, .. } | MenuItem::Action { title, .. } => title == wanted,
            _ => false,
        })
    };
    assert_eq!(
        position("Recent Sessions").map(|at| at + 1),
        position("All Sessions…"),
        "just before All Sessions…"
    );
    assert_eq!(
        core.command(recent[0].1.clone()),
        [Effect::RunInTerminal(resume_line(&mac, S2))],
        "an item opens by the double-click rules"
    );
}

#[test]
fn opening_logs_the_session_id_but_never_titles_or_folders() {
    let (mac, mut core) = mac_with_two_profiles();
    transcript(&mac, S1, "Secret prompt text", "cli");
    select(&mut core, S1);
    core.command(Command::OpenSession(S1.into()));
    let log = mac
        .read_file(format!("{APP_FOLDER}/logs/app.log"))
        .unwrap_or_default();
    assert!(log.contains("session opened"), "{log}");
    assert!(!log.contains("Secret prompt"), "{log}");
    assert!(!log.contains("Dev/app"), "{log}");
}
