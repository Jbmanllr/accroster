//! Launch at login: offered once on first run (ticked), a Settings toggle,
//! and respect for what the user set in System Settings → Login Items.

use accroster_core::{Command, Effect, LoginItem, LoginItemStatus};
use accroster_fake_mac::FakeMac;

const SETTINGS: &str = "Library/Application Support/io.github.jbmanllr.accroster/settings.json";

fn saved_choice(mac: &FakeMac) -> serde_json::Value {
    mac.read_json(SETTINGS).expect("settings.json exists")["launchAtLogin"].clone()
}

#[test]
fn first_run_offers_launch_at_login_ticked_and_accepting_registers_the_app() {
    let mac = FakeMac::new();
    let mut core = mac.launch();

    let offer = core.view().first_run.expect("first run shows the offer");
    assert!(offer.launch_at_login, "the offer is ticked by default");

    assert_eq!(core.command(Command::SetLaunchAtLogin(true)), []);

    assert_eq!(mac.login_item.status(), LoginItemStatus::Enabled);
    assert_eq!(saved_choice(&mac), true);
    assert_eq!(core.view().first_run, None);
    assert!(core.view().settings.launch_at_login.on);
}

#[test]
fn declining_the_offer_registers_nothing_and_it_is_never_offered_again() {
    let mac = FakeMac::new();
    let mut core = mac.launch();

    core.command(Command::SetLaunchAtLogin(false));

    assert_eq!(mac.login_item.registrations(), 0);
    assert_eq!(saved_choice(&mac), false);
    assert_eq!(core.view().first_run, None);
    assert!(!core.view().settings.launch_at_login.on);
    drop(core);
    assert_eq!(mac.launch().view().first_run, None);
}

#[test]
fn the_settings_toggle_turns_launch_at_login_off_and_on_and_the_choice_survives_a_restart() {
    let mac = FakeMac::new();
    let mut core = mac.launch();
    core.command(Command::SetLaunchAtLogin(true));

    assert_eq!(core.command(Command::SetLaunchAtLogin(false)), []);
    assert_eq!(mac.login_item.status(), LoginItemStatus::NotRegistered);
    assert_eq!(saved_choice(&mac), false);
    assert!(!core.view().settings.launch_at_login.on);

    core.command(Command::SetLaunchAtLogin(true));
    drop(core);
    let core = mac.launch();
    assert_eq!(saved_choice(&mac), true);
    let toggle = core.view().settings.launch_at_login;
    assert!(toggle.on && toggle.enabled);
    assert_eq!(toggle.note, None);
}

#[test]
fn turned_off_in_system_settings_shows_off_and_the_app_never_registers_itself_again() {
    let mac = FakeMac::new();
    let mut core = mac.launch();
    core.command(Command::SetLaunchAtLogin(true));
    drop(core);

    mac.login_item.turn_off_in_system_settings();
    let mut core = mac.launch();

    let toggle = core.view().settings.launch_at_login;
    assert!(!toggle.on, "the toggle shows what System Settings says");
    assert!(
        toggle
            .note
            .expect("a note explains why")
            .contains("System Settings → Login Items")
    );
    assert_eq!(core.view().first_run, None);
    core.event(accroster_core::Event::Tick);

    // Turning it back on in the app sends the user to System Settings.
    assert_eq!(
        core.command(Command::SetLaunchAtLogin(true)),
        [Effect::OpenLoginItemsSettings]
    );
    assert_eq!(
        mac.login_item.registrations(),
        1,
        "only the first-run registration"
    );
    assert_eq!(mac.login_item.status(), LoginItemStatus::RequiresApproval);
}

#[test]
fn a_first_run_after_the_user_turned_it_off_in_system_settings_offers_nothing() {
    let mac = FakeMac::new();
    mac.login_item.turn_off_in_system_settings();

    let core = mac.launch();

    assert_eq!(core.view().first_run, None);
    assert!(!core.view().settings.launch_at_login.on);
    assert_eq!(mac.login_item.registrations(), 0);
}

#[test]
fn when_macos_cant_find_this_copy_the_toggle_is_unavailable_and_nothing_is_offered() {
    let mac = FakeMac::new();
    mac.login_item.set_status(LoginItemStatus::NotFound);

    let core = mac.launch();

    assert_eq!(core.view().first_run, None);
    let toggle = core.view().settings.launch_at_login;
    assert!(!toggle.on && !toggle.enabled);
    assert_eq!(
        toggle.note.as_deref(),
        Some(
            "macOS can't offer this for this copy of AccRoster: it needs a signed copy, \
             run from its app bundle."
        ),
        "true for an ad-hoc signed bundle as much as for a bare binary"
    );
}

#[test]
fn a_refused_registration_leaves_the_toggle_off_and_is_logged() {
    let mac = FakeMac::new();
    mac.login_item
        .refuse_registration("Operation not permitted");
    let mut core = mac.launch();

    core.command(Command::SetLaunchAtLogin(true));

    assert!(!core.view().settings.launch_at_login.on);
    assert_eq!(core.view().first_run, None, "the offer was answered");
    let log = mac
        .read_file("Library/Application Support/io.github.jbmanllr.accroster/logs/app.log")
        .expect("the app log exists");
    assert!(log.contains("login item not changed"));
    assert!(log.contains("Operation not permitted"));
}
