//! Backups are pruned at launch: each file keeps its last 5 backups plus any
//! under 30 days old, a Sidebar folder only its last 2; `.zshrc` backups are
//! never pruned.

use accroster_fake_mac::FakeMac;
use jiff::{SignedDuration, Timestamp};

const BACKUPS: &str = "Library/Application Support/io.github.jbmanllr.accroster/backups";

/// A backup file name for a copy taken `days` before the fake Mac's start time.
fn taken_days_ago(days: i64) -> String {
    let start: Timestamp = accroster_fake_mac::START_TIME.parse().unwrap();
    let at = start - SignedDuration::from_hours(24 * days);
    format!("{}.bak", at.strftime("%Y%m%dT%H%M%S%.3fZ"))
}

fn backed_up(mac: &FakeMac, group: &str, days: &[i64]) {
    for day in days {
        mac.write_file(
            format!("{BACKUPS}/{group}/{}", taken_days_ago(*day)),
            "copy",
        );
    }
}

fn kept(mac: &FakeMac, group: &str) -> Vec<String> {
    mac.list(format!("{BACKUPS}/{group}"))
        .into_iter()
        .filter(|name| name.ends_with(".bak"))
        .collect()
}

fn names(days: &[i64]) -> Vec<String> {
    let mut names: Vec<String> = days.iter().map(|day| taken_days_ago(*day)).collect();
    names.sort();
    names
}

#[test]
fn launch_keeps_each_files_last_five_backups_plus_any_under_30_days_old() {
    let mac = FakeMac::new();
    backed_up(
        &mac,
        "settings.json-0a1b2c3d",
        &[1, 2, 3, 4, 5, 6, 100, 200],
    );
    backed_up(&mac, ".claude.json-9f8e7d6c", &[40, 50, 60, 70, 80, 90]);
    backed_up(&mac, "config.json-11223344", &[31, 400]);

    let _core = mac.launch();

    assert_eq!(
        kept(&mac, "settings.json-0a1b2c3d"),
        names(&[1, 2, 3, 4, 5, 6])
    );
    assert_eq!(
        kept(&mac, ".claude.json-9f8e7d6c"),
        names(&[40, 50, 60, 70, 80])
    );
    assert_eq!(kept(&mac, "config.json-11223344"), names(&[31, 400]));
}

#[test]
fn zshrc_backups_are_never_pruned() {
    let mac = FakeMac::new();
    let days = [100, 200, 300, 400, 500, 600, 700];
    backed_up(&mac, ".zshrc-5e6f7a8b", &days);

    let _core = mac.launch();

    assert_eq!(kept(&mac, ".zshrc-5e6f7a8b"), names(&days));
}

fn backed_up_folder(mac: &FakeMac, group: &str, days: &[i64]) {
    for day in days {
        mac.write_file(
            format!("{BACKUPS}/{group}/{}/local_x.json", taken_days_ago(*day)),
            "{}",
        );
    }
}

#[test]
fn launch_keeps_only_a_sidebar_folders_last_two_backups_whatever_their_age() {
    let mac = FakeMac::new();
    backed_up_folder(&mac, "0e000f92-abcd-a7d62cfd", &[1, 2, 3, 4]);

    let _core = mac.launch();

    assert_eq!(kept(&mac, "0e000f92-abcd-a7d62cfd"), names(&[1, 2]));
}
