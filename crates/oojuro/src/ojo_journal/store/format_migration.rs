//! Explicit copy-and-verify conversion; the original KV database is retained.
use super::*;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormatMigration {
    pub session: String,
    pub storage_profile: String,
    pub converted: bool,
    pub recovery_directory: Option<PathBuf>,
}
impl JournalRoot {
    pub fn migrate_session(&self, run: &str) -> Result<FormatMigration> {
        if let Some(report) =
            self.remote(super::super::owner::Action::MigrateSession { run: run.into() })?
        {
            return Ok(report);
        }
        let mut pool = self
            .pool
            .as_ref()
            .map(|pool| pool.lock().map_err(|_| "owner_unhealthy"))
            .transpose()?;
        if let Some(pool) = pool.as_mut() {
            pool.writers.retain(|writer| writer.run != run);
            pool.manifests.remove(run);
        }
        let _lock = self.lock(run)?;
        let path = self.session_path(run)?;
        // The old database, including any incomplete tail, is snapshot input.
        // Read-only validation must precede any recovery or publication.
        let mut source = self.open_db(run, false)?;
        let manifest = read_manifest(&mut source)?;
        let recovery = self.path.join("format-backups").join(run);
        let report = |converted| FormatMigration {
            session: run.into(),
            storage_profile: "ojo-journal/application-events/1".into(),
            converted,
            recovery_directory: recovery.is_dir().then(|| recovery.clone()),
        };
        let mut source = match source {
            Database::Legacy(source) => source,
            current => {
                drop(current);
                // Settle an earlier unknown conversion publication, after the
                // complete Event/application grammar has been validated.
                self.open_db(run, true)?.confirm().map_err(error)?;
                return Ok(report(false));
            }
        };
        let checkpoint = source.checkpoint();
        hosted_capture::directory(recovery.parent().ok_or("invalid_recovery_path")?)?;
        hosted_capture::directory(&recovery)?;
        let mut hash = blake3::Hasher::new();
        hash.update(b"ojo-journal/explicit-event-migration/v1");
        hash.update(run.as_bytes());
        hash.update(&checkpoint.file_id);
        hash.update(&checkpoint.sequence.to_be_bytes());
        hash.update(&checkpoint.digest);
        let identity = hash.finalize().to_hex().to_string();
        let backup = recovery.join(format!("{identity}.juro"));
        let staging = recovery.join(format!(".{identity}.snapshot"));
        if backup.exists() && staging.exists() {
            let published =
                fs::symlink_metadata(&backup).map_err(|_| "recovery_snapshot_unavailable")?;
            let unpublished =
                fs::symlink_metadata(&staging).map_err(|_| "recovery_snapshot_unavailable")?;
            if !published.is_file()
                || !unpublished.is_file()
                || (published.dev(), published.ino()) != (unpublished.dev(), unpublished.ino())
            {
                return Err("recovery_snapshot_conflict");
            }
            fs::remove_file(&staging).map_err(|_| "recovery_snapshot_cleanup_failed")?;
            sync_directory(&recovery)?;
        }
        if !backup.exists() {
            // A partial unpublished snapshot may be retried from the still
            // locked original. It is never a recovery receipt by itself.
            if staging.exists() {
                fs::remove_file(&staging).map_err(|_| "recovery_snapshot_unavailable")?;
            }
            let mut input = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
                .map_err(|_| "migration_source_unavailable")?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&staging)
                .map_err(|_| "recovery_snapshot_unavailable")?;
            io::copy(&mut input, &mut output)
                .and_then(|_| output.sync_all())
                .map_err(|_| "recovery_snapshot_sync_failed")?;
            drop(output);
            let checked = ojo::Database::open(&staging, self.key.as_ref(), false).map_err(error)?;
            if checked.checkpoint() != checkpoint {
                return Err("recovery_snapshot_checkpoint_mismatch");
            }
            drop(checked);
            if digest(&staging)? != digest(&path)? {
                return Err("recovery_snapshot_bytes_mismatch");
            }
            // The app run lock protects this insert-only recovery pathname.
            fs::hard_link(&staging, &backup).map_err(|_| "recovery_snapshot_publication_failed")?;
            sync_directory(&recovery)?;
            fs::remove_file(&staging).map_err(|_| "recovery_snapshot_cleanup_failed")?;
        }
        let checked = ojo::Database::open(&backup, self.key.as_ref(), false).map_err(error)?;
        if checked.checkpoint() != checkpoint || digest(&backup)? != digest(&path)? {
            return Err("recovery_snapshot_conflict");
        }
        drop(checked);
        sync_directory(&recovery)?;

        let temporary = path
            .parent()
            .ok_or("invalid_journal_path")?
            .join(format!(".event-migrate-{identity}.juro"));
        // Only a derived, unpublished conversion is discarded. Source and the
        // fully verified recovery copy remain unchanged through all failures.
        match fs::remove_file(&temporary) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("migration_staging_unavailable"),
        }
        sync_directory(path.parent().unwrap())?;
        let mut converted = Database::create(&temporary, self.key.as_ref()).map_err(error)?;
        let mut after = None;
        let mut copied = 0usize;
        loop {
            let page = legacy_page(&mut source, after.as_deref())?;
            if page.is_empty() {
                break;
            }
            let mut request_bytes = identity.as_bytes().to_vec();
            request_bytes.extend_from_slice(&page.last().unwrap().0);
            let changes = page
                .iter()
                .map(|(key, value)| Change {
                    key,
                    value: Some(value),
                })
                .collect::<Vec<_>>();
            converted
                .commit(
                    request(b"ojo-journal/event-copy/v1", &request_bytes),
                    &changes,
                )
                .map_err(error)?;
            copied += page.len();
            after = Some(page.last().unwrap().0.clone());
        }
        if copied == 0 {
            return Err("empty_session_conversion");
        }
        let actual = read_manifest(&mut converted)?;
        if serde_json::to_vec(&actual).map_err(|_| "invalid_journal_manifest")?
            != serde_json::to_vec(&manifest).map_err(|_| "invalid_journal_manifest")?
        {
            return Err("migration_manifest_mismatch");
        }
        let mut after = None;
        let mut verified = 0usize;
        loop {
            let page = legacy_page(&mut source, after.as_deref())?;
            if page.is_empty() {
                break;
            }
            for (key, value) in &page {
                if converted.get(key).map_err(error)?.as_ref() != Some(value) {
                    return Err("migration_original_mismatch");
                }
                verified += 1;
            }
            after = Some(page.last().unwrap().0.clone());
        }
        if verified != copied {
            return Err("migration_source_changed");
        }
        converted.confirm().map_err(error)?;
        drop(converted);
        drop(source);
        fs::rename(&temporary, &path).map_err(|_| "event_migration_publication_unknown")?;
        sync_directory(path.parent().unwrap())?;
        Ok(report(true))
    }
}
fn digest(path: &Path) -> Result<[u8; 32]> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "migration_file_unavailable")?;
    let metadata = file.metadata().map_err(|_| "migration_file_unavailable")?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o777 != 0o600
    {
        return Err("private_migration_file_required");
    }
    let mut hash = blake3::Hasher::new();
    let mut buffer = [0; 64 << 10];
    loop {
        let length = file
            .read(&mut buffer)
            .map_err(|_| "migration_file_unavailable")?;
        if length == 0 {
            break;
        }
        hash.update(&buffer[..length]);
    }
    Ok(*hash.finalize().as_bytes())
}
fn legacy_page(
    source: &mut ojo::Database,
    after: Option<&[u8]>,
) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut page = Vec::new();
    let mut bytes = 0usize;
    let result = source.scan_prefix(b"", after, 100, |key, value| {
        if bytes + value.len() > MAX_TEXT && !page.is_empty() {
            return Err(ojo::Error::Invalid("migration page complete"));
        }
        if value.len() > MAX_TEXT {
            return Err(ojo::Error::Invalid("migration original bound"));
        }
        bytes += value.len();
        page.push((key.to_vec(), value.to_vec()));
        Ok(())
    });
    match result {
        Ok(_) | Err(ojo::Error::Invalid("migration page complete")) => Ok(page),
        Err(error_value) => Err(error(error_value)),
    }
}
