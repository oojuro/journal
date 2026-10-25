# ojo-journal

Record agent sessions into portable `.juro` journals, find a past decision and open
its exact recorded context. Use it locally without an account, with optional
scoped delivery to [ojo-cloud](https://oojuro.com).

**Experimental alpha source.** The primary `ojo-journal` executable uses the
standalone `.juro` engine. Existing `oojuro`, `oojurod` and `oojuro-native` commands
and directory stores remain compatible. The `.juro` wire format remains
experimental; this application does not declare it stable. New filenames use
`.juro`; existing `.ojo` files remain readable and writable at their original
paths. If both names exist for one identity, Journal refuses the ambiguity.

```sh
cargo build --locked -p oojuro --bin ojo-journal
target/debug/ojo-journal connect --adapter codex --accept-local-capture
# Use --adapter claude-code for Claude Code.
target/debug/ojo-journal sessions
target/debug/ojo-journal search --query "deployment decision"
```

Connect installs a per-user Linux service, excludes existing transcript history
by default, and verifies an owned synthetic transcript through the actual parser
and durable `.juro` path. It never adds a test line to your agent's transcript.
The service keeps healthy session handles and supplies capture, CLI and local MCP
reads through a private Unix socket. Connect registers the selected agent's
read-only MCP server and verifies its synthetic original; restart the agent to
load newly registered tools. Use `--no-mcp` to skip registration. Use `--no-service`
and `ojo-journal serve` for a foreground owner (`watch --adapter codex` remains
an alias). `upgrade-service` updates the managed copy after installing a new CLI.
Capture redacts recognized secrets and never reads referenced tool-result files.
New provider sessions get separate journals; resuming a session keeps its journal.

For optional cloud delivery, pass a protected integration key file to connect:

```sh
ojo-journal connect --adapter codex --api-key-file /private/integration-key \
  --accept-local-capture
```

Keys stay out of command arguments and chat. Local recording continues while the
network is unavailable. `status` distinguishes durable local records, confirmed
cloud records and pending intents. Existing local history is excluded when first
connecting cloud unless you explicitly choose `--include-existing`. Cloud setup
success requires acknowledgement of the synthetic verification event.

The versioned curl installer is `scripts/install-journal.sh`; the operator publishes
its checked Linux x86_64 binaries and notices under the `ojo-journal-v0.1.0` release.
Use the [release page](https://github.com/oojuro/o-journal/releases) to establish
availability. Source installation works independently of binary publication:

```sh
cargo install --locked --path crates/oojuro --bin ojo-journal
```

See the [alpha commands, migration and qualification](docs/OJO_JOURNAL_ALPHA.md).

## Repository boundaries

| Repository | Responsibility |
| --- | --- |
| [oojuro](https://github.com/oojuro/oojuro) | Embedded storage, compression, durability and retrieval; the independently versioned `.juro` database format |
| [o-journal](https://github.com/oojuro/o-journal) | Agent-specific recording, integrations and history tools |
| [o-cloud](https://github.com/oojuro/o-cloud) | Managed hosting, sharing, accounts, billing and the `oojuro.com` website |
| [o-release-operator](https://github.com/oojuro/o-release-operator) | Trusted Oojuro release coordination |

The primary application stores one encrypted `.juro` file per provider session.
Explicit migration preserves existing directory stores and creates a complete
encrypted recovery snapshot before converting their records. Compatibility
commands keep their original formats; no store is silently upgraded.

Oojuro is a general embedded database. Its generic event contract is an engine
acceptance timestamp plus exact opaque data; agent, session, provider and tool
fields belong to Journal's application data. Engine full-text indexes and query
execution provide generic retrieval; Journal chooses capture, session filters,
context expansion and access policy. This alpha's session files use the existing
wire 0.1 key/value API. They do not already implement the additive generic Event
profile; adopting that profile requires an explicit, verified application
migration with exact originals and retry mappings preserved.

## Build and check

Use stable Rust supporting edition 2024, a C compiler and Node.js 24 or newer.

```sh
cargo build --locked --no-default-features -p oojuro --bins
cargo test --locked --no-default-features --lib --tests -p oojuro-capture -p oojuro-proxy -p oojuro -- --test-threads=1
python3 scripts/test-connect-agent.py
python3 scripts/test-connect-project.py
python3 scripts/test-setup-agent.py
python3 scripts/test-journal-namespace.py --binary target/debug/ojo-journal
python3 scripts/test-journal-durability.py --binary target/debug/ojo-journal
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
