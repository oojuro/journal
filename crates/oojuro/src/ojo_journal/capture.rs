use super::{JournalRoot, Record, Result, store};
use crate::{hosted::read_private, hosted_capture as shared};
use oojuro_capture::{FileCtx, Format, ParseCounts, redact};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

const LINE_LIMIT: usize = 16 << 20;
const FILE_LIMIT: usize = 10_000;
const STATE_LIMIT: usize = 40 << 20;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    format: u32,
    adapter: String,
    source_root: PathBuf,
    label: String,
    #[serde(default)]
    include_existing: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    dev: u64,
    ino: u64,
    offset: u64,
    boundary: String,
    skip_partial: bool,
    context: FileCtx,
    run: Option<String>,
    #[serde(default)]
    finished: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    format: u32,
    source_root: PathBuf,
    adapter: String,
    files: BTreeMap<PathBuf, Source>,
}
#[derive(Default, Serialize)]
pub struct CaptureReport {
    pub stored: u64,
    pub deduplicated: u64,
    pub new_lines: usize,
    pub tracked_files: usize,
    pub sessions: Vec<String>,
    pub storage: &'static str,
}
fn json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| "invalid_capture_state")
}
fn save(root: &JournalRoot, path: &Path, bytes: &[u8]) -> Result<()> {
    if bytes.len() > STATE_LIMIT {
        return Err("capture_state_bound");
    }
    let temporary = root.path.join(format!(".capture-{}", shared::random_id()?));
    shared::create_private(&temporary, bytes)?;
    fs::rename(&temporary, path).map_err(|_| "capture_state_outcome_unknown")?;
    File::open(&root.path)
        .and_then(|f| f.sync_all())
        .map_err(|_| "capture_state_outcome_unknown")
}
fn initial_source(path: &Path, format: Format, exclude_existing: bool) -> Result<Source> {
    let mut file = shared::transcript(path)?;
    let m = file.metadata().map_err(|_| "capture_source_io")?;
    let offset = if exclude_existing { m.len() } else { 0 };
    let boundary = shared::boundary(&mut file, offset)?;
    let skip_partial = if offset == 0 {
        false
    } else {
        file.seek(SeekFrom::Start(offset - 1))
            .map_err(|_| "capture_source_io")?;
        let mut last = [0];
        file.read_exact(&mut last)
            .map_err(|_| "capture_source_io")?;
        last[0] != b'\n'
    };
    Ok(Source {
        dev: m.dev(),
        ino: m.ino(),
        offset,
        boundary,
        skip_partial,
        context: shared::context(format, path),
        run: None,
        finished: false,
    })
}
fn paths(root: &JournalRoot, adapter: &str) -> Result<(PathBuf, PathBuf)> {
    Format::from_name(adapter).ok_or("supported_adapters: codex, claude-code")?;
    Ok((
        root.path.join(format!("capture-{adapter}.json")),
        root.path.join(format!("cursor-{adapter}.json")),
    ))
}
fn capture_lock(root: &JournalRoot, adapter: &str) -> Result<File> {
    paths(root, adapter)?;
    let path = root.path.join(format!("capture-{adapter}.lock"));
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|_| "capture_lock_unavailable")?;
    read_private(&path, 0).map_err(|_| "private_capture_lock_required")?;
    lock.try_lock().map_err(|_| "capture_already_running")?;
    Ok(lock)
}
pub fn connect_local(
    root: &JournalRoot,
    source_root: &Path,
    adapter: &str,
    label: &str,
    include_existing: bool,
) -> Result<CaptureReport> {
    let format = Format::from_name(adapter).ok_or("supported_adapters: codex, claude-code")?;
    if label.is_empty()
        || label.len() > 96
        || label.trim() != label
        || label.chars().any(char::is_control)
    {
        return Err("invalid_capture_label");
    }
    let _lock = capture_lock(root, adapter)?;
    let (cfg_path, state_path) = paths(root, adapter)?;
    let listed = shared::scan(source_root, format)?;
    let cfg = Config {
        format: 1,
        adapter: adapter.into(),
        source_root: source_root.into(),
        label: label.into(),
        include_existing,
    };
    if cfg_path.exists() || state_path.exists() {
        let old: Config = serde_json::from_slice(
            &read_private(&cfg_path, 8192).map_err(|_| "private_capture_configuration_required")?,
        )
        .map_err(|_| "invalid_capture_configuration")?;
        if old.format != 1
            || old.adapter != adapter
            || old.source_root != source_root
            || include_existing && !old.include_existing
        {
            return Err("capture_configuration_conflict; existing cursor retained");
        }
        save(root, &cfg_path, &json(&cfg)?)?;
        return Ok(CaptureReport {
            storage: "ojo",
            ..Default::default()
        });
    }
    let mut state = State {
        format: 1,
        source_root: source_root.into(),
        adapter: adapter.into(),
        files: BTreeMap::new(),
    };
    for path in listed {
        state.files.insert(
            path.clone(),
            initial_source(&path, format, !include_existing)?,
        );
    }
    shared::create_private(&state_path, &json(&state)?)?;
    shared::create_private(&cfg_path, &json(&cfg)?)?;
    Ok(CaptureReport {
        tracked_files: state.files.len(),
        storage: "ojo",
        ..Default::default()
    })
}
pub fn capture_once(root: &JournalRoot, adapter: &str) -> Result<CaptureReport> {
    let _lock = capture_lock(root, adapter)?;
    let (cfg_path, state_path) = paths(root, adapter)?;
    capture_configured(root, adapter, &cfg_path, &state_path)
}
fn capture_configured(
    root: &JournalRoot,
    adapter: &str,
    cfg_path: &Path,
    state_path: &Path,
) -> Result<CaptureReport> {
    let format = Format::from_name(adapter).ok_or("supported_adapters: codex, claude-code")?;
    let cfg: Config = serde_json::from_slice(
        &read_private(cfg_path, 8192).map_err(|_| "private_capture_configuration_required")?,
    )
    .map_err(|_| "invalid_capture_configuration")?;
    let mut state: State = serde_json::from_slice(
        &read_private(state_path, STATE_LIMIT).map_err(|_| "private_capture_cursor_required")?,
    )
    .map_err(|_| "invalid_capture_state")?;
    if cfg.format != 1
        || state.format != 1
        || cfg.adapter != adapter
        || state.adapter != adapter
        || cfg.source_root != state.source_root
        || state.files.len() > FILE_LIMIT
    {
        return Err("capture_state_binding_mismatch");
    }
    let mut report = CaptureReport {
        storage: "ojo",
        ..Default::default()
    };
    let red = redact::Redactor::new();
    for path in shared::scan(&cfg.source_root, format)? {
        if !state.files.contains_key(&path) {
            if state.files.len() >= FILE_LIMIT {
                return Err("capture_file_bound");
            }
            state
                .files
                .insert(path.clone(), initial_source(&path, format, false)?);
        }
        let source = state.files.get_mut(&path).ok_or("invalid_capture_state")?;
        let mut file = shared::transcript(&path)?;
        let m = file.metadata().map_err(|_| "capture_source_io")?;
        if source.dev != m.dev()
            || source.ino != m.ino()
            || m.len() < source.offset
            || shared::boundary(&mut file, source.offset)? != source.boundary
        {
            return Err("capture_source_changed; original journal retained");
        }
        file.seek(SeekFrom::Start(source.offset))
            .map_err(|_| "capture_source_io")?;
        let mut reader = BufReader::new(file);
        for _ in 0..100 {
            let mut line = Vec::new();
            reader
                .by_ref()
                .take((LINE_LIMIT + 1) as u64)
                .read_until(b'\n', &mut line)
                .map_err(|_| "capture_source_io")?;
            if line.len() > LINE_LIMIT {
                return Err("capture_line_bound; source not advanced");
            }
            if line.last() != Some(&b'\n') {
                break;
            }
            let offset = source.offset;
            let mut parsed = Vec::new();
            let mut rc = redact::Counts::default();
            let mut pc = ParseCounts::default();
            if !source.skip_partial {
                let parse = match format {
                    Format::Codex => oojuro_capture::codex::parse_line,
                    Format::ClaudeCode => oojuro_capture::claude::parse_line,
                };
                parse(
                    &mut source.context,
                    &line,
                    offset,
                    &red,
                    &mut rc,
                    &mut pc,
                    &mut parsed,
                );
            }
            if pc.bad_lines != 0 {
                return Err("invalid_transcript_line; source not advanced");
            }
            let run = match &source.run {
                Some(run) => run.clone(),
                None => {
                    let run = shared::journal_run(&path, format, root.identity())?;
                    source.run = Some(run.clone());
                    run
                }
            };
            let records = parsed
                .into_iter()
                .enumerate()
                .map(|(k, r)| {
                    shared::journal_record(r, root.identity(), &run, offset, k, &line, format)
                })
                .collect::<Result<Vec<Record>>>()?;
            let reported_finish = if source.skip_partial {
                false
            } else {
                serde_json::from_slice::<serde_json::Value>(&line)
                    .ok()
                    .is_some_and(|value| {
                        format == Format::Codex
                            && value["type"] == "event_msg"
                            && value["payload"]["type"] == "task_complete"
                            || format == Format::ClaudeCode
                                && value["type"] == "system"
                                && value["subtype"] == "session_end"
                    })
            };
            if source.finished && !reported_finish && root.session_path(&run)?.exists() {
                root.finish(&run, false)?;
                source.finished = false;
            }
            if !records.is_empty() {
                let text_bytes = records.iter().map(|r| r.input.text.len()).sum::<usize>();
                if records.len() > 100 || text_bytes > store::MAX_TEXT {
                    return Err("capture_line_event_or_text_bound");
                }
                let name = shared::journal_name(&cfg.label, adapter, &run);
                root.ensure_session(&run, root.identity(), &records[0].adapter, &name)?;
                let request = blake3::hash(
                    [
                        run.as_bytes(),
                        &offset.to_le_bytes(),
                        blake3::hash(&line).as_bytes(),
                    ]
                    .concat()
                    .as_slice(),
                )
                .to_hex()
                .to_string();
                let added = root.append(&run, &request, &records)?;
                report.stored += added;
                report.deduplicated += records.len() as u64 - added;
                if !report.sessions.contains(&run) {
                    report.sessions.push(run.clone());
                }
            }
            if reported_finish && root.session_path(&run)?.exists() {
                root.finish(&run, true)?;
                source.finished = true;
            }
            // Only advance after the entire accepted line is durable in its .ojo.
            // A crash before cursor publication replays stable IDs without another record.
            source.offset += line.len() as u64;
            source.skip_partial = false;
            source.boundary = shared::boundary(&mut shared::transcript(&path)?, source.offset)?;
            report.new_lines += 1;
            if report.new_lines >= 100 {
                break;
            }
        }
        save(root, state_path, &json(&state)?)?;
        if report.new_lines >= 100 {
            break;
        }
    }
    report.tracked_files = state.files.len();
    Ok(report)
}

#[derive(Serialize)]
pub struct Verification {
    pub session: String,
    pub sequence: u64,
    pub synthetic: bool,
    pub parser_and_local_durability_verified: bool,
}
/// Sends an owned synthetic transcript through the same parser/cursor/.ojo path.
/// It never writes to the provider's actual transcript directory.
pub fn verify_pipeline(
    root: &JournalRoot,
    adapter: &str,
    cloud_scope: Option<&str>,
) -> Result<Verification> {
    let format = Format::from_name(adapter).ok_or("supported_adapters: codex, claude-code")?;
    let _lock = capture_lock(root, adapter)?;
    let identity = blake3::hash(
        format!(
            "ojo-journal/setup/v1:{}:{adapter}:{}",
            root.identity(),
            cloud_scope.unwrap_or("local")
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string();
    let verification = root.path.join("verification");
    if !verification.exists() {
        shared::directory(&verification)?;
    }
    let sources = verification.join(&identity);
    if !sources.exists() {
        shared::directory(&sources)?;
    }
    let path = sources.join(if format == Format::Codex {
        "rollout-verification.jsonl"
    } else {
        "verification.jsonl"
    });
    let text = "Synthetic ojo-journal setup verification; this is not a user conversation.";
    let value = match format {
        Format::Codex => {
            serde_json::json!({"type":"session_meta","timestamp":"2026-10-25T00:00:00Z","payload":{"id":identity,"base_instructions":{"text":text}}})
        }
        Format::ClaudeCode => {
            serde_json::json!({"type":"system","subtype":"ojo-journal-setup","sessionId":identity,"timestamp":"2026-10-25T00:00:00Z","content":text})
        }
    };
    let mut bytes = serde_json::to_vec(&value).map_err(|_| "invalid_verification_fixture")?;
    bytes.push(b'\n');
    if path.exists() {
        if read_private(&path, 65536).map_err(|_| "private_verification_fixture_required")? != bytes
        {
            return Err("verification_fixture_conflict");
        }
    } else {
        shared::create_private(&path, &bytes)?;
    }
    let cfg_path = root.path.join(format!("verify-{identity}.json"));
    let state_path = root.path.join(format!("verify-cursor-{identity}.json"));
    if !cfg_path.exists() {
        let cfg = Config {
            format: 1,
            adapter: adapter.into(),
            source_root: sources.clone(),
            label: "Setup verification".into(),
            include_existing: true,
        };
        if !state_path.exists() {
            shared::create_private(
                &state_path,
                &json(&State {
                    format: 1,
                    source_root: sources,
                    adapter: adapter.into(),
                    files: BTreeMap::new(),
                })?,
            )?;
        }
        shared::create_private(&cfg_path, &json(&cfg)?)?;
    }
    capture_configured(root, adapter, &cfg_path, &state_path)?;
    let run = shared::journal_run(&path, format, root.identity())?;
    let record = root.get(&run, 0)?;
    if record.input.text != text || record.input.indexed {
        return Err("capture_verification_failed");
    }
    root.finish(&run, true)?;
    Ok(Verification {
        session: run,
        sequence: 0,
        synthetic: true,
        parser_and_local_durability_verified: true,
    })
}

/// Setup may race the healthy owner's short capture tick. Only this known busy
/// condition is retried; source conflicts and uncertain writes retain their errors.
pub(crate) fn setup_retry<T>(mut action: impl FnMut() -> Result<T>) -> Result<T> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match action() {
            Err("capture_already_running") if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            result => return result,
        }
    }
}
