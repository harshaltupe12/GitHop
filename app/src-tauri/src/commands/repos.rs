//! Repo commands: scan (worker thread + progress events), pin, unpin, pin all,
//! husky snippet.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use githop_core::paths::{folder_key, to_slash};
use githop_core::scan::{self, ScannedRepo};
use githop_core::{hooks, pin as core_pin, Git, ProfileSet, Repo};
use tauri::{AppHandle, Emitter, Runtime};

use crate::dto::{IpcError, IpcResult, ScanProgress};
use crate::events;
use crate::ops::{self, blocking};
use crate::validate;

/// Most scan worker threads (core's scan uses the same cap).
const MAX_THREADS: usize = 8;
/// Minimum gap between two `scan://progress` events.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// Find repos under `folder` and resolve each. Emits `scan://progress`.
#[tauri::command]
pub async fn scan<R: Runtime>(app: AppHandle<R>, folder: String) -> IpcResult<Vec<ScannedRepo>> {
    let root = validate::existing_dir(&folder)?;
    blocking(move || {
        let git = ops::git()?;
        let store = ops::store()?;
        let set = store.profiles().clone();
        let aliases = store.settings().github_host_aliases.clone();
        let label = to_slash(&root);
        let emit = |found: usize, done: usize| {
            let payload = ScanProgress {
                folder: label.clone(),
                found,
                done,
            };
            if let Err(e) = app.emit(events::SCAN_PROGRESS, payload) {
                eprintln!("GitHop: couldn't send scan progress: {e}");
            }
        };
        Ok(scan_with_progress(&git, &root, &set, &aliases, emit))
    })
    .await
}

/// Pin `repo` to profile `id`.
#[tauri::command]
pub async fn pin<R: Runtime>(
    app: AppHandle<R>,
    repo: String,
    id: String,
) -> IpcResult<ScannedRepo> {
    let dir = validate::existing_dir(&repo)?;
    validate::profile_id(&id)?;
    let out = blocking(move || {
        let _guard = ops::write_lock();
        let git = ops::git()?;
        let mut store = ops::store()?;
        let profile = ops::profile(store.profiles(), &id)?.clone();
        let repo = Repo::discover(&git, &dir)?;
        core_pin::pin_repo(&repo, &profile)?;
        let mut settings = store.settings().clone();
        if ops::remember_repo(&mut settings.pinned_repos, repo.root()) {
            store.save_settings(&settings)?;
        }
        rescan_one(&git, repo.root(), &store)
    })
    .await?;
    events::data_changed(&app);
    Ok(out)
}

/// Remove the pin from `repo` (no-op if it isn't pinned).
#[tauri::command]
pub async fn unpin<R: Runtime>(app: AppHandle<R>, repo: String) -> IpcResult<ScannedRepo> {
    let dir = validate::existing_dir(&repo)?;
    let out = blocking(move || {
        let _guard = ops::write_lock();
        let git = ops::git()?;
        let mut store = ops::store()?;
        let repo = Repo::discover(&git, &dir)?;
        core_pin::unpin_repo(&repo)?;
        let mut settings = store.settings().clone();
        if ops::forget_repo(&mut settings.pinned_repos, repo.root()) {
            store.save_settings(&settings)?;
        }
        rescan_one(&git, repo.root(), &store)
    })
    .await?;
    events::data_changed(&app);
    Ok(out)
}

/// Pin each repo to the profile it currently resolves to. Repos that don't
/// resolve are returned unchanged; a repo that fails gets `error` set.
#[tauri::command]
pub async fn pin_all<R: Runtime>(
    app: AppHandle<R>,
    repos: Vec<String>,
) -> IpcResult<Vec<ScannedRepo>> {
    let dirs = validate::repo_list(&repos)?;
    let out = blocking(move || {
        let _guard = ops::write_lock();
        let git = ops::git()?;
        let mut store = ops::store()?;
        let set = store.profiles().clone();
        let mut settings = store.settings().clone();
        let mut changed = false;
        let mut out = Vec::with_capacity(dirs.len());
        for dir in &dirs {
            let scanned = rescan_one(&git, dir, &store)?;
            let Some(profile) = scanned
                .resolution
                .profile_id
                .as_deref()
                .and_then(|id| set.get(id))
            else {
                out.push(scanned);
                continue;
            };
            let pinned = Repo::discover(&git, Path::new(&scanned.root))
                .and_then(|repo| core_pin::pin_repo(&repo, profile).map(|()| repo));
            match pinned {
                Ok(repo) => {
                    changed |= ops::remember_repo(&mut settings.pinned_repos, repo.root());
                    out.push(rescan_one(&git, repo.root(), &store)?);
                }
                Err(e) => out.push(ScannedRepo {
                    error: Some(crate::dto::sentence(&e.to_string())),
                    ..scanned
                }),
            }
        }
        if changed {
            store.save_settings(&settings)?;
        }
        Ok(out)
    })
    .await?;
    events::data_changed(&app);
    Ok(out)
}

/// One-line snippet for a repo's own hook manager (husky, lefthook…).
#[tauri::command]
pub async fn hooks_snippet(stage: String) -> IpcResult<String> {
    let stage = validate::stage(&stage)?;
    blocking(move || {
        let store = ops::store()?;
        let exe = ops::githop_exe(&store.settings().githop_exe)?;
        Ok(hooks::snippet(stage, &to_slash(&exe))?)
    })
    .await
}

/// Re-resolve a single repo (its root only, no walk).
fn rescan_one(git: &Git, root: &Path, store: &githop_core::Store) -> IpcResult<ScannedRepo> {
    scan::scan(
        git,
        root,
        store.profiles(),
        &store.settings().github_host_aliases,
        0,
    )
    .into_iter()
    .next()
    .ok_or_else(|| {
        IpcError::new(
            crate::dto::ErrorCode::NotARepo,
            "This folder is not a git repo.",
        )
    })
}

/// Core's scan, split so progress can be reported: find repos (one walk), then
/// resolve each on a small thread pool, emitting `(found, done)` as it goes.
pub fn scan_with_progress(
    git: &Git,
    root: &Path,
    set: &ProfileSet,
    aliases: &[String],
    emit: impl Fn(usize, usize) + Sync,
) -> Vec<ScannedRepo> {
    let dirs: Vec<PathBuf> = scan::find_repos(root, scan::DEFAULT_MAX_DEPTH);
    let found = dirs.len();
    emit(found, 0);
    if dirs.is_empty() {
        return Vec::new();
    }
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, MAX_THREADS)
        .min(found);
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let last_emit = Mutex::new((Instant::now(), 0usize));
    let results: Mutex<Vec<ScannedRepo>> = Mutex::new(Vec::with_capacity(found));

    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(dir) = dirs.get(i) else {
                    break;
                };
                // max_depth 0 = resolve just this repo (find_repos never
                // descends into a repo, so nothing is missed).
                let one = scan::scan(git, dir, set, aliases, 0);
                if let Ok(mut r) = results.lock() {
                    r.extend(one);
                }
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                // Emit in order (never a smaller `done` after a larger one),
                // throttled, and always the final count.
                if let Ok(mut last) = last_emit.lock() {
                    let (at, sent) = *last;
                    if n > sent && (n == found || at.elapsed() >= PROGRESS_EVERY) {
                        *last = (Instant::now(), n);
                        emit(found, n);
                    }
                }
            });
        }
    });

    let mut out = results.into_inner().unwrap_or_else(|p| p.into_inner());
    out.sort_by(|a, b| {
        folder_key(&a.root)
            .cmp(&folder_key(&b.root))
            .then_with(|| a.root.cmp(&b.root))
    });
    out.dedup_by(|a, b| folder_key(&a.root) == folder_key(&b.root));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// git with every config/home location inside `tmp` (CLAUDE.md rule 3).
    fn isolated_git(tmp: &Path) -> Option<Git> {
        let home = tmp.join("home");
        let data = tmp.join("githop");
        std::fs::create_dir_all(&home).ok()?;
        let global = home.join(".gitconfig");
        std::fs::write(
            &global,
            "[user]\n\tname = Test\n\temail = test@example.com\n",
        )
        .ok()?;
        Some(
            Git::locate()
                .ok()?
                .env("HOME", &home)
                .env("USERPROFILE", &home)
                .env("GIT_CONFIG_GLOBAL", &global)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GITHOP_HOME", &data),
        )
    }

    #[test]
    fn scan_reports_progress_and_finds_every_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let Some(git) = isolated_git(tmp.path()) else {
            eprintln!("git not found; skipping");
            return;
        };
        let root = tmp.path().join("code");
        for rel in ["b-repo", "a-repo", "nested/c-repo", "node_modules/skip-me"] {
            let dir = root.join(rel);
            std::fs::create_dir_all(&dir).unwrap();
            git.run(&dir, &["init", "-q"]).unwrap();
        }
        let events = Mutex::new(Vec::new());
        let out = scan_with_progress(&git, &root, &ProfileSet::default(), &[], |f, d| {
            events.lock().unwrap().push((f, d));
        });
        let names: Vec<String> = out
            .iter()
            .map(|r| r.root.rsplit('/').next().unwrap_or("").to_string())
            .collect();
        assert_eq!(names, ["a-repo", "b-repo", "c-repo"]);
        assert!(out.iter().all(|r| r.error.is_none()), "{out:?}");
        let events = events.into_inner().unwrap();
        assert_eq!(events.first(), Some(&(3, 0)));
        assert_eq!(events.last(), Some(&(3, 3)));
        assert!(events.windows(2).all(|w| w[0].1 < w[1].1));
    }

    #[test]
    fn scan_of_folder_without_repos_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let Some(git) = isolated_git(tmp.path()) else {
            return;
        };
        let events = Mutex::new(Vec::new());
        let out = scan_with_progress(&git, tmp.path(), &ProfileSet::default(), &[], |f, d| {
            events.lock().unwrap().push((f, d));
        });
        assert!(out.is_empty());
        assert_eq!(events.into_inner().unwrap(), vec![(0, 0)]);
    }
}
