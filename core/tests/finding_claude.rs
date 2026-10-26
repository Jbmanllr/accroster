//! Finding Claude Code (`claude`) when AccRoster was opened from Finder or
//! the Dock: launchd's minimal `PATH` has no `claude`, so the usual install
//! folders and the user's login shell are searched, never the desktop app's
//! bundled copy or AccRoster's own shim.

mod common;

use accroster_core::{Core, Event};
use accroster_fake_mac::{FakeMac, StubCall};
use common::*;

const MISSING: &str =
    "Can't find Claude Code (claude). Install it, or open AccRoster from a terminal once.";

/// A Mac with one Profile signed in as `work()` (added while the stub was
/// on `PATH`), then relaunched from Finder.
fn profile_then_finder(setup: impl FnOnce(&FakeMac)) -> (FakeMac, Core) {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);
    assert_eq!(rows(&core).len(), 1);
    drop(core);
    // Forget where the first launch found the stub: these tests are about
    // a Mac where AccRoster was never opened from a terminal.
    let _ = std::fs::remove_file(mac.path(format!("{APP_FOLDER}/state/claude-path")));
    mac.open_from_finder();
    setup(&mac);
    let core = mac.launch();
    (mac, core)
}

/// Ticks until the core has nothing in flight, then forces a usage read by
/// opening the menu, and waits for it.
fn read_limits(mac: &FakeMac, core: &mut Core) {
    settle(core);
    mac.clock.advance(jiff::SignedDuration::from_mins(10));
    core.event(Event::MenuOpened);
    settle(core);
}

fn log(mac: &FakeMac) -> String {
    mac.read_file(format!("{APP_FOLDER}/logs/app.log"))
        .unwrap_or_default()
}

#[test]
fn opened_from_finder_it_finds_claude_in_local_bin_and_puts_its_folder_first_on_path() {
    let (mac, mut core) = profile_then_finder(|mac| {
        mac.install_claude_in(".local/bin");
    });
    let reads_before = usage_reads(&mac);

    read_limits(&mac, &mut core);

    let calls = wait_for_calls(&mac, StubCall::GetUsage, reads_before + 1);
    let path = calls.last().unwrap().var("PATH").unwrap().to_owned();
    let local_bin = mac.path(".local/bin");
    assert!(
        path.starts_with(&format!("{}:", local_bin.display())),
        "Claude Code finds node and git next to it: {path}"
    );
    assert!(path.ends_with("/usr/bin:/bin:/usr/sbin:/sbin"), "{path}");
    assert!(!warnings(&core).contains(&MISSING.to_owned()));
    assert!(
        !log(&mac).contains("usage read not started"),
        "{}",
        log(&mac)
    );
}

#[test]
fn the_login_shells_path_is_searched_when_no_usual_folder_has_claude() {
    let (mac, mut core) = profile_then_finder(|mac| {
        let installed = mac.install_claude_in(".nvm/versions/node/v22/bin");
        let folder = installed.parent().unwrap().display().to_string();
        mac.set_login_shell_path(&format!("/usr/bin:{folder}:/bin"));
    });
    let reads_before = usage_reads(&mac);

    read_limits(&mac, &mut core);

    let calls = wait_for_calls(&mac, StubCall::GetUsage, reads_before + 1);
    let path = calls.last().unwrap().var("PATH").unwrap().to_owned();
    assert!(path.starts_with(&format!(
        "{}:",
        mac.path(".nvm/versions/node/v22/bin").display()
    )));
    assert!(!warnings(&core).contains(&MISSING.to_owned()));
}

#[test]
fn without_claude_anywhere_the_menu_says_so_once_instead_of_logging_every_read() {
    let (mac, mut core) = profile_then_finder(|_| {});
    let calls_before = mac.claude.calls().len();

    read_limits(&mac, &mut core);
    read_limits(&mac, &mut core);

    let missing: Vec<String> = warnings(&core)
        .into_iter()
        .filter(|line| line == MISSING)
        .collect();
    assert_eq!(missing.len(), 1, "{:?}", core.view().menu.items);
    assert_eq!(mac.claude.calls().len(), calls_before, "nothing ran");
    let log = log(&mac);
    assert!(!log.contains("usage read not started"), "{log}");
    assert_eq!(log.matches("claude not found").count(), 1, "{log}");
}

#[test]
fn claude_installed_later_is_found_without_a_relaunch() {
    let (mac, mut core) = profile_then_finder(|_| {});
    read_limits(&mac, &mut core);
    assert!(warnings(&core).contains(&MISSING.to_owned()));
    let reads_before = usage_reads(&mac);

    mac.install_claude_in(".local/bin");
    read_limits(&mac, &mut core);

    assert!(!warnings(&core).contains(&MISSING.to_owned()));
    wait_for_calls(&mac, StubCall::GetUsage, reads_before + 1);
}

#[test]
fn accroster_s_own_shim_and_the_desktop_apps_bundled_copy_are_never_used() {
    let (mac, mut core) = profile_then_finder(|mac| {
        // The desktop app's bundled Claude Code, linked from ~/.local/bin.
        let bundled = mac.install_claude_in(
            "Library/Application Support/Claude/claude-code/2.1.308/abc/claude.app/Contents/MacOS",
        );
        std::fs::create_dir_all(mac.path(".local/bin")).unwrap();
        std::os::unix::fs::symlink(&bundled, mac.path(".local/bin/claude")).unwrap();
        // AccRoster's shim, found through the login shell.
        let shim = mac.install_claude_in(format!("{APP_FOLDER}/bin"));
        mac.set_login_shell_path(&format!(
            "{}:/usr/bin:/bin",
            shim.parent().unwrap().display()
        ));
    });
    let calls_before = mac.claude.calls().len();

    read_limits(&mac, &mut core);

    assert!(warnings(&core).contains(&MISSING.to_owned()));
    assert_eq!(mac.claude.calls().len(), calls_before, "nothing ran");
}

#[test]
fn opened_from_a_terminal_once_later_finder_launches_still_find_claude() {
    // The first launch inherits a terminal's PATH, where the stub `claude`
    // sits in a folder none of the usual places name.
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);
    drop(core);

    mac.open_from_finder();
    let mut core = mac.launch();
    let reads_before = usage_reads(&mac);
    read_limits(&mac, &mut core);

    assert!(!warnings(&core).contains(&MISSING.to_owned()));
    wait_for_calls(&mac, StubCall::GetUsage, reads_before + 1);
}
