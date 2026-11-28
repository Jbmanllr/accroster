//! A file as `stat` sees it: the one answer to "has this file changed since
//! I read it" for every cache of Claude's files (the Session index, the
//! Discovered Profiles, Sidebar sync and the self-check).

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// A file's identity and state. Claude and sync replace a file by renaming
/// a new one over it, which changes the inode; the change time also catches
/// an edit in place whose modification time was put back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    inode: u64,
    len: u64,
    modified: SystemTime,
    changed: (i64, i64),
}

impl Stamp {
    /// `None` where the file system keeps no modification time.
    pub(crate) fn of(meta: &fs::Metadata) -> Option<Stamp> {
        Some(Stamp {
            inode: meta.ino(),
            len: meta.len(),
            modified: meta.modified().ok()?,
            changed: (meta.ctime(), meta.ctime_nsec()),
        })
    }

    /// The stamp of a regular file that isn't a link.
    pub(crate) fn of_regular_file(path: &Path) -> Option<Stamp> {
        let meta = fs::symlink_metadata(path).ok()?;
        meta.file_type()
            .is_file()
            .then(|| Stamp::of(&meta))
            .flatten()
    }

    pub(crate) fn len(&self) -> u64 {
        self.len
    }

    pub(crate) fn modified(&self) -> SystemTime {
        self.modified
    }

    /// The modification time in nanoseconds since 1970, the form the
    /// Session index saves; 0 for a time before then.
    pub(crate) fn modified_ns(&self) -> u64 {
        self.modified
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_replaced_by_one_of_the_same_size_and_time_has_another_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("entry.json");
        fs::write(&file, b"one").unwrap();
        let before = Stamp::of_regular_file(&file).unwrap();
        assert_eq!(Stamp::of_regular_file(&file), Some(before));

        let replacement = dir.path().join("entry.json.tmp");
        fs::write(&replacement, b"two").unwrap();
        fs::File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_modified(before.modified())
            .unwrap();
        fs::rename(&replacement, &file).unwrap();

        let after = Stamp::of_regular_file(&file).unwrap();
        assert_eq!(after.len(), before.len());
        assert_eq!(after.modified_ns(), before.modified_ns());
        assert_ne!(after, before);
    }

    #[test]
    fn a_link_and_a_folder_have_no_regular_file_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("real");
        fs::write(&file, b"x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();

        assert!(Stamp::of_regular_file(&link).is_none());
        assert!(Stamp::of_regular_file(dir.path()).is_none());
        assert!(Stamp::of_regular_file(&dir.path().join("missing")).is_none());
    }
}
