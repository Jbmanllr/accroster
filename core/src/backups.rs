//! Backups: a copy of a file taken right before the app writes it, so every
//! change to a file the app doesn't own can be undone by hand.
//!
//! Layout, inside the app's `backups/` folder:
//!
//! ```text
//! backups/<file name>-<8-hex hash of its absolute path>/
//!     source.json                    {"version": 1, "path": "<absolute path>"}
//!     20261025T140200.123Z.bak       one copy per backup, named by when it was taken
//! ```
//!
//! Each file keeps its last [`KEEP_LAST`] backups plus any younger than
//! [`KEEP_YOUNGER_THAN`]; [`prune`] enforces that at launch. `.zshrc`
//! backups are never pruned.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::logging::short_hash;
use crate::storage::{self, VersionedFile};

/// How many backups of each file are always kept.
pub(crate) const KEEP_LAST: usize = 5;
/// Backups younger than this are kept whatever their number.
pub(crate) const KEEP_YOUNGER_THAN: SignedDuration = SignedDuration::from_hours(30 * 24);

const STAMP_FORMAT: &str = "%Y%m%dT%H%M%S%.3fZ";
const STAMP_PARSE_FORMAT: &str = "%Y%m%dT%H%M%S%.fZ";
const NEVER_PRUNED: &str = ".zshrc";

/// Where a group's backups came from.
#[derive(Debug, Serialize, Deserialize)]
struct Source {
    path: PathBuf,
}

impl VersionedFile for Source {
    const VERSION: u64 = 1;
}

/// Copies `file` into `backups/` and returns the copy's path. Call it right
/// before writing `file`; if it fails, don't write.
pub(crate) fn back_up(backups: &Path, file: &Path, now: Timestamp) -> io::Result<PathBuf> {
    let name = file
        .file_name()
        .ok_or_else(|| io::Error::other("a backed-up file has a name"))?
        .to_string_lossy()
        .into_owned();
    let group = backups.join(format!("{name}-{}", short_hash(&file.to_string_lossy())));
    fs::create_dir_all(&group)?;
    let source = group.join("source.json");
    if !source.exists() {
        storage::write_versioned(
            &source,
            &Source {
                path: file.to_path_buf(),
            },
        )?;
    }
    let stamp = now.strftime(STAMP_FORMAT).to_string();
    let mut copy = group.join(format!("{stamp}.bak"));
    let mut n = 2;
    while copy.exists() {
        copy = group.join(format!("{stamp}-{n}.bak"));
        n += 1;
    }
    fs::copy(file, &copy)?;
    Ok(copy)
}

/// Deletes the backups the keep rule no longer protects, and returns how
/// many were deleted. Files whose names aren't backup names are left alone.
pub(crate) fn prune(backups: &Path, now: Timestamp) -> io::Result<usize> {
    let mut deleted = 0;
    for group in fs::read_dir(backups)? {
        let group = group?;
        if !group.file_type()?.is_dir() || is_never_pruned(&group.file_name().to_string_lossy()) {
            continue;
        }
        let mut copies: Vec<(Timestamp, PathBuf)> = fs::read_dir(group.path())?
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                let taken = taken_at(&path)?;
                Some((taken, path))
            })
            .collect();
        copies.sort_by(|a, b| b.0.cmp(&a.0));
        for (taken, path) in copies.into_iter().skip(KEEP_LAST) {
            if now.duration_since(taken) >= KEEP_YOUNGER_THAN {
                fs::remove_file(path)?;
                deleted += 1;
            }
        }
    }
    Ok(deleted)
}

fn is_never_pruned(group: &str) -> bool {
    group
        .strip_prefix(NEVER_PRUNED)
        .is_some_and(|rest| rest.starts_with('-'))
}

/// When a backup was taken, from its name (`<stamp>.bak` or `<stamp>-<n>.bak`).
fn taken_at(path: &Path) -> Option<Timestamp> {
    let name = path.file_name()?.to_str()?.strip_suffix(".bak")?;
    let stamp = match name.split_once('-') {
        Some((stamp, _)) => stamp,
        None => name,
    };
    let civil = DateTime::strptime(STAMP_PARSE_FORMAT, stamp).ok()?;
    TimeZone::UTC.to_timestamp(civil).ok()
}

#[cfg(test)]
mod tests {
    //! The helper later tickets call before writing a Claude file. Pruning is
    //! tested through the core's interface in `tests/backups.rs`.
    use super::*;

    #[test]
    fn a_backup_is_an_exact_copy_named_by_its_time_and_survives_pruning() {
        let dir =
            std::env::temp_dir().join(format!("accroster-backup-test-{}", std::process::id()));
        let backups = dir.join("backups");
        fs::create_dir_all(&backups).unwrap();
        let file = dir.join(".zshrc");
        fs::write(&file, "export PATH=x\n").unwrap();
        let now: Timestamp = "2026-10-25T14:02:00.5Z".parse().unwrap();

        let first = back_up(&backups, &file, now).unwrap();
        let second = back_up(&backups, &file, now).unwrap();

        assert_eq!(fs::read_to_string(&first).unwrap(), "export PATH=x\n");
        assert!(first.ends_with("20261025T140200.500Z.bak"), "{first:?}");
        assert!(second.ends_with("20261025T140200.500Z-2.bak"), "{second:?}");
        assert_eq!(taken_at(&second), Some(now));
        let group = first.parent().unwrap();
        assert!(
            group
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".zshrc-")
        );
        let much_later = now + SignedDuration::from_hours(24 * 1000);
        assert_eq!(prune(&backups, much_later).unwrap(), 0);
        fs::remove_dir_all(dir).unwrap();
    }
}
