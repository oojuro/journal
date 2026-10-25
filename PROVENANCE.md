# Source provenance

The initial agent-journal source is extracted from the already public repository
[oojuro/oojuro](https://github.com/oojuro/oojuro) at immutable commit
`d1b24f7ed020365e09431aeaa8f4a7ff05e27dc3` (the source candidate in engine PR #92).
This is a source preview, not a published package or a promoted native release.

Copied owners: `crates/oojuro`, `crates/oojuro-capture`, `crates/oojuro-proxy`,
`sdk`, the agent/project setup and native packaging scripts with their synthetic tests. Selected agent
usage documents and the original license notices accompany the source. Engine
libraries were initially dependencies pinned to that same commit in `Cargo.toml` and
`ENGINE_REV`; they are not copied into this repository.

The initial extraction kept the Rust application source and SDK implementation unchanged. Extraction
changes the workspace membership, engine dependency locations and documentation
links. Native packaging now resolves engine/dictionary notices from the pinned
Git dependency instead of assuming a sibling store crate. Compatibility executable names remain `oojuro`, `oojurod` and
`oojuro-native`; creating this repository does not rename existing installations.

Original source remains Apache-2.0. Preserve `LICENSE`, `NOTICE` and `licenses/`.
Engine and dictionary attribution also applies when distributing linked native
artifacts; a source import is not a binary redistribution review.

## Earlier engine dependency adoption

The earlier immutable engine dependency was
`ee9b89177a0a0695232b179b639d9db857b2eabc`, the generic engine's main after
engine-only extraction, checked exact-ID/ranked-original lookup changes and
the qualified cached-projection query-key optimization in engine PR #99.
All five declared engine libraries and the transitive rank library resolve to
this one Git source. Their public signatures, storage formats, authority checks
and dependency versions are unchanged from the extraction pin. The Journal
application and SDK implementations are unchanged by this dependency adoption.
The cached projection helper changed by engine PR #99 has no Journal call site;
adopting it does not establish a Journal performance improvement.

That dependency's source qualification is documented in
[the dependency adoption report](docs/ENGINE_PROJECTION_PIN_ADOPTION.md).
[Earlier local CLI measurements](docs/ENGINE_LOOKUP_ADOPTION.md) retain their
original `128b76f` engine and Journal source scope. A managed Cloud consumer
must advance its direct engine dependencies and its Journal dependency together
to preserve Rust type identity; this Journal change does not update or deploy
the hosted service.

## Standalone session-file adoption

The `.ojo` application lane adds `ojo-journal` without replacing the compatibility
executables or the existing public library APIs. All six legacy engine libraries
and the standalone `ojo` crate resolve to the same immutable engine source
`915b2db9b94a6dde294ecec27fc0e5de6ba16f08`. This includes engine PRs #102–#104:
qualified I/O/archive optimization, generic file tools and bounded prefix pages.
No registry dependency version is upgraded by this pin; the new standalone crate
adds its reviewed getrandom 0.3 platform closure. The engine owns wire encoding,
locking, syncing, cryptography and recovery. The application owns per-session
routing, transcript parsing, secret redaction, optional delivery and archiving.

Historical adoption reports above keep their exact original source scopes. New
application and installed-client qualification is recorded separately; a local
pass does not establish million-record Cloud latency or a production deployment.

## Replayed-prefix confirmation

The replay-confirmation checkpoint used engine pin
`a49fd108f5acc0b8658a84631c103d3e2e6f7be0`, including the separately versioned
Event profile foundation and the durable replay/duplicate confirmation fix.
All seven selected engine packages share that immutable source; registry versions
remain unchanged. Journal's existing session files still use wire 0.1 KV data and
do not silently become Event-profile files. Explicit migration remains required.

Application acknowledgements reconstructed from existing batch fingerprints now
confirm the verified prefix before success. Writable opens confirm both that
prefix and the containing directory before reconstructed metadata or upload
staging. Healthy owner handles retain their successful barrier; repeated known
operations need no additional file sync. The Linux fault regression forces a
completed append's fsync to fail, then refuses a reopened duplicate ACK while
sync still fails. Exact retry succeeds with zero new records after confirmation,
with original file bytes and raw/encrypted originals unchanged.

## Canonical filename decision

Owner decision [#110](https://github.com/oojuro/oojuro/issues/110) selects `.juro`
for newly created database, session archive and recovery filenames. Existing
`.ojo` paths are explicit legacy candidates and are never renamed automatically;
two candidates for one identity are refused. This changes filenames and current
guidance, not API/CLI names, header magic, wire/profile versions or frozen vectors.
Earlier source/release/qualification artifacts above retain their original scope.

## Application namespace confirmation

Root initialization now creates and confirms missing ancestors one at a time,
without changing existing directory modes. Opening the root confirms the same
protected inodes from which its identity and encryption key were read, then the
ancestor and session/lock directory entries. This covers application-created
ancestors beyond the engine file parent's responsibility. Failed synchronization
refuses success and retains the original metadata/key for retry. The engine pin
remains unchanged in this application-only followup.

The synthetic Linux namespace regression forces actual fsync EIO for ancestor
publication, root.json and encryption-key confirmation. It first reproduces an
incorrect acknowledgement on the previous qualified application, then checks
that failed init/reopen retries refuse acknowledgements, a healthy retry succeeds
and all existing file bytes and modes remain unchanged.

## Generic Event session adoption

The Event application lane advances all seven selected engine packages to one
immutable source, `ad0085ef80b53a0af448f3071d4a10c520b8dabd`. This includes the
generic Event API, indexed projection API, file/namespace confirmation fixes,
bounded group durability and exact retained-intent reads. The engine's new zstd
dependency edge uses the already selected package/version; no registry package
is added or upgraded by this adoption.

New sessions use the separately marked engine Event profile plus Journal's own
application profile 1. The engine still owns only UTC acceptance time, exact data
and opaque retrieval/retry bookkeeping. Journal's Begin/Commit grammar owns
agent metadata and application batch visibility. Original engine intents are
retained inside the authoritative encrypted Begin, so a lost acknowledgement
cannot replace a surviving original's time, cursor or bytes. No intent sidecar or
duplicate plaintext original is introduced.

Existing KV sessions remain usable. Explicit `migrate-session` retains a
byte-exact, independently verified recovery database before conversion, copies
originals and retry bindings, verifies the manifest and all live values, then
confirms canonical replacement. Conversion does not silently rename `.ojo` paths
or change earlier qualification artifacts. It gives old KV originals new engine
acceptance times while preserving their application timestamps.
