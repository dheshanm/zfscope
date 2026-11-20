# zfscope

`top` for OpenZFS hosts: a live, read-only terminal view of what a ZFS server is doing and how it is set up.

- **Overview**: pool health and capacity per class, IOPS / bandwidth / latency graphs, ARC, ZIL, TXG, busiest datasets
- **Pool**: vdev/disk tree with live IO, latency, queue and space columns; per-device graphs, a latency heatmap,
  request-size histogram, a disk grid to spot the one slow disk, and `zpool history`
- **Datasets**: space breakdown (snapshots, children, quotas), per-dataset IO and ZIL (ztop), snapshots, properties
- **ARC**: size and composition, hit ratios per access class, ghost hits, tunables
- **ZIL·TXG**: sync-write logging per pool and dataset, SLOG vs pool, transaction groups, the write throttle
- **Disks**: every physical disk behind the pool: enclosure slot, multipath paths, SMART health, temperature,
  util/await
- **Props**: every pool and dataset property with its source, possible values and plain-language help, plus help
  for feature flags and module parameters

Graphs are mirrored braille lines (reads up, writes down); press `?` in any view for a guide to what it shows.

## Install

Static Linux binaries (x86_64, aarch64), built from macOS or Linux without a C toolchain:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
scp target/x86_64-unknown-linux-musl/release/zfscope host:/usr/local/sbin/
```

## Usage

Run it on the ZFS host. Root is needed for `zpool` / `smartctl`; the kstat-based views work as any user.

```sh
zfscope                 # 1-second sampling (+/- to change)
zfscope --dump          # print everything it collects once, no TUI (quick check of a new host)
zfscope --no-smart      # don't run smartctl
zfscope --fixture DIR   # replay a capture instead of the live system
```

Keys: `1`–`7` views · `?` help · `/` filter · click a header to sort · `Enter`/`Esc` drill in/out ·
`p` pause (freezes the screen and frees the mouse for copying text) · `q` quit.

## Requirements

Linux with OpenZFS 2.3 or newer (JSON output of `zpool`/`zfs`); developed against 2.4.4.

## Development

```sh
cargo test
scripts/capture-fixture.sh root@host 90          # capture a replayable fixture (names pseudonymised on the host)
cargo run -- --fixture tests/fixtures/local/host  # run against it, on any OS
scripts/deploy-test.sh root@host                  # build and copy to host:/tmp/zfscope-test
```

Captures go to `tests/fixtures/local/` (gitignored): dataset and snapshot names are replaced on the server before
anything is copied, but they are still real host data and are never committed.

Each release build of changed sources gets the next build number (`BUILD`), shown as `0.1.0+bN` in the header.

## License

Unlicense (public domain).
