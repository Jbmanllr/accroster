//! App storage: the app's own folder and the one way files are written to it.
//!
//! Everything AccRoster owns lives in
//! `~/Library/Application Support/io.github.jbmanllr.accroster/`. No
//! `NSUserDefaults`. Every file is JSON with a `"version"` field and is
//! written temp-file-then-rename, so a crash never leaves half a file. The
//! path contains a space: Rust APIs take it as a [`Path`]; anything that ends
//! up in a shell line must quote it.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The bundle identifier, also the name of the app's folder. Never changes.
pub const BUNDLE_ID: &str = "io.github.jbmanllr.accroster";

/// Where every app file lives, derived from the home-folder root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    root: PathBuf,
}

impl AppPaths {
    pub fn new(home: &Path) -> AppPaths {
        AppPaths {
            root: home
                .join("Library")
                .join("Application Support")
                .join(BUNDLE_ID),
        }
    }

    /// `~/Library/Application Support/io.github.jbmanllr.accroster`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `settings.json`: the user's choices.
    pub fn settings(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    /// `state/`: what the app remembers between runs (backoff, sync record, …).
    pub fn state(&self) -> PathBuf {
        self.root.join("state")
    }

    /// `index/`: the disposable Session index.
    pub fn index(&self) -> PathBuf {
        self.root.join("index")
    }

    /// `profiles/`: one folder per Profile.
    pub fn profiles(&self) -> PathBuf {
        self.root.join("profiles")
    }

    /// `bin/`: the `claude` shim and the CLI pointer.
    pub fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }

    /// `logs/`: `app.log` and its rotated copies.
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// `logs/app.log`.
    pub fn app_log(&self) -> PathBuf {
        self.logs().join("app.log")
    }

    /// `backups/`: copies taken before writing a file the app doesn't own.
    pub fn backups(&self) -> PathBuf {
        self.root.join("backups")
    }

    /// Creates the folder and its subfolders if missing.
    pub(crate) fn ensure_layout(&self) -> io::Result<()> {
        for folder in [
            self.state(),
            self.index(),
            self.profiles(),
            self.bin(),
            self.logs(),
            self.backups(),
        ] {
            fs::create_dir_all(&folder)?;
        }
        Ok(())
    }
}

/// A JSON file the app owns. Its `"version"` field is written and checked by
/// [`write_versioned`] and [`read_versioned`]; the type itself never carries it.
pub(crate) trait VersionedFile: Serialize + DeserializeOwned {
    const VERSION: u64;
}

/// Why a versioned file couldn't be read.
#[derive(Debug)]
pub(crate) enum ReadError {
    Io(io::Error),
    /// Not JSON, not an object, the wrong `"version"`, or the wrong shape.
    Unparsable(String),
}

/// Reads a versioned file; `Ok(None)` when it doesn't exist.
pub(crate) fn read_versioned<T: VersionedFile>(path: &Path) -> Result<Option<T>, ReadError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ReadError::Io(error)),
    };
    let mut value: Value =
        serde_json::from_slice(&bytes).map_err(|error| ReadError::Unparsable(error.to_string()))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| ReadError::Unparsable("not a JSON object".into()))?;
    match object.remove("version") {
        Some(version) if version.as_u64() == Some(T::VERSION) => {}
        Some(version) => {
            return Err(ReadError::Unparsable(format!(
                "version {version}, expected {}",
                T::VERSION
            )));
        }
        None => return Err(ReadError::Unparsable("no version".into())),
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|error| ReadError::Unparsable(error.to_string()))
}

/// Writes a versioned file atomically, with `"version"` first.
pub(crate) fn write_versioned<T: VersionedFile>(path: &Path, contents: &T) -> io::Result<()> {
    let body = serde_json::to_value(contents).map_err(io::Error::other)?;
    let Value::Object(fields) = body else {
        return Err(io::Error::other(
            "a versioned file must serialize to an object",
        ));
    };
    let mut object = serde_json::Map::new();
    object.insert("version".into(), T::VERSION.into());
    object.extend(fields.into_iter().filter(|(name, _)| name != "version"));
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(object)).map_err(io::Error::other)?;
    bytes.push(b'\n');
    write_atomically(path, &bytes)
}

/// Writes `bytes` to a temp file next to `path`, flushes it to disk, then
/// renames it over `path`. Readers see the old file or the new one, never a
/// mix.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("a file to write has a parent folder"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("a file to write has a name"))?;
    let temp = dir.join(format!(
        ".{}.tmp-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
