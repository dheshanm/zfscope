# Contributing

Issues and pull requests are welcome. zfscope reads ZFS through kstats, `/proc`, sysfs and the `zpool` / `zfs`
commands, and everything it reads can be captured into a fixture and replayed. So most of it can be developed and
tested on any OS (macOS included) without a ZFS host.

Captures from hosts with unusual layouts are especially useful: dRAID, special / dedup / log / cache vdevs, many
enclosures, multipath, NVMe-only pools, other OpenZFS versions. See [Testing against a real server](#testing-against-a-real-server).

## Build and test

```sh
cargo test                                        # parsers, model, collectors on fixtures, insta UI snapshots
cargo clippy --all-targets
cargo run -- --fixture tests/fixtures/demo        # the TUI on the bundled demo capture (works on macOS)
cargo run -- --fixture tests/fixtures/demo --dump # what the collectors produce, as text
```

With `--fixture`, write mode is a dry run (commands and file edits are shown, never executed), so the write path
can be exercised safely too.

`tests/render.rs` snapshots every view. UI changes show up as insta snapshot diffs: review them with
`cargo insta review` (or accept the `.snap.new` files once you have checked them). `tests/fixtures.rs` runs every
collector against the demo fixture and against any captures you have under `tests/fixtures/local/`.

## Looking at a frame without a terminal

```sh
FIXTURE=tests/fixtures/demo cargo run --example peek                  # print one rendered frame
FIXTURE=tests/fixtures/demo KEYS="2 Enter" cargo run --example peek   # …after pressing keys
FIXTURE=tests/fixtures/demo KEYS="2" scripts/shot.sh /tmp/pool.png    # one frame as a PNG (headless Chrome)
scripts/screenshots.sh                                                # regenerate docs/*.png and docs/demo.gif
```

`peek` also takes `TICKS` (default 60), `COLS`, `ROWS`, `NAME` and `FORMAT=html`. `KEYS` is a space-separated
list: single characters, `Enter Esc Backspace Tab Up Down Left Right PgDn PgUp Space`, or `click:X,Y`.
`screenshots.sh` needs Chrome or Chromium (set `CHROME=` if it is not found) and `gifski`. It only ever uses the
committed demo fixture, never real server data.

## Testing against a real server

`tests/fixtures/demo` is a sanitised capture of a real 60-disk server. To test with your own data, capture into the
gitignored `tests/fixtures/local/`:

```sh
scripts/capture-fixture.sh root@SERVER 90        # 90 one-second ticks -> tests/fixtures/local/SERVER
cargo test local_captures_parse                  # collector sanity checks on the capture
cargo run -- --fixture tests/fixtures/local/SERVER
```

The capture is read-only. It needs root on the server (for `zpool` and `smartctl`) and `python3`, nothing else.
`SMART=0` skips smartctl and `SSH_OPTS="-i key"` passes options to ssh. `scripts/capture_remote.py` runs on the
server and replaces every dataset path component and snapshot name with a pseudonym (`ds01`, `snap01`, ...)
**before anything is written**. Mountpoints are mapped the same way, and keylocation, comments and user-property
values are redacted. Host names, serials, WWNs and GUIDs are kept, which is why the directory stays out of git.

To turn a capture into a fixture that can be committed (for example to add a test for a layout the demo doesn't
have):

```sh
scripts/sanitize-fixture.py tests/fixtures/local/SERVER tests/fixtures/NAME --frames 40
```

This replaces host names, serials, WWNs / SAS addresses, GUIDs and IPv4 addresses consistently across every file,
and fails if anything original remains. Look through the output before committing it anyway.

## Deploying a test build

The server needs no Rust toolchain. `.cargo/config.toml` links the musl targets with Rust's bundled `rust-lld`, so
cross-compiling needs no extra linker or Docker:

```sh
rustup target add x86_64-unknown-linux-musl   # or aarch64-unknown-linux-musl
scripts/deploy-test.sh root@SERVER [root@SERVER2 ...]
```

This builds a static x86_64 binary and installs it as `/tmp/zfscope-test` on each host (never over an installed
`zfscope`), then prints the version each host now runs.

Local builds carry a build number so test binaries can be told apart: `zfscope 0.2.5+b31`. Every release build of
changed sources bumps the number in `BUILD` (commit it with your change). Debug builds of sources newer than the last
release build show `+bN-dev`. Builds with `ZFSCOPE_RELEASE=1`, as in CI, show just the version.

## Releasing

`.github/workflows/release.yml` runs when a `v*` tag is pushed. It:

1. checks that the tag matches the `version` in `Cargo.toml` (tag `v0.2.5` needs `version = "0.2.5"`)
2. runs `cargo test --locked`
3. builds static `x86_64` and `aarch64` musl binaries with `ZFSCOPE_RELEASE=1`
4. smoke-tests the x86_64 binary (`--version`, and `--dump` against the demo fixture)
5. writes `SHA256SUMS` and publishes a GitHub release with notes generated from the commits since the last tag

The release files are `zfscope-x86_64-unknown-linux-musl`, `zfscope-aarch64-unknown-linux-musl` and `SHA256SUMS`.
The install command in the README downloads them through `releases/latest`, so it picks up a new release with no
README change.

### Steps

```sh
# 1. bump the version in Cargo.toml (here to 0.2.5), then refresh Cargo.lock: CI builds with --locked
cargo build
cargo test && cargo clippy --all-targets

# 2. commit everything that belongs to the release, including Cargo.lock (and BUILD if it changed)
git commit -am "release v0.2.5"

# 3. tag that commit and push both; the tag push starts the release workflow
git tag v0.2.5
git push origin master v0.2.5

# 4. follow the run; the release appears under Releases when it finishes
gh run watch "$(gh run list --workflow release.yml --limit 1 --json databaseId --jq '.[0].databaseId')"
gh release view v0.2.5
```

Release notes can be edited afterwards with `gh release edit v0.2.5 --notes-file NOTES.md` (or on GitHub).

### Dry run

To check a release build without publishing anything, run the workflow by hand: "Run workflow" on the release
action in GitHub, or

```sh
gh workflow run release.yml --ref master
```

That runs the same tests, builds and smoke test, skips the version check and the publish step, and attaches the
binaries and `SHA256SUMS` to the run as the `zfscope-binaries` artifact (`gh run download <run-id>`).

### If the release run fails

Publishing is the last step, so a failed run publishes nothing. Fix the problem, then move the tag to the fixed
commit and push it again:

```sh
git tag -d v0.2.5 && git push origin :refs/tags/v0.2.5    # delete the tag locally and on GitHub
# fix, commit, push master
git tag v0.2.5 && git push origin v0.2.5
```

Common causes:

- **`tag vX != Cargo.toml vY`**: the version wasn't bumped, or was bumped in a commit other than the tagged one.
- **`cargo test --locked` fails on the lock file**: `Cargo.lock` wasn't refreshed or committed after the version
  bump. Run `cargo build` and commit `Cargo.lock`.
- **Smoke test fails**: it expects `--dump` on the demo fixture to print `64 leaf vdevs resolved`. If you replace
  `tests/fixtures/demo` with a capture of a different shape, update that line in the workflow.

Don't move a tag whose release was already published: people may have downloaded it. Release the fix as the next
patch version instead.

## Code layout

| Path | What |
|---|---|
| `src/parse/` | pure parsers: kstats (`kstat.rs`), `zpool`/`zfs` JSON (`zjson.rs`), `zpool iostat` text (`iostat.rs`), `zpool history` (`history.rs`), diskstats / smartctl / vdev_id.conf (`disk.rs`) |
| `src/source.rs` | where raw data comes from: the live system or a fixture directory |
| `src/collect.rs` | gathering raw samples from a `Source` |
| `src/workers.rs` | worker threads (`sampler`, `iostat`, `slow` for config and SMART, `jobs`, `writer`); nothing blocks the UI |
| `src/model/` | raw samples → current state, rates and history (no I/O) |
| `src/app.rs` | app state and key / mouse handling (no I/O) |
| `src/write.rs` | write mode: the only changes zfscope can make, as fixed command shapes and modprobe.d edits |
| `src/help.rs` | built-in help text for views, properties, feature flags and tunables |
| `src/ui/` | ratatui views, one file per tab, plus shared widgets and theme |
| `src/replay.rs` | synchronous fixture driver for tests and `peek` |
| `src/dump.rs` | `--dump` text output |
| `scripts/` | capture, sanitise, deploy and screenshot helpers |
| `build.rs`, `BUILD` | build numbers for local builds |
