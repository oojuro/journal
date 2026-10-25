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
for tool in curl tar sha256sum awk mktemp; do command -v "$tool" >/dev/null 2>&1 || fail "required command unavailable: $tool"; done
tag=ojo-journal-v${version}
asset=${tag}-${target}.tar.gz
temporary=$(mktemp -d "${TMPDIR:-/tmp}/ojo-journal-install.XXXXXXXX")
trap 'rm -rf "$temporary"' EXIT HUP INT TERM
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
[ -f "$temporary/package/LICENSE" ] && [ -f "$temporary/package/NOTICE" ] && [ -d "$temporary/package/licenses" ] || fail 'required license notices missing'
[ "$("$temporary/package/bin/ojo-journal" --version)" = "ojo-journal $version" ] || fail 'binary version does not match release'
[ ! -L "$prefix" ] && [ ! -L "$prefix/bin" ] || fail 'installation destination must not be a symlink'
mkdir -p "$prefix/bin" "$prefix/share/ojo-journal"
for binary in ojo-journal oojuro oojuro-native oojurod; do
    [ -f "$temporary/package/bin/$binary" ] || fail 'required binary missing'
    [ ! -L "$prefix/bin/$binary" ] || fail 'existing binary destination is a symlink'
    install -m 755 "$temporary/package/bin/$binary" "$prefix/bin/.$binary.install-$$"
    mv -f "$prefix/bin/.$binary.install-$$" "$prefix/bin/$binary"
done
notices=$prefix/share/ojo-journal/$version
if [ -e "$notices" ]; then
    [ ! -L "$notices" ] && [ -d "$notices" ] || fail 'existing notice destination is unsafe'
else
    mkdir "$notices"
fi
cp "$temporary/package/LICENSE" "$temporary/package/NOTICE" "$temporary/package/Cargo.lock" "$temporary/package/manifest.json" "$notices/"
cp -R "$temporary/package/licenses" "$notices/"
printf 'Installed ojo-journal %s to %s/bin\n' "$version" "$prefix"
printf 'Run: %s/bin/ojo-journal connect --adapter codex --accept-local-capture\n' "$prefix"
