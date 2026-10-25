//! The real FSEvents watch the UI layer starts for the All Sessions window.

use std::fs;
use std::sync::mpsc;
use std::time::Duration;

use accroster_core::watcher::FolderWatch;

#[test]
fn a_change_in_a_watched_folder_is_reported_debounced_under_the_named_path() {
    let root = tempfile::tempdir().unwrap();
    let projects = root.path().join("projects");
    fs::create_dir_all(projects.join("app")).unwrap();
    let missing = root.path().join("not there");
    let (sender, received) = mpsc::channel();

    let (_watch, skipped) = FolderWatch::start(
        &[projects.clone(), missing.clone()],
        Duration::from_millis(200),
        move |paths| sender.send(paths).unwrap(),
    )
    .unwrap();
    assert_eq!(
        skipped
            .into_iter()
            .map(|(path, _)| path)
            .collect::<Vec<_>>(),
        [missing]
    );
    // FSEvents starts reporting shortly after the stream is created.
    std::thread::sleep(Duration::from_millis(300));
    let transcript = projects.join("app").join("s1.jsonl");
    for line in 0..3 {
        fs::write(&transcript, format!("{line}\n")).unwrap();
    }

    let paths = received
        .recv_timeout(Duration::from_secs(10))
        .expect("a batch of changes");
    assert!(paths.contains(&transcript), "{paths:?}");
}
