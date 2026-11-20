#!/usr/bin/env bash
# shot.sh OUT.png [peek env...]: render one frame with examples/peek.rs and photograph it with headless Chrome.
#   FIXTURE=tests/fixtures/local/<host> KEYS="2" scripts/shot.sh /tmp/pool.png
set -euo pipefail
out=$1
CHROME=${CHROME:-"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"}
cols=${COLS:-160} rows=${ROWS:-48}
html=$(mktemp -t zfscope).html
FORMAT=html COLS=$cols ROWS=$rows cargo run -q --example peek > "$html"
w=$(awk -v c="$cols" 'BEGIN { printf "%d", c * 7.83 + 20 }')
h=$(awk -v r="$rows" 'BEGIN { printf "%d", r * 15.6 + 20 }')
"$CHROME" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=${SCALE:-1} \
  --window-size="$w,$h" --screenshot="$out" "file://$html" >/dev/null 2>&1
rm -f "$html"
echo "$out"
