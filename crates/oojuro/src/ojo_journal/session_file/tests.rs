use super::*;
use crate::hosted_capture;
use std::fs;

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
        let changes = [
            Change {
                key: b"m/\0\0\0\0\0\0\0\0",
                value: Some(b"synthetic metadata"),
            },
            Change {
                key: VALUE,
                value: Some(&original),
            },
        ];
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
        legacy.commit([5; 16], &changes).unwrap();
        drop(legacy);
        let before = fs::read(&legacy_path).unwrap();
        let mut compatibility = SessionFile::open(&legacy_path, key.as_ref(), true).unwrap();
        assert!(matches!(compatibility, SessionFile::Legacy(_)));
        assert_eq!(compatibility.get(VALUE).unwrap().unwrap(), original);
        assert_eq!(fs::read(&legacy_path).unwrap(), before);
    }
}
