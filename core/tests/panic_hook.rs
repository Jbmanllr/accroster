//! The panic hook writes the panic to the log, redacted, before the process
//! exits. Its own test binary: the hook is process-wide.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use accroster_fake_mac::FakeMac;

#[test]
fn a_panic_is_written_to_the_log_redacted_and_nowhere_else_raw() {
    let mac = FakeMac::new();
    let _core = mac.launch();
    // A hook installed before the app's: it would print the raw payload.
    let raw_hook_ran = Arc::new(AtomicBool::new(false));
    {
        let ran = raw_hook_ran.clone();
        std::panic::set_hook(Box::new(move |_| ran.store(true, Ordering::SeqCst)));
    }
    accroster_core::install_panic_hook(mac.home());
    let home = mac.home().to_string_lossy().into_owned();

    let result = std::panic::catch_unwind(|| {
        panic!(
            "boom reading {home}/Dev/secret-client/notes.json titled \"Plan the secret launch\" for someone@example.com with sk-ant-oat01-ZYXWVUTSRQPONMLKJIHGFEDCBA9876543210"
        );
    });

    assert!(result.is_err());
    assert!(
        !raw_hook_ran.load(Ordering::SeqCst),
        "the raw payload never reaches stderr through the previous hook"
    );
    let log = mac
        .read_file("Library/Application Support/io.github.jbmanllr.accroster/logs/app.log")
        .unwrap();
    let panic_line = log
        .lines()
        .find(|line| line.contains("ERROR panic"))
        .unwrap_or_else(|| panic!("panic logged: {log}"));
    assert!(
        panic_line.contains("boom reading <path> titled <text> for <email> with <token>"),
        "{panic_line}"
    );
    assert!(
        panic_line.contains("panic_hook.rs"),
        "location logged: {panic_line}"
    );
    for private in [home.as_str(), "secret-client", "secret launch"] {
        assert!(!panic_line.contains(private), "{private}: {panic_line}");
    }
}
