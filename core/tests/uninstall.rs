//! "Uninstall…" in Settings: unhook the CLI, remove every Profile, unregister
//! launch at login, delete the app's folder, then move the app to the Trash,
//! in an order that keeps `claude` working wherever it stops.

mod common;

use std::sync::{Arc, Mutex};

use accroster_core::{Command, Core, Effect, Event, LoginItem, LoginItemStatus};
use accroster_fake_mac::{FakeMac, FixtureLogin, Reply, StubCall};
use common::*;

const POINTER: &str =
    "Library/Application Support/io.github.jbmanllr.accroster/bin/cli-active-profile";
const SHIM: &str = "Library/Application Support/io.github.jbmanllr.accroster/bin/claude";
const ZSHRC: &str = "alias ll='ls -l'\n";

/// Two Profiles (work, then perso), CLI switching on with the pointer on
/// work, launch at login on.
fn installed() -> (FakeMac, Core) {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.write_zshrc(ZSHRC);
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
    core.command(Command::SetLaunchAtLogin(true));
    let work_row = rows(&core)[0].command.clone().expect("rows switch the CLI");
    core.command(work_row);
    assert!(mac.read_file(POINTER).is_some());
    assert_eq!(mac.login_item.status(), LoginItemStatus::Enabled);
    mac.claude
        .reply(StubCall::Other, Reply::ok("real claude ran\n"));
    (mac, core)
}

/// Uninstall… then its confirmation's button.
fn uninstall(core: &mut Core) -> Vec<Effect> {
    let Some(Effect::ConfirmUninstall { confirm, .. }) = core.command(Command::AskUninstall).pop()
    else {
        panic!("Uninstall… asks first");
    };
    core.command(confirm)
}

/// `claude` runs the real binary, in a terminal opened before the uninstall
/// and in one opened now, in `~/.claude` or a folder that exists.
fn assert_claude_works(mac: &FakeMac) {
    for (terminal, out) in [
        ("open", mac.run_claude_in_open_terminal(&["-p", "hi"])),
        ("new", mac.run_claude_in_new_terminal(&["-p", "hi"])),
    ] {
        assert!(
            out.status.success(),
            "{terminal} terminal: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout), "real claude ran\n");
        let call = mac.claude.calls().pop().expect("the real claude ran");
        if let Some(dir) = call.var("CLAUDE_CONFIG_DIR") {
            assert!(
                std::path::Path::new(dir).is_dir(),
                "{terminal} terminal runs in a missing Profile folder"
            );
        }
    }
}

fn logouts(mac: &FakeMac) -> usize {
    mac.claude
        .calls()
        .iter()
        .filter(|call| call.kind == StubCall::AuthLogout)
        .count()
}

#[test]
fn the_confirmation_says_what_is_removed_and_what_stays() {
    let (_mac, mut core) = installed();

    let Some(Effect::ConfirmUninstall {
        title,
        message,
        confirm,
    }) = core.command(Command::AskUninstall).pop()
    else {
        panic!("Uninstall… asks first");
    };

    assert_eq!(title, "Uninstall AccRoster?");
    assert_eq!(confirm, Command::Uninstall);
    for told in [
        "~/.zshrc",
        "2 Profiles",
        "prompt history",
        "MCP servers and folder trust added after",
        "Launch at login",
        "deleted permanently",
        "moved to the Trash",
        "Sidebar entries",
        "Folders AccRoster created in ~/.claude",
        "notification permission",
        "may not have happened",
    ] {
        assert!(
            message.contains(told),
            "doesn't mention {told:?}:\n{message}"
        );
    }
}

#[test]
fn uninstall_runs_the_steps_in_order_and_ends_by_trashing_the_app() {
    let (mac, mut core) = installed();
    mac.claude.reply(StubCall::AuthLogout, signed_out());
    let seen_at_unregister = Arc::new(Mutex::new(None));
    {
        let mac_home = mac.app_support();
        let profiles = mac.path(PROFILES);
        let zshrc = mac.path(".zshrc");
        let seen = seen_at_unregister.clone();
        mac.login_item.on_unregister(move || {
            *seen.lock().unwrap() = Some((
                std::fs::read_to_string(&zshrc).unwrap(),
                std::fs::read_dir(&profiles).unwrap().count(),
                mac_home.is_dir(),
            ));
        });
    }

    let effects = uninstall(&mut core);

    assert_eq!(
        *seen_at_unregister.lock().unwrap(),
        Some((ZSHRC.to_owned(), 0, true)),
        "launch at login goes after the CLI and the Profiles, before the folder"
    );
    assert_eq!(effects.last(), Some(&Effect::TrashAppAndQuit));
    assert_eq!(logouts(&mac), 2, "every Profile signed out");
    assert_eq!(mac.login_item.status(), LoginItemStatus::NotRegistered);
    assert!(!mac.app_support().exists(), "the app's folder is gone");
    assert_eq!(mac.read_file(".zshrc").unwrap(), ZSHRC);
    assert_claude_works(&mac);
}

#[test]
fn the_cli_is_unhooked_before_any_profile_is_signed_out() {
    let (mac, core) = installed();
    // The first sign-out hangs: the uninstall is stopped mid-way there.
    mac.claude
        .reply(StubCall::AuthLogout, Reply::hanging())
        .reply(StubCall::AuthLogout, Reply::fail(1, "Logout failed\n"));

    let running = std::thread::spawn(move || {
        let mut core = core;
        let effects = uninstall(&mut core);
        (core, effects)
    });
    let call = wait_for_hanging_logout(&mac, 1);

    assert_eq!(mac.read_file(".zshrc").unwrap(), ZSHRC, "block removed");
    assert!(mac.read_file(SHIM).is_none(), "shim removed");
    assert!(mac.read_file(POINTER).is_none(), "pointer removed");
    assert_eq!(mac.list(PROFILES).len(), 2, "no Profile deleted yet");
    assert_eq!(mac.login_item.status(), LoginItemStatus::Enabled);
    assert_claude_works(&mac);

    kill(call.pid);
    let (core, effects) = running.join().unwrap();
    assert!(!effects.contains(&Effect::TrashAppAndQuit));
    drop(core);
}

#[test]
fn a_failed_sign_out_keeps_that_folder_stops_before_the_app_folder_and_offers_retry() {
    let (mac, mut core) = installed();
    mac.claude.reply(StubCall::AuthLogout, signed_out()).reply(
        StubCall::AuthLogout,
        Reply::fail(1, "Logout failed: offline\n"),
    );

    let effects = uninstall(&mut core);

    assert!(!effects.contains(&Effect::TrashAppAndQuit));
    assert_eq!(
        mac.list(PROFILES).len(),
        1,
        "the failed one's folder is kept"
    );
    assert_eq!(rows(&core).len(), 1);
    assert_eq!(
        mac.login_item.status(),
        LoginItemStatus::Enabled,
        "nothing after the Profiles ran"
    );
    let view = core.view().settings.uninstall;
    assert_eq!(view.retry, Some(Command::Uninstall));
    let note = view.note.expect("says why it stopped");
    assert!(note.contains("me@example.com"), "{note}");
    assert!(note.contains("Retry"), "{note}");
    assert_claude_works(&mac);

    mac.claude.reply(StubCall::AuthLogout, signed_out());
    let effects = core.command(Command::Uninstall);

    assert_eq!(effects.last(), Some(&Effect::TrashAppAndQuit));
    assert_eq!(logouts(&mac), 3);
    assert!(!mac.app_support().exists());
    assert_eq!(mac.login_item.status(), LoginItemStatus::NotRegistered);
    assert_claude_works(&mac);
}

#[test]
fn an_interruption_after_any_step_leaves_claude_working_and_rerunning_finishes() {
    let (mac, core) = installed();
    let mac = Arc::new(mac);
    // Interrupted after step 1 and again after one Profile of step 2: the
    // app is killed while a sign-out runs.
    mac.claude
        .reply(StubCall::AuthLogout, signed_out())
        .reply(StubCall::AuthLogout, Reply::hanging());
    let running = std::thread::spawn(move || {
        let mut core = core;
        uninstall(&mut core);
        core
    });
    let call = wait_for_hanging_logout(&mac, 2);
    assert_eq!(mac.list(PROFILES).len(), 1, "mid-way through the Profiles");
    assert_claude_works(&mac);
    kill(call.pid);
    drop(running.join().unwrap());

    // Relaunched, uninstalled again: step 1 has nothing left to do, step 2
    // finishes. Steps 2 and 3 are seen from the moment step 3 runs.
    let checked_at_unregister = Arc::new(Mutex::new(false));
    {
        let seen = checked_at_unregister.clone();
        let mac_at_unregister = Arc::downgrade(&mac);
        mac.login_item.on_unregister(move || {
            assert_claude_works(&mac_at_unregister.upgrade().expect("the Mac is alive"));
            *seen.lock().unwrap() = true;
        });
    }
    mac.claude.reply(StubCall::AuthLogout, signed_out());
    let mut core = mac.launch();
    let effects = uninstall(&mut core);
    assert!(*checked_at_unregister.lock().unwrap());
    assert_eq!(effects.last(), Some(&Effect::TrashAppAndQuit));

    // Interrupted after step 4 (the app wasn't trashed): claude works, and
    // a relaunched app uninstalls again straight to the Trash.
    assert_claude_works(&mac);
    drop(core);
    let mut core = mac.launch();
    let before = logouts(&mac);
    let effects = uninstall(&mut core);
    assert_eq!(effects.last(), Some(&Effect::TrashAppAndQuit));
    assert_eq!(logouts(&mac), before, "no Profile left to sign out");
    assert!(!mac.app_support().exists());
    assert_claude_works(&mac);
}

#[test]
fn a_kept_duplicate_sign_in_folder_is_signed_out_and_deleted_too() {
    let (mac, mut core) = installed();
    // A duplicate sign-in whose sign-out failed: kept, never listed.
    mac.write_json(
        format!("{PROFILES}/duplicate/.claude.json"),
        &work().claude_json(),
    );
    mac.claude.reply(StubCall::AuthLogout, signed_out());

    let effects = uninstall(&mut core);

    assert_eq!(effects.last(), Some(&Effect::TrashAppAndQuit));
    let signed_out_dirs: Vec<String> = mac
        .claude
        .calls()
        .into_iter()
        .filter(|call| call.kind == StubCall::AuthLogout)
        .filter_map(|call| call.var("CLAUDE_CONFIG_DIR").map(str::to_owned))
        .collect();
    assert_eq!(signed_out_dirs.len(), 3, "{signed_out_dirs:?}");
    assert!(
        signed_out_dirs
            .iter()
            .any(|dir| dir.ends_with("/duplicate"))
    );
}

#[test]
fn an_edited_zshrc_block_is_left_and_the_user_is_told_to_remove_it() {
    let (mac, mut core) = installed();
    let zshrc = mac.read_file(".zshrc").unwrap();
    let edited = zshrc.replace("\"$PATH\"", "\"$PATH:/opt/extra\"");
    mac.write_zshrc(&edited);
    mac.claude.reply(StubCall::AuthLogout, signed_out());

    let effects = uninstall(&mut core);

    assert_eq!(effects.last(), Some(&Effect::TrashAppAndQuit));
    assert_eq!(mac.read_file(".zshrc").unwrap(), edited);
    let told = effects
        .iter()
        .any(|effect| matches!(effect, Effect::Notify { body, .. } if body.contains("/opt/extra")));
    assert!(told, "{effects:?}");
    assert_claude_works(&mac);
}

#[test]
fn after_uninstalling_the_app_writes_nothing_more() {
    let (mac, mut core) = installed();
    mac.claude.reply(StubCall::AuthLogout, signed_out());
    uninstall(&mut core);

    core.event(Event::Tick);
    core.event(Event::MenuOpened);
    core.command(Command::SetVerboseLogging(true));

    assert!(!mac.app_support().exists());
}

#[test]
fn when_the_cli_cant_be_unhooked_nothing_is_signed_out() {
    use std::os::unix::fs::PermissionsExt;
    let (mac, mut core) = installed();
    mac.claude.reply(StubCall::AuthLogout, signed_out());
    let zshrc = mac.path(".zshrc");
    std::fs::set_permissions(&zshrc, std::fs::Permissions::from_mode(0o000)).unwrap();

    let effects = uninstall(&mut core);

    std::fs::set_permissions(&zshrc, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!effects.contains(&Effect::TrashAppAndQuit));
    assert_eq!(logouts(&mac), 0);
    assert_eq!(mac.list(PROFILES).len(), 2);
    assert!(
        mac.read_file(SHIM).is_some(),
        "the shim still finds its Profile"
    );
    let view = core.view().settings.uninstall;
    assert_eq!(view.retry, Some(Command::Uninstall));
    assert_claude_works(&mac);
}

#[test]
fn trashing_the_app_without_uninstalling_leaves_claude_routing_through_the_shim() {
    let (mac, core) = installed();
    let work_dir = mac.read_file(POINTER).unwrap();

    drop(core); // The app quit and was trashed; its folder stays.

    for out in [
        mac.run_claude_in_new_terminal(&["-p", "hi"]),
        mac.run_claude_in_open_terminal(&["-p", "hi"]),
    ] {
        assert!(out.status.success());
        let call = mac.claude.calls().pop().unwrap();
        assert_eq!(call.var("CLAUDE_CONFIG_DIR"), Some(work_dir.as_str()));
    }
}

// ----- Helpers ---------------------------------------------------------------------------

/// Waits until the stub runs the `nth` sign-out (1-based), the one whose
/// reply hangs.
fn wait_for_hanging_logout(mac: &FakeMac, nth: usize) -> accroster_fake_mac::RecordedCall {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let logouts: Vec<_> = mac
            .claude
            .calls()
            .into_iter()
            .filter(|call| call.kind == StubCall::AuthLogout)
            .collect();
        if let Some(call) = logouts.get(nth - 1) {
            return call.clone();
        }
        assert!(std::time::Instant::now() < deadline, "no sign-out started");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn kill(pid: u32) {
    std::process::Command::new("/bin/kill")
        .arg(pid.to_string())
        .status()
        .expect("kill the stub");
}
