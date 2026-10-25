# Conditional projection retirement dependency

All seven selected engine packages resolve to the single immutable engine
source `2bb537b4f35b9e5902c1f135a6334c8a5d8c8e4a`. The registry package graph is
unchanged. This adopts the engine's generic conditional retirement helper for a
coherent Cloud consumer; Journal does not add a call site or keyed application
semantics in this change.

`Appender::retire_if_present` retires a live prior immutable ID and returns true.
An absent or already retired prior ID returns false. Pending newly pushed rows
are outside that lookup. Strict retirement retains its existing behavior, and
actual inner storage, authentication and bounds failures poison the generation.
The neutral Event adapter keeps cursor validation separate from inner mutation.
No source wire/profile, original bytes, retry identity or publication guarantee
changes with this dependency.

Qualification runs the existing full synthetic Rust application/compatibility
suite and strict application Clippy from a source-aware build. The acceptance
receipt is `docs/qualification/engine-conditional-retirement-pin-2026-10-25.json`.
Setup scripts, SDKs and application source are unchanged; hosted behavior and
release promotion require their own qualification. In particular, the existing
immutable public0.1 archive and tag keep their original source scope.

Cloud must advance its direct engine dependencies together with this Journal
revision, preserving one Git identity across the entire linked Rust family.
