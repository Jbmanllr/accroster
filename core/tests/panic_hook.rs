//! The panic hook writes the panic to the log, redacted, before the process
//! exits. Its own test binary: the hook is process-wide.

use accroster_fake_mac::FakeMac;

#[test]
fn a_panic_is_written_to_the_log_redacted() {
    let mac = FakeMac::new();
    let _core = mac.launch();
    accroster_core::install_panic_hook(mac.home());
    let home = mac.home().to_string_lossy().into_owned();

    let result = std::panic::catch_unwind(|| {
        panic!(
            "boom reading {home}/.claude.json for someone@example.com with sk-ant-oat01-ZYXWVUTSRQPONMLKJIHGFEDCBA9876543210"
        );
    });

    assert!(result.is_err());
    let log = mac
        .read_file("Library/Application Support/io.github.jbmanllr.accroster/logs/app.log")
        .unwrap();
    let panic_line = log
        .lines()
        .find(|line| line.contains("ERROR panic"))
        .unwrap_or_else(|| panic!("panic logged: {log}"));
    assert!(
        panic_line.contains("boom reading ~/.claude.json for <email> with <token>"),
        "{panic_line}"
    );
    assert!(
        panic_line.contains("panic_hook.rs"),
        "location logged: {panic_line}"
    );
    assert!(!panic_line.contains(&home), "{panic_line}");
}
