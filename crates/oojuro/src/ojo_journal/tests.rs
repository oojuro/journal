use super::*;
use crate::hosted_capture;
use oojuro_engine::record::{Input, Kind};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "ojo-session-test-{}",
            hosted_capture::random_id().unwrap()
        ));
        hosted_capture::directory(&p).unwrap();
        Self(p)
    }
    fn root(&self, encrypted: bool) -> JournalRoot {
        JournalRoot::init(&self.0.join("root"), encrypted).unwrap()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn record(root: &JournalRoot, run: &str, n: u64, text: &str, indexed: bool) -> Record {
    Record {
        sequence: 0,
        event_id: blake3::hash(&n.to_be_bytes()).to_hex().to_string(),
        run_id: run.into(),
        adapter: "synthetic/v1".into(),
        input: Input {
            agent_id: root.identity().into(),
            parent_id: None,
            valid_at: n as i64,
            kind: Kind::Explicit,
            tool: None,
            status: None,
            exit: None,
            key: None,
            text: text.into(),
            attrs: BTreeMap::new(),
            indexed,
        },
    }
}

#[test]
fn raw_and_encrypted_sessions_preserve_originals_retry_conflicts_pages_and_archives() {
    for encrypted in [false, true] {
        let scratch = Scratch::new();
        let root = scratch.root(encrypted);
        let run = "1".repeat(64);
        root.ensure_session(&run, root.identity(), "synthetic/v1", "Synthetic session")
            .unwrap();
        let records = (0..64)
            .map(|i| {
                record(
                    &root,
                    &run,
                    i,
                    &format!("exact\0 original café\r\n{}", "repeat ".repeat(1024)),
                    i != 0,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(root.append(&run, "batch-one", &records).unwrap(), 64);
        let path = root.path.join("journals").join(format!("{run}.juro"));
        let before = fs::read(&path).unwrap();
        assert_eq!(root.append(&run, "batch-one", &records).unwrap(), 0);
        assert_eq!(fs::read(&path).unwrap(), before);
        let mut changed = records[0].clone();
        changed.input.text.push_str(" changed");
        assert_eq!(
            root.append(&run, "different-request", &[changed]).err(),
            Some("journal_event_conflict")
        );
        assert_eq!(root.manifest(&run).unwrap().records, 64);
        assert_eq!(root.get(&run, 0).unwrap().input.text, records[0].input.text);
        let page = root.page(&run, 60, 3).unwrap();
        assert_eq!(page.next, Some(63));
        assert_eq!(page.records.len(), 3);
        let hits = root.search(&run, "exact café", 100).unwrap();
        assert_eq!(hits.len(), 63);
        assert_eq!(hits[0].sequence, 63);
        root.finish(&run, true).unwrap();
        let finished = root.manifest(&run).unwrap();
        assert_eq!(
            root.archive_idle(finished.finished_us.unwrap() + 86_399_999_999)
                .unwrap(),
            0
        );
        assert_eq!(
            root.archive_idle(finished.finished_us.unwrap() + 86_400_000_000)
                .unwrap(),
            1
        );
        assert!(fs::metadata(&path).unwrap().len() < before.len() as u64);
        let reopened = JournalRoot::open(&root.path).unwrap();
        assert_eq!(
            reopened.get(&run, 63).unwrap().input.text,
            records[63].input.text
        );
        assert_eq!(reopened.append(&run, "batch-one", &records).unwrap(), 0);
        assert_eq!(
            reopened
                .append(&run, "replay-after-archive", &records[..2])
                .unwrap(),
            0
        );
        let resumed = record(&reopened, &run, 64, "new after archival", true);
        assert_eq!(reopened.append(&run, "resumed", &[resumed]).unwrap(), 1);
        let m = reopened.manifest(&run).unwrap();
        assert!(m.finished_us.is_none());
        assert!(m.archived_us.is_none());
        assert_eq!(m.records, 65);
    }
}

fn header(id: &str) -> Vec<u8> {
    format!(
        "{}\n",
        serde_json::json!({"type":"session_meta","payload":{"id":id}})
    )
    .into_bytes()
}
fn codex(text: &str) -> Vec<u8> {
    format!("{}\n",serde_json::json!({"type":"response_item","timestamp":"2026-10-24T12:00:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}})).into_bytes()
}
fn append(path: &std::path::Path, bytes: &[u8]) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

#[test]
fn actual_transcript_capture_is_account_free_session_stable_and_cursor_replay_safe() {
    let scratch = Scratch::new();
    let root = scratch.root(true);
    let sources = scratch.0.join("sessions");
    hosted_capture::directory(&sources).unwrap();
    let first = sources.join("rollout-first.jsonl");
    fs::write(
        &first,
        [
            header("provider-session-one"),
            codex("old history excluded"),
        ]
        .concat(),
    )
    .unwrap();
    connect_local(&root, &sources, "codex", "Code reviewer", false).unwrap();
    assert_eq!(capture_once(&root, "codex").unwrap().stored, 0);
    let cursor_before = fs::read(root.path.join("cursor-codex.json")).unwrap();
    append(&first, &codex("new original café\r\ncontext"));
    let report = capture_once(&root, "codex").unwrap();
    assert_eq!(report.stored, 1);
    assert_eq!(report.sessions.len(), 1);
    let run = report.sessions[0].clone();
    assert_eq!(
        root.get(&run, 0).unwrap().input.text,
        "new original café\r\ncontext"
    );
    // A killed process can commit .juro before publishing its private cursor.
    fs::write(root.path.join("cursor-codex.json"), cursor_before).unwrap();
    let replay = capture_once(&root, "codex").unwrap();
    assert_eq!(replay.stored, 0);
    assert_eq!(replay.deduplicated, 1);
    assert_eq!(root.manifest(&run).unwrap().records, 1);
    // A provider resume at a different filename keeps the stable session route.
    let resumed = sources.join("rollout-resumed.jsonl");
    fs::write(
        &resumed,
        [header("provider-session-one"), codex("resumed decision")].concat(),
    )
    .unwrap();
    let partial = sources.join("rollout-second.jsonl");
    let bytes = codex("separate session");
    fs::write(
        &partial,
        [
            header("provider-session-two"),
            bytes[..bytes.len() - 1].to_vec(),
        ]
        .concat(),
    )
    .unwrap();
    capture_once(&root, "codex").unwrap();
    assert_eq!(root.sessions().unwrap().len(), 1);
    assert_eq!(root.manifest(&run).unwrap().records, 2);
    append(&partial, b"\n");
    capture_once(&root, "codex").unwrap();
    assert_eq!(root.sessions().unwrap().len(), 2);
    let hits = root.search(&run, "decision", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].input.text, "resumed decision");
    assert!(!root.path.join("cloud.json").exists());
    assert_eq!(
        fs::read(&first).unwrap(),
        [
            header("provider-session-one"),
            codex("old history excluded"),
            codex("new original café\r\ncontext")
        ]
        .concat()
    );
}

#[test]
fn capture_redacts_secrets_and_rejects_changed_source_without_losing_originals() {
    let scratch = Scratch::new();
    let root = scratch.root(true);
    let sources = scratch.0.join("sources");
    hosted_capture::directory(&sources).unwrap();
    connect_local(&root, &sources, "codex", "Synthetic", false).unwrap();
    let file = sources.join("rollout-test.jsonl");
    let secret = format!("ojc_{}_{}", "1".repeat(64), "A".repeat(43));
    fs::write(&file, [header("run"), codex(&secret)].concat()).unwrap();
    capture_once(&root, "codex").unwrap();
    let run = root.sessions().unwrap()[0].run_id.clone();
    let record = root.get(&run, 0).unwrap();
    assert!(record.input.text.starts_with("[REDACTED:oojuro:"));
    assert!(!record.input.text.contains(&secret));
    fs::write(&file, b"source was replaced\n").unwrap();
    assert!(capture_once(&root, "codex").is_err());
    assert_eq!(root.manifest(&run).unwrap().records, 1);
    assert_eq!(root.get(&run, 0).unwrap().input.text, record.input.text);
}

#[test]
fn duplicate_only_new_batch_is_durably_bound_without_resuming_an_archived_session() {
    for encrypted in [false, true] {
        let scratch = Scratch::new();
        let root = scratch.root(encrypted);
        let run = "2".repeat(64);
        root.ensure_session(&run, root.identity(), "synthetic/v1", "Retry fixture")
            .unwrap();
        let original = record(&root, &run, 1, "exact retained original", true);
        root.append(&run, "original-batch", std::slice::from_ref(&original))
            .unwrap();
        root.finish(&run, true).unwrap();
        root.archive(&run).unwrap();
        let before = serde_json::to_vec(&root.manifest(&run).unwrap()).unwrap();
        assert_eq!(
            root.append(&run, "duplicate-only-ack", std::slice::from_ref(&original))
                .unwrap(),
            0
        );
        assert_eq!(
            serde_json::to_vec(&root.manifest(&run).unwrap()).unwrap(),
            before
        );
        let path = root.session_path(&run).unwrap();
        let accepted = fs::read(&path).unwrap();
        let reopened = JournalRoot::open(&root.path).unwrap();
        assert_eq!(
            reopened
                .append(&run, "duplicate-only-ack", std::slice::from_ref(&original))
                .unwrap(),
            0
        );
        assert_eq!(fs::read(&path).unwrap(), accepted);
        let changed = record(&reopened, &run, 2, "different event must conflict", true);
        assert_eq!(
            reopened
                .append(&run, "duplicate-only-ack", &[changed])
                .err(),
            Some("journal_request_conflict")
        );
        assert_eq!(reopened.manifest(&run).unwrap().records, 1);
        assert_eq!(
            reopened.get(&run, 0).unwrap().input.text,
            original.input.text
        );
        reopened.archive(&run).unwrap();
        assert_eq!(
            JournalRoot::open(&root.path)
                .unwrap()
                .append(&run, "duplicate-only-ack", &[original])
                .unwrap(),
            0
        );
    }
}

#[test]
fn juro_defaults_preserve_explicit_legacy_paths_and_refuse_dual_candidates() {
    for encrypted in [false, true] {
        let scratch = Scratch::new();
        let root = scratch.root(encrypted);
        let run = "3".repeat(64);
        root.ensure_session(&run, root.identity(), "synthetic/v1", "Filename fixture")
            .unwrap();
        let original = record(&root, &run, 1, "unchanged accepted original", true);
        root.append(&run, "first-name-batch", std::slice::from_ref(&original))
            .unwrap();
        let canonical = root.path.join("journals").join(format!("{run}.juro"));
        let legacy = canonical.with_extension("ojo");
        assert!(canonical.exists());
        assert!(!legacy.exists());
        assert_eq!(&fs::read(&canonical).unwrap()[..8], b"OJOFILE\0");
        // Fixture relocation simulates an old store. Application calls keep using
        // the legacy path; no automatic rename or replacement occurs.
        fs::rename(&canonical, &legacy).unwrap();
        let old = fs::read(&legacy).unwrap();
        let reopened = JournalRoot::open(&root.path).unwrap();
        assert_eq!(
            reopened.get(&run, 0).unwrap().input.text,
            original.input.text
        );
        assert_eq!(
            reopened
                .append(&run, "first-name-batch", std::slice::from_ref(&original))
                .unwrap(),
            0
        );
        assert_eq!(fs::read(&legacy).unwrap(), old);
        assert!(!canonical.exists());
        assert_eq!(reopened.sessions().unwrap().len(), 1);
        let next = record(&reopened, &run, 2, "append at original legacy path", true);
        reopened.append(&run, "legacy-next", &[next]).unwrap();
        assert!(legacy.exists());
        assert!(!canonical.exists());
        fs::copy(&legacy, &canonical).unwrap();
        assert!(reopened.get(&run, 0).is_err());
        assert!(reopened.sessions().is_err());
        assert!(reopened.archive(&run).is_err());
        assert!(
            reopened
                .ensure_session(
                    &run,
                    reopened.identity(),
                    "synthetic/v1",
                    "Filename fixture"
                )
                .is_err()
        );
        fs::remove_file(&canonical).unwrap();
        assert_eq!(reopened.manifest(&run).unwrap().records, 2);
    }
}
