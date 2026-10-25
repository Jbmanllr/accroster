//! The real FSEvents watch behind [`crate::Effect::WatchSessionFiles`].
//!
//! The UI layer starts one [`FolderWatch`] when the core asks and drops it on
//! [`crate::Effect::StopWatchingSessionFiles`]; it forwards each debounced
//! batch of paths as [`crate::Event::SessionFilesChanged`]. What the batch
//! means is the core's business, so this module stays a thin adapter.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};

/// How long a path stays quiet before its change is reported.
pub const DEBOUNCE: Duration = Duration::from_secs(1);

/// A recursive FSEvents watch on some folders. Dropping it stops watching.
pub struct FolderWatch {
    _debouncer: Debouncer<RecommendedWatcher>,
}

impl FolderWatch {
    /// Watches every folder in `folders` (one that can't be watched is
    /// skipped and returned in the error list) and calls `on_change` on the
    /// watcher's thread with each debounced batch of changed paths.
    pub fn start(
        folders: &[PathBuf],
        debounce: Duration,
        on_change: impl Fn(Vec<PathBuf>) + Send + 'static,
    ) -> Result<(FolderWatch, Vec<(PathBuf, String)>), String> {
        // FSEvents reports resolved paths (`/private/var/…` for `/var/…`):
        // map them back to the folders as the core named them.
        let aliases: Vec<(PathBuf, PathBuf)> = folders
            .iter()
            .filter_map(|folder| Some((folder.canonicalize().ok()?, folder.clone())))
            .filter(|(real, folder)| real != folder)
            .collect();
        let mut debouncer = new_debouncer(debounce, move |result: DebounceEventResult| {
            // A watcher error carries no paths; the next menu open's rescan
            // catches up.
            if let Ok(events) = result {
                let mut paths: Vec<PathBuf> = events
                    .into_iter()
                    .map(|event| unalias(&aliases, event.path))
                    .collect();
                paths.sort();
                paths.dedup();
                if !paths.is_empty() {
                    on_change(paths);
                }
            }
        })
        .map_err(|error| error.to_string())?;
        let skipped = folders
            .iter()
            .filter_map(|folder| {
                watch(&mut debouncer, folder)
                    .err()
                    .map(|error| (folder.clone(), error))
            })
            .collect();
        Ok((
            FolderWatch {
                _debouncer: debouncer,
            },
            skipped,
        ))
    }
}

fn unalias(aliases: &[(PathBuf, PathBuf)], path: PathBuf) -> PathBuf {
    aliases
        .iter()
        .find_map(|(real, folder)| Some(folder.join(path.strip_prefix(real).ok()?)))
        .unwrap_or(path)
}

fn watch(debouncer: &mut Debouncer<RecommendedWatcher>, folder: &Path) -> Result<(), String> {
    debouncer
        .watcher()
        .watch(folder, RecursiveMode::Recursive)
        .map_err(|error| error.to_string())
}
