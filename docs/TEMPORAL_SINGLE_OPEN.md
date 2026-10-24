# Retain one healthy capture handle for Temporal append

For a new event, the native Temporal bridge opened `LocalCapture` to look up an
original receipt, loaded the current policy, dropped that healthy handle, then
opened and indexed the same authority again. It now retains the first handle and
applies the validated frozen policy and its immutable ID with a consuming
`LocalCapture::with_policy` method. This removes one complete authority replay
and lexical rebuild per new native append. `temporal-position` is unchanged.

The searchable-ACK contract is preserved: the initial handle has a healthy
projection, and append still commits authority before `finish_append` refreshes
that projection. A failure after commit still returns `CommittedUnavailable`,
blocks that handle's reads/new capture, and requires recovery. A policy handoff cannot revive
an unhealthy projection. Existing receipts retain their original policy decisions;
no raw/parsed bytes, clocks, IDs, fingerprints or persisted fields are rewritten.

Exact receipt lookup still precedes loading the current policy. Previously
committed intent can therefore return its original receipt even when current
policy is stopped or unreadable; changed intent still refuses. New intent must
load an enabled current policy. The same policy-ID bounds apply, and incomplete
lines still require the policy ID under which they began. The existing exclusive
Store handle remains held across this check instead of introducing a reopen gap.

## Actual Temporal CLI and SDK measurements

[Harness](../scripts/bench-temporal-process.mjs), [bounded synthetic fixture builder](../crates/oojuro-capture/examples/seed_temporal_cli.rs),
and [complete receipt](benchmarks/TEMPORAL_SINGLE_OPEN_2026_10_24.json).
Baseline is Journal main `f6058add824a57e71da7cc79e89661139d90db50`, runtime change
`41880827881546a4d8d94570be97e5879fd0718b`; harness correction `7ed2a93` changes
only the fixture oracle. Both binaries use immutable engine
`128b76f8a462817910ed5ca622adf156ed1efcc6`. Exact binary/helper hashes, source
hashes, flags and all samples are retained in the receipt.

The seed helper uses the actual LocalCapture API and refuses a populated root or
any identity/policy except its exact synthetic benchmark setup. It appends one
native Temporal frame per event, within existing admission limits, and verifies
complete raw restoration. Seed preparation is excluded from operation timers;
it took roughly 0.3 seconds for 1k and 31 seconds for 10k in the warm fixture run.
There is no direct frame fabrication or lifted namespace guard.

Each fixture mode uses baseline/candidate/candidate/baseline order with 20 samples
per operation/trial. Native append times include a fresh CLI process, stdin,
replay/index, frozen policy admission, durable append, projection refresh, output
and exit; its request is prepared before the timer. Actual SDK `record` times
also include its input preparation and unchanged `temporal-position` subprocess.
The SDK uses a 60-second diagnostic deadline; the shipped default is unchanged.
Twenty native appends and twenty SDK records precede read controls in each trial.

Warm data is compared with best-effort per-fixture `POSIX_FADV_DONTNEED` requests
before each timed operation. The SDK eviction request occurs before the complete
record call, not between its internally managed position and append subprocesses.
Binary/shared-library, metadata and device caches remain uncontrolled. This is
not a claim of a completely cold machine. Runs hold the shared benchmark lock
after builds/tests; hardware frequency scaling and background activity remain.

Each range below contains the two trials' p95 values in milliseconds (19th of
20 samples). The unmodified controls and adverse tails are retained.

| Data | Starting events | Operation | Baseline p95 ms | Candidate p95 ms |
| --- | --- | --- | --- | --- |
| warm | 1,000 | Native append | 76.4–76.8 | 45.4–47.4 |
| warm | 1,000 | Actual SDK record | 122.6–122.7 | 92.2–94.2 |
| warm | 1,000 | Position control | 47.3–48.3 | 45.8–47.6 |
| warm | 1,000 | Original get control | 37.5–40.4 | 37.7–38.7 |
| warm | 1,000 | Uncached search control | 45.2–46.3 | 45.4–46.4 |
| warm | 10,000 | Native append | 475.0–479.9 | 245.8–246.5 |
| warm | 10,000 | Actual SDK record | 698.7–729.2 | 484.1–487.2 |
| warm | 10,000 | Position control | 243.9–250.1 | 240.3–240.3 |
| warm | 10,000 | Original get control | 209.9–212.4 | 202.0–202.4 |
| warm | 10,000 | Uncached search control | 239.9–243.7 | 234.2–234.8 |
| evict | 1,000 | Native append | 78.2–81.9 | 49.7–52.4 |
| evict | 1,000 | Actual SDK record | 122.6–124.3 | 92.5–95.7 |
| evict | 1,000 | Position control | 50.2–51.4 | 46.5–49.7 |
| evict | 1,000 | Original get control | 38.1–38.3 | 37.7–38.1 |
| evict | 1,000 | Uncached search control | 50.2–50.5 | 48.8–50.5 |
| evict | 10,000 | Native append | 461.7–487.0 | 255.1–255.6 |
| evict | 10,000 | Actual SDK record | 700.6–733.4 | 499.2–536.9 |
| evict | 10,000 | Position control | 241.4–257.4 | 242.6–253.0 |
| evict | 10,000 | Original get control | 205.5–212.6 | 205.6–210.3 |
| evict | 10,000 | Uncached search control | 235.6–248.6 | 240.0–244.4 |

The warm 10k
native append p95 falls from about 475–480 ms to 246 ms. Actual SDK `record` falls
from about 699–729 ms to 484–487 ms. At 1k, native append improves about 76–77 to
45–47 ms and SDK record about 123 to 92–94 ms. These are local serial diagnostics,
not a service guarantee or completion of the owner's latency target. In the
eviction-requested 10k runs, native append is about 255–256 ms and SDK record
499–537 ms, also exceeding 250 ms. Unchanged control tails vary; for example
eviction-requested 1k search spans about 50 ms for both binaries, with a slightly
higher candidate maximum. No gain for unchanged read/search paths is asserted.

## Correctness and scope

Build baseline and candidate apps with the identical release flags from the
[adoption report](ENGINE_LOOKUP_ADOPTION.md), preserving a baseline executable
before the candidate build. Build the fixture helper from this candidate:

```sh
RUSTFLAGS='-C target-cpu=x86-64' CFLAGS='-march=x86-64 -mtune=generic' \
  cargo build --locked --release -p oojuro-capture --example seed_temporal_cli
node scripts/bench-temporal-process.mjs --baseline /absolute/baseline-native \
  --candidate /absolute/candidate-native --seed /absolute/seed_temporal_cli \
  --fixture-cache warm --output warm.json
node scripts/bench-temporal-process.mjs --baseline /absolute/baseline-native \
  --candidate /absolute/candidate-native --seed /absolute/seed_temporal_cli \
  --fixture-cache evict --output evict.json
```

Coordinate exclusive benchmark access before timing. The harness creates and
removes its own private roots/keys; it does not accept existing user roots.

- Locked offline full Rust suite: **216 passed, 1 existing ignored proxy overhead
  case**. A first test attempt used a search term shared by its newly stored
  output; disjoint fixture terms corrected the oracle before the passing run.
- Policy handoff preserves original withheld receipts. An exact retry invokes
  neither refresh nor decide on a deliberately panicking replacement policy.
  New output remains searchable, with its newly supplied immutable policy ID.
- Invalid policy IDs refuse without source/ID progress. Changed policy cannot
  consume a partial line; restoring its original policy finishes the retained
  line without reinterpreting previously withheld content.
- Projection failure after policy handoff remains committed/unavailable and
  cannot be revived through another handoff. Reopen returns the same raw bytes
  and receipt. Actual SIGKILL after authority and after projection runs through
  the handoff path and preserves original IDs/raw/retry behavior.
- Actual native Temporal SDK suite: **11 passed**, including lost ACK retry with
  unreadable policy, stopped-policy exact retry, new/changed-intent refusal,
  retained call denial, raw originals, and read/search/cache checks.
- All sixteen measured full-process A/B trials and the 20-event smoke pass.
  The complete raw SHA-256 matches across each size's four trials in each mode.
- Release app/helper builds, harness syntax and diff checks pass. The harness
  smoke corrected one oracle that incorrectly expected JSON-quoted text for
  Temporal string payloads before measured trials. SDK implementation, commands,
  setup, source pins, formats, raw consent and file-adapter capture are unchanged.
  GitHub Actions remains disabled; these are local checks.

At 10k single-event Temporal frames, unchanged position still costs roughly
240–250 ms p95 and original get roughly 202–212 ms in the warm comparison. Full
authority/context replay, original-receipt lookup and the position projection
remain. This namespace has many more sealed batches/contexts than the earlier
Codex file fixture; its timings should not be replaced with that fixture's read
numbers. SDK write/read targets remain unmet here even after the narrow gain.

This does not raise the 100k-event/128-MiB replay-metadata bounds, integrate the
new engine scale prototype, qualify 1M/ten-client or managed Cloud targets, add a
standalone `.ojo` container, deploy, contact a provider or publish a package.
