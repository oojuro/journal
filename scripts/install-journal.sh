#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
set -eu
version=${OJO_JOURNAL_VERSION:-0.1.0}
prefix=${OJO_JOURNAL_PREFIX:-"${HOME}/.local"}
base=${OJO_JOURNAL_RELEASE_BASE:-https://github.com/oojuro/o-journal/releases/download}
fail() { printf 'ojo-journal: %s\n' "$1" >&2; exit 1; }
case "$version" in ''|*[!A-Za-z0-9._-]*) fail 'invalid version';; esac
case "$base" in https://*) ;; *) fail 'release downloads require HTTPS';; esac
case "$prefix" in /*) ;; *) fail 'installation prefix must be absolute';; esac
case "$(uname -s):$(uname -m)" in Linux:x86_64|Linux:amd64) target=x86_64-unknown-linux-gnu;; *) fail 'this release supports Linux x86_64; use the Cargo source install on other supported targets';; esac
for tool in curl tar sha256sum awk mktemp find install; do command -v "$tool" >/dev/null 2>&1 || fail "required command unavailable: $tool"; done
tag=ojo-journal-v${version}
asset=${tag}-${target}.tar.gz
temporary=$(mktemp -d "${TMPDIR:-/tmp}/ojo-journal-install.XXXXXXXX")
binary_stage=
notice_stage=
notice_backup=
notices=
cleanup() {
    status=$1
    trap - EXIT HUP INT TERM
    rm -rf "$temporary"
    [ -z "$binary_stage" ] || rm -rf "$binary_stage"
    [ -z "$notice_stage" ] || rm -rf "$notice_stage"
    if [ -n "$notice_backup" ] && [ -d "$notice_backup/original" ]; then
        if [ ! -e "$notices" ]; then mv "$notice_backup/original" "$notices"; fi
        if [ "$status" -eq 0 ]; then rm -rf "$notice_backup"; fi
    fi
    exit "$status"
}
trap 'cleanup $?' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
chmod 700 "$temporary"
curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 "$base/$tag/SHA256SUMS" --output "$temporary/SHA256SUMS"
curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 "$base/$tag/$asset" --output "$temporary/$asset"
expected=$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1 }' "$temporary/SHA256SUMS")
case "$expected" in ''|*[!0-9a-f]*) fail 'missing or invalid archive checksum';; esac
[ ${#expected} -eq 64 ] || fail 'missing or duplicate archive checksum'
actual=$(sha256sum "$temporary/$asset" | awk '{ print $1 }')
[ "$actual" = "$expected" ] || fail 'archive checksum mismatch'
tar -tzf "$temporary/$asset" > "$temporary/members"
awk 'BEGIN { bad=0 } /^\// || /(^|\/)\.\.(\/|$)/ {bad=1} END {exit bad}' "$temporary/members" || fail 'unsafe archive path'
tar -tvzf "$temporary/$asset" | awk 'substr($0,1,1)!="-" && substr($0,1,1)!="d" {bad=1} END {exit bad}' || fail 'archive links are not supported'
mkdir "$temporary/package"
tar -xzf "$temporary/$asset" --no-same-owner --no-same-permissions -C "$temporary/package"
# Validate the entire payload before creating or changing the prefix. A missing
# compatibility binary or notice inventory must preserve every installed binary.
for metadata in LICENSE NOTICE Cargo.lock manifest.json; do
    [ -s "$temporary/package/$metadata" ] && [ -f "$temporary/package/$metadata" ] || fail 'required package metadata missing'
done
[ -d "$temporary/package/licenses" ] || fail 'required dependency notices missing'
notice_file=$(find "$temporary/package/licenses" -type f -print -quit)
[ -n "$notice_file" ] && [ -s "$notice_file" ] || fail 'required dependency notices missing'
for binary in ojo-journal oojuro oojuro-native oojurod; do
    [ -s "$temporary/package/bin/$binary" ] && [ -f "$temporary/package/bin/$binary" ] || fail 'required binary missing'
done
[ "$("$temporary/package/bin/ojo-journal" --version)" = "ojo-journal $version" ] || fail 'binary version does not match release'
for directory in "$prefix" "$prefix/bin" "$prefix/share" "$prefix/share/ojo-journal"; do
    [ ! -L "$directory" ] || fail 'installation destination must not be a symlink'
done
notices=$prefix/share/ojo-journal/$version
if [ -e "$notices" ] || [ -L "$notices" ]; then
    [ ! -L "$notices" ] && [ -d "$notices" ] || fail 'existing notice destination is unsafe'
fi
for binary in ojo-journal oojuro oojuro-native oojurod; do
    [ ! -L "$prefix/bin/$binary" ] || fail 'existing binary destination is a symlink'
done
mkdir -p "$prefix/bin" "$prefix/share/ojo-journal"
binary_stage=$(mktemp -d "$prefix/bin/.ojo-journal-install.XXXXXXXX")
notice_stage=$(mktemp -d "$prefix/share/ojo-journal/.install.XXXXXXXX")
cp "$temporary/package/LICENSE" "$temporary/package/NOTICE" "$temporary/package/Cargo.lock" "$temporary/package/manifest.json" "$notice_stage/"
cp -R "$temporary/package/licenses" "$notice_stage/"
chmod 755 "$notice_stage"
for binary in ojo-journal oojuro oojuro-native oojurod; do
    install -m 755 "$temporary/package/bin/$binary" "$binary_stage/$binary"
done
# Retain any prior inventory until the new complete notice tree is published and
# every executable replacement succeeds. Failure before publication restores it.
if [ -d "$notices" ]; then
    notice_backup=$(mktemp -d "$prefix/share/ojo-journal/.previous.XXXXXXXX")
    mv "$notices" "$notice_backup/original"
fi
mv "$notice_stage" "$notices"
notice_stage=
for binary in ojo-journal oojuro oojuro-native oojurod; do
    mv -f "$binary_stage/$binary" "$prefix/bin/$binary"
done
printf 'Installed ojo-journal %s to %s/bin\n' "$version" "$prefix"
printf 'Run: %s/bin/ojo-journal connect --adapter codex --accept-local-capture\n' "$prefix"
