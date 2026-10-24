# Oojuro Journal

Record and search agent activity using the [Oojuro embedded database](https://github.com/oojuro/oojuro).
This repository owns agent capture, transcript adapters, recording proxies,
agent/session conventions, the recorder, journal CLI/MCP tools and client SDKs.

**Status: source preview.** Extracted from the existing public source at an
immutable revision; see [provenance](PROVENANCE.md). Engine dependencies are
pinned in `Cargo.toml` and `ENGINE_REV`; the [current adoption report](docs/ENGINE_LOOKUP_ADOPTION.md)
qualifies those dependencies through local CLI capture/read/search. A separate
[cached capture search report](docs/LOCAL_CAPTURE_CACHE_OPTIMIZATION.md) measures
removal of duplicated projection work; the [receipt-read report](docs/LOCAL_RECEIPT_READ_OPTIMIZATION.md)
measures ordinary reads without an unused search index. The
[Temporal append report](docs/TEMPORAL_SINGLE_OPEN.md) measures retaining its healthy
capture handle across policy admission. Existing engine branches and installed
releases remain available during the repository transition.

## Repository boundaries

| Repository | Responsibility |
| --- | --- |
| [oojuro](https://github.com/oojuro/oojuro) | Embedded storage, compression, durability and retrieval; the planned `.ojo` database format |
| [o-journal](https://github.com/oojuro/o-journal) | Agent-specific recording, integrations and history tools |
| [o-cloud](https://github.com/oojuro/o-cloud) | Managed hosting, sharing, accounts, billing and the `oojuro.com` website |
| [o-release-operator](https://github.com/oojuro/o-release-operator) | Trusted Oojuro release coordination |

Local agent journals run without a cloud account. `.ojo` is the selected future
standalone database extension; this source still uses the existing directory
storage and does not add a standalone container reader/writer.

## Build and check

Use stable Rust supporting edition 2024, a C compiler and Node.js 24 or newer.

```sh
cargo build --locked --no-default-features -p oojuro --bins
cargo test --locked --no-default-features --lib --tests -p oojuro-capture -p oojuro-proxy -p oojuro -- --test-threads=1
python3 scripts/test-connect-agent.py
python3 scripts/test-connect-project.py
python3 scripts/test-setup-agent.py
node --test sdk/typescript/test/*.test.js
```

Compatibility executables are `oojuro`, `oojurod` and `oojuro-native`. The
[local journal quickstart](docs/LOCAL_JOURNAL_QUICKSTART.md) explains explicit
records; [combined agent setup](docs/COMBINED_AGENT_SETUP.md) explains capture and
hosted project reads. Runtime capability, SDK publication and hosted acceptance
are separate; the SDK packages remain private experimental packages.

Build scripts and tests belong with their component. Signing, publishing,
release promotion and cloud deployment are coordinated in the private operator
repository. Repository creation does not enable publishing or deploy a service.

## License and contributions

Original code is Apache-2.0. Preserve [LICENSE](LICENSE), [NOTICE](NOTICE) and
third-party attribution under [licenses/](licenses/README.md). Use a topic branch,
include focused verification in its pull request, and sign commits with
`git commit -s`. No real agent transcripts or credentials belong in source or tests.
GitHub Actions stays disabled until the owner enables isolated public checks.
