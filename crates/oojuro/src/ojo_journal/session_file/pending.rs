//! The preceding authenticated Begin is the retained original-intent journal.
//! No accepted original can replay without its bounded intent-bearing Begin.
use super::{
    Events,
    grammar::{self, Batch},
};
use ojo::{
    Change, Error, Result,
    events::{AppendIntent, Cursor},
};

impl Events {
    pub(super) fn commit(&mut self, request: [u8; 16], changes: &[Change<'_>]) -> Result<()> {
        let originals = changes
            .iter()
            .filter(|change| change.key.starts_with(b"v/") || change.key.starts_with(b"s/"))
            .filter_map(|change| change.value)
            .collect::<Vec<_>>();
        if let Some(cursor) = self.requests.get(&request).copied() {
            let retained = self
                .journal
                .get(cursor)?
                .ok_or(Error::Corrupt("Journal accepted Begin missing"))?;
            let batch = Batch::parse(cursor, &retained.data)?;
            let (candidate, _) = grammar::encode(request, changes, &batch.intents())?;
            if !grammar::same_retry(&candidate, &retained.data)? {
                return Err(Error::Conflict);
            }
            self.journal.confirm()?;
            return Ok(());
        }
        if changes.iter().any(|change| {
            change.key != b"manifest"
                && (change.value.is_none() || self.values.contains_key(change.key))
        }) {
            return Err(Error::Conflict);
        }
        let mut previous = self
            .values
            .range(b"v/".to_vec()..b"w".to_vec())
            .next_back()
            .map(|(key, _)| key.clone());
        for change in changes
            .iter()
            .filter(|change| change.key.starts_with(b"v/"))
        {
            if change.key.len() != 10
                || previous
                    .as_ref()
                    .is_some_and(|previous| change.key <= previous.as_slice())
            {
                return Err(Error::Conflict);
            }
            previous = Some(change.key.to_vec());
        }
        if let Some(batch) = self.unfinished.as_ref() {
            if batch.request != request {
                return Err(Error::Invalid(
                    "pending_journal_batch_requires_original_retry",
                ));
            }
            let begin = batch.begin.clone();
            let begin_cursor = batch.begin_cursor;
            let intents = batch.intents();
            let (candidate, _) = grammar::encode(request, changes, &intents)?;
            if !grammar::same_retry(&candidate, &begin)? {
                return Err(Error::Conflict);
            }
            return self.accept(begin, &originals, intents, Some(begin_cursor));
        }
        let intents = originals
            .iter()
            .map(|original| self.journal.prepare(original))
            .collect::<Result<Vec<_>>>()?;
        let (begin, _) = grammar::encode(request, changes, &intents)?;
        self.accept(begin, &originals, intents, None)
    }
    fn accept(
        &mut self,
        begin: Vec<u8>,
        originals: &[&[u8]],
        intents: Vec<AppendIntent>,
        begin_cursor: Option<Cursor>,
    ) -> Result<()> {
        let anchor = self
            .anchor
            .ok_or(Error::Corrupt("Journal profile missing"))?;
        let planned = Batch::parse(begin_cursor.unwrap_or(anchor), &begin)?;
        if originals.len() != planned.original_count() || intents.len() != originals.len() {
            return Err(Error::Invalid("Journal original count"));
        }
        let commit = grammar::commit(planned.request, &begin);
        // Controls are never replayed when they already belong to the verified
        // prefix. Original intents/time are always reused from the same Begin.
        let begin_intent = if begin_cursor.is_none() {
            Some(self.journal.prepare(&begin)?)
        } else {
            None
        };
        let commit_intent = self.journal.prepare(&commit)?;
        let mut group = Vec::with_capacity(originals.len() + 2);
        if let Some(intent) = begin_intent.as_ref() {
            group.push((intent, begin.as_slice()));
        }
        group.extend(
            intents
                .iter()
                .zip(originals.iter())
                .map(|(intent, data)| (intent, *data)),
        );
        group.push((&commit_intent, commit.as_slice()));
        let receipts = self.journal.append_prepared_batch(&group)?;
        let offset = usize::from(begin_cursor.is_none());
        let mut batch = Batch::parse(begin_cursor.unwrap_or(receipts[0].cursor), &begin)?;
        for (receipt, original) in receipts
            .iter()
            .skip(offset)
            .take(originals.len())
            .zip(originals)
        {
            if batch.observe(receipt.cursor, original)? {
                return Err(Error::Corrupt("early Journal Commit"));
            }
        }
        if !batch.observe(receipts.last().unwrap().cursor, &commit)? {
            return Err(Error::Corrupt("missing Journal Commit"));
        }
        self.apply(batch)?;
        self.unfinished = None;
        self.cached = None;
        Ok(())
    }
    pub(super) fn reconcile(&mut self) -> Result<()> {
        let Some(batch) = self.unfinished.as_ref() else {
            return Ok(());
        };
        if !batch.originals_complete() {
            return Ok(());
        }
        let begin = batch.begin.clone();
        let cursor = batch.begin_cursor;
        let intents = batch.intents();
        let originals = batch
            .original_cursors
            .iter()
            .map(|cursor| {
                self.journal
                    .get(*cursor)?
                    .map(|event| event.data)
                    .ok_or(Error::Corrupt("Journal retained original missing"))
            })
            .collect::<Result<Vec<_>>>()?;
        let references = originals.iter().map(Vec::as_slice).collect::<Vec<_>>();
        self.accept(begin, &references, intents, Some(cursor))
    }
}
