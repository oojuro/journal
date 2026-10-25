//! Session journals backed by the standalone `.juro` file engine.
//!
//! The application owns session routing, capture policy and derived views. Every
//! accepted original is stored as opaque UTF-8 bytes in one encrypted file per
//! session. Existing directory journals remain readable by the legacy commands.
mod capture;
pub mod cli;
mod migrate;
mod owner;
mod setup;
mod session_file;
mod store;
mod sync;

pub use capture::{CaptureReport, Verification, capture_once, connect_local, verify_pipeline};
pub use migrate::{MigrationReport, migrate_legacy, restore_migration};
pub use store::{FormatMigration, JournalRoot, Manifest, Page, Record};
pub use sync::{SyncReport, sync_cloud};

pub type Result<T> = std::result::Result<T, &'static str>;

/// Saves a scoped connection and the explicit existing-history choice. Remote
/// acceptance is established separately by a successful setup/sync ACK.
pub fn connect_cloud(
    root: &JournalRoot,
    base: &str,
    key_file: &std::path::Path,
    label: &str,
    include_existing: bool,
) -> Result<crate::hosted_capture::ApiConnectionInfo> {
    let info = crate::hosted_capture::ensure_journal_connection(
        &root.path.join("cloud.json"),
        base,
        key_file,
        label,
    )?;
    sync::initialize(root, &root.path.join("cloud.json"), include_existing)?;
    Ok(info)
}

#[cfg(test)]
mod tests;
