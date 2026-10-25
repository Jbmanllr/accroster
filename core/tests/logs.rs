//! The app log: written at info level, redacted when written, rotated.

use accroster_core::{Command, Event};
use accroster_fake_mac::FakeMac;
use jiff::SignedDuration;

const LOGS: &str = "Library/Application Support/io.github.jbmanllr.accroster/logs";
const APP_LOG: &str = "Library/Application Support/io.github.jbmanllr.accroster/logs/app.log";
const SETTINGS: &str = "Library/Application Support/io.github.jbmanllr.accroster/settings.json";

fn log(mac: &FakeMac) -> String {
    mac.read_file(APP_LOG).unwrap_or_default()
}

#[test]
fn launch_is_logged_at_info_level_with_home_paths_written_as_tilde() {
    let mac = FakeMac::new();

    let _core = mac.launch();

    let log = log(&mac);
    assert!(log.contains("INFO launch"), "{log}");
    assert!(
        log.contains("~/Library/Application Support/io.github.jbmanllr.accroster"),
        "{log}"
    );
    assert!(!log.contains(&*mac.home().to_string_lossy()), "{log}");
    assert!(!log.contains("DEBUG"), "info level by default: {log}");
}

#[test]
fn secrets_quoted_in_an_error_never_reach_the_log() {
    let mac = FakeMac::new();
    let email = "someone.private@example.com";
    let token = "sk-ant-oat01-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-AbCd";
    let home = mac.home().to_string_lossy().into_owned();
    // A settings file whose parse error quotes its contents.
    mac.write_file(
        SETTINGS,
        format!("{{\"version\": \"{email} {token} {home}/x\"}}"),
    );

    let _core = mac.launch();

    let log = log(&mac);
    assert!(log.contains("WARN settings set aside"), "{log}");
    assert!(
        log.contains("<email>"),
        "the error was logged, redacted: {log}"
    );
    assert!(!log.contains(email), "{log}");
    assert!(!log.contains("sk-ant"), "{log}");
    assert!(!log.contains(&home), "{log}");
}

#[test]
fn the_log_rotates_at_one_mebibyte_keeping_three_old_files() {
    let mac = FakeMac::new();
    let _ = mac.launch();
    mac.write_file(format!("{LOGS}/app.log.1"), "first old\n");
    mac.write_file(format!("{LOGS}/app.log.2"), "second old\n");
    mac.write_file(format!("{LOGS}/app.log.3"), "third old\n");
    let full = "x".repeat(1024 * 1024 - 10) + "\n";
    mac.write_file(APP_LOG, &full);

    let _core = mac.launch();

    assert_eq!(
        mac.list(LOGS),
        ["app.log", "app.log.1", "app.log.2", "app.log.3"]
    );
    assert_eq!(mac.read_file(format!("{LOGS}/app.log.1")).unwrap(), full);
    assert_eq!(
        mac.read_file(format!("{LOGS}/app.log.2")).unwrap(),
        "first old\n"
    );
    assert_eq!(
        mac.read_file(format!("{LOGS}/app.log.3")).unwrap(),
        "second old\n"
    );
    assert!(log(&mac).contains("INFO launch"));
}

#[test]
fn verbose_logging_adds_debug_lines_and_turns_itself_off_after_24_hours() {
    let mac = FakeMac::new();
    let mut core = mac.launch();

    core.command(Command::SetVerboseLogging(true));
    assert!(core.view().settings.verbose_logging);
    core.command(Command::OpenAbout);
    assert!(log(&mac).contains("DEBUG command"), "{}", log(&mac));

    // Still on after a restart, a day minus a minute later.
    mac.clock
        .advance(SignedDuration::from_hours(24) - SignedDuration::from_mins(1));
    let mut core = mac.launch();
    core.event(Event::Tick);
    assert!(core.view().settings.verbose_logging);

    mac.clock.advance(SignedDuration::from_mins(2));
    core.event(Event::Tick);
    assert!(!core.view().settings.verbose_logging, "off after 24 h");
    assert!(
        log(&mac).contains("INFO verbose logging state=off"),
        "{}",
        log(&mac)
    );
    let lines_before = log(&mac).lines().count();
    core.command(Command::OpenAbout);
    assert_eq!(
        log(&mac).lines().count(),
        lines_before,
        "no more debug lines"
    );

    let relaunched = mac.launch();
    assert!(
        !relaunched.view().settings.verbose_logging,
        "the switch stays off"
    );
}
