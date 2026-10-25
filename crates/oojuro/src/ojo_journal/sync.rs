use super::{JournalRoot, Record, Result, store};
use crate::{
    hosted::read_private,
    hosted_capture::{self, JournalUpload},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    request: String,
    from: u64,
    count: usize,
    digest: String,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    next: u64,
    label: String,
    pending: Option<Pending>,
    #[serde(default)]
    reuse_journal: Option<String>,
    #[serde(default)]
    confirmed: u64,
    #[serde(default)]
    skipped_existing: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    format: u32,
    root: String,
    scope: String,
    sessions: BTreeMap<String, Cursor>,
}
#[derive(Default, Serialize)]
pub struct SyncReport {
    pub confirmed: u64,
    pub sessions: usize,
    pub batches: usize,
    pub caught_up: bool,
}

fn digest(records: &[Record]) -> Result<String> {
    let mut hash = blake3::Hasher::new();
    hash.update(b"ojo-journal/cloud-originals/v1");
    for record in records {
        let mut metadata = record.clone();
        metadata.input.text.clear();
        let bytes = serde_json::to_vec(&metadata).map_err(|_| "invalid_journal_record")?;
        hash.update(&(bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
        hash.update(&(record.input.text.len() as u64).to_le_bytes());
        hash.update(record.input.text.as_bytes());
    }
    Ok(hash.finalize().to_hex().to_string())
}
fn save(root: &JournalRoot, path: &Path, state: &State) -> Result<()> {
    let bytes = serde_json::to_vec(state).map_err(|_| "invalid_sync_state")?;
    if bytes.len() > 16 << 20 {
        return Err("sync_state_bound");
    }
    let temp = root
        .path
        .join(format!(".sync-{}", hosted_capture::random_id()?));
    hosted_capture::create_private(&temp, &bytes)?;
    fs::rename(&temp, path).map_err(|_| "sync_state_outcome_unknown")?;
    File::open(&root.path)
        .and_then(|f| f.sync_all())
        .map_err(|_| "sync_state_outcome_unknown")
}
pub(crate) fn initialize(
    root: &JournalRoot,
    connection: &Path,
    include_existing: bool,
) -> Result<String> {
    let uploader = JournalUpload::open(connection)?;
    let scope = uploader.scope().to_string();
    let path = root.path.join(format!("sync-{scope}.json"));
    let lock_path = root.path.join(format!("sync-{scope}.lock"));
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&lock_path)
        .map_err(|_| "sync_lock_unavailable")?;
    read_private(&lock_path, 0).map_err(|_| "private_sync_lock_required")?;
    lock.try_lock().map_err(|_| "sync_already_running")?;
    if path.exists() {
        let state: State = serde_json::from_slice(
            &read_private(&path, 16 << 20).map_err(|_| "private_sync_state_required")?,
        )
        .map_err(|_| "invalid_sync_state")?;
        if state.format != 1 || state.root != root.identity() || state.scope != scope {
            return Err("sync_state_binding_mismatch");
        }
        return Ok(scope);
    }
    let mut state = State {
        format: 1,
        root: root.identity().into(),
        scope: scope.clone(),
        sessions: BTreeMap::new(),
    };
    if !include_existing {
        for manifest in root.sessions()? {
            state.sessions.insert(
                manifest.run_id,
                Cursor {
                    next: manifest.records,
                    skipped_existing: manifest.records,
                    label: manifest.label,
                    reuse_journal: uploader.reuse_journal().map(str::to_owned),
                    ..Default::default()
                },
            );
        }
    }
    save(root, &path, &state)?;
    Ok(scope)
}
pub(crate) fn status(root: &JournalRoot) -> Result<Vec<serde_json::Value>> {
    let mut statuses = Vec::new();
    for entry in fs::read_dir(&root.path).map_err(|_| "sync_state_unavailable")? {
        let entry = entry.map_err(|_| "sync_state_unavailable")?;
        let name = entry.file_name();
        let Some(scope) = name
            .to_str()
            .and_then(|s| s.strip_prefix("sync-"))
            .and_then(|s| s.strip_suffix(".json"))
        else {
            continue;
        };
        if !store::opaque(scope) {
            continue;
        }
        let state: State = serde_json::from_slice(
            &read_private(&entry.path(), 16 << 20).map_err(|_| "private_sync_state_required")?,
        )
        .map_err(|_| "invalid_sync_state")?;
        if state.format != 1 || state.root != root.identity() || state.scope != scope {
            return Err("sync_state_binding_mismatch");
        }
        statuses.push(serde_json::json!({"scope":scope,"confirmed_records":state.sessions.values().map(|s|s.confirmed).sum::<u64>(),"excluded_existing_records":state.sessions.values().map(|s|s.skipped_existing).sum::<u64>(),"pending_intents":state.sessions.values().filter(|s|s.pending.is_some()).count(),"sessions":state.sessions.len()}));
    }
    Ok(statuses)
}
pub(crate) fn confirmed(root: &JournalRoot, scope: &str, run: &str, sequence: u64) -> Result<bool> {
    if !store::opaque(scope) || !store::opaque(run) {
        return Err("invalid_sync_scope");
    }
    let path = root.path.join(format!("sync-{scope}.json"));
    let state: State = serde_json::from_slice(
        &read_private(&path, 16 << 20).map_err(|_| "private_sync_state_required")?,
    )
    .map_err(|_| "invalid_sync_state")?;
    if state.format != 1 || state.root != root.identity() || state.scope != scope {
        return Err("sync_state_binding_mismatch");
    }
    Ok(state.sessions.get(run).is_some_and(|cursor| {
        cursor.next > sequence && cursor.confirmed > 0 && sequence >= cursor.skipped_existing
    }))
}
/// Local recording never waits for this network path. A pending intent and its
/// exact original range/digest are durable before any remote mutation.
pub fn sync_cloud(root: &JournalRoot, connection: &Path, max_batches: usize) -> Result<SyncReport> {
    if max_batches == 0 || max_batches > 100_000 {
        return Err("sync_batch_bound");
    }
    let uploader = JournalUpload::open(connection)?;
    let scope = uploader.scope();
    let path = root.path.join(format!("sync-{scope}.json"));
    let lock_path = root.path.join(format!("sync-{scope}.lock"));
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&lock_path)
        .map_err(|_| "sync_lock_unavailable")?;
    read_private(&lock_path, 0).map_err(|_| "private_sync_lock_required")?;
    lock.try_lock().map_err(|_| "sync_already_running")?;
    let mut state = if path.exists() {
        serde_json::from_slice::<State>(
            &read_private(&path, 16 << 20).map_err(|_| "private_sync_state_required")?,
        )
        .map_err(|_| "invalid_sync_state")?
    } else {
        State {
            format: 1,
            root: root.identity().into(),
            scope: scope.into(),
            sessions: BTreeMap::new(),
        }
    };
    if state.format != 1
        || state.root != root.identity()
        || state.scope != scope
        || state.sessions.len() > 10_000
    {
        return Err("sync_state_binding_mismatch");
    }
    let mut report = SyncReport {
        caught_up: true,
        ..Default::default()
    };
    for manifest in root.sessions()? {
        let run = &manifest.run_id;
        state.sessions.entry(run.clone()).or_insert_with(|| Cursor {
            label: manifest.label.clone(),
            reuse_journal: uploader.reuse_journal().map(str::to_owned),
            ..Default::default()
        });
        loop {
            let cursor = state.sessions.get(run).ok_or("invalid_sync_state")?;
            if cursor.next > manifest.records {
                return Err("sync_original_range_lost");
            }
            if cursor.next == manifest.records {
                break;
            }
            if report.batches >= max_batches {
                report.caught_up = false;
                return Ok(report);
            }
            let count = cursor.pending.as_ref().map_or(100, |p| p.count);
            let mut page = root.page(run, cursor.next, count)?;
            if page.records.is_empty() {
                return Err("sync_original_range_lost");
            }
            let bounded = JournalUpload::bounded_records(&page.records)?;
            if cursor.pending.is_some() && bounded != page.records.len() {
                return Err("pending_capture_envelope_refused; original_intent_retained");
            }
            page.records.truncate(bounded);
            let hash = digest(&page.records)?;
            if let Some(pending) = &cursor.pending {
                if pending.from != cursor.next
                    || pending.count != page.records.len()
                    || pending.digest != hash
                    || !store::opaque(&pending.request)
                {
                    return Err("sync_originals_changed; pending intent retained");
                }
            } else {
                let request = blake3::hash(
                    format!(
                        "ojo-journal/cloud-batch/v1:{scope}:{run}:{}:{hash}",
                        cursor.next
                    )
                    .as_bytes(),
                )
                .to_hex()
                .to_string();
                state
                    .sessions
                    .get_mut(run)
                    .ok_or("invalid_sync_state")?
                    .pending = Some(Pending {
                    request,
                    from: cursor.next,
                    count: page.records.len(),
                    digest: hash,
                });
                save(root, &path, &state)?;
            }
            let cursor = state.sessions.get(run).ok_or("invalid_sync_state")?;
            let pending = cursor.pending.as_ref().ok_or("invalid_sync_state")?;
            // page() released its file lock before this request. Capture can keep
            // appending locally while the uploader waits or the network is down.
            uploader.upload(
                run,
                &cursor.label,
                &pending.request,
                &page.records,
                cursor.reuse_journal.as_deref(),
            )?;
            let cursor = state.sessions.get_mut(run).ok_or("invalid_sync_state")?;
            cursor.next = cursor
                .next
                .checked_add(page.records.len() as u64)
                .ok_or("sync_count_bound")?;
            cursor.confirmed = cursor
                .confirmed
                .checked_add(page.records.len() as u64)
                .ok_or("sync_count_bound")?;
            cursor.pending = None;
            save(root, &path, &state)?;
            report.confirmed += page.records.len() as u64;
            report.batches += 1;
        }
        report.sessions += 1;
    }
    Ok(report)
}
