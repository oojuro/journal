#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
[[ $# == 1 ]] || { printf 'Usage: scripts/build_journal.sh /fresh/package\n' >&2; exit 2; }
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
[[ -z "$(git status --porcelain)" ]] || { printf 'Commit reviewed source before building.\n' >&2; exit 1; }
target=x86_64-unknown-linux-gnu
cargo fetch --locked
cargo build --release --locked -p oojuro --bin ojo-journal --bin oojuro --bin oojuro-native --bin oojurod --target "$target"
python3 scripts/package_native.py "${CARGO_TARGET_DIR:-target}/$target/release" "$1" --target "$target" --journal
