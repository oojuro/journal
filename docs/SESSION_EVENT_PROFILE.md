# Journal application event profile 1

This is Journal's application grammar inside the Oojuro engine's generic Event
profile. It does not change the engine wire format or add agent fields to the
engine. Each engine event still has only its engine-assigned UTC acceptance time
and exact opaque data, with an engine cursor for retrieval.

The first application event's data are exactly
`ojo-journal/application-events/1\0` (including the final zero byte). A generic
Event file with another first event is refused as a Journal application file.
Existing KV session files remain compatible and require explicit conversion.

Open validates this grammar in the engine's existing observed replay, before
writable tail recovery. Complete application refusal therefore preserves a
following incomplete tail. It does not scan all original bodies a second time.
The KV fallback uses a read-only checked callback to validate its complete latest
manifest, metadata, ID map, eligibility, UTF-8 originals and request-key shapes
before recovery. Only explicit session initialization may adopt an interrupted
empty file.

A complete application transaction consists of a Begin event, its declared
number of original events, and a matching Commit event. Original-event data are
the exact accepted UTF-8 text bytes, including empty text, NULs and line endings.
No application prefix or JSON envelope is added to those original bytes. Agent,
session, provider, event ID and indexing policy belong to the Begin metadata.

Begin uses this bounded binary encoding. Integers are unsigned big endian.

| Field | Encoding |
| --- | --- |
| Marker/version | Eight bytes `OJJBEG1\0` |
| Application request | 16 nonzero bytes |
| Change count | u32, 1–1,000 |
| Each key | u32 byte length, then 1–1,024 exact key bytes |
| Each value | One tag byte, followed by its tag-specific representation |

Tag 0 encodes deletion, which profile 1 refuses. Tag 1 contains a u32 byte length
and inline metadata bytes; `v/` and `s/` originals must use tag 2.
Tag 2 contains the original's u32 length, 32-byte BLAKE3 digest and
80 opaque bytes from the engine's `AppendIntent::to_bytes()`. Tag 2 is permitted
only for `v/` (indexed) and `s/` (unindexed) originals. Duplicate keys and trailing
bytes are refused. Begin is at most 16 MiB; the sum of original bytes is at most
16 MiB, preserving the previous complete-original allowance. Metadata values are
the same application values used by the compatibility KV implementation.

Original events follow in Begin's tag-2 order. The reader checks length and
digest, then uses the engine's `AppendIntent::check_event` on the observed
receipt and data to verify the retained cursor, acceptance time and exact data.
Original text that resembles a control marker is still interpreted as an
original in that declared position.

Commit is exactly the eight bytes `OJJCOM1\0`, the same 16-byte application
request, and the 32-byte BLAKE3 digest of the complete Begin bytes. Application
state becomes visible only after that matching Commit. An unrelated event,
changed original, wrong order or mismatched Commit refuses the application
grammar; source bytes remain available to generic engine tools.

Only the manifest may be replaced. Other application keys, original bytes,
record identity and indexing eligibility remain immutable. New indexed sequence
keys must be strictly greater than the retained indexed prefix. Replay validates
these transitions before recovery; live writes validate them before appending.
This allows the disposable lexical index to extend an authenticated unchanged
prefix while retaining the same source of original context.

A new transaction submits Begin, original events and Commit with the engine's
bounded group-durability API. Success requires the final file and directory
barrier. This API does not itself provide application atomic visibility; the
grammar above supplies it. The application retains no separate plaintext copy of
originals or operational intent sidecar. Encrypted sources encrypt Begin and its
opaque original intents along with every other event.

After an unknown acknowledgement, every surviving original has a preceding
verified Begin containing its original intent. Retry validates the same app
request, metadata, original hashes and intent list, and reuses those original
engine timestamps/cursors. A surviving Commit means deduplication plus engine
confirmation. A surviving Begin with all originals can finish its matching
Commit. A missing original requires the exact caller/source retry; another new
mutation cannot bypass it. Lifecycle clock fields rebuilt during that retry use
the retained Begin's original values. A torn Begin cannot have an original in
the authoritative verified prefix, so an unaccepted control may be prepared
again. Control timestamps are not substituted for original acceptance times.

`ojo-journal migrate-session --session ID` explicitly converts an existing KV
session. It first retains and verifies a byte-exact recovery database under the
root's `format-backups/ID/`, using the original encryption mode and root key.
The old source is opened read-only, so that copy retains any incomplete tail too.
Conversion copies every live application key, including retry fingerprints and
unindexed originals, to an unpublished Event file, verifies values and manifest,
then replaces the selected session path and confirms its parent. A legacy
`.ojo` pathname is retained during this explicit conversion. KV files had no
generic per-original engine acceptance timestamp; conversion assigns new engine
acceptance times while preserving every original application timestamp.

The recovery database remains a KV database, readable with the previous engine
API and the original root key. Export/import is an independent JSONL interchange
path; importing creates a new engine acceptance clock. Copy/archive the `.juro`
file to retain original engine cursors and times. Stop recording before manually
restoring a recovery database; retaining
the current file separately preserves records accepted after conversion.
