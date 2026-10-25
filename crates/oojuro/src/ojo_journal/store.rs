use super::Result;
use super::session_file::SessionFile as Database;
use crate::{hosted::read_private, hosted_capture};
use ojo::Change;
mod format_migration;
pub use format_migration::FormatMigration;
use oojuro_engine::record::Input;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

pub const MAX_TEXT: usize = 16 << 20;
pub const MAX_METADATA: usize = 64 << 10;
const FORMAT: u32 = 1;
const MANIFEST: &[u8] = b"manifest";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RootConfig {
    format: u32,
    identity: String,
    encrypted: bool,
    #[serde(default = "yes")]
    automatic_archive: bool,
}
fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub run_id: String,
    pub agent_id: String,
    pub adapter: String,
    pub label: String,
    pub records: u64,
    pub created_us: i64,
    pub last_activity_us: i64,
    pub finished_us: Option<i64>,
    pub archived_us: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub sequence: u64,
    pub event_id: String,
    pub run_id: String,
    pub adapter: String,
    pub input: Input,
}

#[derive(Serialize, Deserialize)]
pub struct Page {
    pub records: Vec<Record>,
    pub next: Option<u64>,
    pub total: u64,
}

#[derive(Clone)]
pub struct JournalRoot {
    pub(crate) path: PathBuf,
    cfg: RootConfig,
    key: Option<[u8; 32]>,
    pool: Option<Arc<Mutex<Pool>>>,
    remote: bool,
}

pub(crate) fn opaque(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
/// New files use .juro. Existing .ojo files retain their exact names. Two
/// candidates are an ambiguity, regardless of whether their bytes happen to match.
pub(crate) fn database_path(directory: &Path, stem: &str) -> Result<PathBuf> {
    if !opaque(stem) {
        return Err("invalid_database_identity");
    }
    fn present(path: &Path) -> Result<bool> {
        match fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err("database_path_unavailable"),
        }
    }
    let canonical = directory.join(format!("{stem}.juro"));
    let legacy = directory.join(format!("{stem}.ojo"));
    match (present(&canonical)?, present(&legacy)?) {
        (true, true) => Err("ambiguous_database_filename; both .juro and legacy .ojo exist"),
        (false, true) => Ok(legacy),
        _ => Ok(canonical),
    }
}
fn name(s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 128 || s.trim() != s || s.chars().any(char::is_control) {
        return Err("invalid_journal_label");
    }
    Ok(())
}
pub(crate) fn now() -> i64 {
    oojuro_capture::time::now_us()
}
pub(crate) fn request(domain: &[u8], bytes: &[u8]) -> [u8; 16] {
    let mut hash = blake3::Hasher::new();
    hash.update(domain);
    hash.update(bytes);
    hash.finalize().as_bytes()[..16].try_into().unwrap()
}
fn key(prefix: &[u8], sequence: u64) -> Vec<u8> {
    let mut key = prefix.to_vec();
    key.extend_from_slice(&sequence.to_be_bytes());
    key
}
fn id_key(event: &str) -> Vec<u8> {
    [b"i/".as_slice(), event.as_bytes()].concat()
}
fn batch_fingerprint(records: &[Record]) -> Result<[u8; 32]> {
    let mut hash = blake3::Hasher::new();
    hash.update(b"ojo-journal/batch-originals/v1");
    hash.update(&(records.len() as u64).to_le_bytes());
    for record in records {
        let mut metadata = record.clone();
        metadata.sequence = 0;
        metadata.input.text.clear();
        let bytes = serde_json::to_vec(&metadata).map_err(|_| "invalid_journal_record")?;
        hash.update(&(bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
        hash.update(&(record.input.text.len() as u64).to_le_bytes());
        hash.update(record.input.text.as_bytes());
    }
    Ok(*hash.finalize().as_bytes())
}
fn private_dir(path: &Path) -> Result<()> {
    hosted_capture::ancestors(path)?;
    let m = fs::symlink_metadata(path).map_err(|_| "private_journal_directory_required")?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o777 != 0o700 {
        return Err("private_journal_directory_required");
    }
    Ok(())
}
fn sync_directory(path: &Path) -> Result<()> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "journal_directory_sync_failed")
}
/// A failed mkdir/parent sync can leave a valid, visible path. Confirm every
/// ancestor again on reopen; an existing pathname is not a durability receipt.
/// Existing ancestor permissions are validated but never changed.
fn directory_chain(path: &Path, create: bool) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err("absolute_private_path_required");
    }
    let uid = unsafe { libc::geteuid() };
    let chain = path.ancestors().collect::<Vec<_>>();
    for part in chain.into_iter().rev() {
        let metadata = match fs::symlink_metadata(part) {
            Ok(metadata) => metadata,
            Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(part)
                    .map_err(|_| "journal_root_creation_failed")?;
                fs::symlink_metadata(part).map_err(|_| "journal_root_creation_failed")?
            }
            Err(_) => return Err("private_path_unavailable"),
        };
        if !metadata.is_dir()
            || ![0, uid].contains(&metadata.uid())
            || metadata.mode() & 0o022 != 0
                && !(metadata.uid() == 0 && metadata.mode() & 0o1000 != 0)
        {
            return Err("unsafe_path_ancestor");
        }
        if let Some(parent) = part.parent() {
            sync_directory(parent)?;
        }
    }
    sync_directory(path)
}
/// Read and confirm the same protected inode before using persisted identity or
/// key material. A prior failed file sync must not become success on retry.
fn confirmed_private(path: &Path, limit: usize) -> Result<Vec<u8>> {
    hosted_capture::ancestors(path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "private_journal_metadata_required")?;
    let metadata = file
        .metadata()
        .map_err(|_| "private_journal_metadata_required")?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o777 != 0o600
        || metadata.len() > limit as u64
    {
        return Err("private_journal_metadata_required");
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "private_journal_metadata_required")?;
    if bytes.len() > limit {
        return Err("private_journal_metadata_required");
    }
    file.sync_all()
        .map_err(|_| "journal_metadata_sync_failed")?;
    Ok(bytes)
}
fn error(error: ojo::Error) -> &'static str {
    match error {
        ojo::Error::Invalid("pending_journal_batch_requires_original_retry") => {
            "pending_journal_batch_requires_original_retry"
        }
        _ => "ojo_journal_unavailable_or_corrupt",
    }
}
fn read_manifest(db: &mut Database) -> Result<Manifest> {
    let bytes = db
        .get(MANIFEST)
        .map_err(error)?
        .ok_or("missing_journal_manifest")?;
    decode_manifest(&bytes)
}
pub(super) fn decode_manifest(bytes: &[u8]) -> Result<Manifest> {
    let m: Manifest = serde_json::from_slice(bytes).map_err(|_| "invalid_journal_manifest")?;
    if m.format != FORMAT || !opaque(&m.run_id) || !opaque(&m.agent_id) || m.records > 3_000_000 {
        return Err("invalid_journal_manifest");
    }
    name(&m.label)?;
    Ok(m)
}
fn encoded_manifest(m: &Manifest) -> Result<Vec<u8>> {
    serde_json::to_vec(m).map_err(|_| "invalid_journal_manifest")
}

impl JournalRoot {
    pub fn init(path: &Path, encrypted: bool) -> Result<Self> {
        if !path.is_absolute() {
            return Err("absolute_journal_root_required");
        }
        if path.exists()
            && !path.join("root.json").exists()
            && ["native.json", "record.json", "state.json", "trace"]
                .iter()
                .any(|name| path.join(name).exists())
        {
            return Err("legacy_journal_root_requires_explicit_migration_to_a_new_directory");
        }
        directory_chain(path, true)?;
        private_dir(path)?;
        for part in ["journals", "locks"] {
            let p = path.join(part);
            if !p.exists() {
                hosted_capture::directory(&p)?;
            }
            private_dir(&p)?;
        }
        if path.join("root.json").exists() {
            let root = Self::open(path)?;
            if root.cfg.encrypted != encrypted {
                return Err("journal_encryption_mode_conflict");
            }
            return Ok(root);
        }
        let key_path = path.join("key");
        if encrypted && !key_path.exists() {
            hosted_capture::create_private(&key_path, &ojo::generate_key().map_err(error)?)?;
        }
        if encrypted && confirmed_private(&key_path, 32)?.len() != 32 {
            return Err("invalid_journal_key");
        }
        let cfg = RootConfig {
            format: FORMAT,
            identity: hosted_capture::random_id()?,
            encrypted,
            automatic_archive: true,
        };
        hosted_capture::create_private(
            &path.join("root.json"),
            &serde_json::to_vec(&cfg).map_err(|_| "invalid_journal_root")?,
        )?;
        Self::open(path)
    }
    pub fn open(path: &Path) -> Result<Self> {
        let mut root = Self::open_standalone(path)?;
        root.remote = super::owner::available(&root)?;
        Ok(root)
    }
    pub(crate) fn open_standalone(path: &Path) -> Result<Self> {
        private_dir(path)?;
        for part in ["journals", "locks"] {
            private_dir(&path.join(part))?;
        }
        let cfg: RootConfig = serde_json::from_slice(
            &confirmed_private(&path.join("root.json"), 4096)
                .map_err(|_| "private_journal_root_required")?,
        )
        .map_err(|_| "invalid_journal_root")?;
        if cfg.format != FORMAT || !opaque(&cfg.identity) {
            return Err("invalid_journal_root");
        }
        let key = if cfg.encrypted {
            Some(
                confirmed_private(&path.join("key"), 32)
                    .map_err(|_| "private_journal_key_required")?
                    .try_into()
                    .map_err(|_| "invalid_journal_key")?,
            )
        } else {
            None
        };
        // Confirm app-owned namespace and metadata before any local/remote ACK.
        // Healthy capture retains this root; this is an open-time barrier.
        directory_chain(path, false)?;
        for part in ["journals", "locks"] {
            sync_directory(&path.join(part))?;
        }
        Ok(Self {
            path: fs::canonicalize(path).map_err(|_| "journal_root_unavailable")?,
            cfg,
            key,
            pool: None,
            remote: false,
        })
    }
    pub(crate) fn owned(mut self) -> Self {
        self.pool = Some(Arc::new(Mutex::new(Pool::default())));
        self
    }
    fn remote<T: serde::de::DeserializeOwned>(
        &self,
        action: super::owner::Action,
    ) -> Result<Option<T>> {
        if self.remote {
            super::owner::call(self, action).map(Some)
        } else {
            Ok(None)
        }
    }
    fn with_db<T>(&self, run: &str, action: impl FnOnce(&mut Database) -> Result<T>) -> Result<T> {
        if let Some(pool) = &self.pool {
            let mut pool = pool.lock().map_err(|_| "owner_unhealthy")?;
            let mut writer = if let Some(i) = pool.writers.iter().position(|w| w.run == run) {
                let writer = pool.writers.remove(i).unwrap();
                if writer.stamp == self.stamp(run)? {
                    writer
                } else {
                    drop(writer);
                    pool.manifests.remove(run);
                    pool.opens += 1;
                    self.direct_writer(run)?
                }
            } else {
                if pool.writers.len() >= 8 {
                    pool.writers.pop_front();
                }
                pool.opens += 1;
                self.direct_writer(run)?
            };
            let result = action(&mut writer.db);
            // A failed database operation may have an unknown commit outcome. Never
            // keep using that handle: verified reopen establishes its durable state.
            if result.is_ok() {
                let manifest = read_manifest(&mut writer.db)?;
                let stamp = self.stamp(run)?;
                writer.stamp = stamp.clone();
                if pool.manifests.len() >= 10_000 && !pool.manifests.contains_key(run) {
                    pool.manifests.pop_first();
                }
                pool.manifests.insert(run.into(), (stamp, manifest));
                pool.writers.push_back(writer);
            } else {
                pool.manifests.remove(run);
            }
            result
        } else {
            let mut writer = self.direct_writer(run)?;
            action(&mut writer.db)
        }
    }
    pub(crate) fn owner_status(&self) -> Result<Option<serde_json::Value>> {
        if self.remote {
            return super::owner::call(self, super::owner::Action::Hello).map(Some);
        }
        self.pool.as_ref().map(|pool| {
            let pool=pool.lock().map_err(|_|"owner_unhealthy")?;
            Ok(serde_json::json!({"healthy":true,"root":self.identity(),"storage":"ojo","handle_capacity":8,"open_handles":pool.writers.len(),"verified_opens":pool.opens,"socket":super::owner::socket(self)?}))
        }).transpose()
    }
    fn stamp(&self, run: &str) -> Result<Stamp> {
        let m = fs::symlink_metadata(self.session_path(run)?).map_err(|_| "journal_not_found")?;
        if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o777 != 0o600 {
            return Err("private_journal_file_required");
        }
        Ok(Stamp {
            dev: m.dev(),
            ino: m.ino(),
            size: m.len(),
            modified: m.mtime(),
            modified_ns: m.mtime_nsec(),
            changed: m.ctime(),
            changed_ns: m.ctime_nsec(),
        })
    }
    pub fn identity(&self) -> &str {
        &self.cfg.identity
    }
    pub fn encrypted(&self) -> bool {
        self.cfg.encrypted
    }
    pub(crate) fn file_key(&self) -> Option<&[u8; 32]> {
        self.key.as_ref()
    }
    pub(crate) fn session_path(&self, run: &str) -> Result<PathBuf> {
        if !opaque(run) {
            return Err("invalid_session_identity");
        }
        database_path(&self.path.join("journals"), run)
    }
    fn lock(&self, run: &str) -> Result<File> {
        self.session_path(run)?;
        let path = self.path.join("locks").join(format!("{run}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|_| "journal_lock_unavailable")?;
        read_private(&path, 0).map_err(|_| "private_journal_lock_required")?;
        file.try_lock().map_err(|_| "journal_busy")?;
        Ok(file)
    }
    fn open_db(&self, run: &str, writable: bool) -> Result<Database> {
        self.open_db_mode(run, writable, false)
    }
    fn open_db_mode(&self, run: &str, writable: bool, initialize_empty: bool) -> Result<Database> {
        let path = self.session_path(run)?;
        let info = fs::symlink_metadata(&path).map_err(|_| "journal_not_found")?;
        if !info.is_file()
            || info.uid() != unsafe { libc::geteuid() }
            || info.mode() & 0o777 != 0o600
        {
            return Err("private_journal_file_required");
        }
        let mut db = if initialize_empty {
            Database::open_initializing(&path, self.key.as_ref()).map_err(error)?
        } else {
            Database::open(&path, self.key.as_ref(), writable).map_err(error)?
        };
        // Replay validates bytes, while a prior writer may have lost its sync
        // acknowledgement. Confirm before reconstructed application ACKs or sync staging.
        if writable {
            db.confirm().map_err(error)?;
            File::open(path.parent().ok_or("invalid_journal_path")?)
                .and_then(|directory| directory.sync_all())
                .map_err(|_| "journal_publication_outcome_unknown")?;
        }
        Ok(db)
    }
    pub fn ensure_session(
        &self,
        run: &str,
        agent: &str,
        adapter: &str,
        label: &str,
    ) -> Result<Manifest> {
        if let Some(value) = self.remote(super::owner::Action::Ensure {
            run: run.into(),
            agent: agent.into(),
            adapter: adapter.into(),
            label: label.into(),
        })? {
            return Ok(value);
        }
        if self.pool.is_some() && self.session_path(run)?.exists() {
            let m = self.manifest(run)?;
            if m.agent_id != agent || m.adapter != adapter {
                return Err("session_binding_conflict");
            }
            name(label)?;
            return Ok(m);
        }
        if !opaque(agent) || adapter.is_empty() || adapter.len() > 128 {
            return Err("invalid_session_identity");
        }
        name(label)?;
        let pool_guard = self
            .pool
            .as_ref()
            .map(|pool| pool.lock().map_err(|_| "owner_unhealthy"))
            .transpose()?;
        if pool_guard.is_some() && self.session_path(run)?.exists() {
            drop(pool_guard);
            return self.ensure_session(run, agent, adapter, label);
        }
        let _lock = self.lock(run)?;
        let path = self.session_path(run)?;
        let mut db = if path.exists() {
            self.open_db_mode(run, true, true)?
        } else {
            Database::create(path, self.key.as_ref()).map_err(error)?
        };
        if let Some(bytes) = db.get(MANIFEST).map_err(error)? {
            let m: Manifest =
                serde_json::from_slice(&bytes).map_err(|_| "invalid_journal_manifest")?;
            if m.run_id != run || m.agent_id != agent || m.adapter != adapter {
                return Err("session_binding_conflict");
            }
            // Labels are display metadata. Capture retries never rename an existing journal.
            return Ok(m);
        }
        let time = now();
        let m = Manifest {
            format: FORMAT,
            run_id: run.into(),
            agent_id: agent.into(),
            adapter: adapter.into(),
            label: label.into(),
            records: 0,
            created_us: time,
            last_activity_us: time,
            finished_us: None,
            archived_us: None,
        };
        db.commit(
            request(b"ojo-journal/manifest/v1", run.as_bytes()),
            &[Change {
                key: MANIFEST,
                value: Some(&encoded_manifest(&m)?),
            }],
        )
        .map_err(error)?;
        Ok(m)
    }
    /// Original text bytes and their derived metadata become durable in one transaction.
    /// Stable event IDs make source replay safe even if the application ACK is lost.
    fn direct_writer(&self, run: &str) -> Result<DirectWriter> {
        let guard = self.lock(run)?;
        let db = self.open_db(run, true)?;
        Ok(DirectWriter {
            _guard: guard,
            db,
            run: run.into(),
            stamp: self.stamp(run)?,
        })
    }
    pub(crate) fn writer(&self, run: &str) -> Result<SessionWriter> {
        self.session_path(run)?;
        if self.pool.is_none() && !self.remote {
            Ok(SessionWriter::Direct(Box::new(self.direct_writer(run)?)))
        } else {
            Ok(SessionWriter::Shared {
                root: self.clone(),
                run: run.into(),
            })
        }
    }
    pub(crate) fn append(&self, run: &str, batch_request: &str, records: &[Record]) -> Result<u64> {
        if let Some(value) = self.remote(super::owner::Action::Append {
            run: run.into(),
            request: batch_request.into(),
            records: records.to_vec(),
        })? {
            return Ok(value);
        }
        self.with_db(run, |db| append_to(db, run, batch_request, records))
    }
    pub fn manifest(&self, run: &str) -> Result<Manifest> {
        if let Some(value) = self.remote(super::owner::Action::Manifest { run: run.into() })? {
            return Ok(value);
        }
        if let Some(pool) = &self.pool {
            let pool = pool.lock().map_err(|_| "owner_unhealthy")?;
            if let Some((stamp, manifest)) = pool.manifests.get(run)
                && *stamp == self.stamp(run)?
            {
                return Ok(manifest.clone());
            }
        }
        self.with_db(run, read_manifest)
    }
    pub fn sessions(&self) -> Result<Vec<Manifest>> {
        if let Some(value) = self.remote(super::owner::Action::Sessions)? {
            return Ok(value);
        }
        let mut identities = std::collections::BTreeSet::new();
        let mut sessions = Vec::new();
        for entry in
            fs::read_dir(self.path.join("journals")).map_err(|_| "journal_directory_unavailable")?
        {
            let entry = entry.map_err(|_| "journal_directory_unavailable")?;
            let name = entry.file_name();
            let Some(run) = name
                .to_str()
                .and_then(|s| s.strip_suffix(".juro").or_else(|| s.strip_suffix(".ojo")))
            else {
                continue;
            };
            if opaque(run) {
                identities.insert(run.to_string());
            }
            if identities.len() > 10_000 {
                return Err("journal_count_bound");
            }
        }
        for run in identities {
            sessions.push(self.manifest(&run)?);
        }
        sessions.sort_by(|a, b| {
            b.last_activity_us
                .cmp(&a.last_activity_us)
                .then(a.run_id.cmp(&b.run_id))
        });
        Ok(sessions)
    }
    pub fn get(&self, run: &str, sequence: u64) -> Result<Record> {
        if let Some(value) = self.remote(super::owner::Action::Get {
            run: run.into(),
            sequence,
        })? {
            return Ok(value);
        }
        self.with_db(run, |db| get_from(db, sequence))
    }
    pub fn page(&self, run: &str, start: u64, limit: usize) -> Result<Page> {
        if let Some(value) = self.remote(super::owner::Action::Page {
            run: run.into(),
            start,
            limit,
        })? {
            return Ok(value);
        }
        self.with_db(run, |db| page_from(db, start, limit))
    }
    pub fn search(&self, run: &str, query: &str, limit: usize) -> Result<Vec<Record>> {
        if let Some(value) = self.remote(super::owner::Action::Search {
            run: run.into(),
            query: query.into(),
            limit,
        })? {
            return Ok(value);
        }
        self.with_db(run, |db| search_from(db, query, limit))
    }
    pub fn finish(&self, run: &str, finished: bool) -> Result<Manifest> {
        if let Some(value) = self.remote(super::owner::Action::Finish {
            run: run.into(),
            finished,
        })? {
            return Ok(value);
        }
        self.with_db(run, |db| finish_from(db, finished))
    }
    pub fn archive(&self, run: &str) -> Result<Manifest> {
        if let Some(value) = self.remote(super::owner::Action::Archive { run: run.into() })? {
            return Ok(value);
        }
        let mut pool = self
            .pool
            .as_ref()
            .map(|p| p.lock().map_err(|_| "owner_unhealthy"))
            .transpose()?;
        if let Some(pool) = pool.as_mut() {
            pool.writers.retain(|w| w.run != run);
            pool.manifests.remove(run);
        }
        self.archive_direct(run)
    }
    fn archive_direct(&self, run: &str) -> Result<Manifest> {
        let _lock = self.lock(run)?;
        let mut db = self.open_db(run, false)?;
        let mut m = read_manifest(&mut db)?;
        let path = self.session_path(run)?;
        let temporary = self
            .path
            .join("journals")
            .join(format!(".archive-{}.juro", hosted_capture::random_id()?));
        db.archive_to(&temporary).map_err(error)?;
        drop(db);
        let checked = (|| {
            let mut archive = Database::open(&temporary, self.key.as_ref(), true).map_err(error)?;
            let actual = read_manifest(&mut archive)?;
            if actual.run_id != run || actual.records != m.records {
                return Err("archive_manifest_mismatch");
            }
            m.archived_us = Some(now());
            archive
                .commit(
                    ojo::request_id().map_err(error)?,
                    &[Change {
                        key: MANIFEST,
                        value: Some(&encoded_manifest(&m)?),
                    }],
                )
                .map_err(error)?;
            drop(archive);
            fs::rename(&temporary, &path).map_err(|_| "archive_publication_outcome_unknown")?;
            File::open(path.parent().ok_or("invalid_journal_path")?)
                .and_then(|f| f.sync_all())
                .map_err(|_| "archive_publication_outcome_unknown")?;
            Ok(m)
        })();
        if checked.is_err() && temporary.exists() {
            let _ = fs::remove_file(temporary);
        }
        checked
    }
    pub fn archive_idle(&self, time_us: i64) -> Result<usize> {
        if let Some(value) = self.remote(super::owner::Action::ArchiveIdle { time_us })? {
            return Ok(value);
        }
        let policy: RootConfig = serde_json::from_slice(
            &read_private(&self.path.join("root.json"), 4096)
                .map_err(|_| "private_journal_root_required")?,
        )
        .map_err(|_| "invalid_journal_root")?;
        if policy.identity != self.cfg.identity || policy.format != FORMAT {
            return Err("journal_root_binding_mismatch");
        }
        if !policy.automatic_archive {
            return Ok(0);
        }
        let mut count = 0;
        for m in self.sessions()? {
            if m.archived_us.is_none()
                && m.finished_us.is_some_and(|finished| {
                    time_us.saturating_sub(finished.max(m.last_activity_us)) >= 86_400_000_000
                })
            {
                self.archive(&m.run_id)?;
                count += 1;
            }
        }
        Ok(count)
    }
    pub fn set_automatic_archive(&mut self, enabled: bool) -> Result<()> {
        if let Some(()) = self.remote(super::owner::Action::ArchivePolicy { enabled })? {
            return Ok(());
        }
        self.cfg.automatic_archive = enabled;
        let temporary = self
            .path
            .join(format!(".root-{}", hosted_capture::random_id()?));
        hosted_capture::create_private(
            &temporary,
            &serde_json::to_vec(&self.cfg).map_err(|_| "invalid_journal_root")?,
        )?;
        fs::rename(&temporary, self.path.join("root.json"))
            .map_err(|_| "root_policy_outcome_unknown")?;
        File::open(&self.path)
            .and_then(|f| f.sync_all())
            .map_err(|_| "root_policy_outcome_unknown")
    }
    pub fn export(&self, run: &str, output: &mut impl Write) -> Result<u64> {
        let total = self.manifest(run)?.records;
        let mut next = 0;
        while next < total {
            let page = self.page(run, next, (total - next).min(100) as usize)?;
            if page.records.is_empty() {
                return Err("journal_export_range_lost");
            }
            for record in &page.records {
                serde_json::to_writer(&mut *output, record).map_err(|_| "journal_export_failed")?;
                output
                    .write_all(b"\n")
                    .map_err(|_| "journal_export_failed")?;
            }
            next += page.records.len() as u64;
        }
        Ok(total)
    }
}
#[derive(Default)]
struct Pool {
    writers: VecDeque<DirectWriter>,
    manifests: BTreeMap<String, (Stamp, Manifest)>,
    opens: u64,
}
#[derive(Clone, Eq, PartialEq)]
struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    modified: i64,
    modified_ns: i64,
    changed: i64,
    changed_ns: i64,
}
pub(crate) struct DirectWriter {
    _guard: File,
    db: Database,
    run: String,
    stamp: Stamp,
}
pub(crate) enum SessionWriter {
    Direct(Box<DirectWriter>),
    Shared { root: JournalRoot, run: String },
}
impl SessionWriter {
    pub(crate) fn append(&mut self, request: &str, records: &[Record]) -> Result<u64> {
        match self {
            Self::Direct(writer) => append_to(&mut writer.db, &writer.run, request, records),
            Self::Shared { root, run } => root.append(run, request, records),
        }
    }
}
fn append_to(db: &mut Database, run: &str, batch_request: &str, records: &[Record]) -> Result<u64> {
    if records.is_empty()
        || records.len() > 100
        || batch_request.is_empty()
        || batch_request.len() > 512
    {
        return Err("journal_batch_bound");
    }
    let mut manifest = read_manifest(db)?;
    let fingerprint = batch_fingerprint(records)?;
    let batch_key = [
        b"b/".as_slice(),
        blake3::hash(batch_request.as_bytes()).as_bytes(),
    ]
    .concat();
    if let Some(previous) = db.get(&batch_key).map_err(error)? {
        if previous.as_slice() != fingerprint {
            return Err("journal_request_conflict");
        }
        db.confirm().map_err(error)?;
        return Ok(0);
    }
    let mut changes: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut added = 0u64;
    let mut seen = std::collections::BTreeSet::new();
    for record in records {
        if !opaque(&record.event_id)
            || record.run_id != run
            || record.input.agent_id != manifest.agent_id
            || record.adapter != manifest.adapter
            || record.input.text.len() > MAX_TEXT
            || !seen.insert(&record.event_id)
        {
            return Err("invalid_journal_record");
        }
        let lookup = id_key(&record.event_id);
        if let Some(index) = db.get(&lookup).map_err(error)? {
            let index = u64::from_be_bytes(
                index
                    .as_slice()
                    .try_into()
                    .map_err(|_| "invalid_journal_index")?,
            );
            let old = read_record(db, index)?;
            let mut proposed = record.clone();
            proposed.sequence = index;
            if serde_json::to_vec(&old).map_err(|_| "invalid_journal_record")?
                != serde_json::to_vec(&proposed).map_err(|_| "invalid_journal_record")?
            {
                return Err("journal_event_conflict");
            }
            continue;
        }
        let index = manifest
            .records
            .checked_add(added)
            .ok_or("journal_record_bound")?;
        let mut metadata = record.clone();
        metadata.sequence = index;
        let text = std::mem::take(&mut metadata.input.text).into_bytes();
        let metadata = serde_json::to_vec(&metadata).map_err(|_| "invalid_journal_record")?;
        if metadata.len() > MAX_METADATA {
            return Err("journal_metadata_bound");
        }
        changes.push((lookup, index.to_be_bytes().to_vec()));
        changes.push((key(b"m/", index), metadata));
        changes.push((
            key(if record.input.indexed { b"v/" } else { b"s/" }, index),
            text,
        ));
        added += 1;
    }
    if added > 0 {
        manifest.records = manifest
            .records
            .checked_add(added)
            .ok_or("journal_record_bound")?;
        manifest.last_activity_us = now();
        manifest.finished_us = None;
        manifest.archived_us = None;
        changes.push((MANIFEST.to_vec(), encoded_manifest(&manifest)?));
    }
    // An acknowledgement binds this request even if every event was already
    // accepted through another batch. Preserve the journal lifecycle metadata.
    changes.push((batch_key, fingerprint.to_vec()));
    let refs = changes
        .iter()
        .map(|(key, value)| Change {
            key,
            value: Some(value),
        })
        .collect::<Vec<_>>();
    db.commit(
        request(b"ojo-journal/batch/v1", batch_request.as_bytes()),
        &refs,
    )
    .map_err(error)?;
    Ok(added)
}
fn read_record(db: &mut Database, index: u64) -> Result<Record> {
    let metadata = db
        .get(&key(b"m/", index))
        .map_err(error)?
        .ok_or("record_not_found")?;
    let mut r: Record = serde_json::from_slice(&metadata).map_err(|_| "invalid_journal_record")?;
    if r.sequence != index || !r.input.text.is_empty() || !opaque(&r.event_id) || !opaque(&r.run_id)
    {
        return Err("invalid_journal_record");
    }
    let text = db
        .get(&key(if r.input.indexed { b"v/" } else { b"s/" }, index))
        .map_err(error)?
        .ok_or("original_not_found")?;
    r.input.text = String::from_utf8(text).map_err(|_| "invalid_journal_utf8")?;
    Ok(r)
}

fn get_from(db: &mut Database, sequence: u64) -> Result<Record> {
    let m = read_manifest(db)?;
    if sequence >= m.records {
        return Err("record_not_found");
    }
    read_record(db, sequence)
}

fn page_from(db: &mut Database, start: u64, limit: usize) -> Result<Page> {
    if limit == 0 || limit > 100 {
        return Err("journal_page_bound");
    }
    let m = read_manifest(db)?;
    let end = m.records.min(start.saturating_add(limit as u64));
    let mut records = Vec::new();
    if start >= end {
        return Ok(Page {
            records,
            next: None,
            total: m.records,
        });
    }
    let after = start.checked_sub(1).map(|i| key(b"m/", i));
    db.scan_prefix(
        b"m/",
        after.as_deref(),
        (end - start) as usize,
        |k, value| {
            if k.len() != 10
                || u64::from_be_bytes(k[2..].try_into().unwrap()) != start + records.len() as u64
            {
                return Err(ojo::Error::Corrupt("journal metadata sequence"));
            }
            let record: Record = serde_json::from_slice(value)
                .map_err(|_| ojo::Error::Corrupt("journal metadata"))?;
            if record.sequence != start + records.len() as u64
                || !record.input.text.is_empty()
                || !opaque(&record.event_id)
                || !opaque(&record.run_id)
            {
                return Err(ojo::Error::Corrupt("journal metadata"));
            }
            records.push(record);
            Ok(())
        },
    )
    .map_err(error)?;
    if records.len() != (end - start) as usize {
        return Err("invalid_journal_index");
    }
    let mut bytes = 0usize;
    let mut position = 0;
    while position < records.len() {
        let prefix = if records[position].input.indexed {
            b"v/"
        } else {
            b"s/"
        };
        let group_start = position;
        let eligibility = records[position].input.indexed;
        let mut group_end = position + 1;
        while group_end < records.len() && records[group_end].input.indexed == eligibility {
            group_end += 1;
        }
        let after = records[position]
            .sequence
            .checked_sub(1)
            .map(|i| key(prefix, i));
        let mut full = false;
        let result = db.scan_prefix(
            prefix,
            after.as_deref(),
            group_end - group_start,
            |k, value| {
                if k.len() != 10
                    || u64::from_be_bytes(k[2..].try_into().unwrap()) != records[position].sequence
                {
                    return Err(ojo::Error::Corrupt("journal original sequence"));
                }
                if bytes + value.len() > MAX_TEXT {
                    full = true;
                    return Err(ojo::Error::Invalid("journal page complete"));
                }
                records[position].input.text = String::from_utf8(value.to_vec())
                    .map_err(|_| ojo::Error::Corrupt("journal original UTF-8"))?;
                bytes += value.len();
                position += 1;
                Ok(())
            },
        );
        if full {
            records.truncate(position);
            break;
        }
        result.map_err(error)?;
        if position != group_end {
            return Err("original_not_found");
        }
    }
    let next = start.saturating_add(records.len() as u64);
    Ok(Page {
        records,
        next: (next < m.records).then_some(next),
        total: m.records,
    })
}

fn search_from(db: &mut Database, query: &str, limit: usize) -> Result<Vec<Record>> {
    if query.is_empty() || query.len() > 4096 || limit == 0 || limit > 100 {
        return Err("journal_query_bound");
    }
    let m = read_manifest(db)?;
    let terms = query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    if terms.is_empty() || terms.len() > 32 {
        return Err("journal_query_bound");
    }
    let mut matches = Vec::new();
    let mut failure = None;
    db.scan(|_, _, k, value| {
        if failure.is_some() || !k.starts_with(b"v/") || k.len() != 10 {
            return;
        }
        let index = u64::from_be_bytes(k[2..].try_into().unwrap());
        if index >= m.records {
            failure = Some("invalid_journal_index");
            return;
        }
        let Some(value) = value else {
            failure = Some("deleted_journal_original");
            return;
        };
        let Ok(text) = std::str::from_utf8(value) else {
            failure = Some("invalid_journal_utf8");
            return;
        };
        let lower = text.to_lowercase();
        if terms.iter().all(|term| lower.contains(term)) {
            matches.push(index);
            if matches.len() > limit {
                matches.remove(0);
            }
        }
    })
    .map_err(error)?;
    if let Some(e) = failure {
        return Err(e);
    }
    matches.reverse();
    let mut records = Vec::new();
    let mut bytes = 0usize;
    for index in matches {
        let record = read_record(db, index)?;
        bytes = bytes
            .checked_add(record.input.text.len())
            .ok_or("search_output_bound")?;
        if bytes > MAX_TEXT {
            break;
        }
        if !record.input.indexed {
            return Err("journal_index_eligibility_mismatch");
        }
        records.push(record);
    }
    Ok(records)
}

fn finish_from(db: &mut Database, finished: bool) -> Result<Manifest> {
    let mut m = read_manifest(db)?;
    m.finished_us = finished.then(now);
    if !finished {
        m.archived_us = None;
        m.last_activity_us = now();
    }
    db.commit(
        ojo::request_id().map_err(error)?,
        &[Change {
            key: MANIFEST,
            value: Some(&encoded_manifest(&m)?),
        }],
    )
    .map_err(error)?;
    Ok(m)
}
