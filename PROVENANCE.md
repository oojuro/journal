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

The Rust application source and SDK implementation are unchanged. Extraction
changes the workspace membership, engine dependency locations and documentation
links. Native packaging now resolves engine/dictionary notices from the pinned
Git dependency instead of assuming a sibling store crate. Compatibility executable names remain `oojuro`, `oojurod` and
`oojuro-native`; creating this repository does not rename existing installations.

Original source remains Apache-2.0. Preserve `LICENSE`, `NOTICE` and `licenses/`.
Engine and dictionary attribution also applies when distributing linked native
artifacts; a source import is not a binary redistribution review.

## Engine dependency adoption

The current immutable engine dependency is
`128b76f8a462817910ed5ca622adf156ed1efcc6`, the generic engine's main after
engine-only extraction and checked exact-ID/ranked-original lookup changes.
All five declared engine libraries and the transitive rank library resolve to
this one Git source. Their public signatures, storage formats, authority checks
and dependency versions are unchanged from the extraction pin. The Journal
application and SDK implementations are unchanged by this adoption.

Actual local CLI measurements and their limits are documented in
[the adoption report](docs/ENGINE_LOOKUP_ADOPTION.md). A managed Cloud consumer
must advance its direct engine dependencies and its Journal dependency together
to preserve Rust type identity; this Journal change does not update or deploy
the hosted service.
