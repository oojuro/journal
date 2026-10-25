//! Disposable lexical search over caller-validated immutable original events.
use super::{Events, SessionFile};
use crate::{hosted::read_private, hosted_capture};
use ojo::{Error, Result};
use oojuro_engine::record::{
    Filter, Kind,
    projection::{
        Binding, Query, Row,
        disk::{Appender, Builder, Compression, Index, Limits},
    },
};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

pub(super) struct Search {
    index: Index,
    through: Option<Vec<u8>>,
    // This caller lease spans all retained index generations and any rebuild.
    _lease: File,
    builds: u64,
    refreshes: u64,
    reopens: u64,
}
pub(super) fn view() -> blake3::Hasher {
    let mut hash = blake3::Hasher::new();
    hash.update(b"ojo-journal/indexed-original-view/v1");
    hash
}
pub(super) fn from_values(
    values: &std::collections::BTreeMap<Vec<u8>, super::Cell>,
) -> Result<blake3::Hasher> {
    let mut hash = view();
    for (key, cell) in values.range(b"v/".to_vec()..b"w".to_vec()) {
        if key.len() != 10 {
            return Err(Error::Corrupt("Journal indexed sequence"));
        }
        hash.update(key);
        hash.update(
            &cell
                .original_proof
                .ok_or(Error::Corrupt("Journal indexed original proof"))?,
        );
    }
    Ok(hash)
}
fn error(_: oojuro_engine::record::Error) -> Error {
    Error::Invalid("derived_journal_search_unavailable")
}
fn limits() -> Limits {
    Limits {
        rows: 3_000_000,
        source_bytes: 64 << 30,
        index_bytes: 8 << 30,
        metadata_bytes: 256 << 20,
        original_bytes_per_query: 16 << 20,
        query_terms: 32,
        compression: Compression::Zstd,
    }
}
fn id(sequence: u64) -> String {
    format!("{sequence:016x}")
}
fn sequence(id: &str) -> Result<u64> {
    if id.len() != 16
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Corrupt("Journal search identity"));
    }
    u64::from_str_radix(id, 16).map_err(|_| Error::Corrupt("Journal search identity"))
}
fn key(sequence: u64) -> Vec<u8> {
    [b"v/".as_slice(), &sequence.to_be_bytes()].concat()
}
impl SessionFile {
    pub fn discard_search(&mut self) {
        if let Self::Events(file) = self {
            file.query = None;
        }
    }
    pub fn search_status(&self) -> Option<serde_json::Value> {
        let Self::Events(file) = self else {
            return None;
        };
        file.query.as_ref().map(|state| serde_json::json!({"rows":state.index.row_count(),"index_bytes":state.index.index_bytes(),"estimated_heap_bytes":state.index.estimated_heap_bytes(),"builds":state.builds,"refreshes":state.refreshes,"reopens":state.reopens}))
    }
    pub fn indexed_search(
        &mut self,
        root: &Path,
        cache_key: &[u8; 32],
        namespace: [u8; 32],
        writer: &str,
        text: &str,
        limit: usize,
    ) -> Result<Option<Vec<u64>>> {
        let Self::Events(file) = self else {
            return Ok(None);
        };
        file.search(root, cache_key, namespace, writer, text, limit)
            .map(Some)
    }
}
impl Events {
    fn row(&mut self, sequence: u64) -> Result<Row> {
        let cell = self
            .values
            .get(&key(sequence))
            .ok_or(Error::Corrupt("Journal indexed original missing"))?;
        let event = self
            .journal
            .get(cell.cursor)?
            .ok_or(Error::Corrupt("Journal indexed event missing"))?;
        Ok(Row {
            id: id(sequence),
            key: None,
            text: String::from_utf8(event.data)
                .map_err(|_| Error::Corrupt("Journal original UTF8"))?,
            valid_at: event.accepted_at,
            reported_recorded_at: event.accepted_at,
            kind: Kind::Raw,
            tool: None,
            status: None,
            agent_id: String::new(),
            indexed: true,
        })
    }
    fn keys_after(&self, after: Option<&[u8]>) -> Vec<Vec<u8>> {
        use std::ops::Bound::{Excluded, Included};
        let start = after.map_or(Included(b"v/".as_slice()), Excluded);
        self.values
            .range::<[u8], _>((start, Excluded(b"w".as_slice())))
            .take(100)
            .map(|(key, _)| key.clone())
            .collect()
    }
    fn stream(
        &mut self,
        mut after: Option<Vec<u8>>,
        mut push: impl FnMut(Row) -> oojuro_engine::record::Result<()>,
    ) -> Result<Option<Vec<u8>>> {
        loop {
            let page = self.keys_after(after.as_deref());
            if page.is_empty() {
                return Ok(after);
            }
            for key in &page {
                if key.len() != 10 {
                    return Err(Error::Corrupt("Journal indexed sequence"));
                }
                let sequence = u64::from_be_bytes(key[2..].try_into().unwrap());
                push(self.row(sequence)?).map_err(error)?;
            }
            after = page.last().cloned();
        }
    }
    fn build(
        &mut self,
        root: &Path,
        cache_key: &[u8; 32],
        binding: Binding,
    ) -> Result<(Index, Option<Vec<u8>>)> {
        let mut builder = Builder::create(root, cache_key, binding, limits()).map_err(error)?;
        let through = self.stream(None, |row| builder.push(row))?;
        Ok((builder.finish().map_err(error)?, through))
    }
    fn search(
        &mut self,
        root: &Path,
        cache_key: &[u8; 32],
        namespace: [u8; 32],
        writer: &str,
        text: &str,
        limit: usize,
    ) -> Result<Vec<u64>> {
        if text.is_empty()
            || text.len() > 4096
            || text.split_whitespace().count() > 32
            || !(1..=100).contains(&limit)
        {
            return Err(Error::Invalid("Journal search bounds"));
        }
        let binding = Binding {
            namespace,
            writer: writer.into(),
            view: *self.indexed_view.finalize().as_bytes(),
        };
        let mut state = if let Some(state) = self.query.take() {
            state
        } else {
            let parent = root.parent().ok_or(Error::Invalid("Journal search root"))?;
            hosted_capture::directory(parent)
                .map_err(|_| Error::Invalid("private Journal search parent required"))?;
            hosted_capture::directory(root)
                .map_err(|_| Error::Invalid("private Journal search root required"))?;
            let lock = parent.join(format!("{writer}.lease"));
            let lease = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&lock)?;
            read_private(&lock, 0)
                .map_err(|_| Error::Invalid("private Journal search lease required"))?;
            lease.try_lock()?;
            let (index, through, builds, reopens) =
                match Index::open_with_limits(root, cache_key, &binding, limits()) {
                    Ok(index) => (
                        index,
                        self.values
                            .range(b"v/".to_vec()..b"w".to_vec())
                            .next_back()
                            .map(|(key, _)| key.clone()),
                        0,
                        1,
                    ),
                    Err(_) => {
                        // These are application-owned disposable files; the source
                        // and retained originals never depend on cache publication.
                        fs::remove_dir_all(root)?;
                        hosted_capture::directory(root)
                            .map_err(|_| Error::Invalid("private Journal search root required"))?;
                        let (index, through) = self.build(root, cache_key, binding.clone())?;
                        (index, through, 1, 0)
                    }
                };
            Search {
                index,
                through,
                _lease: lease,
                builds,
                refreshes: 0,
                reopens,
            }
        };
        if state.index.binding() != &binding {
            // app profile1 permits only new immutable original keys. This
            // healthy source's validated retained prefix remains unchanged.
            let mut append =
                Appender::extend_validated_prefix(&state.index, binding.clone(), limits())
                    .map_err(error)?;
            let through = self.stream(state.through.clone(), |row| append.push(row))?;
            state.index = append.finish().map_err(error)?;
            state.through = through;
            state.refreshes += 1;
        }
        let query = Query {
            text,
            k: limit,
            from: i64::MIN,
            to: i64::MAX,
            filter: Filter::default(),
            excerpt_bytes: 2048,
            max_bytes: 16 << 20,
        };
        let hits = state
            .index
            .search(&binding, &query, |id| {
                self.row(sequence(id).map_err(|_| oojuro_engine::record::Error::Invalid)?)
                    .map_err(|_| oojuro_engine::record::Error::Authority)
            })
            .map_err(error)?;
        let result = hits
            .iter()
            .map(|hit| sequence(&hit.id))
            .collect::<Result<Vec<_>>>()?;
        if *self.indexed_view.finalize().as_bytes() != binding.view {
            return Err(Error::Rollback);
        }
        self.query = Some(state);
        Ok(result)
    }
}
