//! CLI switching (ADR 0002): the `.zshrc` block, the `claude` shim and the
//! pointer naming the CLI's Active Profile.

mod common;

use accroster_core::{Command, Core, Effect, MenuItem};
use accroster_fake_mac::{FakeMac, FixtureLogin, Reply, StubCall};
use common::*;

const BIN: &str = "Library/Application Support/io.github.jbmanllr.accroster/bin";
const POINTER: &str =
    "Library/Application Support/io.github.jbmanllr.accroster/bin/cli-active-profile";
const SHIM: &str = "Library/Application Support/io.github.jbmanllr.accroster/bin/claude";

fn turn_on(core: &mut Core) {
    core.command(Command::SetCliSwitching(true));
}

fn zshrc_backups(mac: &FakeMac) -> Vec<String> {
    let groups: Vec<String> = mac
        .list(format!("{APP_FOLDER}/backups"))
        .into_iter()
        .filter(|name| name.starts_with(".zshrc-"))
        .collect();
    groups
        .iter()
        .flat_map(|group| mac.list(format!("{APP_FOLDER}/backups/{group}")))
        .filter(|name| name.ends_with(".bak"))
        .collect()
}

/// The exact block AccRoster appends, for this Mac's app folder.
fn expected_block(mac: &FakeMac) -> String {
    format!(
        "# >>> accroster >>>\n\
         # AccRoster's claude shim for CLI switching. Turn CLI switching off in AccRoster to remove this block.\n\
         export PATH='{}':\"$PATH\"\n\
         # <<< accroster <<<\n",
        mac.path(BIN).display()
    )
}

#[test]
fn settings_show_the_exact_line_before_cli_switching_is_turned_on() {
    let mac = FakeMac::new();
    let core = mac.launch();

    let cli = core.view().settings.cli_switching;
    assert!(!cli.on);
    assert_eq!(
        cli.line,
        format!("export PATH='{}':\"$PATH\"", mac.path(BIN).display())
    );
    assert!(mac.read_file(SHIM).is_none());
}

#[test]
fn turning_cli_switching_on_backs_up_zshrc_and_appends_the_marked_block_at_the_end() {
    let mac = FakeMac::new();
    let original = "export PATH=\"$HOME/.local/bin:$PATH\"\nalias ll='ls -l'\n";
    mac.write_zshrc(original);
    mac.write_file(".local/bin/claude", "the native installer's link target\n");
    let before = mac.snapshot();
    let mut core = mac.launch();

    turn_on(&mut core);

    assert_eq!(
        mac.read_file(".zshrc").unwrap(),
        format!("{original}\n{}", expected_block(&mac))
    );
    assert_eq!(zshrc_backups(&mac).len(), 1);
    let backup_group = mac
        .list(format!("{APP_FOLDER}/backups"))
        .into_iter()
        .find(|name| name.starts_with(".zshrc-"))
        .unwrap();
    let backup = zshrc_backups(&mac).remove(0);
    assert_eq!(
        mac.read_file(format!("{APP_FOLDER}/backups/{backup_group}/{backup}"))
            .unwrap(),
        original
    );
    assert_eq!(
        mac.snapshot()
            .get(std::path::Path::new(".local/bin/claude")),
        before.get(std::path::Path::new(".local/bin/claude")),
        "~/.local/bin/claude is never touched"
    );
    assert!(core.view().settings.cli_switching.on);
    let shim = mac.path(SHIM);
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&shim).unwrap().permissions());
    assert_eq!(mode & 0o111, 0o111, "the shim is executable");
    assert!(mac.read_file(SHIM).unwrap().starts_with("#!/bin/sh\n"));
    assert!(mac.read_file(POINTER).is_none(), "no Profile chosen yet");
}

#[test]
fn turning_cli_switching_on_twice_adds_one_block() {
    let mac = FakeMac::new();
    mac.write_zshrc("alias ll='ls -l'\n");
    let mut core = mac.launch();

    turn_on(&mut core);
    turn_on(&mut core);

    let zshrc = mac.read_file(".zshrc").unwrap();
    assert_eq!(zshrc.matches("# >>> accroster >>>").count(), 1, "{zshrc}");
}

#[test]
fn turning_cli_switching_on_without_a_zshrc_creates_one_holding_the_block() {
    let mac = FakeMac::new();
    let mut core = mac.launch();

    turn_on(&mut core);

    assert_eq!(mac.read_file(".zshrc").unwrap(), expected_block(&mac));
}

#[test]
fn a_zshrc_kept_elsewhere_through_a_link_is_written_through_the_link() {
    let mac = FakeMac::new();
    mac.write_file("dotfiles/zshrc", "alias ll='ls -l'\n");
    std::os::unix::fs::symlink(mac.path("dotfiles/zshrc"), mac.path(".zshrc")).unwrap();
    let mut core = mac.launch();

    turn_on(&mut core);

    assert!(
        std::fs::symlink_metadata(mac.path(".zshrc"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        mac.read_file("dotfiles/zshrc")
            .unwrap()
            .contains("# >>> accroster >>>")
    );
}

#[test]
fn turning_cli_switching_off_removes_the_block_byte_exact_then_the_shim_and_the_pointer() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let original = "export PATH=\"$HOME/.local/bin:$PATH\"\nalias ll='ls -l'\n";
    mac.write_zshrc(original);
    let mut core = mac.launch();
    add_profile(&mut core);
    turn_on(&mut core);
    click_row(&mut core, 0);
    assert!(mac.read_file(POINTER).is_some());

    core.command(Command::SetCliSwitching(false));

    assert_eq!(mac.read_file(".zshrc").unwrap(), original);
    assert!(mac.read_file(SHIM).is_none());
    assert!(mac.read_file(POINTER).is_none());
    assert_eq!(mac.list(PROFILES).len(), 1, "Profiles stay");
    assert_eq!(rows(&core).len(), 1);
    let cli = core.view().settings.cli_switching;
    assert!(!cli.on);
    assert_eq!(cli.note, None);
    assert_eq!(zshrc_backups(&mac).len(), 2, "backed up before each write");
}

#[test]
fn an_edited_block_is_left_in_place_and_its_lines_are_listed() {
    let mac = FakeMac::new();
    mac.write_zshrc("alias ll='ls -l'\n");
    let mut core = mac.launch();
    turn_on(&mut core);
    let zshrc = mac.read_file(".zshrc").unwrap();
    let edited = zshrc.replace("\"$PATH\"", "\"$PATH:/opt/extra\"");
    mac.write_zshrc(&edited);

    core.command(Command::SetCliSwitching(false));

    assert_eq!(mac.read_file(".zshrc").unwrap(), edited, "left alone");
    assert!(mac.read_file(SHIM).is_none());
    let cli = core.view().settings.cli_switching;
    assert!(!cli.on);
    let note = cli.note.expect("the edited lines are listed");
    for line in [
        "# >>> accroster >>>".to_string(),
        format!(
            "export PATH='{}':\"$PATH:/opt/extra\"",
            mac.path(BIN).display()
        ),
        "# <<< accroster <<<".to_string(),
    ] {
        assert!(note.contains(&line), "{note}");
    }
    assert!(!note.contains("alias ll"), "{note}");
}

// ----- Clicking a row ------------------------------------------------------------------

/// Clicks the `index`th Profile row, as the row's custom view does.
fn click_row(core: &mut Core, index: usize) {
    let command = rows(core)[index]
        .command
        .clone()
        .expect("the row is clickable");
    core.command(command);
}

fn submenu_command(core: &Core, row: usize, title: &str) -> Command {
    rows(core)[row]
        .submenu
        .iter()
        .find_map(|item| match item {
            MenuItem::Action { title: t, command }
            | MenuItem::Check {
                title: t, command, ..
            } if t == title => Some(command.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the submenu has {title}: {:?}", rows(core)[row].submenu))
}

fn checked(core: &Core, row: usize) -> bool {
    rows(core)[row]
        .submenu
        .iter()
        .find_map(|item| match item {
            MenuItem::Check { title, checked, .. } if title == "Use for new CLI runs" => {
                Some(*checked)
            }
            _ => None,
        })
        .expect("the submenu has Use for new CLI runs")
}

fn header(core: &Core) -> Option<String> {
    core.view()
        .menu
        .items
        .into_iter()
        .find_map(|item| match item {
            MenuItem::Detail(text) if text.starts_with("CLI: ") => Some(text),
            _ => None,
        })
}

/// A Mac with two Profiles (work, then perso), CLI switching on.
fn two_profiles() -> (FakeMac, Core) {
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
    turn_on(&mut core);
    (mac, core)
}

fn profile_dir(mac: &FakeMac, core: &Core, row: usize) -> String {
    let title = &rows(core)[row].title;
    mac.list(PROFILES)
        .into_iter()
        .map(|id| mac.path(format!("{PROFILES}/{id}")).display().to_string())
        .find(|dir| {
            let account: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(format!("{dir}/.claude.json")).unwrap(),
            )
            .unwrap();
            title.starts_with(account["oauthAccount"]["emailAddress"].as_str().unwrap())
        })
        .unwrap()
}

#[test]
fn clicking_a_row_rewrites_only_the_pointer_with_the_profile_path_verbatim() {
    let (mac, mut core) = two_profiles();
    let before = mac.snapshot();

    click_row(&mut core, 1);

    let after = mac.snapshot();
    let changed: Vec<_> = after
        .iter()
        .filter(|(path, bytes)| before.get(*path) != Some(bytes))
        .map(|(path, _)| path.display().to_string())
        .chain(
            before
                .keys()
                .filter(|path| !after.contains_key(*path))
                .map(|path| path.display().to_string()),
        )
        .filter(|path| !path.ends_with("app.log"))
        .collect();
    assert_eq!(changed, [POINTER]);
    assert_eq!(mac.read_file(POINTER).unwrap(), profile_dir(&mac, &core, 1));
    assert!(
        !mac.list(BIN).iter().any(|name| name.contains(".tmp")),
        "written through a temp file then renamed"
    );
}

#[test]
fn the_menu_shows_the_clis_active_profile_and_its_tightest_limit() {
    let (_mac, mut core) = two_profiles();
    assert_eq!(header(&core).as_deref(), Some("CLI: Default"));
    assert!(rows(&core).iter().all(|row| row.badges.is_empty()));

    click_row(&mut core, 0);

    let view = core.view();
    assert_eq!(view.menu_bar.label, "me 92%", "Fable 92% is the tightest");
    assert!(!view.menu_bar.dimmed);
    assert_eq!(header(&core).as_deref(), Some("CLI: me"));
    assert_eq!(rows(&core)[0].badges, ["CLI"]);
    assert!(rows(&core)[1].badges.is_empty());
    assert!(checked(&core, 0));
    assert!(!checked(&core, 1));

    let use_second = submenu_command(&core, 1, "Use for new CLI runs");
    core.command(use_second);

    assert_eq!(rows(&core)[1].badges, ["CLI"]);
    assert!(rows(&core)[0].badges.is_empty());
    assert!(checked(&core, 1));
}

#[test]
fn copy_shell_override_copies_an_export_line_for_that_profile() {
    let (mac, mut core) = two_profiles();
    let pointer_before = mac.read_file(POINTER);

    let copy = submenu_command(&core, 0, "Copy shell override");
    let effects = core.command(copy);

    assert_eq!(
        effects,
        [Effect::CopyToClipboard(format!(
            "export CLAUDE_CONFIG_DIR='{}'",
            profile_dir(&mac, &core, 0)
        ))]
    );
    assert_eq!(
        mac.read_file(POINTER),
        pointer_before,
        "the global choice stays"
    );
}

#[test]
fn with_cli_switching_off_rows_dont_switch_and_the_menu_bar_shows_the_app_name() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);

    assert_eq!(rows(&core)[0].command, None);
    assert_eq!(core.view().menu_bar.label, "AccRoster");
    assert_eq!(header(&core), None);
    assert!(rows(&core)[0].submenu.iter().any(
        |item| matches!(item, MenuItem::Action { title, .. } if title == "Copy shell override")
    ));
}

#[test]
fn the_cli_choice_survives_a_relaunch() {
    let (mac, mut core) = two_profiles();
    click_row(&mut core, 1);
    drop(core);

    let core = mac.launch();

    assert_eq!(rows(&core)[1].badges, ["CLI"]);
    assert!(core.view().settings.cli_switching.on);
}

// ----- The shim ------------------------------------------------------------------------

/// A Mac with CLI switching on and the pointer naming the work Profile.
fn switched_to_work() -> (FakeMac, Core, String) {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);
    turn_on(&mut core);
    click_row(&mut core, 0);
    let dir = mac.read_file(POINTER).unwrap();
    mac.claude
        .reply(StubCall::Other, Reply::ok("real claude ran\n"));
    (mac, core, dir)
}

fn last_call(mac: &FakeMac) -> accroster_fake_mac::RecordedCall {
    mac.claude.calls().pop().expect("the real claude ran")
}

const CLEARED: [&str; 9] = [
    "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    "CLAUDE_CODE_ACCOUNT_UUID",
    "CLAUDE_CODE_USER_EMAIL",
    "CLAUDE_CODE_ORGANIZATION_UUID",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    "CLAUDE_CODE_OAUTH_SCOPES",
    "CLAUDE_CODE_SUBSCRIPTION_TYPE",
    "CLAUDE_CODE_RATE_LIMIT_TIER",
];

const PASSED_THROUGH: [(&str, &str); 6] = [
    ("ANTHROPIC_API_KEY", "sk-ant-api-test"),
    ("ANTHROPIC_AUTH_TOKEN", "auth-test"),
    ("ANTHROPIC_PROFILE", "dev"),
    ("ANTHROPIC_BASE_URL", "https://gateway.example"),
    ("CLAUDE_CODE_USE_BEDROCK", "1"),
    ("CLAUDE_CODE_USE_VERTEX", "1"),
];

#[test]
fn the_shim_runs_the_real_claude_in_the_pointers_profile_with_the_same_arguments() {
    let (mac, _core, dir) = switched_to_work();

    let out = mac.run_shim(&["--resume", "a b", "-p", "it's"], &[]);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "real claude ran\n");
    let call = last_call(&mac);
    assert_eq!(call.args, ["--resume", "a b", "-p", "it's"]);
    assert_eq!(call.var("CLAUDE_CONFIG_DIR"), Some(dir.as_str()));
}

#[test]
fn the_shim_clears_identity_and_token_variables_and_passes_api_key_and_provider_ones() {
    let (mac, _core, dir) = switched_to_work();
    let mut vars: Vec<(&str, &str)> = CLEARED.iter().map(|name| (*name, "injected")).collect();
    vars.extend(PASSED_THROUGH);

    let out = mac.run_shim(&[], &vars);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty(), "silently");
    let call = last_call(&mac);
    for name in CLEARED {
        assert_eq!(call.var(name), None, "{name} is cleared");
    }
    for (name, value) in PASSED_THROUGH {
        assert_eq!(call.var(name), Some(value), "{name} passes through");
    }
    assert_eq!(call.var("CLAUDE_CONFIG_DIR"), Some(dir.as_str()));
}

#[test]
fn an_inherited_claude_config_dir_wins_and_the_variables_are_still_cleared() {
    let (mac, _core, _dir) = switched_to_work();
    let other = mac.path("elsewhere profile");
    let other = other.to_str().unwrap();
    let mut vars: Vec<(&str, &str)> = CLEARED.iter().map(|name| (*name, "injected")).collect();
    vars.push(("CLAUDE_CONFIG_DIR", other));

    let out = mac.run_shim(&[], &vars);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let call = last_call(&mac);
    assert_eq!(call.var("CLAUDE_CONFIG_DIR"), Some(other));
    for name in CLEARED {
        assert_eq!(call.var(name), None, "{name} is cleared");
    }
}

#[test]
fn with_no_pointer_the_shim_passes_through_untouched() {
    let mac = FakeMac::new();
    let mut core = mac.launch();
    turn_on(&mut core);
    mac.claude.reply(StubCall::Other, Reply::ok(""));

    let out = mac.run_shim(&["--version"], &[("CLAUDE_CODE_OAUTH_TOKEN", "injected")]);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let call = last_call(&mac);
    assert_eq!(call.args, ["--version"]);
    assert_eq!(
        call.var("CLAUDE_CONFIG_DIR"),
        None,
        "Claude Code uses ~/.claude"
    );
    assert_eq!(call.var("CLAUDE_CODE_OAUTH_TOKEN"), None, "still cleared");
}

#[test]
fn a_pointer_naming_a_missing_folder_is_an_error_never_a_fallback() {
    let (mac, _core, dir) = switched_to_work();
    std::fs::remove_dir_all(&dir).unwrap();
    let calls_before = mac.claude.calls().len();

    let out = mac.run_shim(&[], &[]);

    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(&dir), "{stderr}");
    assert_eq!(
        mac.claude.calls().len(),
        calls_before,
        "the real claude never ran"
    );
}

#[test]
fn the_shim_skips_its_own_folder_and_empty_entries_when_walking_path() {
    let (mac, _core, _dir) = switched_to_work();
    // A second copy of the app's bin/ later on PATH (written with a
    // trailing slash) must not loop back into the shim.
    let bin = mac.path(BIN);
    let path = format!(
        "{}::{}/:{}:/usr/bin:/bin",
        bin.display(),
        bin.display(),
        mac.claude.bin_dir().display()
    );
    let out = std::process::Command::new(bin.join("claude"))
        .env_clear()
        .env("HOME", mac.home())
        .env("PATH", path)
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "real claude ran\n");
}

#[test]
fn with_no_other_claude_on_path_the_shim_says_so() {
    let (mac, _core, _dir) = switched_to_work();
    let bin = mac.path(BIN);
    let out = std::process::Command::new(bin.join("claude"))
        .env_clear()
        .env("HOME", mac.home())
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no claude"));
}

#[test]
fn a_nickname_is_the_clis_short_label() {
    let (_mac, mut core) = two_profiles();
    click_row(&mut core, 0);
    let rename = submenu_command(&core, 0, "Rename…");
    let Some(Effect::AskNickname { target, .. }) = core.command(rename).pop() else {
        panic!("Rename… asks for a nickname");
    };

    core.command(Command::SetNickname {
        target,
        nickname: Some("Work".into()),
    });

    assert_eq!(core.view().menu_bar.label, "Work 92%");
    assert_eq!(header(&core).as_deref(), Some("CLI: Work"));
}

/// The choices under the CLI's Active Profile's "Remove…" submenu.
fn removal_choices(core: &Core, row: usize) -> Vec<(String, Command)> {
    rows(core)[row]
        .submenu
        .iter()
        .find_map(|item| match item {
            MenuItem::Submenu { title, items } if title == "Remove…" => Some(
                items
                    .iter()
                    .filter_map(|item| match item {
                        MenuItem::Action { title, command } => {
                            Some((title.clone(), command.clone()))
                        }
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_else(|| panic!("Remove… is a submenu: {:?}", rows(core)[row].submenu))
}

fn confirm(core: &mut Core, command: Command) {
    let Some(Effect::ConfirmRemoval { confirm, .. }) = core.command(command).pop() else {
        panic!("the removal is confirmed first");
    };
    core.command(confirm);
}

#[test]
fn removing_the_clis_active_profile_asks_which_profile_is_next_and_moves_the_pointer_first() {
    let (mac, mut core) = two_profiles();
    mac.claude.reply(StubCall::AuthLogout, signed_out());
    click_row(&mut core, 0);
    let perso_dir = profile_dir(&mac, &core, 1);
    assert!(
        !rows(&core)[1]
            .submenu
            .iter()
            .any(|item| matches!(item, MenuItem::Submenu { .. })),
        "only the CLI's Active Profile asks"
    );
    let choices = removal_choices(&core, 0);
    let titles: Vec<&str> = choices.iter().map(|(title, _)| title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "me@example.com · me@example.com's Organization",
            "Default (~/.claude)"
        ]
    );

    confirm(&mut core, choices[0].1.clone());

    assert_eq!(mac.read_file(POINTER).unwrap(), perso_dir);
    assert_eq!(mac.list(PROFILES).len(), 1);
    assert_eq!(rows(&core).len(), 1);
    assert_eq!(rows(&core)[0].badges, ["CLI"]);
}

#[test]
fn removing_the_clis_active_profile_with_default_next_deletes_the_pointer() {
    let (mac, mut core) = two_profiles();
    mac.claude.reply(StubCall::AuthLogout, signed_out());
    click_row(&mut core, 0);

    let default = removal_choices(&core, 0).pop().unwrap().1;
    confirm(&mut core, default);

    assert!(mac.read_file(POINTER).is_none());
    assert_eq!(header(&core).as_deref(), Some("CLI: Default"));
    assert_eq!(mac.list(PROFILES).len(), 1);
}

#[test]
fn the_pointer_moves_before_the_sign_out_so_a_failed_sign_out_keeps_the_folder_but_not_the_cli() {
    let (mac, mut core) = two_profiles();
    mac.claude.reply(
        StubCall::AuthLogout,
        Reply::fail(1, "Logout failed: offline\n"),
    );
    click_row(&mut core, 0);
    let perso_dir = profile_dir(&mac, &core, 1);

    let next_perso = removal_choices(&core, 0)[0].1.clone();
    confirm(&mut core, next_perso);

    assert_eq!(
        mac.read_file(POINTER).unwrap(),
        perso_dir,
        "rewritten first"
    );
    assert_eq!(mac.list(PROFILES).len(), 2, "the folder is kept");
}
