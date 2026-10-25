//! Validates application state during the engine's existing verified replay,
//! before any writable tail recovery. No complete originals are retained.
use super::grammar::{Batch, Cell, PROFILE};
use ojo::{
    Error, Result,
    events::{Cursor, Replay},
};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct State {
    pub values: BTreeMap<Vec<u8>, Cell>,
    pub requests: BTreeMap<[u8; 16], Cursor>,
    pub unfinished: Option<Batch>,
    pub anchor: Option<Cursor>,
}
impl State {
    pub fn observe(&mut self, replay: Replay<'_>) -> Result<()> {
        let Replay::Event { receipt, data } = replay else {
            return Ok(());
        };
        if self.anchor.is_none() {
            if data != PROFILE {
                return Err(Error::Invalid("Journal application profile required"));
            }
            self.anchor = Some(receipt.cursor);
            return Ok(());
        }
        match self.unfinished.take() {
            None => self.unfinished = Some(Batch::parse(receipt.cursor, data)?),
            Some(mut batch) => {
                if let Some(intent) = batch.next_intent() {
                    intent.check_event(receipt, data)?;
                }
                if batch.observe(receipt.cursor, data)? {
                    apply(&mut self.values, &mut self.requests, batch)?;
                } else {
                    self.unfinished = Some(batch);
                }
            }
        }
        Ok(())
    }
}
pub(super) fn apply(
    values: &mut BTreeMap<Vec<u8>, Cell>,
    requests: &mut BTreeMap<[u8; 16], Cursor>,
    batch: Batch,
) -> Result<()> {
    // App profile1 leaves original values, eligibility and record identity
    // immutable; only manifest lifecycle metadata may be replaced. Validate
    // this in replay as well as live writes before trusting an index prefix.
    let mut previous = values
        .range(b"v/".to_vec()..b"w".to_vec())
        .next_back()
        .map(|(key, _)| key.clone());
    for (key, value) in &batch.changes {
        if key != b"manifest" && (value.is_none() || values.contains_key(key)) {
            return Err(Error::Corrupt("immutable Journal application value"));
        }
        if key.starts_with(b"v/") {
            if key.len() != 10
                || previous.as_ref().is_some_and(|previous| key <= previous)
                || value
                    .as_ref()
                    .is_none_or(|cell| cell.original_proof.is_none())
            {
                return Err(Error::Corrupt("Journal indexed immutable prefix"));
            }
            previous = Some(key.clone());
        }
    }
    if requests.contains_key(&batch.request) {
        return Err(Error::Corrupt("duplicate application transaction"));
    }
    requests.insert(batch.request, batch.begin_cursor);
    for (key, cell) in batch.changes {
        match cell {
            Some(cell) => {
                values.insert(key, cell);
            }
            None => {
                values.remove(&key);
            }
        }
    }
    Ok(())
}
