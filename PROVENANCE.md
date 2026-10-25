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
