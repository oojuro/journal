# Local capture cache: remove a duplicate projection

Cached native transcript searches previously opened `LocalCapture`, which
replayed authority and eagerly tokenized a complete in-memory index, then opened
and validated a second persistent index. The loaded-cache path did both jobs.

`search --cache` and `cache-merge` now use an immutable `LocalRead` handle. It
opens the same encrypted authority with the same writer/agent/first-ID/adapter,
validates the same options and every adapter context, including historical
contexts behind a later valid receipt, and runs Temporal's complete stream
validation when selected. It does not construct an unused lexical projection.

`PersistentIndex` then runs its existing manifest authentication, origin and
exact receipt-prefix checks, row/host/deduplication validation and authenticated
segment decode. Search still checks the current open log instance and tip. An
absent/corrupt/foreign cache rebuilds from authority; a valid older prefix extends
from authority. No mtime, row count or cache content becomes an authority grant.
This still performs work proportional to journal size even for a loaded cache.

Capture writes, retry/ACK handling, ordinary captured reads, uncached searches,
typed Record commands and both Record clocks retain their existing paths and
guards. Output fields, time/byte bounds, commands/configuration, engine pins and
storage formats are unchanged. No cloud account is needed.

## Actual process measurements

[Full receipt and samples](benchmarks/NATIVE_CAPTURE_CACHE_2026_10_24.json),
[harness](../scripts/bench-native-journal.py). Baseline is adopted Journal main
`ccf5bcfb4f0c87ec3784ef0c4ff7c56fd2684ed9`; runtime candidate is
`f001054237da1c227e4b5438c1bbda124757e95e`. Both link immutable engine
`128b76f8a462817910ed5ca622adf156ed1efcc6`. The preserved baseline binary matches
the adopted main tree; its SHA-256 and the independently rebuilt candidate's
distinct SHA-256 are in the receipt.

Each mode/size uses baseline/candidate/candidate/baseline order and 20 samples per
operation/trial. Timers include spawn, key/config loading, authority reopen,
operation, JSON output and exit. Every command is a fresh process. Each trial
starts at 1,000/10,000 synthetic records and appends 20 before sampling reads.
Ranges below are the two trials' p95 values in milliseconds (19th of 20 samples),
not confidence intervals or service promises.

| Fixture data state | Starting events | Loaded-cache search baseline p95 | Candidate p95 |
| --- | --- | --- | --- |
| Warm OS cache | 1,000 | 17.9–18.1 ms | 7.1–7.6 ms |
| Warm OS cache | 10,000 | 87.1–95.0 ms | 45.3–45.8 ms |
| Eviction requested | 1,000 | 18.2–18.3 ms | 7.0–7.1 ms |
| Eviction requested | 10,000 | 91.7–101.4 ms | 44.7–51.5 ms |

The missing-cache first build is measured separately, once per trial. At 10k it
falls from 111–116 to 70–74 ms with warm fixture data, and 111–114 to 70–78 ms
with eviction requested. These two observations per binary are **not p95**.

The eviction mode requests `POSIX_FADV_DONTNEED` on synthetic authority/cache/
source/input files before each process, outside its timer. This is a best-effort
fixture-data eviction request. It does not flush metadata, binary/shared-library
or device caches, and cannot establish a completely cold machine or cloud cold
start. Warm data and eviction-requested data are reported separately. No global
cache drop, real history or provider access is used.

Full-process controls are retained below. They demonstrate the narrow scope:
there is no general read/write/search speedup claim. In particular the warm 10k
typed lexical tail is slower in these trials, and eviction-requested 10k write
and uncached-search samples include adverse tails. Those paths are unchanged;
the shared laptop/frequency scaling and two short trials do not establish their
cause or a stable regression rate. All samples remain in the receipt.

| Data | Starting records | Command | Baseline p95 ms | Candidate p95 ms |
| --- | --- | --- | --- | --- |
| Warm | 1,000 | Typed append ACK | 5.3–5.3 | 5.2–5.3 |
| Warm | 1,000 | Typed get | 5.5–6.3 | 5.6–5.8 |
| Warm | 1,000 | Typed lexical search | 7.5–7.6 | 7.7–7.8 |
| Warm | 1,000 | Incremental capture ACK | 17.1–17.3 | 17.1–17.2 |
| Warm | 1,000 | Captured get | 16.9–16.9 | 16.9–17.0 |
| Warm | 1,000 | Uncached transcript search | 16.9–17.0 | 16.5–16.8 |
| Warm | 10,000 | Typed append ACK | 29.0–29.8 | 29.0–29.3 |
| Warm | 10,000 | Typed get | 29.7–29.9 | 29.7–30.1 |
| Warm | 10,000 | Typed lexical search | 50.1–51.4 | 52.7–55.0 |
| Warm | 10,000 | Incremental capture ACK | 76.8–78.5 | 76.1–76.8 |
| Warm | 10,000 | Captured get | 72.1–77.6 | 72.1–75.3 |
| Warm | 10,000 | Uncached transcript search | 71.9–77.0 | 71.8–74.5 |
| Evict | 1,000 | Typed append ACK | 5.4–5.5 | 5.6–5.7 |
| Evict | 1,000 | Typed get | 5.5–5.6 | 5.6–6.0 |
| Evict | 1,000 | Typed lexical search | 7.5–7.8 | 7.6–7.7 |
| Evict | 1,000 | Incremental capture ACK | 17.6–17.9 | 17.2–17.3 |
| Evict | 1,000 | Captured get | 16.9–16.9 | 16.7–16.8 |
| Evict | 1,000 | Uncached transcript search | 16.8–17.0 | 16.3–16.9 |
| Evict | 10,000 | Typed append ACK | 28.6–29.6 | 29.4–32.7 |
| Evict | 10,000 | Typed get | 28.9–29.7 | 29.0–29.4 |
| Evict | 10,000 | Typed lexical search | 50.6–50.6 | 49.0–51.4 |
| Evict | 10,000 | Incremental capture ACK | 76.2–83.3 | 78.7–82.3 |
| Evict | 10,000 | Captured get | 75.5–94.8 | 71.5–82.2 |
| Evict | 10,000 | Uncached transcript search | 75.1–87.5 | 72.1–91.1 |

Both modes verify sampled original typed fields, exact append retry ACKs, unique
lexical IDs and serialized scores across binaries, cached/uncached equality and
cross-binary reopen. Complete captured raw bytes restore exactly through the
other binary; SHA-256 matches across all four trials at each size. This synthetic
corpus is highly repetitive. The receipt's root storage excludes the separate
index directory, and this change makes no storage or memory reduction claim.

## Verification and reproduction

- Locked offline full Rust suite: **213 passed, 1 existing proxy overhead test
  ignored**. Includes native capture/Record/context, malformed historical context,
  crash/retry/raw preservation, local API/MCP and loopback protocol cases.
- Final native CLI suite: **8 passed**, including new missing/loaded/stale/corrupt/
  foreign-cache and foreign-writer checks for Codex/Claude/Pi, original rankings,
  time/byte limits and exact capture retry.
- Temporal SDK actual-native suite: **10 passed**, including cached/uncached
  equality through the new Temporal read handle and existing retry/original tests.
- Sixteen bounded 1k/10k full-process A/B trials passed, plus an eviction harness
  smoke. Final release binary built after runtime edits. `git diff --check` passes.
- SDK/setup/package implementations are unchanged. Their successful adopted-main
  checks are in the [adoption report](ENGINE_LOOKUP_ADOPTION.md); they were not
  rerun for this cache-only runtime change. GitHub Actions remains disabled.

Use the adoption report's release build flags and preserve a baseline executable
before building the candidate. Run modes sequentially, after builds/tests finish:

```sh
python3 scripts/bench-native-journal.py --baseline /absolute/baseline \
  --candidate /absolute/candidate --fixture-cache warm --output warm.json
python3 scripts/bench-native-journal.py --baseline /absolute/baseline \
  --candidate /absolute/candidate --fixture-cache evict --output evict.json
```

This is a bounded Linux local CLI qualification, not 1M records, ten concurrent
clients, universal SQLite/JSONL superiority, a standalone `.ojo` container, or
managed Cloud latency acceptance. It does not change Cloud, deploy, enroll a
source, publish an SDK or promote a release. Capture and ordinary read projection
costs, authority replay and the persistent cache's full validation remain future
work independent of new engine scale APIs.
