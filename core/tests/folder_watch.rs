//! The real FSEvents watch the UI layer starts for the All Sessions window.

use std::fs;
use std::sync::mpsc;
use std::time::{Duration, Instant};

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
    // The first batches can be the folders' own creation (FSEvents reports
    // recent history), and a write made before the stream is live is never
    // reported: write again until a batch names the Transcript, or give up.
    let transcript = projects.join("app").join("s1.jsonl");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen = Vec::new();
    for attempt in 0.. {
        for line in 0..3 {
            fs::write(&transcript, format!("{attempt} {line}\n")).unwrap();
        }
        let retry_at = Instant::now() + Duration::from_secs(2);
        while let Some(wait) = retry_at
            .min(deadline)
            .checked_duration_since(Instant::now())
        {
            let Ok(paths) = received.recv_timeout(wait) else {
                break;
            };
            if paths.contains(&transcript) {
                return;
            }
            seen.push(paths);
        }
        assert!(Instant::now() < deadline, "no batch named it: {seen:?}");
    }
}
