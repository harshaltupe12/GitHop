//! GitHop core library.
//!
//! Single source of truth shared by the CLI, the git hooks and the tray app:
//! profiles, storage, repo → profile resolution and safe git config reads/writes.
//! Synchronous, no UI, no network.

pub mod backup;
pub mod check;
pub mod doctor;
pub mod error;
pub mod git;
pub mod gitconfig;
pub mod hooks;
pub mod identity;
pub mod log;
pub mod paths;
pub mod pin;
pub mod profile;
pub mod remote;
pub mod repo;
pub mod resolver;
pub mod scan;
pub mod settings;
pub mod ssh;
pub mod store;
pub mod wipe;

pub use error::{CoreError, Result};
pub use git::Git;
pub use gitconfig::{ConfigEntry, ConfigScope, ConfigSnapshot};
pub use identity::{EffectiveIdentity, Identity};
pub use paths::Paths;
pub use profile::{Profile, ProfileColor, ProfileSet, Severity, ValidationIssue};
pub use remote::RemoteInfo;
pub use repo::Repo;
pub use resolver::{resolve, Resolution, ResolutionSource, ResolutionWarning};
pub use settings::{EnforceMode, Settings};
pub use store::Store;
