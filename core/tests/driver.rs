//! The driver runs the core off the UI thread and reports every update.

use std::sync::mpsc;
use std::time::Duration;

use accroster_core::driver;
use accroster_core::{Command, Effect};
use accroster_fake_mac::FakeMac;

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
