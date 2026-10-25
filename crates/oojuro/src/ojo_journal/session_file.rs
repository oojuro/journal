//! Journal's versioned application grammar over generic timestamp-and-bytes
//! events. The core knows none of these keys, session fields or commit markers.
mod grammar;
mod pending;
#[cfg(test)]
mod tests;
use grammar::{Batch, Cell, PROFILE};
use ojo::{
    Change, Checkpoint, Database, Error, Result,
    events::{Cursor, Journal},
};
use std::{collections::BTreeMap, path::Path};

pub(super) enum SessionFile {
    Legacy(Database),
    Events(Events),
}
pub(super) struct Events {
    journal: Journal,
    values: BTreeMap<Vec<u8>, Cell>,
    requests: BTreeMap<[u8; 16], Cursor>,
    unfinished: Option<Batch>,
    cached: Option<(Cursor, Vec<u8>)>,
    anchor: Option<Cursor>,
}
impl SessionFile {
    pub fn create(path: impl AsRef<Path>, key: Option<&[u8; 32]>) -> Result<Self> {
        let mut journal = Journal::create(path.as_ref(), key)?;
        let anchor = journal.append(PROFILE)?.cursor;
        Ok(Self::Events(Events {
            journal,
            values: BTreeMap::new(),
            requests: BTreeMap::new(),
            unfinished: None,
            cached: None,
            anchor: Some(anchor),
        }))
    }
    pub fn open(path: impl AsRef<Path>, key: Option<&[u8; 32]>, writable: bool) -> Result<Self> {
        let path = path.as_ref();
        let mut journal = match Journal::open(path, key, writable) {
            Ok(journal) => journal,
            Err(Error::Invalid("Event profile required")) => {
                return Database::open(path, key, writable).map(Self::Legacy);
            }
            Err(error) => return Err(error),
        };
        if journal.is_empty() && writable {
            // Interrupted application initialization has no accepted originals.
            journal.append(PROFILE)?;
        }
        let mut file = Events {
            journal,
            values: BTreeMap::new(),
            requests: BTreeMap::new(),
            unfinished: None,
            cached: None,
            anchor: None,
        };
        let mut first = true;
        let mut after = None;
        loop {
            let mut events = Vec::new();
            // One source event at a time keeps raw originals out of the replay
            // index. Only key locations and bounded current batch metadata stay.
            let page = file.journal.scan(after, 1, |event| {
                events.push(event);
                Ok(())
            })?;
            for event in events {
                if first {
                    if event.data != PROFILE {
                        return Err(Error::Invalid("Journal application profile required"));
                    }
                    first = false;
                    file.anchor = Some(event.cursor);
                } else {
                    file.observe(event.cursor, &event.data)?;
                }
            }
            after = page.cursor;
            if !page.has_more {
                break;
            }
        }
        if first {
            return Err(Error::Invalid("Journal application profile required"));
        }
        if writable {
            file.reconcile()?;
        }
        Ok(Self::Events(file))
    }
    pub fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        match self {
            Self::Legacy(db) => db.get(key),
            Self::Events(file) => file.get(key),
        }
    }
    pub fn confirm(&mut self) -> Result<Checkpoint> {
        match self {
            Self::Legacy(db) => db.confirm(),
            Self::Events(file) => file.journal.confirm(),
        }
    }
    pub fn commit(&mut self, request: [u8; 16], changes: &[Change<'_>]) -> Result<()> {
        match self {
            Self::Legacy(db) => db.commit(request, changes).map(|_| ()),
            Self::Events(file) => file.commit(request, changes),
        }
    }
    pub fn archive_to(&mut self, destination: impl AsRef<Path>) -> Result<Checkpoint> {
        match self {
            Self::Legacy(db) => db.archive_to(destination),
            Self::Events(file) => {
                if file.unfinished.is_some() {
                    return Err(Error::Invalid(
                        "pending_journal_batch_requires_original_retry",
                    ));
                }
                file.journal.archive_to(destination)
            }
        }
    }
    pub fn scan_prefix(
        &mut self,
        prefix: &[u8],
        after: Option<&[u8]>,
        limit: usize,
        mut visit: impl FnMut(&[u8], &[u8]) -> Result<()>,
    ) -> Result<Option<Vec<u8>>> {
        match self {
            Self::Legacy(db) => db.scan_prefix(prefix, after, limit, visit),
            Self::Events(file) => {
                if !(1..=1024).contains(&limit) {
                    return Err(Error::Invalid("prefix page bounds"));
                }
                let mut keys = file
                    .values
                    .range(prefix.to_vec()..)
                    .filter(|(key, _)| after.is_none_or(|after| key.as_slice() > after))
                    .take_while(|(key, _)| key.starts_with(prefix))
                    .take(limit + 1)
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                let continuation = if keys.len() > limit {
                    keys.pop();
                    keys.last().cloned()
                } else {
                    None
                };
                for key in keys {
                    let value = file
                        .get(&key)?
                        .ok_or(Error::Corrupt("Journal original missing"))?;
                    visit(&key, &value)?;
                }
                Ok(continuation)
            }
        }
    }
    pub fn scan(&mut self, mut visit: impl FnMut(u64, u32, &[u8], Option<&[u8]>)) -> Result<()> {
        match self {
            Self::Legacy(db) => db.scan(visit),
            Self::Events(file) => {
                let mut after: Option<Vec<u8>> = None;
                loop {
                    use std::ops::Bound::{Excluded, Unbounded};
                    let next = match &after {
                        None => file.values.first_key_value(),
                        Some(after) => file
                            .values
                            .range((Excluded(after.clone()), Unbounded))
                            .next(),
                    }
                    .map(|(key, _)| key.clone());
                    let Some(key) = next else {
                        break;
                    };
                    let value = file
                        .get(&key)?
                        .ok_or(Error::Corrupt("Journal original missing"))?;
                    visit(0, 0, &key, Some(&value));
                    after = Some(key);
                }
                Ok(())
            }
        }
    }
}
impl Events {
    fn observe(&mut self, cursor: Cursor, data: &[u8]) -> Result<()> {
        match self.unfinished.take() {
            None => self.unfinished = Some(Batch::parse(cursor, data)?),
            Some(mut batch) => {
                if let Some(intent) = batch.next_intent() {
                    let original = self
                        .journal
                        .get_prepared(intent)?
                        .ok_or(Error::Corrupt("Journal original intent missing"))?;
                    if original.cursor != cursor {
                        return Err(Error::Corrupt("Journal original intent order"));
                    }
                }
                if batch.observe(cursor, data)? {
                    self.apply(batch)?;
                } else {
                    self.unfinished = Some(batch);
                }
            }
        }
        Ok(())
    }
    fn apply(&mut self, batch: Batch) -> Result<()> {
        if self
            .requests
            .insert(batch.request, batch.begin_cursor)
            .is_some()
        {
            return Err(Error::Corrupt("duplicate application transaction"));
        }
        for (key, cell) in batch.changes {
            match cell {
                Some(cell) => {
                    self.values.insert(key, cell);
                }
                None => {
                    self.values.remove(&key);
                }
            }
        }
        Ok(())
    }
    fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let Some(cell) = self.values.get(key) else {
            return Ok(None);
        };
        let cursor = cell.cursor;
        let range = cell.range;
        if self
            .cached
            .as_ref()
            .is_none_or(|(cached, _)| *cached != cursor)
        {
            let event = self
                .journal
                .get(cursor)?
                .ok_or(Error::Corrupt("Journal value event missing"))?;
            self.cached = Some((cursor, event.data));
        }
        let data = &self.cached.as_ref().unwrap().1;
        let value = match range {
            None => data.as_slice(),
            Some((start, length)) => data
                .get(start..start.checked_add(length).ok_or(Error::Limit)?)
                .ok_or(Error::Corrupt("Journal value range"))?,
        };
        Ok(Some(value.to_vec()))
    }
}
