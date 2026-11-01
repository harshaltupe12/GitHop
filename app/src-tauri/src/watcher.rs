//! Watches the GitHop data dir so edits made by the CLI (or another window)
//! refresh the UI (`profiles://changed`) and the tray.
//!
//! Non-recursive watch of the data dir, debounced. If the data dir doesn't
//! exist yet (nothing saved so far) the watcher waits for it instead of
//! creating it (core creates dirs lazily, on first write).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use githop_core::Paths;
use notify_debouncer_mini::notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult};
use tauri::{AppHandle, Runtime};

use crate::tray::model::LAST_BLOCK_FILE;

/// Debounce window: atomic temp-file + rename writes arrive as a burst.
const DEBOUNCE: Duration = Duration::from_millis(300);
/// How often to look for the data dir before it exists.
const WAIT_FOR_DIR: Duration = Duration::from_secs(5);

/// What a changed file means for the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// `profiles.toml` / `settings.toml`: tell the UI and refresh the tray.
    Data,
    /// `last-block`: refresh the tray only.
    Blocked,
}

/// Classify a changed path (by file name; other files are ignored).
pub fn classify(path: &Path) -> Option<Change> {
    let name = path.file_name().and_then(OsStr::to_str)?;
    if name.eq_ignore_ascii_case("profiles.toml") || name.eq_ignore_ascii_case("settings.toml") {
        Some(Change::Data)
    } else if name.eq_ignore_ascii_case(LAST_BLOCK_FILE) {
        Some(Change::Blocked)
    } else {
        None
    }
}

/// Start the watcher thread. Never fails the app: problems are logged.
pub fn start<R: Runtime>(app: AppHandle<R>) {
    let spawned = std::thread::Builder::new()
        .name("githop-watcher".into())
        .spawn(move || {
            let root: PathBuf = match Paths::resolve() {
                Ok(p) => p.root().to_path_buf(),
                Err(e) => {
                    eprintln!("GitHop: not watching data files: {e}");
                    return;
                }
            };
            while !root.is_dir() {
                std::thread::sleep(WAIT_FOR_DIR);
            }
            // The dir may have appeared with files already in it.
            crate::events::data_changed(&app);
            watch(&app, &root);
        });
    if let Err(e) = spawned {
        eprintln!("GitHop: couldn't start the file watcher: {e}");
    }
}

fn watch<R: Runtime>(app: &AppHandle<R>, root: &Path) {
    let (tx, rx) = std::sync::mpsc::channel::<DebounceEventResult>();
    let mut debouncer = match new_debouncer(DEBOUNCE, tx) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("GitHop: couldn't start the file watcher: {e}");
            return;
        }
    };
    if let Err(e) = debouncer.watcher().watch(root, RecursiveMode::NonRecursive) {
        eprintln!("GitHop: couldn't watch {}: {e}", root.display());
        return;
    }
    // `debouncer` lives as long as this loop; the channel closes if it stops.
    for result in rx {
        match result {
            Ok(events) => {
                let changes: Vec<Change> =
                    events.iter().filter_map(|e| classify(&e.path)).collect();
                if changes.contains(&Change::Data) {
                    crate::events::data_changed(app);
                } else if changes.contains(&Change::Blocked) {
                    crate::tray::refresh(app);
                }
            }
            Err(e) => eprintln!("GitHop: file watcher error: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_file_name() {
        assert_eq!(
            classify(Path::new("C:/data/profiles.toml")),
            Some(Change::Data)
        );
        assert_eq!(
            classify(Path::new("C:/data/Settings.TOML")),
            Some(Change::Data)
        );
        assert_eq!(
            classify(Path::new("C:/data/last-block")),
            Some(Change::Blocked)
        );
        assert_eq!(classify(Path::new("C:/data/.lock")), None);
        assert_eq!(classify(Path::new("C:/data/profiles.toml.tmp123")), None);
        assert_eq!(classify(Path::new("C:/data/backups")), None);
    }
}
