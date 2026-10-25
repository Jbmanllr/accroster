//! A Profile after it's added: Login expired and Sign in again…, Rename…,
//! reordering, and Remove….

mod common;

use accroster_core::{Command, Core, Effect, NicknameTarget};
use accroster_fake_mac::{FakeMac, FixtureLogin, Reply, StubCall};
use common::*;
use serde_json::json;

fn nickname_prompt(effects: Vec<Effect>) -> (NicknameTarget, String, Option<String>) {
    match effects.as_slice() {
        [
            Effect::AskNickname {
                target,
                name,
                current,
            },
        ] => (target.clone(), name.clone(), current.clone()),
        other => panic!("expected one nickname prompt: {other:?}"),
    }
}

#[test]
fn rename_sets_a_nickname_that_titles_the_row_and_survives_a_relaunch() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);

    let (target, name, current) = nickname_prompt(click_in_row(&mut core, 0, "Rename…"));
    assert_eq!(name, "me@work.example · Work Inc");
    assert_eq!(current, None);
    core.command(Command::SetNickname {
        target: target.clone(),
        nickname: Some("  Work  ".into()),
    });

    assert_eq!(rows(&core)[0].title, "Work");
    let (_, _, current) = nickname_prompt(click_in_row(&mut core, 0, "Rename…"));
    assert_eq!(current.as_deref(), Some("Work"));
    drop(core);
    let mut core = mac.launch();
    assert_eq!(rows(&core)[0].title, "Work", "kept in settings.json");

    core.command(Command::SetNickname {
        target,
        nickname: Some(" ".into()),
    });
    assert_eq!(
        rows(&core)[0].title,
        "me@work.example · Work Inc",
        "a blank nickname clears it"
    );
}

const WORK: &str = "me@work.example · Work Inc";
const PERSO: &str = "me@example.com · me@example.com's Organization";

/// A Mac whose first two Add Profile… sign in as `work()`, then as perso.
fn mac_with_two_sign_ins() -> FakeMac {
    let mac = mac_signing_in_as(&work(), limits_body());
    let perso = FixtureLogin::perso();
    mac.claude
        .reply(StubCall::AuthLogin, sign_in_as(&perso))
        .reply(StubCall::AuthStatus, auth_status(&perso));
    mac
}

fn titles(core: &Core) -> Vec<String> {
    rows(core).into_iter().map(|row| row.title).collect()
}

#[test]
fn profiles_can_be_reordered_and_the_order_survives_a_relaunch() {
    let mac = mac_with_two_sign_ins();
    let mut core = mac.launch();
    add_profile(&mut core);
    add_profile(&mut core);
    assert_eq!(titles(&core), [WORK, PERSO], "add order at first");
    assert_eq!(
        row_actions(&rows(&core)[0]),
        [
            "Copy shell override",
            "Limit resets → claude.ai",
            "Move Down",
            "Rename…",
            "Remove…"
        ]
    );
    assert_eq!(
        row_actions(&rows(&core)[1]),
        [
            "Copy shell override",
            "Limit resets → claude.ai",
            "Move Up",
            "Rename…",
            "Remove…"
        ]
    );

    click_in_row(&mut core, 1, "Move Up");

    assert_eq!(titles(&core), [PERSO, WORK]);
    drop(core);
    let core = mac.launch();
    assert_eq!(titles(&core), [PERSO, WORK], "kept in settings.json");
}

/// Clicks Remove… on the row at `row` and returns the confirmation's
/// message and confirm command.
fn removal_prompt(core: &mut Core, row: usize) -> (String, Command) {
    match click_in_row(core, row, "Remove…").as_slice() {
        [
            Effect::ConfirmRemoval {
                message, confirm, ..
            },
        ] => (message.clone(), confirm.clone()),
        other => panic!("expected one removal confirmation: {other:?}"),
    }
}

#[test]
fn remove_signs_out_then_deletes_the_folder_but_never_shared_transcripts() {
    let mac = mac_signing_in_as(&work(), limits_body());
    mac.claude.reply(StubCall::AuthLogout, signed_out());
    let transcript = mac.write_transcript("-Users-me-code-app", "abc", &[json!({"type": "user"})]);
    let mut core = mac.launch();
    add_profile(&mut core);
    let folder = only_profile_folder(&mac);
    assert!(
        mac.path(format!("{folder}/projects")).is_symlink(),
        "the folder links to the shared projects folder"
    );
    let (target, ..) = nickname_prompt(click_in_row(&mut core, 0, "Rename…"));
    core.command(Command::SetNickname {
        target,
        nickname: Some("Work".into()),
    });

    let (message, confirm) = removal_prompt(&mut core, 0);
    assert!(!message.contains("configs"), "{message}");
    assert!(
        mac.list(PROFILES).len() == 1 && rows(&core).len() == 1,
        "nothing happens before the confirmation"
    );
    core.command(confirm);

    assert!(mac.list(PROFILES).is_empty(), "the folder is gone");
    assert!(rows(&core).is_empty());
    assert!(transcript.exists(), "shared Transcripts survive");
    let calls = mac.claude.calls();
    let logout = calls
        .iter()
        .find(|call| call.kind == StubCall::AuthLogout)
        .expect("signed out first");
    assert!(logout.var("CLAUDE_CONFIG_DIR").unwrap().ends_with(&folder));
    let settings = mac
        .read_json(format!("{APP_FOLDER}/settings.json"))
        .unwrap();
    assert_eq!(
        settings["nicknames"],
        json!({}),
        "its nickname is forgotten"
    );
    drop(core);
    assert!(
        rows(&mac.launch()).is_empty(),
        "still gone after a relaunch"
    );
}

#[test]
fn a_failed_sign_out_keeps_the_profile_and_its_folder() {
    let mac = mac_signing_in_as(&work(), limits_body());
    mac.claude.reply(
        StubCall::AuthLogout,
        Reply::fail(1, "Logout failed: offline\n"),
    );
    let mut core = mac.launch();
    add_profile(&mut core);

    let (_, confirm) = removal_prompt(&mut core, 0);
    core.command(confirm);

    assert_eq!(
        mac.list(PROFILES).len(),
        1,
        "never deleted without sign-out"
    );
    assert_eq!(titles(&core), [WORK]);
    assert_eq!(
        warnings(&core),
        ["Couldn't sign out me@work.example · Work Inc · nothing was removed"]
    );
    drop(core);
    assert_eq!(titles(&mac.launch()), [WORK]);
}

/// A `get_usage` answer for a Login whose token Claude Code blanked.
fn rate_limits_unavailable() -> Reply {
    Reply::json(&json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": "accroster-usage", "response": {
            "subscription_type": null,
            "rate_limits_available": false,
            "rate_limits": null,
            "behaviors": null,
        }},
    }))
}

/// `claude auth status --json` for a blanked login: identical to signed out.
fn logged_out_status() -> Reply {
    Reply::json(&json!({ "loggedIn": false, "authMethod": "none", "apiProvider": "firstParty" }))
}

fn notifications(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Notify { title, body } => Some(format!("{title}: {body}")),
            _ => None,
        })
        .collect()
}

const EXPIRED_NOTICE: &str = "Login expired: me@work.example · Work Inc needs signing in again. Choose Sign in again… in its menu.";

fn assert_expired(core: &Core) {
    let rows = rows(core);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].title, WORK, "keeps its name");
    assert!(rows[0].dimmed, "dimmed");
    assert_eq!(rows[0].note.as_deref(), Some("Login expired"));
    let actions = row_actions(&rows[0]);
    assert!(
        actions.contains(&"Sign in again…".to_string()),
        "{actions:?}"
    );
    assert!(
        !actions.contains(&"Limit resets → claude.ai".to_string()),
        "{actions:?}"
    );
}

/// A Mac whose next Add Profile… signs in as `work()` and whose usage read
/// answers `usage`.
fn mac_signing_in_work_reading(usage: Reply) -> FakeMac {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.claude
        .reply(StubCall::AuthLogin, sign_in_as(&work()))
        .reply(StubCall::AuthStatus, auth_status(&work()))
        .reply(StubCall::GetUsage, usage);
    mac
}

#[test]
fn rate_limits_unavailable_means_login_expired_notified_once() {
    let mac = mac_signing_in_work_reading(rate_limits_unavailable());
    let mut core = mac.launch();

    let effects = add_profile(&mut core);

    assert_expired(&core);
    assert_eq!(notifications(&effects), [EXPIRED_NOTICE]);
    drop(core);
    let core = mac.launch();
    assert_expired(&core);
}

#[test]
fn logged_out_while_the_account_remains_means_login_expired() {
    let mac = mac_signing_in_work_reading(Reply::fail(1, "boom"));
    mac.claude
        .reply(StubCall::SlashUsage, Reply::fail(1, "boom"))
        .reply(StubCall::AuthStatus, logged_out_status());
    let mut core = mac.launch();

    let effects = add_profile(&mut core);

    assert_expired(&core);
    assert_eq!(notifications(&effects), [EXPIRED_NOTICE]);
}

#[test]
fn a_folder_without_an_account_is_signed_out() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);
    let folder = only_profile_folder(&mac);
    mac.write_json(
        format!("{folder}/.claude.json"),
        &json!({ "hasCompletedOnboarding": true }),
    );
    drop(core);

    let mut core = mac.launch();

    let rows = rows(&core);
    assert_eq!(rows.len(), 1, "still listed: {rows:?}");
    assert_eq!(rows[0].title, "Signed-out Profile");
    assert_eq!(rows[0].note.as_deref(), Some("Signed out"));
    assert!(rows[0].limits.is_empty());
    assert_eq!(
        row_actions(&rows[0]),
        ["Sign in again…", "Rename…", "Remove…"]
    );
    mac.claude.reply(StubCall::AuthLogin, Reply::hanging());
    click_in_row(&mut core, 0, "Sign in again…");
    let call = sign_in_call_number(&mac, 2);
    assert_eq!(call.args, ["auth", "login", "--claudeai"], "no email known");
    click(&mut core, WAITING);
    assert_eq!(mac.list(PROFILES).len(), 1, "cancelling keeps the folder");
}

/// The `n`th `claude auth login` call (1-based), once recorded.
fn sign_in_call_number(mac: &FakeMac, n: usize) -> accroster_fake_mac::RecordedCall {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(call) = mac
            .claude
            .calls()
            .into_iter()
            .filter(|call| call.kind == StubCall::AuthLogin)
            .nth(n - 1)
        {
            return call;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "sign-in {n} never ran"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// An app with one expired Profile signed in as `work()`.
fn mac_with_expired_work() -> (FakeMac, Core) {
    let mac = mac_signing_in_work_reading(rate_limits_unavailable());
    let mut core = mac.launch();
    add_profile(&mut core);
    assert_expired(&core);
    (mac, core)
}

#[test]
fn sign_in_again_in_the_same_folder_with_the_email_fixes_an_expired_login() {
    let (mac, mut core) = mac_with_expired_work();
    let folder = only_profile_folder(&mac);
    mac.claude
        .reply(StubCall::AuthLogin, sign_in_as(&work()))
        .reply(StubCall::GetUsage, usage_reply(limits_body()));
    // A browser sign-in takes a while; one Profile is never read twice
    // within a minute.
    mac.clock.advance(jiff::SignedDuration::from_mins(1));

    click_in_row(&mut core, 0, "Sign in again…");
    let effects = wait_for_sign_in(&mut core);

    let call = sign_in_call_number(&mac, 2);
    assert_eq!(
        call.args,
        ["auth", "login", "--claudeai", "--email", "me@work.example"]
    );
    assert!(call.var("CLAUDE_CONFIG_DIR").unwrap().ends_with(&folder));
    assert_eq!(mac.list(PROFILES).len(), 1, "no new folder");
    let rows = rows(&core);
    assert_eq!(rows[0].title, WORK);
    assert!(!rows[0].dimmed);
    assert_eq!(rows[0].note, None);
    assert_eq!(
        rows[0].limits,
        [bar("Session", 62), bar("Weekly", 41), bar("Fable", 92)]
    );
    assert!(
        !notifications(&effects)
            .iter()
            .any(|notice| notice.starts_with("Login expired")),
        "{effects:?}"
    );
    assert!(warnings(&core).is_empty(), "{:?}", warnings(&core));
    drop(core);
    assert!(
        !common::rows(&mac.launch())[0].dimmed,
        "no longer expired after a relaunch"
    );
}

#[test]
fn signing_in_again_as_another_login_is_undone_and_the_profile_stays_expired() {
    let (mac, mut core) = mac_with_expired_work();
    let folder = only_profile_folder(&mac);
    mac.claude
        .reply(StubCall::AuthLogin, sign_in_as(&FixtureLogin::perso()))
        .reply(
            StubCall::AuthLogout,
            signed_out().writing(".claude.json", "{}"),
        );

    click_in_row(&mut core, 0, "Sign in again…");
    let effects = wait_for_sign_in(&mut core);

    let logout = mac
        .claude
        .calls()
        .into_iter()
        .find(|call| call.kind == StubCall::AuthLogout)
        .expect("the other Login was signed out");
    assert!(logout.var("CLAUDE_CONFIG_DIR").unwrap().ends_with(&folder));
    assert_expired(&core);
    assert_eq!(
        warnings(&core),
        [format!(
            "Signed in as {PERSO}, not {WORK} · that sign-in was undone"
        )]
    );
    assert!(
        notifications(&effects).is_empty(),
        "already notified for this expiry"
    );
    let account = &mac.read_json(format!("{folder}/.claude.json")).unwrap()["oauthAccount"];
    assert_eq!(
        account["accountUuid"],
        json!(work().account_uuid),
        "the Profile's Login is kept"
    );
    assert!(
        !mac.list(format!("{APP_FOLDER}/backups")).is_empty(),
        "backed up before writing Claude Code's file"
    );
    drop(core);
    assert_expired(&mac.launch());
}

#[test]
fn remove_warns_first_when_a_shared_anthropic_configs_folder_exists() {
    let mac = mac_signing_in_as(&work(), limits_body());
    mac.write_file(".config/anthropic/configs/default.json", "{}");
    let mut core = mac.launch();
    add_profile(&mut core);

    let (message, _) = removal_prompt(&mut core, 0);

    assert!(
        message.contains("~/.config/anthropic/configs/"),
        "{message}"
    );
}
