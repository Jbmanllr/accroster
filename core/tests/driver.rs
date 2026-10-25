//! The driver runs the core off the UI thread and reports every update.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use accroster_core::driver;
use accroster_core::{Command, Effect, MenuItem};
use accroster_fake_mac::{FakeMac, FixtureLogin, Reply, StubCall};

#[test]
fn the_driver_reports_the_initial_view_then_each_commands_view_and_effects() {
    let mac = FakeMac::new();
    let (updates_tx, updates) = mpsc::channel();
    let handle = driver::start(mac.launch(), move |update| {
        let _ = updates_tx.send(update);
    });
    let next = || {
        updates
            .recv_timeout(Duration::from_secs(5))
            .expect("an update")
    };

    let initial = next();
    assert!(initial.effects.is_empty());
    assert!(!initial.view.settings.verbose_logging);

    handle.command(Command::SetVerboseLogging(true));
    let after_switch = next();
    assert!(after_switch.view.settings.verbose_logging);

    handle.command(Command::OpenSettings);
    assert_eq!(next().effects, [Effect::ShowSettings]);
}

#[test]
fn the_end_of_a_sign_in_shows_within_seconds_without_waiting_for_the_next_tick() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.claude
        .reply(
            StubCall::AuthLogin,
            Reply::ok("Login successful.\n").writing(
                ".claude.json",
                FixtureLogin::perso().claude_json().to_string(),
            ),
        )
        .reply(StubCall::AuthStatus, Reply::fail(1, ""))
        .reply(StubCall::GetUsage, Reply::fail(1, ""))
        .reply(StubCall::SlashUsage, Reply::fail(1, ""));
    let (updates_tx, updates) = mpsc::channel();
    let handle = driver::start(mac.launch(), move |update| {
        let _ = updates_tx.send(update);
    });
    let has_row = |update: &driver::Update| {
        update
            .view
            .menu
            .items
            .iter()
            .any(|item| matches!(item, MenuItem::Profile(_)))
    };
    updates
        .recv_timeout(Duration::from_secs(5))
        .expect("initial view");

    handle.command(Command::AddProfile);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let update = updates
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("the new Profile shows within seconds");
        if has_row(&update) {
            break;
        }
    }
}
