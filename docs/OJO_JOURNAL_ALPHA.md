# ojo-journal .ojo alpha

The supported new application is `ojo-journal`, on Linux, backed by the standalone
experimental `.ojo` file engine. Each provider session has its own file under the
private root's `journals/` directory. A separate 0600 local key encrypts originals
by default. Operational configuration/cursors are private 0600 files. Existing
compatibility commands and library APIs are preserved.

## Local capture and original context

```sh
ojo-journal connect --adapter codex --accept-local-capture
ojo-journal connect --adapter claude-code --accept-local-capture
ojo-journal sessions
ojo-journal search --query "deployment decision"
ojo-journal timeline --session SESSION_ID --from 0 --limit 100
ojo-journal get --session SESSION_ID --id 0
ojo-journal context --session SESSION_ID --id 0 --before 3 --after 3
```

`--root` selects another absolute private directory. Connect excludes preexisting
transcript bytes, including incomplete old lines through their next newline.
`--include-existing` is an explicit import choice. A provider's stable session ID
keeps resume/compaction files in the same logical journal; Claude sidechain files
retain their separate stable identity. Display labels do not route writes.

The capture owner reuses the existing Codex and Claude parsers/redactor. It never
reads referenced tool-result side files. Accepted text preserves its exact UTF-8
bytes, including line endings and NUL characters. Recognized secrets are redacted
before acceptance. Original text and derived metadata commit atomically; a cursor
can advance only afterward. Replaying a lost cursor deduplicates stable event IDs.
Changed input under an existing event/request ID conflicts.

Connect starts one root-scoped systemd user service unless `--no-service` is
supplied. It monitors both configured adapters. Its synthetic setup fixture lives
in the application's own private directory, goes through the real parser and
cursor, remains unindexed, and can be read by the returned session/sequence.
Setup retries only a short cooperating capture tick's busy response, with a
bounded deadline. Source conflicts and uncertain effects retain their errors.

Connect registers a read-only local MCP server through the selected agent CLI,
checks the installed inventory, then opens the exact accepted setup original
through that launch command. Existing matching entries resume; conflicting
entries remain unchanged. `--no-mcp` opts out. `--no-service` also leaves agent
configuration alone unless `--register-mcp` is explicit. The result distinguishes
verified MCP protocol/original reads from a future agent model tool invocation;
restart or reconnect the agent to load new tools. It does not claim every future
provider entry or an actual agent-model invocation was verified. Codex setup uses
its [documented stdio registration command](https://learn.chatgpt.com/docs/extend/mcp?surface=cli).

`serve` runs the healthy owner in the foreground; `watch --adapter codex` remains
a compatible entrypoint. The owner also runs the archive scheduler and optional
uploader. `upgrade-service` replaces only the managed executable and root-bound
unit, restarts it, and requires a live owner handshake. Exact old alpha units for
that same root can migrate; unrelated units and agent entries stay untouched.

The current search path streams original values using case-insensitive AND
substrings and returns recent matches with their complete originals. It has
bounded result memory but scans history. It does not claim a persistent inverted
index or million-record latency. Timeline and upload pages use bounded engine
prefix traversal and share verified encoded bodies within each page. With the
owner running, capture, CLI, read-only MCP and sync staging reuse a pool of at
most eight healthy engine handles. The private mode0700 socket parent, mode0600
socket, peer UID, root binding, message/worker bounds and deadlines protect local
routing. Database mutations serialize; slow clients and network requests hold no
database mutex. A failed database operation drops its handle before verified
reopen. Status reports actual handles and verified opens. These are count bounds,
not a process-RSS claim. Without a live owner, standalone commands use verified
file reopen. Stale-socket fallback requires a failed live handshake and a free
owner lock; a lock or PID file alone is not a liveness test. The generic disk
full-text projection remains separate work, so substring search is still linear.

## Optional cloud delivery

```sh
ojo-journal connect --adapter codex --api-key-file /private/integration-key \
  --accept-local-capture
ojo-journal status
ojo-journal sync --max-batches 1000
```

The integration key is read from an owned 0600 file, never from an argument or a
URL. HTTPS origins are required except literal loopback test fixtures. A new
cloud connection excludes existing local records unless `--include-existing` is
explicit. The setup result distinguishes local durability and a real cloud ACK
for the synthetic event. A failed cloud verification leaves local configuration
and originals available for retry; it is not reported as successful delivery.

Each upload binds a durable request ID, source range, original digest and initial
session routing policy before remote mutation. Unknown session/event ACKs keep
that intent. Local capture can continue while cloud is unavailable, and upload
releases its local journal lock before network I/O. Expiry/revocation and routing
remain enforced by the supported Cloud discovery/session/framed-events API.
Project read keys remain separate from capture grants.

## Export, import and existing directories

```sh
ojo-journal export --session SESSION_ID > session.jsonl
ojo-journal import --file session.jsonl
ojo-journal migrate --legacy-root /private/old-journal --allow-raw-source
ojo-journal restore-migration --migration RECOVERY_ARTIFACT_ID --out /private/new-restore
```

Export includes the manifest and complete original records. Import is retry-safe
and leaves the input file untouched. Migration supports native capture, direct
Record2 and original Trace journal directories. Stop the legacy recorder first;
all existing source locks must be acquired. The destination must use encryption.

Migration snapshots every regular source file, including raw/control state and
old keys, into a separate encrypted `.ojo` recovery artifact before converting
records. Legacy recovery/decoding runs only on a private copy. The source files
are never changed. Converted records preserve accepted original text and retain
a source/record reference; the recovery artifact preserves all original metadata,
keys, raw bytes and file contents. Bounded batches and reusable file handles avoid
reopening a growing destination once per record.

Repeated migration uses stable source/event IDs and deduplicates prior records.
The recovery artifact ID identifies an exact source snapshot. Restore verifies
every file's digest and atomically publishes a new destination without replacing
an existing path. It never merges into or overwrites a live store. Private copy
staging is removed on normal completion/error; interrupted process staging can
be inspected before cleanup. No real customer or agent data appears in tests.

## Archive policy and direct records

`finish --session ID` marks a journal finished; `resume` clears that mark.
Finished journals inactive for 24 hours are automatically archived by the watcher.
`archive-policy --enabled false` opts out. `archive --session ID` explicitly
archives now. An archive is verified and atomically replaces the application's
canonical file under a stable application lock. Exact logical records and retry
IDs survive; physical file IDs/checkpoints change. Resumed writes append raw
transactions and reset the idle/finished state.

`record --session-id STABLE_ID --event-id STABLE_ID --text-file FILE` accepts an
explicit original. Unlike automatic transcript capture, direct records/imports
are caller-authorized exact data and are not silently redacted. Reusing those IDs
with changed content conflicts. Empty text is valid; texts must be UTF-8 and at
most 16 MiB. A page has at most 100 records and 16 MiB original text. The generic
engine's cumulative key/operation admission bounds also apply.

## Installation and qualification

`scripts/install-journal.sh` downloads the versioned Linux x86_64 archive over
HTTPS, verifies SHA-256, refuses traversal/links and retains all required notices.
The release operator publishes the qualified archive and checksums. Source:

```sh
cargo install --locked --path crates/oojuro --bin ojo-journal
```

`scripts/build_journal.sh` builds all four binaries and packages complete engine,
registry, dictionary and Rust notices. Installed aliases are `ojo-journal`,
`oojuro`, `oojuro-native` and `oojurod`. The read-only local MCP server is
`ojo-journal mcp --root ROOT`; it exposes sessions/search/get over stdio and does
not grant Cloud project reads.

Focused checks use actual binaries and synthetic fixtures: exact originals,
archive/resume, session mapping, incomplete source lines, cursor replay,
redaction, both lost Cloud ACKs, byte-exact migration/restore, MCP errors and
installer checksum/path/link refusal. Full source qualification, published
artifact installation, live setup, capacity and Cloud launch acceptance remain
distinct evidence. No universal engine superiority or stable-format claim is made.

## Generic engine event profile compatibility

This alpha stores immutable accepted originals and application metadata in the
existing wire 0.1 key/value API. Session/provider/tool metadata, source timestamps,
redaction, routing and context policy belong to Journal. They are not mandatory
core event fields. The general engine Event contract is an engine acceptance
timestamp plus exact opaque data, with generic get, time range, full-text search
and follow operations. An engine acceptance timestamp must not be replaced by a
provider's claimed timestamp; the latter remains application data.

A future application migration must copy and verify every original byte, preserve
application event IDs, source session mapping and unknown-ACK retry intents, and
retain a recovery artifact before switching formats. Existing alpha key/value
files are not implicitly Event-profile files. Their supported readers and frozen
wire 0.1 contract continue to apply independently of engine release versions.
