//! IPC commands (`app/IPC.md`). Every command validates its input in Rust,
//! runs git/file work on a blocking worker thread and maps `CoreError` to
//! `IpcError`.

pub mod fix;
pub mod keys;
pub mod profiles;
pub mod repos;
pub mod settings;
pub mod system;

#[cfg(test)]
mod validation_tests;
