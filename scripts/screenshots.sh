#!/usr/bin/env bash
# Regenerate the README screenshots (docs/*.png) and the demo GIF (docs/demo.gif) from the committed demo fixture —
# never from real server data.
#
#   scripts/screenshots.sh
#
# Frames come from examples/peek.rs (FORMAT=html, braille drawn as dots) photographed by headless Chrome; gifski
# (brew install gifski) assembles the GIF. Set CHROME= if Chrome isn't in the default location.
set -euo pipefail
cd "$(dirname "$0")/.."

CHROME=${CHROME:-}
for c in "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" google-chrome chromium chromium-browser; do
  [ -n "$CHROME" ] && break
  if [ -x "$c" ] || command -v "$c" >/dev/null 2>&1; then CHROME=$c; fi
done
[ -n "$CHROME" ] || { echo "no Chrome/Chromium found; set CHROME=" >&2; exit 1; }

cargo build -q --example peek
PEEK=target/debug/examples/peek
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p docs
COLS=160 ROWS=46

# frame OUT.png TICKS KEYS: one rendered frame after TICKS fixture samples, then KEYS.
frame() {
  FIXTURE=tests/fixtures/demo NAME=zfs-demo TICKS=$2 COLS=$COLS ROWS=$ROWS KEYS=$3 FORMAT=html "$PEEK" > "$TMP/frame.html"
  local w h
  w=$(awk -v c="$COLS" 'BEGIN { printf "%d", c * 7.83 + 20 }')
  h=$(awk -v r="$ROWS" 'BEGIN { printf "%d", r * 15.6 + 20 }')
  "$CHROME" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 \
    --window-size="$w,$h" --screenshot="$1" "file://$TMP/frame.html" >/dev/null 2>&1
}

# shot NAME KEYS: a still in docs/NAME.png, with full graph history.
shot() {
  frame "$PWD/docs/$1.png" 300 "$2"
  echo "docs/$1.png"
}

shot overview "1"
shot pool "2 j"
shot disk-grid "2 d"
shot datasets "3 j j Enter ]"
shot arc "4"
shot zil-txg "5"
shot disks "6 j Enter"
shot props "7 j Enter / c o m p r e s s i o n Enter Enter"

if ! command -v gifski >/dev/null 2>&1; then
  echo "gifski not found (brew install gifski): skipping docs/demo.gif" >&2
  exit 0
fi
# The GIF: a tour, two frames a second, one fixture sample per frame so the graphs keep moving.
mkdir "$TMP/frames"
n=0
scene() { # FRAMES KEYS
  local i
  for ((i = 0; i < $1; i++)); do
    frame "$TMP/frames/$(printf %03d $n).png" $((240 + n)) "$2"
    n=$((n + 1))
  done
}
scene 8 "1"                       # overview: pool health, IOPS/bandwidth/latency, ARC, ZIL, TXG
scene 6 "2 j"                     # topology with live per-disk IO, heatmap and request sizes
scene 5 "2 m click:62,2"          # latency columns, sorted: slowest vdev and disk first
scene 5 "2 d"                     # disk grid: one tile per disk
scene 6 "3 j j Enter ]"           # datasets: space and per-dataset IO / ZIL
scene 4 "4"                       # ARC
scene 4 "5"                       # ZIL and TXGs
scene 5 "6 j Enter"               # disks: slot, multipath, SMART
scene 6 "7 j Enter / c o m p r e s s i o n Enter Enter"   # properties with help
gifski --quiet --fps 2 --width 1100 --quality 80 -o docs/demo.gif "$TMP"/frames/*.png
echo "docs/demo.gif ($n frames)"
