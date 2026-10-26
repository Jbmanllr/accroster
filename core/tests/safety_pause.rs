//! Format checks and Safety pauses (ADR 0004): a write feature stops before
//! any write when Claude changes something it relies on, says so once, and
//! resumes only on a click in Diagnostics after the checks pass again.

mod common;

use accroster_core::{CheckRow, CheckStatus, Command, Core, Effect, Event, SafetyFeature};
use accroster_fake_mac::{FakeMac, FixtureLogin, Reply, StubCall};
use common::*;
use serde_json::json;

const POINTER: &str =
    "Library/Application Support/io.github.jbmanllr.accroster/bin/cli-active-profile";

/// A Mac with two Profiles (work, then perso), CLI switching on and the
/// work Profile chosen for the CLI.
fn cli_on_with_two_profiles() -> (FakeMac, Core) {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.write_zshrc("alias ll='ls -l'\n");
    for login in [work(), FixtureLogin::perso()] {
        mac.claude
            .reply(StubCall::AuthLogin, sign_in_as(&login))
            .reply(StubCall::AuthStatus, auth_status(&login))
            .reply(StubCall::GetUsage, usage_reply(limits_body()));
    }
    let mut core = mac.launch();
    add_profile(&mut core);
    add_profile(&mut core);
    core.command(Command::SetCliSwitching(true));
    click_row(&mut core, 0);
    (mac, core)
}

/// From now on `auth status` answers honestly, whichever folder asks (the
/// sign-in replies queued before are used up).
fn honest_auth_status(mac: &FakeMac) {
    mac.claude
        .reply(StubCall::AuthStatus, Reply::auth_status_from_folder());
}

fn click_row(core: &mut Core, index: usize) {
    let command = rows(core)[index]
        .command
        .clone()
        .expect("the row is clickable");
    core.command(command);
}

fn notifications(effects: &[Effect]) -> Vec<(String, String)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Notify { title, body } => Some((title.clone(), body.clone())),
            _ => None,
        })
        .collect()
}

/// The shared `~/.claude/settings.json` with this `env` block.
fn shared_env(mac: &FakeMac, env: serde_json::Value) {
    mac.write_json(".claude/settings.json", &json!({ "env": env }));
}

fn resume_button(core: &Core, feature: SafetyFeature) -> Option<Command> {
    core.view()
        .diagnostics
        .paused
        .into_iter()
        .find(|paused| paused.feature == feature)
        .and_then(|paused| paused.resume)
}

fn is_paused(core: &Core, feature: SafetyFeature) -> bool {
    core.view()
        .diagnostics
        .paused
        .iter()
        .any(|paused| paused.feature == feature)
}

// ----- Settings env ------------------------------------------------------------------

#[test]
fn a_config_dir_in_the_shared_settings_env_pauses_cli_switching_on_menu_open() {
    let (mac, mut core) = cli_on_with_two_profiles();
    let pointer = mac.read_file(POINTER).unwrap();
    shared_env(&mac, json!({ "CLAUDE_CONFIG_DIR": "/somewhere/else" }));

    let effects = core.event(Event::MenuOpened);

    let view = core.view();
    assert_eq!(view.menu_bar.label.as_deref(), Some("?"));
    assert_eq!(
        warnings(&core),
        ["settings.json sets CLAUDE_CONFIG_DIR · CLI switching paused"]
    );
    let notes = notifications(&effects);
    assert_eq!(notes.len(), 1, "{effects:?}");
    assert_eq!(notes[0].0, "CLI switching paused");
    assert!(
        rows(&core).iter().all(|row| row.command.is_none()),
        "row clicks don't switch"
    );
    core.command(Command::UseForNewCliRuns("anything".into()));
    for id in mac.list(PROFILES) {
        core.command(Command::UseForNewCliRuns(id));
    }
    assert_eq!(mac.read_file(POINTER).unwrap(), pointer, "pointer frozen");

    let again = core.event(Event::MenuOpened);
    assert!(notifications(&again).is_empty(), "notified once");
    assert_eq!(warnings(&core).len(), 1);
}

#[test]
fn a_paused_feature_resumes_only_on_the_click_after_the_checks_pass() {
    let (mac, mut core) = cli_on_with_two_profiles();
    honest_auth_status(&mac);
    shared_env(&mac, json!({ "CLAUDE_CODE_OAUTH_TOKEN": "x" }));
    core.event(Event::MenuOpened);
    assert!(is_paused(&core, SafetyFeature::CliSwitching));
    assert_eq!(
        resume_button(&core, SafetyFeature::CliSwitching),
        None,
        "no Resume while the check still fails"
    );

    shared_env(&mac, json!({}));
    core.event(Event::MenuOpened);
    let core_relaunched = mac.launch();

    for core in [&core, &core_relaunched] {
        assert!(
            is_paused(core, SafetyFeature::CliSwitching),
            "no auto-resume"
        );
        assert_eq!(core.view().menu_bar.label.as_deref(), Some("?"));
    }
    drop(core_relaunched);
    let resume = resume_button(&core, SafetyFeature::CliSwitching).expect("Resume offered");
    assert_eq!(resume, Command::ResumeFeature(SafetyFeature::CliSwitching));

    core.command(resume);
    settle(&mut core);

    assert!(!is_paused(&core, SafetyFeature::CliSwitching));
    assert!(warnings(&core).is_empty());
    assert_ne!(core.view().menu_bar.label.as_deref(), Some("?"));
    click_row(&mut core, 1);
    assert!(
        mac.read_file(POINTER)
            .unwrap()
            .ends_with(&profile_of(&mac, "me@example.com"))
    );
}

/// The id of the Profile folder signed in as `email`.
fn profile_of(mac: &FakeMac, email: &str) -> String {
    mac.list(PROFILES)
        .into_iter()
        .find(|id| {
            mac.read_json(format!("{PROFILES}/{id}/.claude.json"))
                .is_some_and(|body| body["oauthAccount"]["emailAddress"] == email)
        })
        .expect("a Profile signed in as that email")
}

#[test]
fn resume_does_nothing_while_the_check_still_fails() {
    let (mac, mut core) = cli_on_with_two_profiles();
    shared_env(&mac, json!({ "CLAUDE_CODE_OAUTH_REFRESH_TOKEN": "x" }));
    core.event(Event::MenuOpened);

    core.command(Command::ResumeFeature(SafetyFeature::CliSwitching));

    assert!(is_paused(&core, SafetyFeature::CliSwitching));
    assert_eq!(core.view().menu_bar.label.as_deref(), Some("?"));
}

#[test]
fn managed_settings_env_pauses_cli_switching_at_launch() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.os
        .set_managed_settings(Some(&json!({ "env": { "CLAUDE_CONFIG_DIR": "/x" } })));

    let core = mac.launch();

    assert!(is_paused(&core, SafetyFeature::CliSwitching));
    assert_eq!(
        warnings(&core),
        ["Managed settings set CLAUDE_CONFIG_DIR · CLI switching paused"]
    );
}

#[test]
fn launch_notifications_reach_the_ui_once() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    shared_env(&mac, json!({ "CLAUDE_CONFIG_DIR": "/x" }));

    let mut core = mac.launch();

    assert_eq!(notifications(&core.take_effects()).len(), 1);
    assert!(core.take_effects().is_empty());
    let relaunched = mac.launch();
    let mut relaunched = relaunched;
    assert!(
        notifications(&relaunched.take_effects()).is_empty(),
        "a pause still in place is not announced again"
    );
}

#[test]
fn the_settings_env_is_checked_again_before_a_pointer_write() {
    let (mac, mut core) = cli_on_with_two_profiles();
    let pointer = mac.read_file(POINTER).unwrap();
    shared_env(&mac, json!({ "CLAUDE_CONFIG_DIR": "/x" }));

    click_row(&mut core, 1);

    assert_eq!(mac.read_file(POINTER).unwrap(), pointer);
    assert!(is_paused(&core, SafetyFeature::CliSwitching));
}

#[test]
fn turning_cli_switching_on_is_refused_while_paused_and_off_still_works() {
    let mac = FakeMac::new();
    mac.write_zshrc("alias ll='ls -l'\n");
    shared_env(&mac, json!({ "CLAUDE_CONFIG_DIR": "/x" }));
    let mut core = mac.launch();

    core.command(Command::SetCliSwitching(true));

    assert_eq!(mac.read_file(".zshrc").unwrap(), "alias ll='ls -l'\n");
    let cli = core.view().settings.cli_switching;
    assert!(!cli.on);
    assert!(cli.note.unwrap().contains("paused"));
}

#[test]
fn api_key_provider_and_scrub_variables_only_warn_once() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    shared_env(
        &mac,
        json!({ "ANTHROPIC_API_KEY": "sk-x", "CLAUDE_CODE_USE_BEDROCK": "1",
                "CLAUDE_CODE_SUBPROCESS_ENV_SCRUB": "1" }),
    );

    let core = mac.launch();

    assert!(!is_paused(&core, SafetyFeature::CliSwitching));
    let lines = warnings(&core);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains("ANTHROPIC_API_KEY") && lines[0].contains("CLAUDE_CODE_USE_BEDROCK"));
    assert!(lines[1].contains("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1"));
    assert!(!lines.iter().any(|line| line.contains("sk-x")));
    drop(core);

    let relaunched = mac.launch();
    assert!(warnings(&relaunched).is_empty(), "warned once");
    let env_check = relaunched
        .view()
        .diagnostics
        .checks
        .into_iter()
        .find(|check| check.name == "Settings env")
        .unwrap();
    assert!(env_check.detail.unwrap().contains("ANTHROPIC_API_KEY"));
}

#[test]
fn removing_and_adding_back_a_warned_variable_warns_again() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    shared_env(&mac, json!({ "ANTHROPIC_AUTH_TOKEN": "t" }));
    drop(mac.launch());
    shared_env(&mac, json!({}));
    drop(mac.launch());
    shared_env(&mac, json!({ "ANTHROPIC_AUTH_TOKEN": "t" }));

    let core = mac.launch();

    assert_eq!(warnings(&core).len(), 1);
}

// ----- Claude Code honouring Profile folders ------------------------------------------

/// Installs Claude Code `version` the way the native installer does:
/// `~/.local/bin/claude` linking to `~/.local/share/claude/versions/<v>`.
fn install_claude_code(mac: &FakeMac, version: &str) {
    let target = mac.write_file(format!(".local/share/claude/versions/{version}"), "binary");
    let link = mac.path(".local/bin/claude");
    let _ = std::fs::remove_file(&link);
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(target, link).unwrap();
}

fn auth_status_calls(mac: &FakeMac) -> usize {
    mac.claude
        .calls()
        .iter()
        .filter(|call| call.kind == StubCall::AuthStatus)
        .count()
}

#[test]
fn a_claude_code_update_that_ignores_profile_folders_pauses_cli_switching() {
    let (mac, mut core) = cli_on_with_two_profiles();
    install_claude_code(&mac, "2.1.400");
    // The new version reports the same account in every folder.
    mac.claude
        .reply(StubCall::AuthStatus, auth_status(&FixtureLogin::perso()));
    let pointer = mac.read_file(POINTER).unwrap();

    let mut effects = core.event(Event::MenuOpened);
    effects.extend(settle(&mut core));

    assert_eq!(
        warnings(&core),
        ["Claude Code 2.1.400 ignores Profile folders · CLI switching paused"]
    );
    assert_eq!(notifications(&effects).len(), 1);
    assert_eq!(core.view().menu_bar.label.as_deref(), Some("?"));
    assert!(rows(&core).iter().all(|row| row.command.is_none()));
    assert_eq!(mac.read_file(POINTER).unwrap(), pointer);
}

#[test]
fn auth_status_disagreeing_with_one_folder_pauses_cli_switching() {
    let (mac, mut core) = cli_on_with_two_profiles();
    install_claude_code(&mac, "2.1.400");
    // Work's folder (checked first) answers as a third account; perso's
    // honestly.
    let stranger = FixtureLogin::new(
        "55555555-5555-4555-8555-555555555555",
        "66666666-6666-4666-8666-666666666666",
        "someone@else.example",
        "Else Ltd",
    );
    mac.claude
        .reply(StubCall::AuthStatus, auth_status(&stranger));
    honest_auth_status(&mac);

    core.event(Event::MenuOpened);
    settle(&mut core);

    assert_eq!(
        warnings(&core),
        ["Claude Code 2.1.400 didn't use a Profile folder's login · CLI switching paused"]
    );
    assert_eq!(
        diagnostics_check(&core, "Claude Code honours Profile folders")
            .detail
            .as_deref(),
        Some("auth status names another account in 1 Profile")
    );
}

#[test]
fn honest_answers_after_an_update_pause_nothing_and_auth_status_stays_rare() {
    let (mac, mut core) = cli_on_with_two_profiles();
    honest_auth_status(&mac);
    let before = auth_status_calls(&mac);

    core.event(Event::MenuOpened);
    drop(mac.launch());
    assert_eq!(
        auth_status_calls(&mac),
        before,
        "no auth status without a version change"
    );

    install_claude_code(&mac, "2.1.400");
    core.event(Event::MenuOpened);
    core.event(Event::MenuOpened);
    settle(&mut core);

    assert_eq!(auth_status_calls(&mac), before + 2, "once per Profile");
    assert!(!is_paused(&core, SafetyFeature::CliSwitching));
    let check = diagnostics_check(&core, "Claude Code honours Profile folders");
    assert_eq!(check.status, CheckStatus::Pass);
    assert_eq!(
        core.view().diagnostics.versions.claude_code.as_deref(),
        Some("2.1.400")
    );
}

#[test]
fn a_version_change_is_also_noticed_on_a_tick() {
    let (mac, mut core) = cli_on_with_two_profiles();
    mac.claude
        .reply(StubCall::AuthStatus, auth_status(&FixtureLogin::perso()));
    mac.os.set_desktop_app_version(Some("3.7000.0"));

    let mut effects = core.event(Event::Tick);
    effects.extend(settle(&mut core));

    assert_eq!(notifications(&effects).len(), 1);
    assert!(is_paused(&core, SafetyFeature::CliSwitching));
    assert_eq!(
        core.view().diagnostics.versions.desktop.as_deref(),
        Some("3.7000.0")
    );
}

#[test]
fn an_auth_status_that_never_ends_doesnt_hold_up_the_menu_and_is_reported() {
    let (mac, core) = cli_on_with_two_profiles();
    install_claude_code(&mac, "2.1.400");
    mac.claude.reply(StubCall::AuthStatus, Reply::hanging());
    let before = auth_status_calls(&mac);

    let (core, _) = event_promptly(core, Event::MenuOpened);
    let hanging = wait_for_calls(&mac, StubCall::AuthStatus, before + 1)
        .pop()
        .unwrap();
    let (mut core, _) = event_promptly(core, Event::Tick);

    // One Profile at a time: each is given up on after 30 s.
    mac.clock.advance(jiff::SignedDuration::from_secs(30));
    core.event(Event::Tick);
    wait_for_calls(&mac, StubCall::AuthStatus, before + 2);
    pass(&mac, &mut core, jiff::SignedDuration::from_secs(30));

    assert!(!hanging.still_running(), "given up on");
    assert!(!is_paused(&core, SafetyFeature::CliSwitching));
    let check = diagnostics_check(&core, "Claude Code honours Profile folders");
    assert_eq!(check.status, CheckStatus::Fail);
    assert_eq!(
        check.detail.as_deref(),
        Some("auth status didn't answer in 2 Profiles")
    );
}

#[test]
fn unreadable_auth_status_answers_pause_nothing() {
    let (mac, mut core) = cli_on_with_two_profiles();
    install_claude_code(&mac, "2.1.400");
    mac.claude
        .reply(StubCall::AuthStatus, Reply::ok("not json\n"));

    core.event(Event::MenuOpened);
    settle(&mut core);

    assert!(!is_paused(&core, SafetyFeature::CliSwitching));
}

fn diagnostics_check(core: &Core, name: &str) -> CheckRow {
    core.view()
        .diagnostics
        .checks
        .into_iter()
        .find(|check| check.name == name)
        .unwrap_or_else(|| panic!("Diagnostics lists {name}"))
}

/// Relaunches after a Claude Code update, with `auth status` never
/// answering: the launch self-check stays out until each run times out.
fn relaunch_while_auth_status_hangs(mac: &FakeMac) -> Core {
    install_claude_code(mac, "2.1.400");
    mac.claude.reply(StubCall::AuthStatus, Reply::hanging());
    let before = auth_status_calls(mac);
    let core = mac.launch();
    wait_for_calls(mac, StubCall::AuthStatus, before + 1);
    core
}

/// Lets both hanging `auth status` runs time out (30 s each, one at a time).
fn let_the_self_check_time_out(mac: &FakeMac, core: &mut Core) {
    let before = auth_status_calls(mac);
    mac.clock.advance(jiff::SignedDuration::from_secs(30));
    core.event(Event::Tick);
    wait_for_calls(mac, StubCall::AuthStatus, before + 1);
    pass(mac, core, jiff::SignedDuration::from_secs(30));
}

fn menu_header(core: &Core) -> Option<String> {
    core.view()
        .menu
        .items
        .into_iter()
        .find_map(|item| match item {
            accroster_core::MenuItem::Detail(text) if text.starts_with("CLI: ") => Some(text),
            _ => None,
        })
}

#[test]
fn cli_switching_waits_while_the_launch_self_check_is_out() {
    let (mac, core) = cli_on_with_two_profiles();
    drop(core);
    let pointer = mac.read_file(POINTER).unwrap();
    let mut core = relaunch_while_auth_status_hangs(&mac);

    assert_eq!(core.view().menu_bar.label.as_deref(), Some("?"));
    assert_eq!(
        menu_header(&core).as_deref(),
        Some("CLI: me · Work Inc · checking…")
    );
    assert!(
        rows(&core).iter().all(|row| row.command.is_none()
            && !row_actions(row).iter().any(|t| t == "Use for new CLI runs")),
        "rows don't switch yet"
    );
    for id in mac.list(PROFILES) {
        core.command(Command::UseForNewCliRuns(id));
    }
    assert_eq!(
        mac.read_file(POINTER).unwrap(),
        pointer,
        "pointer untouched"
    );
    assert_eq!(
        core.view().settings.cli_switching.note.as_deref(),
        Some("Checking Claude Code in each Profile · CLI switching waits until that's done")
    );

    let_the_self_check_time_out(&mac, &mut core);

    assert!(!is_paused(&core, SafetyFeature::CliSwitching));
    assert_eq!(menu_header(&core).as_deref(), Some("CLI: me · Work Inc"));
    assert_eq!(core.view().settings.cli_switching.note, None);
    click_row(&mut core, 1);
    assert_ne!(mac.read_file(POINTER).unwrap(), pointer, "switches again");
}

#[test]
fn turning_cli_switching_on_waits_for_the_launch_self_check() {
    let (mac, mut core) = cli_on_with_two_profiles();
    core.command(Command::SetCliSwitching(false));
    drop(core);
    let mut core = relaunch_while_auth_status_hangs(&mac);

    core.command(Command::SetCliSwitching(true));

    assert!(!core.view().settings.cli_switching.on, "refused for now");
    assert_eq!(
        core.view().settings.cli_switching.note.as_deref(),
        Some("Checking Claude Code in each Profile · CLI switching waits until that's done")
    );

    let_the_self_check_time_out(&mac, &mut core);
    core.command(Command::SetCliSwitching(true));

    assert!(core.view().settings.cli_switching.on);
}

// ----- Profile links -------------------------------------------------------------------

/// Replaces `name`'s link in the Profile signed in as `email` by a real
/// folder holding one file.
fn replace_link(mac: &FakeMac, email: &str, name: &str) -> String {
    let dir = format!("{PROFILES}/{}/{name}", profile_of(mac, email));
    std::fs::remove_file(mac.path(&dir)).unwrap();
    mac.write_file(format!("{dir}/kept.txt"), "the user's own file");
    dir
}

/// The folders under `profiles/` that `before` didn't list.
fn new_folders(mac: &FakeMac, before: &[String]) -> Vec<String> {
    mac.list(PROFILES)
        .into_iter()
        .filter(|id| !before.contains(id))
        .map(|id| format!("{PROFILES}/{id}"))
        .collect()
}

fn links_in(mac: &FakeMac, folder: &str) -> Vec<String> {
    mac.list(folder)
        .into_iter()
        .filter(|name| {
            std::fs::symlink_metadata(mac.path(format!("{folder}/{name}")))
                .is_ok_and(|meta| meta.file_type().is_symlink())
        })
        .collect()
}

#[test]
fn while_links_are_paused_no_link_is_made_or_repaired() {
    let (mac, core) = cli_on_with_two_profiles();
    drop(core);
    replace_link(&mac, "me@work.example", "projects");
    let perso = format!("{PROFILES}/{}", profile_of(&mac, "me@example.com"));
    std::fs::remove_file(mac.path(format!("{perso}/skills"))).unwrap();

    let mut core = mac.launch();

    assert!(is_paused(&core, SafetyFeature::Links));
    assert!(
        !links_in(&mac, &perso).contains(&"skills".to_owned()),
        "a missing link isn't repaired"
    );
    let before = mac.list(PROFILES);
    mac.claude.reply(StubCall::AuthLogin, Reply::hanging());
    core.command(Command::AddProfile);
    let added = new_folders(&mac, &before);
    assert_eq!(added.len(), 1);
    assert!(
        links_in(&mac, &added[0]).is_empty(),
        "a new Profile gets none: {:?}",
        mac.list(&added[0])
    );
    assert!(
        mac.read_file(format!("{}/.claude.json", added[0]))
            .is_some(),
        "seeding isn't paused"
    );
    core.command(Command::CancelSignIn);
}

#[test]
fn a_changed_claude_json_account_format_pauses_seeding_too() {
    let (mac, core) = cli_on_with_two_profiles();
    drop(core);
    let work = format!(
        "{PROFILES}/{}/.claude.json",
        profile_of(&mac, "me@work.example")
    );
    mac.write_json(
        &work,
        &json!({ "oauthAccount": { "accountUuid": 7, "organizationUuid": "o", "emailAddress": "me@work.example" } }),
    );
    mac.write_json(".claude.json", &json!({ "theme": "dark" }));

    let mut core = mac.launch();

    assert!(is_paused(&core, SafetyFeature::Seeding));
    let before = mac.list(PROFILES);
    mac.claude.reply(StubCall::AuthLogin, Reply::hanging());
    core.command(Command::AddProfile);
    let added = new_folders(&mac, &before);
    assert_eq!(added.len(), 1);
    assert_eq!(
        mac.read_file(format!("{}/.claude.json", added[0])),
        None,
        "not seeded while paused"
    );
    assert!(!links_in(&mac, &added[0]).is_empty(), "links aren't paused");
    core.command(Command::CancelSignIn);
}

fn row_titled<'a>(
    rows: &'a [accroster_core::ProfileRow],
    email: &str,
) -> &'a accroster_core::ProfileRow {
    rows.iter()
        .find(|row| row.title.starts_with(email))
        .expect("a row for that email")
}

#[test]
fn a_replaced_projects_link_pauses_profile_links_and_nothing_is_overwritten() {
    let (mac, core) = cli_on_with_two_profiles();
    drop(core);
    let replaced = replace_link(&mac, "me@work.example", "projects");

    let mut core = mac.launch();

    assert!(is_paused(&core, SafetyFeature::Links));
    assert_eq!(
        warnings(&core),
        ["projects link replaced in 1 Profile · Profile links paused"]
    );
    assert_eq!(
        notifications(&core.take_effects()),
        [(
            "Profile links paused".to_owned(),
            "projects link replaced in 1 Profile. Nothing was written. Resume it in Diagnostics once this is fixed.".to_owned()
        )]
    );
    assert_eq!(
        mac.read_file(format!("{replaced}/kept.txt")).as_deref(),
        Some("the user's own file")
    );
    let rows = rows(&core);
    assert_eq!(
        row_titled(&rows, "me@work.example").note.as_deref(),
        Some("Transcripts not shared")
    );
    assert_eq!(row_titled(&rows, "me@example.com").note, None);
    assert!(
        !is_paused(&core, SafetyFeature::CliSwitching),
        "CLI switching is unaffected"
    );

    // Fixed by hand, then resumed by the click.
    std::fs::remove_dir_all(mac.path(&replaced)).unwrap();
    core.event(Event::MenuOpened);
    assert!(is_paused(&core, SafetyFeature::Links));
    let resume = resume_button(&core, SafetyFeature::Links).expect("Resume offered");
    core.command(resume);
    settle(&mut core);

    assert!(!is_paused(&core, SafetyFeature::Links));
    assert!(
        std::fs::symlink_metadata(mac.path(&replaced))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link is made again after Resume"
    );
    assert_eq!(row_titled(&rows_of(&core), "me@work.example").note, None);
}

fn rows_of(core: &Core) -> Vec<accroster_core::ProfileRow> {
    rows(core)
}

#[test]
fn a_profile_added_during_the_pause_gets_no_links_and_says_transcripts_arent_shared() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    for login in [work(), FixtureLogin::perso()] {
        mac.claude
            .reply(StubCall::AuthLogin, sign_in_as(&login))
            .reply(StubCall::AuthStatus, auth_status(&login))
            .reply(StubCall::GetUsage, usage_reply(limits_body()));
    }
    let mut core = mac.launch();
    add_profile(&mut core);
    drop(core);
    replace_link(&mac, "me@work.example", "file-history");
    let mut core = mac.launch();

    add_profile(&mut core);

    let perso = format!("{PROFILES}/{}", profile_of(&mac, "me@example.com"));
    for name in ["projects", "file-history", "session-env", "skills"] {
        assert!(
            std::fs::symlink_metadata(mac.path(format!("{perso}/{name}"))).is_err(),
            "{name} not linked during the pause"
        );
    }
    assert_eq!(
        row_titled(&rows(&core), "me@example.com").note.as_deref(),
        Some("Transcripts not shared")
    );
}

#[test]
fn another_replaced_link_only_warns_in_details_and_diagnostics() {
    let (mac, core) = cli_on_with_two_profiles();
    drop(core);
    replace_link(&mac, "me@work.example", "skills");

    let core = mac.launch();

    assert!(core.view().diagnostics.paused.is_empty());
    assert!(warnings(&core).is_empty());
    let rows = rows(&core);
    assert!(
        details(row_titled(&rows, "me@work.example"))
            .contains(&"skills link replaced · not shared".to_owned())
    );
    assert_eq!(row_titled(&rows, "me@work.example").note, None);
    let check = diagnostics_check(&core, "Other Profile links");
    assert_eq!(check.status, CheckStatus::Fail);
    assert_eq!(
        check.detail.as_deref(),
        Some("skills link replaced in 1 Profile")
    );
}

// ----- Diagnostics ---------------------------------------------------------------------

#[test]
fn on_an_empty_mac_every_check_is_never_seen_except_the_settings_env() {
    let mac = FakeMac::new();

    let core = mac.launch();

    let checks = core.view().diagnostics.checks;
    let names: Vec<&str> = checks.iter().map(|check| check.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "get_usage response",
            ".claude.json oauthAccount",
            ".claude.json cachedUsageUtilization",
            "Claude Code honours Profile folders",
            "Settings env",
            "Shared-Transcript links",
            "Other Profile links",
            "Sidebar entries",
            "Live-process files",
            "Transcript records",
            "Dormant multi-account mode",
        ]
    );
    for check in &checks {
        let expected = if check.name == "Settings env" {
            CheckStatus::Pass
        } else {
            CheckStatus::NeverSeen
        };
        assert_eq!(check.status, expected, "{}", check.name);
    }
}

#[test]
fn copy_report_holds_check_names_field_names_and_versions_only() {
    let (mac, core) = cli_on_with_two_profiles();
    drop(core);
    honest_auth_status(&mac);
    install_claude_code(&mac, "2.1.310");
    mac.os.set_desktop_app_version(Some("3.6556.2"));
    std::fs::create_dir_all(mac.path("Library/Application Support/Claude/claude-code/2.1.308"))
        .unwrap();
    let account = FixtureLogin::perso();
    mac.write_sidebar_entry(
        &account.account_uuid,
        &account.organization_uuid,
        "0f1e",
        &json!({ "sessionId": "local_0f1e", "cliSessionId": "3c", "title": "Fix login bug",
                 "newField": "secret-ish value", "isArchived": false }),
    );
    shared_env(&mac, json!({ "ANTHROPIC_API_KEY": "sk-ant-123" }));
    let mut core = mac.launch();

    let effects = core.command(Command::CopyDiagnosticsReport);

    let report = match effects.as_slice() {
        [Effect::CopyToClipboard(text)] => text.clone(),
        other => panic!("one copy: {other:?}"),
    };
    assert_eq!(report, core.view().diagnostics.report);
    for expected in [
        "Claude Code: 2.1.310",
        "Desktop's Claude Code: 2.1.308",
        "Claude desktop app: 3.6556.2",
        "pass: Sidebar entries (1 entries read · new fields seen: newField)",
        "settings.json sets ANTHROPIC_API_KEY",
    ] {
        assert!(report.contains(expected), "{expected} in:\n{report}");
    }
    for leaked in [
        "me@example.com",
        "me@work.example",
        "Work Inc",
        account.account_uuid.as_str(),
        account.organization_uuid.as_str(),
        "Fix login bug",
        "secret-ish value",
        "sk-ant-123",
        "tester",
        "local_0f1e",
    ] {
        assert!(!report.contains(leaked), "{leaked} leaked into:\n{report}");
    }
}

#[test]
fn sidebar_entries_with_known_fields_pass() {
    let mac = FakeMac::new();
    let account = FixtureLogin::perso();
    mac.write_sidebar_entry(
        &account.account_uuid,
        &account.organization_uuid,
        "0f1e",
        &json!({ "sessionId": "local_0f1e", "cliSessionId": "3c", "title": "t", "titleSource": "auto",
                 "isArchived": false, "createdAt": 1, "lastActivityAt": 2, "cwd": "/x",
                 "bridgeSessionIds": [], "enabledMcpTools": {} }),
    );

    let core = mac.launch();

    assert_eq!(
        diagnostics_check(&core, "Sidebar entries").status,
        CheckStatus::Pass
    );
}

#[test]
fn a_sidebar_entry_named_unlike_its_session_id_fails_the_check() {
    let mac = FakeMac::new();
    let account = FixtureLogin::perso();
    mac.write_sidebar_entry(
        &account.account_uuid,
        &account.organization_uuid,
        "0f1e",
        &json!({ "sessionId": "local_other", "title": "t" }),
    );

    let core = mac.launch();

    let check = diagnostics_check(&core, "Sidebar entries");
    assert_eq!(check.status, CheckStatus::Fail);
    assert_eq!(
        check.detail.as_deref(),
        Some("sessionId differs from the file name in 1")
    );
}

#[test]
fn live_process_files_missing_a_field_fail_the_check() {
    let mac = FakeMac::new();
    mac.write_live_process(
        ".claude",
        4242,
        &json!({ "pid": 4242, "sessionId": "s", "procStart": "Sat Oct 25 14:02:11 2026",
                 "entrypoint": "claude-desktop", "status": "busy" }),
    );
    mac.write_live_process(".claude", 4243, &json!({ "pid": "4243", "sessionId": "s" }));
    mac.write_file(".claude/sessions/4242.key", "never read");

    let core = mac.launch();

    let check = diagnostics_check(&core, "Live-process files");
    assert_eq!(check.status, CheckStatus::Fail);
    assert_eq!(
        check.detail.as_deref(),
        Some("missing or wrong type: entrypoint, pid, procStart, status")
    );
}

#[test]
fn transcript_records_are_checked_and_bad_lines_tolerated() {
    let mac = FakeMac::new();
    mac.write_transcript(
        "-Users-me-Dev-app",
        "3c",
        &[
            json!({"type":"user","message":{"role":"user","content":"hi"},"entrypoint":"cli","sessionId":"3c"}),
            json!({"type":"custom-title","customTitle":"Mine","sessionId":"3c"}),
        ],
    );
    let good = mac.launch();
    assert_eq!(
        diagnostics_check(&good, "Transcript records").status,
        CheckStatus::Pass
    );
    drop(good);
    mac.write_transcript(
        "-Users-me-Dev-app",
        "4d",
        &[json!({"type":"ai-title","title":"moved","sessionId":"4d"})],
    );
    let path = mac.path(".claude/projects/-Users-me-Dev-app/4d.jsonl");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("{\"type\":\"user\",\"mess");
    std::fs::write(&path, text).unwrap();

    let core = mac.launch();

    let check = diagnostics_check(&core, "Transcript records");
    assert_eq!(check.status, CheckStatus::Fail);
    assert_eq!(
        check.detail.as_deref(),
        Some("missing or wrong type: ai-title.aiTitle")
    );
}

// ----- Reads degrade per item ----------------------------------------------------------

#[test]
fn an_unknown_limit_kind_is_a_bar_and_a_diagnostics_line() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    let mut body = limits_body();
    body["limits"].as_array_mut().unwrap().push(json!({
        "kind": "weekly_cowork", "group": "weekly", "percent": 30, "resets_at": null, "scope": null
    }));
    mac.claude
        .reply(StubCall::AuthLogin, sign_in_as(&work()))
        .reply(StubCall::AuthStatus, auth_status(&work()))
        .reply(StubCall::GetUsage, usage_reply(body));
    let mut core = mac.launch();

    add_profile(&mut core);

    assert!(ignoring_reset_times(&rows(&core)[0].limits).contains(&bar("weekly_cowork", 30)));
    let check = diagnostics_check(&core, "get_usage response");
    assert_eq!(check.status, CheckStatus::Pass);
    assert_eq!(
        check.detail.as_deref(),
        Some("New limit kind seen: weekly_cowork")
    );
}

#[test]
fn one_unreadable_usage_answer_degrades_only_its_own_row() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.claude
        .reply(StubCall::AuthLogin, sign_in_as(&work()))
        .reply(StubCall::AuthStatus, auth_status(&work()))
        .reply(StubCall::GetUsage, usage_reply(limits_body()))
        .reply(StubCall::AuthLogin, sign_in_as(&FixtureLogin::perso()))
        .reply(StubCall::AuthStatus, auth_status(&FixtureLogin::perso()))
        .reply(StubCall::GetUsage, Reply::ok("garbage\n"))
        .reply(StubCall::SlashUsage, Reply::ok("garbage\n"));
    let mut core = mac.launch();

    add_profile(&mut core);
    add_profile(&mut core);

    let rows = rows(&core);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].limits.len(), 3, "work keeps its bars");
    assert_eq!(rows[1].note.as_deref(), Some("Couldn't read limits"));
    assert!(rows[1].limits.is_empty());
    let check = diagnostics_check(&core, "get_usage response");
    assert_eq!(check.status, CheckStatus::Fail);
    assert!(
        core.view().diagnostics.paused.is_empty(),
        "reads never pause"
    );
}

#[test]
fn a_malformed_oauth_account_pauses_cli_switching_and_hides_only_that_profile() {
    let (mac, core) = cli_on_with_two_profiles();
    drop(core);
    let work = format!(
        "{PROFILES}/{}/.claude.json",
        profile_of(&mac, "me@work.example")
    );
    mac.write_json(
        &work,
        &json!({ "oauthAccount": { "accountUuid": 7, "organizationUuid": "o", "emailAddress": "me@work.example" } }),
    );

    let core = mac.launch();

    assert!(is_paused(&core, SafetyFeature::CliSwitching));
    assert_eq!(
        warnings(&core),
        [
            "Claude Code changed .claude.json accounts · CLI switching paused",
            "Claude Code changed .claude.json accounts · Seeding .claude.json paused"
        ]
    );
    let titles: Vec<String> = rows(&core).into_iter().map(|row| row.title).collect();
    assert_eq!(
        titles,
        ["Signed-out Profile", "me@example.com"],
        "the unreadable account degrades to its own row only"
    );
    assert_eq!(
        diagnostics_check(&core, ".claude.json oauthAccount")
            .detail
            .as_deref(),
        Some("missing or not text: accountUuid")
    );
}
