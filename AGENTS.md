# Working on Oojuro Journal

Read README.md and PROVENANCE.md before editing. Agent integrations belong here;
embedded storage and `.juro` format changes belong in oojuro/oojuro. Keep engine
dependencies pinned to immutable commits. Local use must not require a cloud
account. Preserve existing command/config compatibility during migration.

Use topic branches and signed commits. Run the relevant synthetic Rust, setup
and SDK checks from README.md. Keep private service code, credentials and real
agent history out of this public repository. Signing, publishing and deployment
are separate operator actions; a source change does not authorize a release.

New database filenames use `.juro`. Preserve explicit legacy `.ojo` lookup without
automatic renames; reject dual-candidate ambiguity. Do not change header magic,
wire/profile versions or frozen vectors for a filename decision.
