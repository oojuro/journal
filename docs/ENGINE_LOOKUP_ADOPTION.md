# Engine lookup adoption through Journal's local commands

This historical report qualifies the initial `128b76f` dependency adoption and
retains its original source, checks and measurements. The current immutable
pin and its local checks are recorded in the
[current adoption report](ENGINE_PROJECTION_PIN_ADOPTION.md). Later Journal
optimizations have their own reports linked below.

Journal adopts immutable engine main
`128b76f8a462817910ed5ca622adf156ed1efcc6` from extraction pin
`d1b24f7ed020365e09431aeaa8f4a7ff05e27dc3`. All five declared engine libraries,
the transitive rank library and `ENGINE_REV` move together. The lockfile changes
only six Git source identities; registry versions, application/SDK source,
commands, configuration, public engine signatures and storage formats stay the
same. Local commands still require no cloud account.

This qualifies engine adoption, rather than completing Journal performance work.
The checked engine lookup helps its already-open Record API. Journal's process
startup, authority replay and index rebuilding remain substantial. At 10,000
records, these trials show a modest typed lexical-search improvement, slightly
slower typed exact reads, and no established append, capture or storage gain.

## Actual CLI measurements

[Reproducible harness](../scripts/bench-native-journal.py) and
[full synthetic receipt](benchmarks/NATIVE_CLI_ENGINE_ADOPTION_2026_10_24.json).
These are Linux release `oojuro-native` processes, not engine-only API calls.
Baseline Journal main is `8a936b68d8ecbdece8303075ff6d22176cfa41e2`; candidate
application source is identical, linked to the new engine. Independent release
target directories produced distinct binary hashes recorded in the receipt.

Each size uses baseline/candidate/candidate/baseline order, with 20 measured
processes per operation in each trial. The table gives the range of the two
trial p95 values, in milliseconds; p95 is the nearest-rank 19th of 20 samples.
Every timer includes spawn, root/key/config loading, replay, operation, JSON
output and process exit, while excluding Python's later response validation.
Each trial starts with 1,000 or 10,000 synthetic records and adds 20 single-record
writes before sampling reads. Transcript commands use the Codex file adapter;
Claude/Pi/Temporal correctness is covered separately by the suites below.

| Starting records | Command path | Baseline p95 ms | Candidate p95 ms |
| --- | --- | --- | --- |
| 1,000 | Typed single-record append ACK | 10.3–12.1 | 10.4–10.6 |
| 1,000 | Typed exact-ID read | 9.7–11.7 | 9.5–10.6 |
| 1,000 | Typed lexical search | 14.7–15.3 | 13.7–14.7 |
| 1,000 | Transcript incremental capture ACK | 18.7–19.1 | 18.8–19.5 |
| 1,000 | Captured event read | 17.4–17.8 | 18.2–19.0 |
| 1,000 | Transcript lexical search | 19.2–21.0 | 17.6–19.2 |
| 1,000 | Transcript lexical search with loaded cache | 19.7–19.9 | 19.0–20.1 |
| 10,000 | Typed single-record append ACK | 30.1–30.6 | 29.9–30.8 |
| 10,000 | Typed exact-ID read | 30.3–31.1 | 31.7–32.1 |
| 10,000 | Typed lexical search | 50.6–52.5 | 47.7–48.9 |
| 10,000 | Transcript incremental capture ACK | 69.4–70.3 | 69.6–69.8 |
| 10,000 | Captured event read | 65.3–66.1 | 65.1–65.9 |
| 10,000 | Transcript lexical search | 65.5–66.1 | 66.4–67.5 |
| 10,000 | Transcript lexical search with loaded cache | 77.4–78.2 | 77.2–78.3 |

The initial whole-file transcript imports are separate one-shot observations,
not p95 measurements: about 25–34 ms for 1,000 events and 149–153 ms for 10,000.
Allocated root storage is unchanged between binaries in these runs: typed roots
use 45,056/98,304 bytes and captured roots 65,536/266,240 bytes at the two sizes.
These highly repetitive synthetic inputs compress well; this is no general
storage-ratio claim. Small logical-size differences come from independently
recorded timing/provenance metadata and establish no storage improvement.

The benchmark uses a warm OS filesystem cache, a shared Intel Core Ultra 7 258V
laptop with frequency scaling, and serial subprocesses. It does not flush OS
caches, isolate CPUs, test crash durability on the measured filesystem, compare
SQLite/JSONL, use a real transcript, contact a provider, or measure managed Cloud
latency. Twenty samples give a descriptive tail diagnostic, not a service SLO.
There is no 1M-record, ten-client, `.ojo` container or cloud-target acceptance.
The existing 100,000-record/128-MiB typed Record namespace bounds remain intact.

To reproduce, build the baseline and candidate release executables from their
respective checkouts using separate `CARGO_TARGET_DIR` directories:

```sh
RUSTFLAGS='-C target-cpu=x86-64' CFLAGS='-march=x86-64 -mtune=generic' \
  cargo build --locked --release --no-default-features -p oojuro --bin oojuro-native
python3 scripts/bench-native-journal.py \
  --baseline /absolute/baseline/target/release/oojuro-native \
  --candidate /absolute/candidate/target/release/oojuro-native \
  --sizes 1000,10000 --samples 20 --output local-result.json
```

The harness creates and deletes its own private synthetic roots/keys. It checks
sampled original typed fields (including Unicode, CRLF and NUL), exact append
retry receipts, unique lexical hit IDs and serialized scores across both
binaries, cached/uncached capture equality, and reopening writes using the other
binary. The complete captured raw source is restored through the other binary
and compared byte for byte; each size's raw SHA-256 matches across all trials.
Source generation/file writes and their fsync happen before capture timers.

## Checks on the adopted dependency

- Locked offline Cargo metadata: one immutable source identity for all six
  engine libraries; no package version or dependency-list change in the lockfile.
- README Rust command: **212 passed, 1 existing ignored**, across 23 targets.
  Includes 8 actual Record CLI tests, 7 capture CLI tests, Record context,
  capture crash/retry/raw preservation, local API/MCP, and 12 loopback hosted
  protocol tests. The ignored case is the pre-existing proxy `overhead` test.
- Agent/project/combined setup: **2 + 3 + 3 passed**; native packaging: **6 passed**.
- TypeScript SDK: **114 passed**, eight strict NodeNext/ES2023 declaration smoke
  files, and offline package/install/export checks passed.
- Temporal SDK: **24 passed**, including 10 actual-native recording/retry/read/
  search/raw tests, plus synthetic in-memory codec/recall clients. Link the local
  `sdk/typescript` peer as `sdk/temporal/node_modules/@oojuro/v2-client` and set
  `OOJURO_NATIVE_TEST_BIN` to the candidate binary; no Temporal server is used.
- Final bounded A/B run: all eight trials passed. An earlier harness smoke
  exposed lexical normalization of alphabetic fixture suffixes; unique numbered
  tokens replaced those inputs before measurement. An initial Temporal attempt
  lacked the local peer; all suites passed after linking it. Initial A/B fixture
  validation overlapped SDK tests; the recorded run followed those checks.

GitHub Actions remains disabled. These are local checks using Rust/Cargo 1.99.0
and Node 24.22.1, not a claim of remotely executed CI or a published release.

## Remaining work

This section records the adoption baseline. A subsequent
[cached capture search change](LOCAL_CAPTURE_CACHE_OPTIMIZATION.md) removes the
duplicated projection for cached searches. A separate
[receipt-read change](LOCAL_RECEIPT_READ_OPTIMIZATION.md) removes the unused
projection for original receipt reads; capture and uncached search stay eager.

1. `native/records.rs` opens/replays Record authority for every command, then
   rebuilds the full lexical index for each `record-search`/`record-context`.
   A faster lookup inside the open View cannot remove those process costs.
2. `LocalCapture::open_adapter` eagerly rebuilds its in-memory projection before
   every capture/read. Native `search --cache` subsequently opens a persistent
   index as well. The loaded-cache trials are slower than uncached searches;
   removing duplicated projection work requires a separate qualified change,
   preserving authoritative replay, scope/watermarks and corrupt-cache recovery.
3. Keep the generic engine, agent Journal and managed Cloud owners separate.
   Cloud must adopt a new Journal revision and direct engine revision
   `128b76f8a462817910ed5ca622adf156ed1efcc6` together; mixing Git sources produces
   incompatible Rust types. This change does not update Cloud or deploy anything.
