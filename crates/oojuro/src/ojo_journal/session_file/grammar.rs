use ojo::{
    Change, Error, Result,
    events::{AppendIntent, Cursor},
};
use std::collections::BTreeSet;

pub(super) const PROFILE: &[u8] = b"ojo-journal/application-events/1\0";
const BEGIN: &[u8; 8] = b"OJJBEG1\0";
const COMMIT: &[u8; 8] = b"OJJCOM1\0";
const MAX_DATA: usize = 16 << 20;
const MAX_CHANGES: usize = 1000;

#[derive(Clone)]
pub(super) struct Cell {
    pub cursor: Cursor,
    pub range: Option<(usize, usize)>,
    pub original_proof: Option<[u8; 32]>,
}
struct Original {
    key: Vec<u8>,
    length: usize,
    hash: [u8; 32],
    intent: AppendIntent,
}
pub(super) struct Batch {
    pub request: [u8; 16],
    pub changes: Vec<(Vec<u8>, Option<Cell>)>,
    originals: Vec<Original>,
    pub original_cursors: Vec<Cursor>,
    pub begin: Vec<u8>,
    pub begin_cursor: Cursor,
}
impl Batch {
    pub fn parse(cursor: Cursor, data: &[u8]) -> Result<Self> {
        if data.len() > MAX_DATA {
            return Err(Error::Corrupt("Journal Begin size"));
        }
        let mut reader = Reader::new(data);
        if reader.take(8)? != BEGIN {
            return Err(Error::Invalid("Journal application Begin required"));
        }
        let request = reader.take(16)?.try_into().unwrap();
        let count = reader.u32()?;
        if request == [0; 16] || !(1..=MAX_CHANGES).contains(&count) {
            return Err(Error::Corrupt("Journal Begin bounds"));
        }
        let mut unique = BTreeSet::new();
        let mut changes = Vec::new();
        let mut originals = Vec::new();
        let mut original_bytes = 0usize;
        for _ in 0..count {
            let length = reader.u32()?;
            if !(1..=1024).contains(&length) {
                return Err(Error::Corrupt("Journal application key"));
            }
            let key = reader.take(length)?.to_vec();
            if !unique.insert(key.clone()) {
                return Err(Error::Corrupt("duplicate Journal application key"));
            }
            match reader.take(1)?[0] {
                0 => changes.push((key, None)),
                1 => {
                    if key.starts_with(b"v/") || key.starts_with(b"s/") {
                        return Err(Error::Corrupt("Journal original event required"));
                    }
                    let length = reader.u32()?;
                    if length > MAX_DATA {
                        return Err(Error::Corrupt("Journal inline value bound"));
                    }
                    let start = reader.position;
                    let value = reader.take(length)?;
                    validate_inline(&key, value)?;
                    changes.push((
                        key,
                        Some(Cell {
                            cursor,
                            range: Some((start, length)),
                            original_proof: None,
                        }),
                    ));
                }
                2 => {
                    if !key.starts_with(b"v/") && !key.starts_with(b"s/") {
                        return Err(Error::Corrupt("Journal original key"));
                    }
                    let length = reader.u32()?;
                    original_bytes = original_bytes.checked_add(length).ok_or(Error::Limit)?;
                    if original_bytes > MAX_DATA {
                        return Err(Error::Corrupt("Journal originals bound"));
                    }
                    originals.push(Original {
                        key,
                        length,
                        hash: reader.take(32)?.try_into().unwrap(),
                        intent: AppendIntent::from_bytes(reader.take(80)?.try_into().unwrap())?,
                    });
                }
                _ => return Err(Error::Corrupt("Journal application value tag")),
            }
        }
        reader.finish()?;
        Ok(Self {
            request,
            changes,
            originals,
            original_cursors: Vec::new(),
            begin: data.to_vec(),
            begin_cursor: cursor,
        })
    }
    /// Payload events are interpreted only in the bounded position declared by
    /// Begin. Arbitrary original bytes resembling a marker remain originals.
    pub fn observe(&mut self, cursor: Cursor, data: &[u8]) -> Result<bool> {
        if let Some(original) = self.originals.get(self.original_cursors.len()) {
            if data.len() != original.length || blake3::hash(data).as_bytes() != &original.hash {
                return Err(Error::Corrupt("Journal original binding"));
            }
            if std::str::from_utf8(data).is_err() {
                return Err(Error::Corrupt("Journal original UTF8"));
            }
            self.changes.push((
                original.key.clone(),
                Some(Cell {
                    cursor,
                    range: None,
                    original_proof: Some(*blake3::hash(&original.intent.to_bytes()).as_bytes()),
                }),
            ));
            self.original_cursors.push(cursor);
            Ok(false)
        } else {
            if data != commit(self.request, &self.begin) {
                return Err(Error::Corrupt("Journal Commit binding"));
            }
            Ok(true)
        }
    }
    pub fn originals_complete(&self) -> bool {
        self.original_cursors.len() == self.originals.len()
    }
    pub fn original_count(&self) -> usize {
        self.originals.len()
    }
    pub fn intents(&self) -> Vec<AppendIntent> {
        self.originals
            .iter()
            .map(|original| original.intent)
            .collect()
    }
    pub fn next_intent(&self) -> Option<&AppendIntent> {
        self.originals
            .get(self.original_cursors.len())
            .map(|original| &original.intent)
    }
}
fn validate_inline(key: &[u8], value: &[u8]) -> Result<()> {
    use super::super::store::{MAX_METADATA, Record, decode_manifest, opaque};
    if key == b"manifest" {
        decode_manifest(value).map_err(|_| Error::Corrupt("Journal manifest grammar"))?;
    } else if key.starts_with(b"m/") {
        if key.len() != 10 || value.len() > MAX_METADATA {
            return Err(Error::Corrupt("Journal metadata bounds"));
        }
        let record: Record = serde_json::from_slice(value)
            .map_err(|_| Error::Corrupt("Journal metadata grammar"))?;
        if record.sequence != u64::from_be_bytes(key[2..].try_into().unwrap())
            || !record.input.text.is_empty()
            || !opaque(&record.event_id)
            || !opaque(&record.run_id)
            || !opaque(&record.input.agent_id)
            || record.adapter.is_empty()
            || record.adapter.len() > 128
        {
            return Err(Error::Corrupt("Journal metadata binding"));
        }
    } else if key.starts_with(b"i/") {
        if key.len() != 66 || value.len() != 8 || !std::str::from_utf8(&key[2..]).is_ok_and(opaque)
        {
            return Err(Error::Corrupt("Journal event identity grammar"));
        }
    } else if key.starts_with(b"b/") && (key.len() != 34 || value.len() != 32) {
        return Err(Error::Corrupt("Journal request grammar"));
    }
    Ok(())
}
pub(super) fn commit(request: [u8; 16], begin: &[u8]) -> Vec<u8> {
    [COMMIT.as_slice(), &request, blake3::hash(begin).as_bytes()].concat()
}
pub(super) fn encode<'a>(
    request: [u8; 16],
    changes: &[Change<'a>],
    intents: &[AppendIntent],
) -> Result<(Vec<u8>, Vec<&'a [u8]>)> {
    if request == [0; 16] || !(1..=MAX_CHANGES).contains(&changes.len()) {
        return Err(Error::Invalid("Journal application transaction bounds"));
    }
    let mut begin = [
        BEGIN.as_slice(),
        &request,
        &(changes.len() as u32).to_be_bytes(),
    ]
    .concat();
    let mut originals = Vec::new();
    let mut original_bytes = 0usize;
    let mut unique = BTreeSet::new();
    for change in changes {
        if change.key.is_empty() || change.key.len() > 1024 || !unique.insert(change.key) {
            return Err(Error::Invalid("Journal application key"));
        }
        begin.extend_from_slice(&(change.key.len() as u32).to_be_bytes());
        begin.extend_from_slice(change.key);
        match change.value {
            None => begin.push(0),
            Some(value) if change.key.starts_with(b"v/") || change.key.starts_with(b"s/") => {
                original_bytes = original_bytes
                    .checked_add(value.len())
                    .ok_or(Error::Limit)?;
                if original_bytes > MAX_DATA {
                    return Err(Error::Invalid("Journal originals bound"));
                }
                begin.push(2);
                begin.extend_from_slice(&(value.len() as u32).to_be_bytes());
                begin.extend_from_slice(blake3::hash(value).as_bytes());
                begin.extend_from_slice(
                    &intents
                        .get(originals.len())
                        .ok_or(Error::Invalid("Journal original intent count"))?
                        .to_bytes(),
                );
                originals.push(value);
            }
            Some(value) => {
                if value.len() > MAX_DATA {
                    return Err(Error::Invalid("Journal inline value bound"));
                }
                begin.push(1);
                begin.extend_from_slice(&(value.len() as u32).to_be_bytes());
                begin.extend_from_slice(value);
            }
        }
        if begin.len() > MAX_DATA {
            return Err(Error::Invalid("Journal Begin bound"));
        }
    }
    if originals.len() != intents.len() {
        return Err(Error::Invalid("Journal original intent count"));
    }
    Ok((begin, originals))
}
/// Only engine-independent lifecycle clock values can differ when rebuilding
/// the same unacknowledged application intent. All other metadata and original
/// lengths/hashes must be identical; the retained Begin remains authoritative.
pub(super) fn same_retry(candidate: &[u8], retained: &[u8]) -> Result<bool> {
    fn normalized(data: &[u8]) -> Result<Vec<u8>> {
        let mut reader = Reader::new(data);
        let marker = reader.take(24)?.to_vec();
        let count = reader.u32()?;
        if count > MAX_CHANGES {
            return Err(Error::Corrupt("Journal retry bounds"));
        }
        let mut out = marker;
        out.extend_from_slice(&(count as u32).to_be_bytes());
        for _ in 0..count {
            let length = reader.u32()?;
            let key = reader.take(length)?;
            let tag = reader.take(1)?[0];
            out.extend_from_slice(&(length as u32).to_be_bytes());
            out.extend_from_slice(key);
            out.push(tag);
            match tag {
                0 => {}
                1 => {
                    let length = reader.u32()?;
                    let value = reader.take(length)?;
                    let value = if key == b"manifest" {
                        let mut manifest: super::super::store::Manifest =
                            serde_json::from_slice(value)
                                .map_err(|_| Error::Corrupt("Journal manifest retry"))?;
                        manifest.created_us = 0;
                        manifest.last_activity_us = 0;
                        manifest.finished_us = manifest.finished_us.map(|_| 0);
                        manifest.archived_us = manifest.archived_us.map(|_| 0);
                        serde_json::to_vec(&manifest)
                            .map_err(|_| Error::Corrupt("Journal manifest retry"))?
                    } else {
                        value.to_vec()
                    };
                    out.extend_from_slice(&(value.len() as u32).to_be_bytes());
                    out.extend_from_slice(&value);
                }
                2 => out.extend_from_slice(reader.take(116)?),
                _ => return Err(Error::Corrupt("Journal retry value tag")),
            }
        }
        reader.finish()?;
        Ok(out)
    }
    Ok(normalized(candidate)? == normalized(retained)?)
}
pub(super) struct Reader<'a> {
    pub bytes: &'a [u8],
    pub position: usize,
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
    pub fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self.position.checked_add(length).ok_or(Error::Limit)?;
        let data = self
            .bytes
            .get(self.position..end)
            .ok_or(Error::Corrupt("Journal application bounds"))?;
        self.position = end;
        Ok(data)
    }
    pub fn u32(&mut self) -> Result<usize> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as usize)
    }
    pub fn finish(&self) -> Result<()> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(Error::Corrupt("Journal application trailing bytes"))
        }
    }
}
