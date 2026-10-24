# Read original receipts without building a search index

Native `get`, `timeline`, `stats` and `export` read committed receipts through
`ReceiptView`. They previously opened `LocalCapture` and built a complete lexical
projection that none of these operations used. These four commands now open the
existing immutable `LocalRead` handle, then execute their unchanged ReceiptView
dispatch and output logic. No engine dependency, format or public CLI/config
change is involved.

Authority is still fully replayed. `CaptureLog` validates stored IDs, agents,
revisions and receipt coverage. `LocalRead` validates the same adapter identity,
options and every historical context, including Temporal stream validation.
ReceiptView independently checks writer/agent scope, origin, exact receipt prefix,
cursor/range binding and entry/byte limits. These checks remain on every read.
Committed withholding decisions and original eligibility/provenance remain
receipts rather than being reinterpreted using the current search projection.

Capture/append acknowledgements, retry behavior, cached and uncached search and
typed Record commands retain their existing paths. Raw restoration still uses
its existing separate consent and capture path. No raw bytes, source descriptors
or parser contexts are newly exposed by the receipt commands. Hosted permission
and revocation paths are unchanged; local receipt scope still is not a hosted
authorization grant.

## Actual CLI comparison

[Full synthetic receipt](benchmarks/NATIVE_RECEIPT_READS_2026_10_24.json),
[harness](../scripts/bench-native-journal.py). Baseline is Journal main
`0c31ae7839a7df062715b914b1971b820e89daac`, runtime candidate is
`e4f30c7906baa79b9975cbaea62872b8506a775d`. Both use engine
`128b76f8a462817910ed5ca622adf156ed1efcc6`. The preserved baseline executable is
tree-equivalent to main; it and the rebuilt candidate have distinct recorded
SHA-256 hashes and identical release flags/dependency versions.

Both modes use baseline/candidate/candidate/baseline order at each size, with 20
samples per command/trial. Each sample is a fresh CLI process, including spawn,
root/key/config loading, full authority replay, operation, JSON output and exit.
The table gives the range of the two trials' p95 values in milliseconds, using
nearest-rank p95 (19th of 20 samples). These are descriptive diagnostics.

Fixtures start with 1,000/10,000 synthetic Codex events and add 20 single-event
captures before reads. `get` selects one exact ID. Timeline/export/stats select
one stored receipt ordinal; this does **not** measure whole-journal export or
full-journal statistics throughput. The empty session metadata line is a
separate receipt ordinal and is accounted for explicitly.

| Fixture data | Starting events | Command | Baseline p95 ms | Candidate p95 ms |
| --- | --- | --- | --- | --- |
| Warm | 1,000 | get | 16.5–17.3 | 5.6–5.8 |
| Warm | 1,000 | timeline | 16.8–17.7 | 5.7–5.8 |
| Warm | 1,000 | stats | 17.1–17.5 | 5.6–5.6 |
| Warm | 1,000 | export | 17.0–17.3 | 5.6–5.6 |
| Warm | 10,000 | get | 67.6–72.7 | 30.1–31.1 |
| Warm | 10,000 | timeline | 69.0–72.8 | 30.8–30.8 |
| Warm | 10,000 | stats | 67.1–72.9 | 30.3–31.1 |
| Warm | 10,000 | export | 70.6–73.3 | 30.2–30.7 |
| Evict | 1,000 | get | 16.9–18.9 | 5.7–5.8 |
| Evict | 1,000 | timeline | 17.1–17.5 | 5.8–7.0 |
| Evict | 1,000 | stats | 17.3–18.6 | 5.7–9.1 |
| Evict | 1,000 | export | 17.4–18.3 | 5.9–8.7 |
| Evict | 10,000 | get | 65.5–125.7 | 31.0–31.6 |
| Evict | 10,000 | timeline | 64.0–111.9 | 30.6–31.0 |
| Evict | 10,000 | stats | 65.7–105.7 | 30.7–30.7 |
| Evict | 10,000 | export | 64.4–99.2 | 30.9–30.9 |

The retained 10k controls below show why the claim is limited to receipt reads.
The complete 1k controls and all individual samples are in the receipt. Some
unchanged paths have adverse tails: for example eviction-requested 1k typed
append p95 is 5.3–5.7 ms baseline versus 6.7–9.5 ms candidate. This shared laptop
with frequency scaling and two short trials do not establish their cause or a
stable regression rate. No broad write/search gain is claimed.

| Data | Unchanged command, 10k records | Baseline p95 ms | Candidate p95 ms |
| --- | --- | --- | --- |
| Warm | Typed append ACK | 27.7–30.0 | 29.0–29.7 |
| Warm | Typed exact-ID read | 28.3–30.1 | 26.5–28.7 |
| Warm | Typed lexical search | 48.0–48.2 | 46.9–48.9 |
| Warm | Incremental capture ACK | 70.5–84.3 | 68.7–74.0 |
| Warm | Uncached transcript search | 68.8–72.7 | 69.3–72.2 |
| Warm | Cached transcript search | 46.1–49.4 | 43.9–44.2 |
| Evict | Typed append ACK | 28.2–30.0 | 30.0–30.1 |
| Evict | Typed exact-ID read | 29.6–30.9 | 29.9–29.9 |
| Evict | Typed lexical search | 47.6–52.5 | 48.3–52.8 |
| Evict | Incremental capture ACK | 69.6–71.7 | 70.4–70.6 |
| Evict | Uncached transcript search | 66.1–102.8 | 65.6–66.4 |
| Evict | Cached transcript search | 43.2–62.9 | 43.0–43.5 |

Eviction mode requests `POSIX_FADV_DONTNEED` on only the synthetic authority,
cache, source and input files before each process, outside timers. It is a
best-effort fixture-data eviction request; binary/shared-library, metadata and
device caches are uncontrolled. It establishes neither a fully cold machine
nor cloud cold-start performance. The final modes run sequentially after builds
and tests; no concurrent large engine benchmark campaign was running.

## Checks and limits

- Locked offline full Rust suite: **214 passed, 1 existing ignored proxy overhead
  test**. The 9 actual native CLI cases cover all receipt paths, historical
  withholding, raw consent, key permissions, scopes and tampered cursors.
- A new Codex/Claude/Pi case preserves Unicode/CRLF/NUL originals, old cursor
  prefixes after later append, all four commands' foreign-writer/byte refusals,
  and entry/range/cursor guards.
- Temporal actual-native SDK suite: **10 passed**, with added timeline/export/
  stats parity and original-text checks through the new read routing.
- All sixteen 1k/10k full-process A/B trials and the eviction harness smoke pass.
  The harness checks selected receipt IDs/text/counts, sampled typed originals,
  exact retry ACKs, lexical IDs/scores, cached/uncached equality, cross-binary
  reopen, and complete byte-exact raw restoration. Raw SHA-256 matches across
  each size's four trials in both modes.
- Release build, `git diff --check` and receipt/source hash checks pass. SDK/setup/
  package implementations are unchanged; their adopted-main checks were not
  rerun here. GitHub Actions remains disabled.

Reproduce with the release flags and two sequential harness commands in the
[cache report](LOCAL_CAPTURE_CACHE_OPTIMIZATION.md). The harness now also records
the three additional receipt commands. No real history or provider is used.

Full authority replay and bounded retained metadata remain. Capture and uncached
lexical search still build their projections. This change establishes no 1M/
ten-client, managed Cloud, storage/memory, complete export-throughput or standalone
`.ojo` container claim. It neither changes Cloud nor deploys, enrolls a source,
publishes an SDK or promotes a release.
