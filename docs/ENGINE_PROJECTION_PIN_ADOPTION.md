# Qualified engine dependency adoption — 2026-10-24

Journal advances its five declared engine dependencies and transitive rank
library from `128b76f8a462817910ed5ca622adf156ed1efcc6` to immutable generic
engine main `ee9b89177a0a0695232b179b639d9db857b2eabc`. The latter includes
[engine PR #99](https://github.com/oojuro/oojuro/pull/99), which removes repeated
source-key cloning from cached `record::projection` queries. Journal does not
call that public helper. This adoption therefore makes no Journal speedup claim;
the engine's bounded cached-query measurements do not transfer to Journal's
capture, read, lexical-search or Temporal command paths.

The Journal base is `1b0f286`, after its separately qualified cached-search,
receipt-read and Temporal single-open changes. Pin-only runtime commit
`14467a0` changes `Cargo.toml`, `Cargo.lock` and `ENGINE_REV`. Application and SDK
code, command/config compatibility, registry versions, public engine signatures,
storage formats, authority replay and admission limits remain unchanged. Local
use still requires no cloud account.

## Single immutable source

Locked offline Cargo metadata resolves `oojuro-core`, `oojuro-engine`,
`oojuro-nine`, `oojuro-rank`, `oojuro-store` and `oojuro-trace` to exactly:

```text
git+https://github.com/oojuro/oojuro.git?rev=ee9b89177a0a0695232b179b639d9db857b2eabc#ee9b89177a0a0695232b179b639d9db857b2eabc
```

All six package versions remain `0.1.0`. Replacing that source identity with
the previous one yields the exact previous lockfile; dependency lists,
registry versions and checksums do not change. The five workspace dependency
declarations and `ENGINE_REV` use the same full commit. No sibling checkout,
floating branch, local patch or second engine source is introduced.

## Local qualification

All checks passed locally on pin-only runtime commit
`14467a013bf5c9623b4b8cd3ecf053bd723db80c`, using a fresh dedicated target and
the shared host's exclusive cooperating benchmark lock:

- Full locked offline Rust suite: **216 passed, 1 existing ignored**, across
  23 targets. This includes 9 actual native CLI tests, 8 Record CLI tests,
  Record context, scope/byte limits, raw original preservation, retry/kill
  recovery, searchable-ACK/CommittedUnavailable, local API/MCP and loopback
  hosted protocol tests. The ignored case is the existing proxy `overhead`.
- Compatibility executables `oojuro`, `oojurod`, `oojuro-native` build.
- Agent/project/combined setup: **2 + 3 + 3 passed**; native packaging:
  **6 passed**.
- TypeScript SDK: **114 passed**; offline package/install/export check passed.
- Temporal SDK: **25 passed**, including **11 actual-native** cases using the
  newly built `oojuro-native`. These cover recording/retry/reopen/read/search,
  exact original raw bytes, stopped/unreadable policy, lost ACK, denied intent
  and recall permissions/budgets. The local SDK peer is linked only for these
  tests; no Temporal server or provider is used.

The [machine-readable qualification receipt](ENGINE_PROJECTION_PIN_ADOPTION_2026_10_24.json)
records exact sources, commands, counts, native binary hash and local log hashes.
Rust/Cargo 1.99.0 and Node 24.22.1 were used. Native correctness tests use the
debug binary, not a release latency measurement. GitHub Actions remains
disabled; these results do not claim remote CI.

Core reproduction commands are:

```sh
cargo metadata --locked --offline --format-version 1
cargo test --locked --offline --no-default-features --lib --tests \
  -p oojuro-capture -p oojuro-proxy -p oojuro -- --test-threads=1
cargo build --locked --offline --no-default-features -p oojuro --bins
OOJURO_NATIVE_TEST_BIN=/absolute/dedicated-target/debug/oojuro-native \
  node --test sdk/temporal/test/*.test.js
```

As in the previous adoption report, Temporal tests need the local
`sdk/typescript` package linked at
`sdk/temporal/node_modules/@oojuro/v2-client`; this temporary peer is removed
after the gate. Give fixtures a private real-directory `TMPDIR` with short
socket paths. Full setup/SDK commands are in the receipt and README.
Previous measured benchmarks retain their original revisions; they are not
rerun or relabeled as results for this pin.

## Consumer coordination and remaining work

Cloud must adopt the final Journal revision from this change together with
direct engine dependencies at `ee9b89177a0a0695232b179b639d9db857b2eabc`.
Mixing Git source revisions produces distinct Rust types even when public
signatures match. This source PR does not update Cloud or select a serving
artifact.

Journal still replays full local authority at process startup; typed searches
rebuild their lexical index, and capture/uncached-search paths still construct
their in-memory projection. The [Temporal report](TEMPORAL_SINGLE_OPEN.md)
records its remaining position/context cost and missed end-to-end SDK-write
target even at 10k. Existing 100k-record/event and 128 MiB metadata bounds remain;
this pin does not qualify one million records, ten clients or Cloud SLOs.

The generic engine, agent Journal and managed Cloud remain separate owners.
Retrieval stays lexical. No real logs, transcripts, keys, provider action,
source enrollment, release, deployment or standalone `.ojo` container is part
of this change.
