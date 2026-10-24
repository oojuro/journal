# Source provenance

The initial agent-journal source is extracted from the already public repository
[oojuro/oojuro](https://github.com/oojuro/oojuro) at immutable commit
`d1b24f7ed020365e09431aeaa8f4a7ff05e27dc3` (the source candidate in engine PR #92).
This is a source preview, not a published package or a promoted native release.

Copied owners: `crates/oojuro`, `crates/oojuro-capture`, `crates/oojuro-proxy`,
`sdk`, the agent/project setup and native packaging scripts with their synthetic tests. Selected agent
usage documents and the original license notices accompany the source. Engine
libraries are dependencies pinned to the same commit in `Cargo.toml` and
`ENGINE_REV`; they are not copied into this repository.

The Rust application source and SDK implementation are unchanged. Extraction
changes the workspace membership, engine dependency locations and documentation
links. Native packaging now resolves engine/dictionary notices from the pinned
Git dependency instead of assuming a sibling store crate. Compatibility executable names remain `oojuro`, `oojurod` and
`oojuro-native`; creating this repository does not rename existing installations.

Original source remains Apache-2.0. Preserve `LICENSE`, `NOTICE` and `licenses/`.
Engine and dictionary attribution also applies when distributing linked native
artifacts; a source import is not a binary redistribution review.
