//! Complete latest KV layout validation, before writable engine tail recovery.
use super::super::store::{MAX_METADATA, MAX_TEXT, Record, decode_manifest, opaque};
use ojo::{Database, Error, Result};
struct Entry {
    event: [u8; 32],
    flags: u8,
}
fn invalid() -> Error {
    Error::Invalid("Journal legacy application layout required")
}
fn event(bytes: &[u8]) -> Result<[u8; 32]> {
    let id = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    if !opaque(id) {
        return Err(invalid());
    }
    Ok(*blake3::Hash::from_hex(id)
        .map_err(|_| invalid())?
        .as_bytes())
}
pub(super) fn validate(db: &mut Database, initialize_empty: bool) -> Result<()> {
    if initialize_empty && db.checkpoint().sequence == 0 {
        return Ok(());
    }
    let bytes = db.get(b"manifest")?.ok_or_else(invalid)?;
    let manifest = decode_manifest(&bytes).map_err(|_| invalid())?;
    let count = usize::try_from(manifest.records).map_err(|_| Error::Limit)?;
    let mut entries = Vec::new();
    entries.try_reserve_exact(count).map_err(|_| Error::Limit)?;
    entries.resize_with(count, || Entry {
        event: [0; 32],
        flags: 0,
    });
    let mut after = None;
    let mut metadata_count = 0u64;
    loop {
        let next = db.scan_prefix(b"", after.as_deref(), 100, |key, value| {
            if key == b"manifest" {
                return Ok(());
            }
            if key.starts_with(b"b/") {
                return if key.len() == 34 && value.len() == 32 {
                    Ok(())
                } else {
                    Err(invalid())
                };
            }
            if key.starts_with(b"i/") {
                if key.len() != 66 || value.len() != 8 {
                    return Err(invalid());
                }
                let position = usize::try_from(u64::from_be_bytes(value.try_into().unwrap()))
                    .map_err(|_| invalid())?;
                let entry = entries.get_mut(position).ok_or_else(invalid)?;
                if entry.flags != 0 {
                    return Err(invalid());
                }
                entry.event = event(&key[2..])?;
                entry.flags = 1;
                return Ok(());
            }
            if key.len() != 10 {
                return Err(invalid());
            }
            let position = u64::from_be_bytes(key[2..].try_into().unwrap());
            let entry = entries
                .get_mut(usize::try_from(position).map_err(|_| invalid())?)
                .ok_or_else(invalid)?;
            match &key[..2] {
                b"m/" => {
                    if value.len() > MAX_METADATA || position != metadata_count || entry.flags != 1
                    {
                        return Err(invalid());
                    }
                    let record: Record = serde_json::from_slice(value).map_err(|_| invalid())?;
                    if record.sequence != position
                        || record.run_id != manifest.run_id
                        || record.adapter != manifest.adapter
                        || record.input.agent_id != manifest.agent_id
                        || !record.input.text.is_empty()
                        || event(record.event_id.as_bytes())? != entry.event
                    {
                        return Err(invalid());
                    }
                    entry.flags |= 2 | if record.input.indexed { 4 } else { 0 };
                    metadata_count += 1;
                }
                b"s/" | b"v/" => {
                    let indexed = &key[..2] == b"v/";
                    if entry.flags & 3 != 3
                        || entry.flags & 8 != 0
                        || (entry.flags & 4 != 0) != indexed
                        || value.len() > MAX_TEXT
                        || std::str::from_utf8(value).is_err()
                    {
                        return Err(invalid());
                    }
                    entry.flags |= 8;
                }
                _ => return Err(invalid()),
            }
            Ok(())
        })?;
        after = next;
        if after.is_none() {
            break;
        }
    }
    if metadata_count != manifest.records || entries.iter().any(|entry| entry.flags & 11 != 11) {
        return Err(invalid());
    }
    Ok(())
}
