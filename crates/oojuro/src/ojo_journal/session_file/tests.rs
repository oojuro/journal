use super::*;
use crate::hosted_capture;
use std::{fs, io::Write};

struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ojo-event-app-{}",
            hosted_capture::random_id().unwrap()
        ));
        hosted_capture::directory(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
const VALUE: &[u8] = b"v/\0\0\0\0\0\0\0\0";
fn legacy_values(original: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let run = "1".repeat(64);
    let agent = "2".repeat(64);
    let event = "3".repeat(64);
    let manifest = super::super::store::Manifest {
        format: 1,
        run_id: run.clone(),
        agent_id: agent.clone(),
        adapter: "synthetic/v1".into(),
        label: "Legacy".into(),
        records: 1,
        created_us: 0,
        last_activity_us: 0,
        finished_us: None,
        archived_us: None,
    };
    let metadata = super::super::store::Record {
        sequence: 0,
        event_id: event.clone(),
        run_id: run,
        adapter: "synthetic/v1".into(),
        input: oojuro_engine::record::Input {
            agent_id: agent,
            parent_id: None,
            valid_at: 0,
            kind: oojuro_engine::record::Kind::Explicit,
            tool: None,
            status: None,
            exit: None,
            key: None,
            text: String::new(),
            attrs: Default::default(),
            indexed: true,
        },
    };
    vec![
        (b"manifest".to_vec(), serde_json::to_vec(&manifest).unwrap()),
        (
            [b"i/".as_slice(), event.as_bytes()].concat(),
            0u64.to_be_bytes().to_vec(),
        ),
        (
            b"m/\0\0\0\0\0\0\0\0".to_vec(),
            serde_json::to_vec(&metadata).unwrap(),
        ),
        (VALUE.to_vec(), original.to_vec()),
    ]
}
fn accepted(path: &Path, key: Option<&[u8; 32]>, original: &[u8]) -> Vec<(Cursor, i64)> {
    let mut journal = Journal::open(path, key, false).unwrap();
    let mut found = Vec::new();
    let mut after = None;
    loop {
        let page = journal
            .scan(after, 32, |event| {
                if event.data == original {
                    found.push((event.cursor, event.accepted_at));
                }
                Ok(())
            })
            .unwrap();
        after = page.cursor;
        if !page.has_more {
            break;
        }
    }
    found
}
#[test]
fn original_intents_survive_torn_begin_original_commit_and_lost_ack() {
    for key in [None, Some([61; 32])] {
        let scratch = Scratch::new();
        let path = scratch.0.join("session.juro");
        let original = format!("OJJCOM1\0 exact café\0{}", "synthetic ".repeat(256)).into_bytes();
        let values = legacy_values(&original);
        let changes = values
            .iter()
            .map(|(key, value)| Change {
                key,
                value: Some(value),
            })
            .collect::<Vec<_>>();
        let mut file = SessionFile::create(&path, key.as_ref()).unwrap();
        let start = fs::metadata(&path).unwrap().len() as usize;
        file.commit([2; 16], &changes).unwrap();
        drop(file);
        let full = fs::read(&path).unwrap();
        let original_receipt = accepted(&path, key.as_ref(), &original);
        assert_eq!(original_receipt.len(), 1);
        // Measure frame overhead through the public engine, rather than parse
        // private offsets. New group frames are raw under either codec feature.
        let mut probe = Journal::open(&path, key.as_ref(), true).unwrap();
        probe.append(b"").unwrap();
        drop(probe);
        let overhead = fs::metadata(&path).unwrap().len() as usize - full.len();
        let commit_end = full.len();
        let original_end = commit_end - overhead - 56;
        let begin_end = original_end - overhead - original.len();
        for cut in [
            start,
            start + 1,
            begin_end - 1,
            begin_end,
            begin_end + 1,
            original_end - 1,
            original_end,
            original_end + 1,
            commit_end - 1,
            commit_end,
        ] {
            fs::write(&path, &full[..cut]).unwrap();
            let mut replay = SessionFile::open(&path, key.as_ref(), true).unwrap();
            if (begin_end..original_end).contains(&cut) {
                assert_eq!(
                    replay.get(VALUE).unwrap(),
                    None,
                    "uncommitted original escaped"
                );
                assert!(matches!(
                    replay.commit([3; 16], &changes),
                    Err(Error::Invalid(
                        "pending_journal_batch_requires_original_retry"
                    ))
                ));
                let different = [Change {
                    key: VALUE,
                    value: Some(b"different original"),
                }];
                assert!(replay.commit([2; 16], &different).is_err());
            }
            replay.commit([2; 16], &changes).unwrap();
            assert_eq!(
                replay.get(VALUE).unwrap().as_deref(),
                Some(original.as_slice())
            );
            let confirmed = fs::read(&path).unwrap();
            replay.commit([2; 16], &changes).unwrap();
            assert_eq!(
                fs::read(&path).unwrap(),
                confirmed,
                "exact retry appended control or data duplicates"
            );
            drop(replay);
            let surviving = accepted(&path, key.as_ref(), &original);
            assert_eq!(surviving.len(), 1);
            if cut >= begin_end {
                assert_eq!(
                    surviving, original_receipt,
                    "retained original engine time/cursor changed"
                );
            }
        }
    }
}
#[test]
fn full_original_limit_and_legacy_kv_compatibility_remain_available() {
    for key in [None, Some([62; 32])] {
        let scratch = Scratch::new();
        let path = scratch.0.join("session.juro");
        let original = vec![b'x'; 16 << 20];
        let changes = [Change {
            key: VALUE,
            value: Some(&original),
        }];
        let mut file = SessionFile::create(&path, key.as_ref()).unwrap();
        file.commit([4; 16], &changes).unwrap();
        drop(file);
        let mut replay = SessionFile::open(&path, key.as_ref(), true).unwrap();
        assert_eq!(replay.get(VALUE).unwrap().unwrap(), original);
        drop(replay);
        let legacy_path = scratch.0.join("legacy.ojo");
        let mut legacy = Database::create(&legacy_path, key.as_ref()).unwrap();
        let values = legacy_values(&original);
        let legacy_changes = values
            .iter()
            .map(|(key, value)| Change {
                key,
                value: Some(value),
            })
            .collect::<Vec<_>>();
        legacy.commit([5; 16], &legacy_changes).unwrap();
        drop(legacy);
        let before = fs::read(&legacy_path).unwrap();
        let mut compatibility = SessionFile::open(&legacy_path, key.as_ref(), true).unwrap();
        assert!(matches!(compatibility, SessionFile::Legacy(_)));
        assert_eq!(compatibility.get(VALUE).unwrap().unwrap(), original);
        assert_eq!(fs::read(&legacy_path).unwrap(), before);
    }
}

#[test]
fn application_profile_and_commit_refusal_never_recovers_foreign_tail() {
    for key in [None, Some([63; 32])] {
        let scratch = Scratch::new();
        for malformed_commit in [false, true] {
            let path = scratch.0.join(if malformed_commit {
                "bad-commit.juro"
            } else {
                "foreign.juro"
            });
            if malformed_commit {
                drop(SessionFile::create(&path, key.as_ref()).unwrap());
                let mut journal = Journal::open(&path, key.as_ref(), true).unwrap();
                let original = b"exact private synthetic original\0";
                let intent = journal.prepare(original).unwrap();
                let changes = [Change {
                    key: VALUE,
                    value: Some(original),
                }];
                let (begin, _) = super::grammar::encode([6; 16], &changes, &[intent]).unwrap();
                journal.append(&begin).unwrap();
                journal.append_prepared(&intent, original).unwrap();
                journal.append(b"unrelated complete control").unwrap();
            } else {
                Journal::create(&path, key.as_ref())
                    .unwrap()
                    .append(b"different application profile")
                    .unwrap();
            }
            fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(b"torn physical tail")
                .unwrap();
            let before = fs::read(&path).unwrap();
            for writable in [false, true] {
                assert!(SessionFile::open(&path, key.as_ref(), writable).is_err());
                assert_eq!(
                    fs::read(&path).unwrap(),
                    before,
                    "application refusal truncated another profile's tail"
                );
            }
        }
    }
}

#[test]
fn full_latest_legacy_layout_is_checked_before_any_tail_recovery() {
    for key in [None, Some([64; 32])] {
        for damage in [
            "missing_manifest",
            "bad_manifest",
            "bad_metadata",
            "wrong_eligibility",
            "bad_id",
            "unexpected_key",
            "invalid_utf8",
            "missing_original",
        ] {
            let scratch = Scratch::new();
            let path = scratch.0.join("legacy.juro");
            let mut values = legacy_values(b"exact original");
            match damage {
                "missing_manifest" => values.retain(|(key, _)| key != b"manifest"),
                "bad_manifest" => values[0].1 = b"unknown application".to_vec(),
                "bad_metadata" => values[2].1 = b"malformed record".to_vec(),
                "wrong_eligibility" => values[3].0[0] = b's',
                "bad_id" => values[1].1 = 1u64.to_be_bytes().to_vec(),
                "unexpected_key" => {
                    values.push((b"foreign/required-feature".to_vec(), b"opaque".to_vec()))
                }
                "invalid_utf8" => values[3].1 = vec![255],
                "missing_original" => {
                    values.pop();
                }
                _ => unreachable!(),
            }
            let mut db = Database::create(&path, key.as_ref()).unwrap();
            let changes = values
                .iter()
                .map(|(key, value)| Change {
                    key,
                    value: Some(value),
                })
                .collect::<Vec<_>>();
            db.commit([7; 16], &changes).unwrap();
            drop(db);
            fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(b"preserved incomplete tail")
                .unwrap();
            let before = fs::read(&path).unwrap();
            for writable in [false, true] {
                assert!(
                    SessionFile::open(&path, key.as_ref(), writable).is_err(),
                    "accepted {damage}"
                );
                assert_eq!(
                    fs::read(&path).unwrap(),
                    before,
                    "recovered {damage} before refusing its layout"
                );
            }
        }
    }
}

#[test]
fn only_explicit_initialization_adopts_empty_interrupted_files() {
    for key in [None, Some([65; 32])] {
        for event_profile in [false, true] {
            let scratch = Scratch::new();
            let path = scratch.0.join("empty.juro");
            if event_profile {
                drop(Journal::create(&path, key.as_ref()).unwrap());
            } else {
                drop(Database::create(&path, key.as_ref()).unwrap());
            }
            fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(b"interrupted initialization")
                .unwrap();
            let before = fs::read(&path).unwrap();
            for writable in [false, true] {
                assert!(SessionFile::open(&path, key.as_ref(), writable).is_err());
                assert_eq!(fs::read(&path).unwrap(), before);
            }
            drop(SessionFile::open_initializing(&path, key.as_ref()).unwrap());
            assert!(fs::read(&path).unwrap().len() < before.len() || event_profile);
        }
    }
}
