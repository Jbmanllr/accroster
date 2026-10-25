//! AccRoster: a menu bar app for Claude Code accounts.
//!
//! This binary is the thin UI layer. It builds the real [`Environment`],
//! launches the core and hands it to [`ui`]. All behaviour is in
//! `accroster-core`.

mod ui;

use accroster_core::{Core, Environment, install_panic_hook};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

fn main() {
    let mtm = MainThreadMarker::new().expect("AccRoster starts on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    // A menu bar app: no Dock icon, no main menu.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let launched = Environment::system()
        .map_err(|error| error.to_string())
        .and_then(|env| {
            install_panic_hook(&env.home);
            Core::launch(env).map_err(|error| error.to_string())
        });
    match launched {
        Ok(core) => {
            ui::start(core, mtm);
            app.run();
        }
        Err(message) => {
            ui::show_fatal_error(mtm, &message);
            std::process::exit(1);
        }
    }
}
