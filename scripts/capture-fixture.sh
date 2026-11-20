#!/usr/bin/env bash
# Capture a replayable fixture from a ZFS host (read-only; needs root there for smartctl / zpool).
#
#   scripts/capture-fixture.sh root@host [ticks] [outdir]
#
# Dataset and snapshot names are pseudonymised ON THE SERVER (see scripts/capture_remote.py), so real names never
# reach this machine. The default outdir is tests/fixtures/local/<host>, which is gitignored; committed fixtures are
# produced from such a capture by scripts/sanitize-fixture.py.
set -euo pipefail
host=${1:?usage: capture-fixture.sh user@host [ticks] [outdir]}
ticks=${2:-60}
out=${3:-tests/fixtures/local/${host#*@}}
ssh_opts=(${SSH_OPTS:-})
here=$(cd "$(dirname "$0")" && pwd)

rm -rf "$out" && mkdir -p "$out"
ssh "${ssh_opts[@]}" "$host" "TICKS=$ticks SMART=${SMART:-1} python3 -" <"$here/capture_remote.py" | tar -xzf - -C "$out"
echo "captured $(find "$out" -type f | wc -l | tr -d ' ') files into $out"
cat "$out/meta.json"
