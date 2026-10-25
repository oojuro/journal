//! Explicit copy-first migration. The source is locked, snapshotted byte-for-byte
//! into an encrypted .ojo recovery artifact, and decoded only from a private copy.
use super::{JournalRoot, Record, Result, store};
use crate::{hosted::read_private, hosted_capture as shared};
use ojo::{Change, Database};
use oojuro_engine::record::{Input, Kind, Status};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
};
const CHUNK: usize = 4 << 20;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceIdentity {
    format: u32,
    source: PathBuf,
    id: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    format: u32,
    source_id: String,
    id: String,
    layout: String,
    files: usize,
    bytes: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: String,
    size: u64,
    mode: u32,
    digest: String,
}
#[derive(Serialize)]
pub struct MigrationReport {
    pub source_id: String,
    pub recovery_artifact: String,
    pub records: u64,
    pub deduplicated: u64,
    pub sessions: usize,
    pub source_untouched: bool,
}
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_path_buf()];
    let mut output = Vec::new();
    let mut names = 0usize;
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).map_err(|_| "legacy_source_unavailable")? {
            let entry = entry.map_err(|_| "legacy_source_unavailable")?;
            let path = entry.path();
            let info = fs::symlink_metadata(&path).map_err(|_| "legacy_source_unavailable")?;
            if info.uid() != unsafe { libc::geteuid() } || info.mode() & 0o022 != 0 {
                return Err("legacy_source_owner_permissions_required");
            }
            if info.is_dir() {
                pending.push(path);
            } else if info.is_file() {
                let relative = path.strip_prefix(root).map_err(|_| "invalid_legacy_path")?;
                let name = relative.to_str().ok_or("legacy_utf8_paths_required")?;
                names += name.len();
                if name.len() > 4096 || names > 4 << 20 || output.len() >= 10_000 {
                    return Err("legacy_file_inventory_bound");
                }
                output.push(relative.to_path_buf());
            } else {
                return Err("legacy_source_contains_link_or_special_file; stop_the_recorder_first");
            }
        }
    }
    output.sort();
    Ok(output)
}
fn plain_file(path: &Path) -> Result<File> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "legacy_source_unavailable")?;
    if !f
        .metadata()
        .map_err(|_| "legacy_source_unavailable")?
        .is_file()
    {
        return Err("regular_legacy_file_required");
    }
    Ok(f)
}
fn read_chunk(input: &mut impl Read, buffer: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match input.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err("legacy_source_unavailable"),
        }
    }
    Ok(filled)
}
fn sync_directories(root: &Path) -> Result<()> {
    let mut pending = vec![root.to_path_buf()];
    let mut dirs = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).map_err(|_| "migration_restore_failed")? {
            let entry = entry.map_err(|_| "migration_restore_failed")?;
            if entry
                .file_type()
                .map_err(|_| "migration_restore_failed")?
                .is_dir()
            {
                pending.push(entry.path());
            }
        }
        dirs.push(dir);
    }
    for dir in dirs.into_iter().rev() {
        File::open(dir)
            .and_then(|f| f.sync_all())
            .map_err(|_| "migration_restore_failed")?;
    }
    Ok(())
}
fn chunk_key(file: usize, chunk: u64) -> Vec<u8> {
    format!("file/{file:08x}/{chunk:016x}").into_bytes()
}
fn descriptor_key(file: usize) -> Vec<u8> {
    format!("entry/{file:08x}").into_bytes()
}
fn map(_: ojo::Error) -> &'static str {
    "migration_recovery_artifact_unavailable_or_corrupt"
}
fn source_id(root: &JournalRoot, source: &Path, directory: &Path) -> Result<String> {
    let key = blake3::hash(source.as_os_str().as_encoded_bytes())
        .to_hex()
        .to_string();
    let path = directory.join(format!("source-{key}.json"));
    if path.exists() {
        let saved: SourceIdentity = serde_json::from_slice(
            &read_private(&path, 8192).map_err(|_| "private_migration_identity_required")?,
        )
        .map_err(|_| "invalid_migration_identity")?;
        if saved.format != 1 || saved.source != source || !store::opaque(&saved.id) {
            return Err("migration_identity_conflict");
        }
        return Ok(saved.id);
    }
    let id = blake3::hash(
        format!(
            "ojo-journal/migration-source/v1:{}:{}",
            root.identity(),
            source.display()
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string();
    shared::create_private(
        &path,
        &serde_json::to_vec(&SourceIdentity {
            format: 1,
            source: source.into(),
            id: id.clone(),
        })
        .map_err(|_| "invalid_migration_identity")?,
    )?;
    Ok(id)
}
pub fn migrate_legacy(root: &JournalRoot, source: &Path) -> Result<MigrationReport> {
    if !root.encrypted() {
        return Err("migration_requires_an_encrypted_destination_root");
    }
    shared::ancestors(source)?;
    let info = fs::symlink_metadata(source).map_err(|_| "legacy_source_unavailable")?;
    if !info.is_dir()
        || info.uid() != unsafe { libc::geteuid() }
        || info.mode() & 0o777 != 0o700
        || root.path.starts_with(source)
    {
        return Err("separate_private_legacy_source_required");
    }
    let layout = if source.join("native.json").is_file() {
        "native"
    } else if source.join("record.json").is_file() {
        "record"
    } else if crate::journal::exists(source) {
        "trace"
    } else {
        return Err("unsupported_legacy_journal");
    };
    let inventory = files(source)?;
    let mut locks = Vec::new();
    for relative in inventory
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "LOCK"))
    {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(source.join(relative))
            .map_err(|_| "legacy_lock_unavailable")?;
        file.try_lock()
            .map_err(|_| "legacy_journal_busy; stop_the_recorder_before_migration")?;
        locks.push(file);
    }
    if locks.is_empty() {
        return Err("legacy_journal_lock_missing");
    }
    let directory = root.path.join("migrations");
    if !directory.exists() {
        shared::directory(&directory)?;
    }
    let _migration_lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join("lock"))
        .map_err(|_| "migration_lock_unavailable")?;
    _migration_lock
        .try_lock()
        .map_err(|_| "migration_already_running")?;
    let source_id = source_id(root, source, &directory)?;
    let scratch = Scratch(
        root.path
            .join(format!(".migration-work-{}", shared::random_id()?)),
    );
    shared::directory(&scratch.0)?;
    let mut entries = Vec::new();
    let mut total = 0u64;
    let mut identity = blake3::Hasher::new();
    identity.update(source_id.as_bytes());
    for relative in &inventory {
        let from = source.join(relative);
        let mut input = plain_file(&from)?;
        let info = input.metadata().map_err(|_| "legacy_source_unavailable")?;
        let to = scratch.0.join(relative);
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(to.parent().ok_or("invalid_legacy_path")?)
            .map_err(|_| "migration_copy_failed")?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(info.mode() & 0o777)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&to)
            .map_err(|_| "migration_copy_failed")?;
        let mut hash = blake3::Hasher::new();
        let mut copied = 0u64;
        let mut buffer = vec![0; CHUNK];
        loop {
            let n = read_chunk(&mut input, &mut buffer)?;
            if n == 0 {
                break;
            }
            output
                .write_all(&buffer[..n])
                .map_err(|_| "migration_copy_failed")?;
            hash.update(&buffer[..n]);
            copied += n as u64;
        }
        let after = input.metadata().map_err(|_| "legacy_source_unavailable")?;
        if copied != info.len()
            || after.len() != info.len()
            || after.mtime() != info.mtime()
            || after.mtime_nsec() != info.mtime_nsec()
        {
            return Err("legacy_source_changed; original_source_retained");
        }
        let entry = Entry {
            path: relative
                .to_str()
                .ok_or("legacy_utf8_paths_required")?
                .into(),
            size: copied,
            mode: info.mode() & 0o777,
            digest: hash.finalize().to_hex().to_string(),
        };
        let meta = serde_json::to_vec(&entry).map_err(|_| "invalid_migration_inventory")?;
        identity.update(&(meta.len() as u64).to_le_bytes());
        identity.update(&meta);
        total = total.checked_add(copied).ok_or("migration_byte_bound")?;
        entries.push(entry);
    }
    let artifact = identity.finalize().to_hex().to_string();
    let final_path = directory.join(format!("{artifact}.ojo"));
    if !final_path.exists() {
        let temporary = directory.join(format!(".migration-pending-{}.ojo", shared::random_id()?));
        let result: Result<()> = (|| {
            let mut db = Database::create(&temporary, root.file_key()).map_err(map)?;
            for (index, entry) in entries.iter().enumerate() {
                let descriptor =
                    serde_json::to_vec(entry).map_err(|_| "invalid_migration_inventory")?;
                db.commit(
                    ojo::request_id().map_err(map)?,
                    &[Change {
                        key: &descriptor_key(index),
                        value: Some(&descriptor),
                    }],
                )
                .map_err(map)?;
                let mut input = plain_file(&scratch.0.join(&entry.path))?;
                let mut buffer = vec![0; CHUNK];
                let mut chunk = 0;
                loop {
                    let n = read_chunk(&mut input, &mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    db.commit(
                        ojo::request_id().map_err(map)?,
                        &[Change {
                            key: &chunk_key(index, chunk),
                            value: Some(&buffer[..n]),
                        }],
                    )
                    .map_err(map)?;
                    chunk += 1;
                }
            }
            let manifest = serde_json::to_vec(&Snapshot {
                format: 1,
                source_id: source_id.clone(),
                id: artifact.clone(),
                layout: layout.into(),
                files: entries.len(),
                bytes: total,
            })
            .map_err(|_| "invalid_migration_inventory")?;
            db.commit(
                ojo::request_id().map_err(map)?,
                &[Change {
                    key: b"migration",
                    value: Some(&manifest),
                }],
            )
            .map_err(map)?;
            drop(db);
            fs::rename(&temporary, &final_path)
                .map_err(|_| "migration_publication_outcome_unknown")?;
            File::open(&directory)
                .and_then(|f| f.sync_all())
                .map_err(|_| "migration_publication_outcome_unknown")?;
            Ok(())
        })();
        if result.is_err() && temporary.exists() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
    } else {
        let mut db = Database::open(&final_path, root.file_key(), false).map_err(map)?;
        let m: Snapshot = serde_json::from_slice(
            &db.get(b"migration")
                .map_err(map)?
                .ok_or("incomplete_migration_artifact")?,
        )
        .map_err(|_| "invalid_migration_inventory")?;
        if m.format != 1 || m.id != artifact || m.source_id != source_id {
            return Err("migration_artifact_conflict");
        }
    }
    // Only the private copy is opened by legacy code: recovery and derived cache
    // work can never mutate the source the user asked to preserve.
    let mut importer = Importer {
        root,
        source_id: &source_id,
        report: MigrationReport {
            source_id: source_id.clone(),
            recovery_artifact: artifact.clone(),
            records: 0,
            deduplicated: 0,
            sessions: 0,
            source_untouched: true,
        },
        sessions: BTreeSet::new(),
        active: BTreeMap::new(),
        order: VecDeque::new(),
    };
    match layout {
        "native" => native(&mut importer, &scratch.0)?,
        "record" => direct(&mut importer, &scratch.0)?,
        _ => trace(&mut importer, &scratch.0)?,
    }
    for run in importer.order.clone() {
        importer.flush(&run)?;
    }
    importer.report.sessions = importer.sessions.len();
    drop(locks);
    Ok(importer.report)
}
struct Active {
    writer: store::SessionWriter,
    records: Vec<Record>,
    bytes: usize,
}
struct Importer<'a> {
    root: &'a JournalRoot,
    source_id: &'a str,
    report: MigrationReport,
    sessions: BTreeSet<String>,
    active: BTreeMap<String, Active>,
    order: VecDeque<String>,
}
impl Importer<'_> {
    fn flush(&mut self, run: &str) -> Result<()> {
        let active = self.active.get_mut(run).ok_or("invalid_migration_writer")?;
        if active.records.is_empty() {
            return Ok(());
        }
        let mut hash = blake3::Hasher::new();
        hash.update(b"ojo-journal/migration-batch/v1");
        hash.update(self.source_id.as_bytes());
        hash.update(run.as_bytes());
        for record in &active.records {
            hash.update(record.event_id.as_bytes());
        }
        let request = hash.finalize().to_hex().to_string();
        let added = active.writer.append(&request, &active.records)?;
        self.report.records += added;
        self.report.deduplicated += active.records.len() as u64 - added;
        active.records.clear();
        active.bytes = 0;
        Ok(())
    }
    fn put(&mut self, session: &str, writer: &str, old_id: u64, mut input: Input) -> Result<()> {
        let source_id = self.source_id;
        let agent =
            blake3::hash(format!("ojo-journal/migrated-agent/v1:{source_id}:{writer}").as_bytes())
                .to_hex()
                .to_string();
        let run = blake3::hash(
            format!("ojo-journal/migrated-session/v1:{source_id}:{writer}:{session}").as_bytes(),
        )
        .to_hex()
        .to_string();
        let event =
            blake3::hash(format!("ojo-journal/migrated-event/v1:{source_id}:{old_id}").as_bytes())
                .to_hex()
                .to_string();
        input.agent_id = agent.clone();
        input.attrs.insert(
            "legacy_reference".into(),
            json!({"source_id":source_id,"id":old_id,"writer":writer,"session":session}),
        );
        let adapter = "ojo-journal/migration-v1";
        if !self.active.contains_key(&run) {
            if self.active.len() >= 8 {
                let old = self.order.pop_front().ok_or("invalid_migration_writer")?;
                self.flush(&old)?;
                self.active.remove(&old);
            }
            if self.sessions.insert(run.clone()) {
                self.root
                    .ensure_session(&run, &agent, adapter, "Imported legacy session")?;
            }
            self.active.insert(
                run.clone(),
                Active {
                    writer: self.root.writer(&run)?,
                    records: Vec::new(),
                    bytes: 0,
                },
            );
            self.order.push_back(run.clone());
        }
        let active = self.active.get(&run).ok_or("invalid_migration_writer")?;
        if !active.records.is_empty()
            && (active.records.len() >= 100 || active.bytes + input.text.len() > store::MAX_TEXT)
        {
            self.flush(&run)?;
        }
        let active = self
            .active
            .get_mut(&run)
            .ok_or("invalid_migration_writer")?;
        active.bytes += input.text.len();
        active.records.push(Record {
            sequence: 0,
            event_id: event,
            run_id: run,
            adapter: adapter.into(),
            input,
        });
        Ok(())
    }
}
fn kind(code: u8) -> Kind {
    match code {
        0 => Kind::System,
        1 => Kind::Input,
        2 => Kind::Thinking,
        3 => Kind::Output,
        4 => Kind::ToolCall,
        5 => Kind::ToolResult,
        _ => Kind::Explicit,
    }
}
fn native(importer: &mut Importer<'_>, copy: &Path) -> Result<()> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {
        version: u32,
        harness: String,
        writer: String,
        agent: u32,
        first_id: u64,
    }
    let cfg: Config = serde_json::from_slice(
        &read_private(&copy.join("native.json"), 4096)
            .map_err(|_| "invalid_legacy_configuration")?,
    )
    .map_err(|_| "invalid_legacy_configuration")?;
    if cfg.version != 1 {
        return Err("unsupported_legacy_configuration");
    }
    let adapter = match cfg.harness.as_str() {
        "codex" => oojuro_capture::local::Adapter::Codex,
        "claude_code" => oojuro_capture::local::Adapter::ClaudeCode,
        "pi" => oojuro_capture::local::Adapter::Pi,
        "temporal" => oojuro_capture::local::Adapter::Temporal,
        _ => return Err("unsupported_legacy_adapter"),
    };
    let key: [u8; 32] = read_private(&copy.join("key"), 32)
        .map_err(|_| "private_legacy_key_required")?
        .try_into()
        .map_err(|_| "invalid_legacy_key")?;
    let mut options =
        oojuro_capture::local::Options::new(&cfg.writer, cfg.agent, "migration-read-only");
    options.first_id = cfg.first_id;
    let reader =
        oojuro_capture::local::LocalRead::open(&copy.join("journal"), &key, options, adapter)
            .map_err(|_| "legacy_authority_refused")?;
    for batch in reader
        .authority()
        .batches()
        .map_err(|_| "legacy_authority_refused")?
    {
        for event in &batch.events {
            let input = Input {
                agent_id: String::new(),
                parent_id: None,
                valid_at: event.ts_us,
                kind: kind(event.kind),
                tool: (!event.tool.is_empty()).then(|| event.tool.clone()),
                status: match event.status {
                    2 => Some(Status::Ok),
                    3 => Some(Status::Error),
                    _ => None,
                },
                exit: event.exit_code,
                key: None,
                text: event.text.clone(),
                attrs: BTreeMap::new(),
                indexed: event.indexed,
            };
            importer.put(
                &hex(&event.session.to_be_bytes()),
                &cfg.writer,
                event.id,
                input,
            )?;
        }
    }
    Ok(())
}
fn direct(importer: &mut Importer<'_>, copy: &Path) -> Result<()> {
    use oojuro_engine::{
        capture_log::Identity,
        record::{AsOf, Filter, Limits, RecordLog, Scope, Versions, View},
    };
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {
        version: u32,
        writer: String,
        owner: u32,
        first_id: u64,
        adapter: String,
    }
    let cfg: Config = serde_json::from_slice(
        &read_private(&copy.join("record.json"), 4096)
            .map_err(|_| "invalid_legacy_configuration")?,
    )
    .map_err(|_| "invalid_legacy_configuration")?;
    if cfg.version != 1 {
        return Err("unsupported_legacy_configuration");
    }
    let key: [u8; 32] = read_private(&copy.join("key"), 32)
        .map_err(|_| "private_legacy_key_required")?
        .try_into()
        .map_err(|_| "invalid_legacy_key")?;
    let log = RecordLog::open(
        &copy.join("journal"),
        &key,
        Identity {
            writer: cfg.writer.clone(),
            agent: cfg.owner,
            first_id: cfg.first_id,
            adapter: cfg.adapter,
        },
    )
    .map_err(|_| "legacy_authority_refused")?;
    let scope = Scope {
        writer: &cfg.writer,
        owner: cfg.owner,
    };
    let view = View::open(&log, scope, AsOf::default()).map_err(|_| "legacy_authority_refused")?;
    let limits = Limits {
        entries: 100,
        bytes: 64 << 20,
    };
    let mut page = view
        .range(&log, Versions::All, Filter::default(), limits)
        .map_err(|_| "legacy_authority_refused")?;
    loop {
        for record in page.records {
            let session = record.agent_id.clone();
            let input = Input {
                agent_id: record.agent_id,
                parent_id: record.parent_id,
                valid_at: record.valid_at,
                kind: record.kind,
                tool: record.tool,
                status: record.status,
                exit: record.exit,
                key: record.key,
                text: record.text,
                attrs: record.attrs,
                indexed: record.indexed,
            };
            importer.put(&session, &cfg.writer, record.id, input)?;
        }
        match page.next {
            Some(cursor) => {
                page = View::continue_page(&log, scope, &cursor, limits)
                    .map_err(|_| "legacy_authority_refused")?
            }
            None => break,
        }
    }
    Ok(())
}
fn trace(importer: &mut Importer<'_>, copy: &Path) -> Result<()> {
    let journal = crate::Journal::open_existing(copy).map_err(|_| "legacy_authority_refused")?;
    journal
        .reader()
        .export(&crate::Filters::default(), &mut |record| {
            let input = Input {
                agent_id: String::new(),
                parent_id: record.parent_id,
                valid_at: record.ts_us,
                kind: kind(oojuro_capture::kind::code(&record.kind).unwrap_or(6)),
                tool: (!record.tool.is_empty()).then_some(record.tool),
                status: None,
                exit: None,
                key: record.key,
                text: record.text,
                attrs: BTreeMap::new(),
                indexed: record.kind != "system",
            };
            importer
                .put(&record.session, &record.writer, record.id, input)
                .map_err(Into::into)
        })
        .map_err(|_| "legacy_migration_failed")?;
    Ok(())
}
pub fn restore_migration(root: &JournalRoot, artifact: &str, out: &Path) -> Result<usize> {
    if !store::opaque(artifact) || !out.is_absolute() || out.exists() {
        return Err("new_absolute_restore_directory_required");
    }
    shared::ancestors(out)?;
    let path = root.path.join("migrations").join(format!("{artifact}.ojo"));
    let mut db = Database::open(path, root.file_key(), false).map_err(map)?;
    let manifest: Snapshot = serde_json::from_slice(
        &db.get(b"migration")
            .map_err(map)?
            .ok_or("incomplete_migration_artifact")?,
    )
    .map_err(|_| "invalid_migration_inventory")?;
    if manifest.format != 1 || manifest.id != artifact || manifest.files > 10_000 {
        return Err("invalid_migration_inventory");
    }
    let staging = out
        .parent()
        .ok_or("invalid_restore_path")?
        .join(format!(".ojo-restore-{}", shared::random_id()?));
    shared::directory(&staging)?;
    let result = (|| {
        for index in 0..manifest.files {
            let entry: Entry = serde_json::from_slice(
                &db.get(&descriptor_key(index))
                    .map_err(map)?
                    .ok_or("incomplete_migration_artifact")?,
            )
            .map_err(|_| "invalid_migration_inventory")?;
            let relative = Path::new(&entry.path);
            if relative.as_os_str().is_empty()
                || relative
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                || entry.path.len() > 4096
                || entry.mode & 0o022 != 0
            {
                return Err("invalid_migration_path");
            }
            let destination = staging.join(relative);
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(destination.parent().ok_or("invalid_migration_path")?)
                .map_err(|_| "migration_restore_failed")?;
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(entry.mode)
                .custom_flags(libc::O_NOFOLLOW)
                .open(destination)
                .map_err(|_| "migration_restore_failed")?;
            let mut size = 0u64;
            let mut hash = blake3::Hasher::new();
            for chunk in 0..entry.size.div_ceil(CHUNK as u64) {
                let bytes = db
                    .get(&chunk_key(index, chunk))
                    .map_err(map)?
                    .ok_or("incomplete_migration_artifact")?;
                if bytes.is_empty() || bytes.len() > CHUNK {
                    return Err("invalid_migration_chunk");
                }
                file.write_all(&bytes)
                    .map_err(|_| "migration_restore_failed")?;
                hash.update(&bytes);
                size += bytes.len() as u64;
            }
            if size != entry.size || hash.finalize().to_hex().as_str() != entry.digest {
                return Err("migration_restore_digest_mismatch");
            }
            file.sync_all().map_err(|_| "migration_restore_failed")?;
        }
        sync_directories(&staging)?;
        if out.exists() {
            return Err("restore_destination_exists");
        }
        // renameat2 NOREPLACE prevents replacing an independently created destination.
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let from =
            CString::new(staging.as_os_str().as_bytes()).map_err(|_| "invalid_restore_path")?;
        let to = CString::new(out.as_os_str().as_bytes()).map_err(|_| "invalid_restore_path")?;
        if unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err("migration_restore_publication_failed");
        }
        File::open(out.parent().ok_or("invalid_restore_path")?)
            .and_then(|f| f.sync_all())
            .map_err(|_| "migration_restore_publication_outcome_unknown")?;
        Ok(manifest.files)
    })();
    if result.is_err() && staging.exists() {
        let _ = fs::remove_dir_all(staging);
    }
    result
}
