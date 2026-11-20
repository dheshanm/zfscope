#!/usr/bin/env bash
# Build a static x86_64 binary and copy it to test hosts as /tmp/zfscope-test (never over an installed zfscope).
#   scripts/deploy-test.sh root@host1 [root@host2 ...]
# SSH_OPTS="-i ~/.ssh/key" for a key. Prints the build number each host now runs.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q --release --target x86_64-unknown-linux-musl
bin=target/x86_64-unknown-linux-musl/release/zfscope
ssh_opts=(${SSH_OPTS:-})
for h in "$@"; do
  # upload beside it and rename: replaces the file even while an old copy is running ("text file busy")
  scp -q "${ssh_opts[@]}" "$bin" "$h:/tmp/.zfscope-test.new"
  ssh "${ssh_opts[@]}" "$h" 'chmod 755 /tmp/.zfscope-test.new && mv -f /tmp/.zfscope-test.new /tmp/zfscope-test'
  printf '%s: ' "$h"
  ssh "${ssh_opts[@]}" "$h" /tmp/zfscope-test --version 2>/dev/null | tail -1
done
