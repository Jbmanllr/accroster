//! Add Profile…: a new Profile folder, Claude Code's own browser sign-in, and
//! the new Profile's limits from one usage read.

mod common;

use accroster_core::{Command, Event, MenuItem};
use accroster_fake_mac::{FakeMac, FixtureLogin, Reply, StubCall};
use common::*;
use jiff::SignedDuration;

#[test]
fn a_signed_in_profile_appears_with_its_session_weekly_and_model_bars() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();

    click(&mut core, "Add Profile…");
    assert!(
        action_titles(&core).contains(&WAITING.to_string()),
        "{:?}",
        core.view().menu.items
    );
    assert!(!action_titles(&core).contains(&"Add Profile…".to_string()));
    wait_for_sign_in(&mut core);

    let rows = rows(&core);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].title, "me@work.example · Work Inc");
    assert_eq!(
        rows[0].limits,
        [bar("Session", 62), bar("Weekly", 41), bar("Fable", 92)]
    );
    assert_eq!(
        details(&rows[0]),
        ["me@work.example · Work Inc · Max", "Limits as of 14:02"]
    );
    assert!(rows[0].submenu.contains(&MenuItem::Action {
        title: "Limit resets → claude.ai".into(),
        command: Command::OpenLimitResets,
    }));
    assert_eq!(rows[0].note, None);
    assert!(rows[0].offers.is_empty(), "{:?}", rows[0].offers);
    assert!(action_titles(&core).contains(&"Add Profile…".to_string()));
    assert_eq!(mac.list(PROFILES).len(), 1);
    assert!(warnings(&core).is_empty(), "{:?}", warnings(&core));
    let kinds: Vec<StubCall> = mac.claude.calls().iter().map(|call| call.kind).collect();
    assert_eq!(
        kinds,
        [
            StubCall::AuthLogin,
            StubCall::AuthStatus,
            StubCall::GetUsage
        ],
        "one sign-in, one identity cross-check, one usage read"
    );
    let log = mac
        .read_file(format!("{APP_FOLDER}/logs/app.log"))
        .unwrap_or_default();
    let id = mac.list(PROFILES).remove(0);
    assert!(log.contains("INFO profile added"), "{log}");
    assert!(!log.contains("me@work.example"), "{log}");
    assert!(!log.contains("Work Inc"), "{log}");
    assert!(
        !log.contains(&id),
        "Profiles are logged as <Profile xxxx>: {log}"
    );
}

#[test]
fn the_new_profile_is_still_listed_after_a_relaunch() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);
    drop(core);

    let core = mac.launch();

    let rows = rows(&core);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].title, "me@work.example · Work Inc");
    assert_eq!(
        details(&rows[0]),
        ["me@work.example · Work Inc"],
        "no plan known until the next usage read"
    );
}

#[test]
fn cancel_kills_the_sign_in_and_leaves_nothing_behind() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.claude.reply(StubCall::AuthLogin, Reply::hanging());
    let mut core = mac.launch();

    click(&mut core, "Add Profile…");
    let call = sign_in_call(&mac);
    assert_eq!(call.args, ["auth", "login", "--claudeai"]);
    assert_eq!(
        mac.list(PROFILES).len(),
        1,
        "the folder exists while waiting"
    );
    click(&mut core, WAITING);

    assert!(!call.still_running(), "the sign-in was killed");
    assert!(mac.list(PROFILES).is_empty(), "the folder is gone");
    assert!(rows(&core).is_empty());
    assert!(warnings(&core).is_empty(), "{:?}", warnings(&core));
    assert!(action_titles(&core).contains(&"Add Profile…".to_string()));
    assert_eq!(
        mac.claude.calls().len(),
        1,
        "nothing to sign out: no login landed"
    );
}

#[test]
fn a_sign_in_still_waiting_after_ten_minutes_is_killed_and_removed() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.claude.reply(StubCall::AuthLogin, Reply::hanging());
    let mut core = mac.launch();

    click(&mut core, "Add Profile…");
    let call = sign_in_call(&mac);
    mac.clock.advance(SignedDuration::from_mins(9));
    core.event(Event::Tick);
    assert!(call.still_running(), "nine minutes is not yet too long");
    assert_eq!(mac.list(PROFILES).len(), 1);

    mac.clock.advance(SignedDuration::from_mins(1));
    core.event(Event::Tick);

    assert!(!call.still_running(), "the sign-in was killed");
    assert!(mac.list(PROFILES).is_empty(), "the folder is gone");
    assert_eq!(
        warnings(&core),
        ["Sign-in timed out after 10 minutes · nothing was added"]
    );
    assert!(action_titles(&core).contains(&"Add Profile…".to_string()));
}

#[test]
fn a_failed_sign_in_adds_nothing_and_says_so() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.claude.reply(
        StubCall::AuthLogin,
        Reply::fail(1, "Login failed: No authorization code received\n"),
    );
    let mut core = mac.launch();

    add_profile(&mut core);

    assert!(mac.list(PROFILES).is_empty());
    assert!(rows(&core).is_empty());
    assert_eq!(
        warnings(&core),
        ["Sign-in didn't complete · nothing was added"]
    );
    assert_eq!(mac.claude.calls().len(), 1);
}

#[test]
fn signing_in_to_a_profile_that_exists_is_undone_and_the_existing_one_named() {
    let mac = mac_signing_in_as(&work(), limits_body());
    mac.claude.reply(StubCall::AuthLogout, signed_out());
    let mut core = mac.launch();
    add_profile(&mut core);
    let first = mac.list(PROFILES);

    add_profile(&mut core);

    assert_eq!(mac.list(PROFILES), first, "the second folder is gone");
    assert_eq!(rows(&core).len(), 1);
    assert_eq!(
        warnings(&core),
        ["Already added: me@work.example · Work Inc"]
    );
    let calls = mac.claude.calls();
    let logout = calls
        .iter()
        .find(|call| call.kind == StubCall::AuthLogout)
        .expect("the duplicate sign-in was signed out");
    let second_dir = calls
        .iter()
        .filter(|call| call.kind == StubCall::AuthLogin)
        .nth(1)
        .and_then(|call| call.var("CLAUDE_CONFIG_DIR"))
        .expect("a second sign-in ran");
    assert_eq!(logout.var("CLAUDE_CONFIG_DIR"), Some(second_dir));
    assert!(!second_dir.ends_with(&first[0]), "not the existing Profile");
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.kind == StubCall::GetUsage)
            .count(),
        1,
        "the duplicate's limits were never read"
    );

    // The next Add Profile… starts clean.
    mac.claude.reply(StubCall::AuthLogin, Reply::hanging());
    click(&mut core, "Add Profile…");
    assert!(warnings(&core).is_empty(), "{:?}", warnings(&core));
    click(&mut core, WAITING);
}

#[test]
fn a_duplicate_whose_sign_out_fails_keeps_its_folder() {
    let mac = mac_signing_in_as(&work(), limits_body());
    mac.claude.reply(
        StubCall::AuthLogout,
        Reply::fail(1, "Logout failed: offline\n"),
    );
    let mut core = mac.launch();
    add_profile(&mut core);

    add_profile(&mut core);

    assert_eq!(
        mac.list(PROFILES).len(),
        2,
        "a folder whose login couldn't be revoked is never deleted"
    );
    assert_eq!(rows(&core).len(), 1);
    assert_eq!(
        warnings(&core),
        ["Already added: me@work.example · Work Inc · the new sign-in couldn't be undone"]
    );
    drop(core);
    let core = mac.launch();
    assert_eq!(rows(&core).len(), 1, "the kept folder is not listed later");
    assert_eq!(mac.list(PROFILES).len(), 2);
}

#[test]
fn a_sign_in_claude_code_does_not_confirm_is_kept_with_a_warning() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.claude
        .reply(StubCall::AuthLogin, sign_in_as(&work()))
        .reply(StubCall::AuthStatus, auth_status(&FixtureLogin::perso()))
        .reply(StubCall::GetUsage, usage_reply(limits_body()));
    let mut core = mac.launch();

    add_profile(&mut core);

    assert_eq!(rows(&core).len(), 1);
    assert_eq!(
        warnings(&core),
        ["Claude Code didn't confirm the sign-in of me@work.example · Work Inc"]
    );
}
