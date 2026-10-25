//! The minimal menu (Add Profile…, Settings…, About, Quit), the Settings window and About.
//!
//! This file is also the worked example of a fake-Mac test: set up the Mac,
//! launch the core, click through the view model, and assert on what the
//! user would see and on the files left alone.

use accroster_core::{Command, Effect, MenuItem};
use accroster_fake_mac::{FakeMac, FixtureLogin};

const APP_FOLDER: &str = "Library/Application Support/io.github.jbmanllr.accroster";

/// Clicks the menu item titled `title`, the way the UI layer does: by
/// sending the command the view model attached to it.
fn click(core: &mut accroster_core::Core, title: &str) -> Vec<Effect> {
    let command = core
        .view()
        .menu
        .items
        .into_iter()
        .find_map(|item| match item {
            MenuItem::Action { title: t, command } if t == title => Some(command),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the menu has {title}"));
    core.command(command)
}

fn titles(core: &accroster_core::Core) -> Vec<String> {
    core.view()
        .menu
        .items
        .into_iter()
        .filter_map(|item| match item {
            MenuItem::Action { title, .. } => Some(title),
            _ => None,
        })
        .collect()
}

#[test]
fn the_menu_offers_add_profile_settings_diagnostics_about_and_quit() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.write_zshrc("export PATH=\"$HOME/.local/bin:$PATH\"\n");
    let before = mac.snapshot();

    let mut core = mac.launch();

    assert_eq!(
        titles(&core),
        ["Add Profile…", "Settings…", "Diagnostics…", "About", "Quit"]
    );
    assert_eq!(click(&mut core, "Settings…"), [Effect::ShowSettings]);
    assert_eq!(click(&mut core, "Diagnostics…"), [Effect::ShowDiagnostics]);
    assert_eq!(click(&mut core, "About"), [Effect::ShowAbout]);
    assert_eq!(click(&mut core, "Quit"), [Effect::Terminate]);

    // Launching touched nothing outside the app's own folder and ran no claude.
    let after = mac.snapshot();
    let outside = |snapshot: &std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>| {
        snapshot
            .iter()
            .filter(|(path, _)| !path.starts_with(APP_FOLDER))
            .map(|(path, bytes)| (path.clone(), bytes.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(outside(&after), outside(&before));
    assert!(mac.claude.calls().is_empty());
}

#[test]
fn settings_offers_verbose_logging_and_reveals_the_logs_in_finder() {
    let mac = FakeMac::new();
    let mut core = mac.launch();

    let settings = core.view().settings;
    assert!(!settings.verbose_logging);
    assert_eq!(
        settings.verbose_logging_note,
        "Turns itself off after 24 hours."
    );

    core.command(Command::SetVerboseLogging(true));
    assert!(core.view().settings.verbose_logging);
    core.command(Command::SetVerboseLogging(false));
    assert!(!core.view().settings.verbose_logging);

    assert_eq!(
        core.command(Command::RevealLogs),
        [Effect::RevealInFinder(
            mac.path(format!("{APP_FOLDER}/logs/app.log"))
        )]
    );
}

#[test]
fn about_shows_the_tagline_and_the_non_affiliation_disclaimer() {
    let mac = FakeMac::new();
    let core = mac.launch();

    let about = core.view().about;

    assert_eq!(about.name, "AccRoster");
    assert_eq!(
        about.tagline,
        "A menu bar app for Claude Code accounts: limits, sessions and one-click switching."
    );
    assert_eq!(
        about.disclaimer,
        "AccRoster is an independent open-source project. It is not affiliated with, endorsed by, or sponsored by Anthropic. Claude and Claude Code are trademarks of Anthropic, PBC."
    );
}
