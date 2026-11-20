//! Where raw data comes from: the live system, or a captured fixture directory (see `scripts/capture-fixture.sh`).
//!
//! Everything above this layer only sees file contents and command output, so the whole app (parsers, model, UI)
//! runs unchanged against a fixture on any OS.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, after, select};

pub type Res<T> = Result<T, String>;

/// Root of the ZFS kstats.
pub const KSTAT: &str = "/proc/spl/kstat/zfs";

/// A raw data source. Cheap to clone; clones share the fixture clock.
pub trait Source: Send + Sync {
    /// Read a file by absolute path (kstat, /proc, /sys, /etc).
    fn read(&self, path: &str) -> Res<String>;
    /// Names in a directory (sorted).
    fn list(&self, path: &str) -> Res<Vec<String>>;
    /// Raw target of a symlink.
    fn read_link(&self, path: &str) -> Res<String>;
    /// Run a command; stdout on success.
    fn run(&self, prog: &str, args: &[&str], timeout: Duration) -> Res<String>;
    /// Like `run`, but a non-zero exit with output still counts (smartctl's exit code is a bitmask of findings).
    fn run_lenient(&self, prog: &str, args: &[&str], timeout: Duration) -> Res<String> {
        self.run(prog, args, timeout)
    }
    /// Stream `zpool iostat -Hpvlq -y <interval>`: one message per interval block, until `stop` is set.
    fn iostat_stream(&self, interval: Duration, stop: Arc<AtomicBool>, tx: Sender<Res<String>>);
    /// Run a command that changes the system (write mode only: `zfs set` / `zpool set` / `zfs inherit`).
    fn write(&self, prog: &str, _args: &[&str], _timeout: Duration) -> Res<String> {
        Err(format!("{prog}: read-only source"))
    }
    /// Seconds on the source's clock (monotonic; fixture time advances one capture interval per tick).
    fn now(&self) -> f64;
    /// Advance to the next sample (fixtures step a frame; live is a no-op).
    fn tick(&self) {}
    fn describe(&self) -> String;
    fn is_fixture(&self) -> bool {
        false
    }
}

pub fn exists(src: &dyn Source, path: &str) -> bool {
    src.read(path).is_ok()
}

// ------------------------------------------------------------------------------------------------ live

pub struct LiveSource {
    t0: Instant,
}

impl LiveSource {
    pub fn new() -> Self {
        Self { t0: Instant::now() }
    }
}

impl Default for LiveSource {
    fn default() -> Self {
        Self::new()
    }
}

/// Run a command with a timeout; kill it if it overruns (a suspended pool can hang `zpool` forever).
pub fn run_cmd(prog: &str, args: &[&str], timeout: Duration) -> Res<String> {
    run_cmd_inner(prog, args, timeout, false)
}

fn run_cmd_inner(prog: &str, args: &[&str], timeout: Duration, lenient: bool) -> Res<String> {
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("LC_ALL", "C")
        .spawn()
        .map_err(|e| format!("{prog}: {e}"))?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stdout.read_to_end(&mut b);
        b
    });
    let err_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stderr.read_to_end(&mut b);
        b
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{prog} {}: timed out after {:.0?}", args.join(" "), timeout));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(format!("{prog}: {e}")),
        }
    };
    let out = String::from_utf8_lossy(&out_t.join().unwrap_or_default()).into_owned();
    let err = String::from_utf8_lossy(&err_t.join().unwrap_or_default()).into_owned();
    if status.success() || (lenient && !out.trim().is_empty()) {
        Ok(out)
    } else {
        let msg = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        Err(format!(
            "{prog} {}: {}",
            args.first().copied().unwrap_or(""),
            if msg.is_empty() { status.to_string() } else { msg }
        ))
    }
}

impl Source for LiveSource {
    fn read(&self, path: &str) -> Res<String> {
        std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
    }

    fn list(&self, path: &str) -> Res<Vec<String>> {
        let mut v: Vec<String> = std::fs::read_dir(path)
            .map_err(|e| format!("{path}: {e}"))?
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        v.sort();
        Ok(v)
    }

    fn read_link(&self, path: &str) -> Res<String> {
        std::fs::read_link(path).map(|p| p.to_string_lossy().into_owned()).map_err(|e| format!("{path}: {e}"))
    }

    fn run(&self, prog: &str, args: &[&str], timeout: Duration) -> Res<String> {
        run_cmd(prog, args, timeout)
    }

    fn run_lenient(&self, prog: &str, args: &[&str], timeout: Duration) -> Res<String> {
        run_cmd_inner(prog, args, timeout, true)
    }

    fn write(&self, prog: &str, args: &[&str], timeout: Duration) -> Res<String> {
        run_cmd(prog, args, timeout)
    }

    fn iostat_stream(&self, interval: Duration, stop: Arc<AtomicBool>, tx: Sender<Res<String>>) {
        let secs = interval.as_secs().max(1).to_string();
        let child = Command::new("zpool")
            .args(["iostat", "-Hpvlq", "-y", &secs])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("LC_ALL", "C")
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Err(format!("zpool iostat: {e}")));
                return;
            }
        };
        let stdout = child.stdout.take().expect("piped");
        let (ltx, lrx) = crossbeam_channel::unbounded::<String>();
        std::thread::Builder::new()
            .name("iostat-read".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if ltx.send(line).is_err() {
                        break;
                    }
                }
            })
            .ok();
        // A block is complete when the first row's name comes round again, or when output pauses (zpool writes
        // each interval in one burst), whichever is first.
        let mut block = String::new();
        let mut first: Option<String> = None;
        let flush = |block: &mut String| {
            if !block.is_empty() {
                let _ = tx.send(Ok(std::mem::take(block)));
            }
        };
        while !stop.load(Ordering::Relaxed) {
            select! {
                recv(lrx) -> l => match l {
                    Ok(line) => {
                        let name = line.split('\t').next().unwrap_or("").to_string();
                        if first.is_none() && !name.is_empty() { first = Some(name.clone()); }
                        if first.as_deref() == Some(name.as_str()) { flush(&mut block); }
                        block.push_str(&line);
                        block.push('\n');
                    }
                    Err(_) => {
                        flush(&mut block);
                        let mut err = String::new();
                        if let Some(mut e) = child.stderr.take() { let _ = e.read_to_string(&mut err); }
                        let _ = tx.send(Err(format!("zpool iostat exited: {}", err.trim())));
                        break;
                    }
                },
                recv(after(Duration::from_millis(150))) -> _ => flush(&mut block),
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    fn describe(&self) -> String {
        "live".into()
    }
}

// ------------------------------------------------------------------------------------------------ fixture

/// Replays a capture directory. Per-tick files (`frames/NNN/...`) loop forever; counters that grew monotonically
/// over the capture keep growing on later loops, so rates stay continuous instead of re-baselining every loop.
#[derive(Clone)]
pub struct FixtureSource {
    dir: PathBuf,
    frames: usize,
    interval: f64,
    hist_every: usize,
    clock: Arc<AtomicUsize>,
    /// per-path numeric-token deltas over one loop (None = not a counter)
    #[allow(clippy::type_complexity)]
    deltas: Arc<Mutex<HashMap<String, Arc<Vec<Option<u64>>>>>>,
}

impl FixtureSource {
    pub fn new(dir: &Path) -> Res<Self> {
        let meta: serde_json::Value = std::fs::read_to_string(dir.join("meta.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let frames = std::fs::read_dir(dir.join("frames"))
            .map_err(|e| format!("{}: {e}", dir.join("frames").display()))?
            .filter(|e| e.as_ref().is_ok_and(|e| e.path().is_dir()))
            .count();
        if frames == 0 {
            return Err(format!("{}: no frames", dir.display()));
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            frames,
            interval: meta["interval"].as_f64().unwrap_or(1.0),
            hist_every: meta["hist_every"].as_u64().unwrap_or(5).max(1) as usize,
            clock: Arc::new(AtomicUsize::new(0)),
            deltas: Default::default(),
        })
    }

    pub fn set_tick(&self, t: usize) {
        self.clock.store(t, Ordering::Relaxed);
    }

    fn t(&self) -> usize {
        self.clock.load(Ordering::Relaxed)
    }

    fn frame_path(&self, f: usize, path: &str) -> PathBuf {
        self.dir.join(format!("frames/{f:03}")).join(path.trim_start_matches('/'))
    }

    fn is_framed(path: &str) -> bool {
        path.starts_with("/proc/")
    }

    fn read_frame(&self, path: &str) -> Res<String> {
        let t = self.t();
        let (lap, f) = (t / self.frames, t % self.frames);
        let text = std::fs::read_to_string(self.frame_path(f, path)).map_err(|e| format!("{path}: {e}"))?;
        if lap == 0 {
            return Ok(text);
        }
        let deltas = self.deltas_for(path, |i| std::fs::read_to_string(self.frame_path(i, path)).ok());
        Ok(extrapolate(&text, &deltas, lap as u64))
    }

    fn deltas_for(&self, key: &str, load: impl Fn(usize) -> Option<String>) -> Arc<Vec<Option<u64>>> {
        if let Some(d) = self.deltas.lock().expect("lock").get(key) {
            return d.clone();
        }
        let series: Vec<Vec<Option<u64>>> = (0..self.frames).filter_map(&load).map(|s| numeric_tokens(&s)).collect();
        let d = Arc::new(counter_deltas(&series));
        self.deltas.lock().expect("lock").insert(key.to_string(), d.clone());
        d
    }

    /// Histogram snapshot for the current tick (captured every `hist_every` ticks), extrapolated like frames.
    fn hist(&self, kind: char) -> Res<String> {
        let n_hist = self.frames.div_ceil(self.hist_every);
        let t = self.t() / self.hist_every;
        let (lap, i) = (t / n_hist, t % n_hist);
        let path = |i: usize| self.dir.join(format!("hist/{:03}-{kind}.txt", i * self.hist_every));
        let text = std::fs::read_to_string(path(i)).map_err(|e| format!("hist: {e}"))?;
        if lap == 0 {
            return Ok(text);
        }
        let key = format!("hist-{kind}");
        let deltas = {
            if let Some(d) = self.deltas.lock().expect("lock").get(&key) {
                d.clone()
            } else {
                let series: Vec<Vec<Option<u64>>> = (0..n_hist)
                    .filter_map(|i| std::fs::read_to_string(path(i)).ok())
                    .map(|s| numeric_tokens(&s))
                    .collect();
                let d = Arc::new(counter_deltas(&series));
                self.deltas.lock().expect("lock").insert(key, d.clone());
                d
            }
        };
        Ok(extrapolate(&text, &deltas, lap as u64))
    }

    fn cmd_file(&self, name: &str) -> Res<String> {
        std::fs::read_to_string(self.dir.join("cmd").join(name)).map_err(|e| format!("fixture cmd/{name}: {e}"))
    }

    /// `zfs list` emulation over the captured `-t all -o all` document: honour `-t` and a trailing dataset arg
    /// (`-d 1` for its snapshots), so the app can ask for the same things it asks a live system.
    fn zfs_list(&self, args: &[&str]) -> Res<String> {
        let mut doc: serde_json::Value =
            serde_json::from_str(&self.cmd_file("zfs-list.json")?).map_err(|e| format!("zfs-list.json: {e}"))?;
        let types: Vec<String> = args
            .windows(2)
            .find(|w| w[0] == "-t")
            .map(|w| w[1].split(',').map(|s| s.to_uppercase()).collect())
            .unwrap_or_else(|| vec!["FILESYSTEM".into(), "VOLUME".into()]);
        let target = args.last().filter(|a| !a.starts_with('-') && a.contains(|c: char| c.is_alphanumeric()));
        let target = target.filter(|_| {
            // the last arg might be the value of -o / -t / -s
            let i = args.len() - 1;
            i == 0 || !matches!(args[i - 1], "-o" | "-t" | "-s" | "-S" | "-d")
        });
        if let Some(ds) = doc.get_mut("datasets").and_then(|d| d.as_object_mut()) {
            ds.retain(|name, v| {
                let ty = v["type"].as_str().unwrap_or("");
                let ok_type = types.iter().any(|t| t == "ALL" || t == ty);
                let ok_target = match target {
                    None => true,
                    Some(t) if ty == "SNAPSHOT" => name.split('@').next() == Some(t),
                    Some(t) => name == t || name.starts_with(&format!("{t}/")),
                };
                ok_type && ok_target
            });
        }
        Ok(doc.to_string())
    }
}

/// All numeric tokens of a text in order (non-numeric tokens are None placeholders, keeping positions aligned).
fn numeric_tokens(s: &str) -> Vec<Option<u64>> {
    s.split(|c: char| c.is_whitespace()).filter(|t| !t.is_empty()).map(|t| t.parse().ok()).collect()
}

/// For each token position: the per-loop growth if it never decreased across the capture.
fn counter_deltas(series: &[Vec<Option<u64>>]) -> Vec<Option<u64>> {
    let Some(first) = series.first() else { return Vec::new() };
    let last = series.last().expect("non-empty");
    (0..first.len())
        .map(|i| {
            let mut prev = first[i]?;
            for s in series {
                let v = (*s.get(i)?)?;
                if v < prev {
                    return None;
                }
                prev = v;
            }
            let d = last.get(i).copied().flatten()?.checked_sub(first[i]?)?;
            // Include one capture interval's worth so the last→first wrap continues the trend.
            (d > 0).then(|| d + d / (series.len().max(2) as u64 - 1))
        })
        .collect()
}

/// Rewrite numeric tokens: `v + lap * delta` where the token is a counter. Whitespace is preserved.
fn extrapolate(text: &str, deltas: &[Option<u64>], lap: u64) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    let mut idx = 0;
    let mut rest = text;
    while !rest.is_empty() {
        let ws = rest.len() - rest.trim_start().len();
        out.push_str(&rest[..ws]);
        rest = &rest[ws..];
        if rest.is_empty() {
            break;
        }
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let tok = &rest[..end];
        match (tok.parse::<u64>(), deltas.get(idx).copied().flatten()) {
            (Ok(v), Some(d)) => out.push_str(&(v + lap * d).to_string()),
            _ => out.push_str(tok),
        }
        idx += 1;
        rest = &rest[end..];
    }
    out
}

impl Source for FixtureSource {
    fn read(&self, path: &str) -> Res<String> {
        if Self::is_framed(path) {
            self.read_frame(path)
        } else {
            let p = self.dir.join("root").join(path.trim_start_matches('/'));
            std::fs::read_to_string(&p).map_err(|e| format!("{path}: {e}"))
        }
    }

    fn list(&self, path: &str) -> Res<Vec<String>> {
        let p = if Self::is_framed(path) {
            self.frame_path(self.t() % self.frames, path)
        } else {
            self.dir.join("root").join(path.trim_start_matches('/'))
        };
        let mut v: Vec<String> = std::fs::read_dir(&p)
            .map_err(|e| format!("{path}: {e}"))?
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        v.sort();
        Ok(v)
    }

    fn read_link(&self, path: &str) -> Res<String> {
        let p = self.dir.join("root").join(path.trim_start_matches('/'));
        std::fs::read_link(&p).map(|p| p.to_string_lossy().into_owned()).map_err(|e| format!("{path}: {e}"))
    }

    fn run(&self, prog: &str, args: &[&str], _timeout: Duration) -> Res<String> {
        let has = |a: &str| args.contains(&a);
        match (prog, args.first().copied()) {
            ("zpool", Some("status")) => self.cmd_file("zpool-status.json"),
            ("zpool", Some("list")) => self.cmd_file("zpool-list.json"),
            ("zpool", Some("get")) => self.cmd_file("zpool-get.json"),
            ("zpool", Some("history")) => {
                self.cmd_file(&format!("zpool-history-{}.txt", args.last().copied().unwrap_or("")))
            }
            ("zpool", Some("iostat")) if has("-Hpvw") => self.hist('w'),
            ("zpool", Some("iostat")) if has("-Hpvr") => self.hist('r'),
            ("zfs", Some("list")) => self.zfs_list(args),
            ("zfs", Some("version")) => self.cmd_file("zfs-version.json"),
            ("smartctl", _) => {
                let dev = args.last().and_then(|d| d.strip_prefix("/dev/")).unwrap_or("");
                std::fs::read_to_string(self.dir.join("smart").join(format!("{dev}.json")))
                    .map_err(|_| format!("smartctl: no capture for {dev}"))
            }
            _ => Err(format!("fixture: no capture for `{prog} {}`", args.join(" "))),
        }
    }

    /// Fixtures never change: a write is a dry run, so write mode can be tried (and tested) anywhere.
    fn write(&self, _prog: &str, _args: &[&str], _timeout: Duration) -> Res<String> {
        Ok("dry run (fixture): not executed".into())
    }

    fn iostat_stream(&self, interval: Duration, stop: Arc<AtomicBool>, tx: Sender<Res<String>>) {
        let text = match std::fs::read_to_string(self.dir.join("iostat.txt")) {
            Ok(t) => t,
            Err(e) => {
                let _ = tx.send(Err(format!("fixture iostat.txt: {e}")));
                return;
            }
        };
        let blocks = crate::parse::iostat::split_stream(&text);
        if blocks.is_empty() {
            let _ = tx.send(Err("fixture iostat.txt: empty".into()));
            return;
        }
        // Follow the shared clock, so iostat rows line up with the kstat frame of the same tick.
        let mut last = usize::MAX;
        while !stop.load(Ordering::Relaxed) {
            let t = self.t();
            if t != last {
                last = t;
                if tx.send(Ok(blocks[t % blocks.len()].clone())).is_err() {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(20).min(interval));
        }
    }

    fn now(&self) -> f64 {
        self.t() as f64 * self.interval
    }

    fn tick(&self) {
        self.clock.fetch_add(1, Ordering::Relaxed);
    }

    fn describe(&self) -> String {
        format!("fixture {}", self.dir.display())
    }

    fn is_fixture(&self) -> bool {
        true
    }
}

#[derive(Clone, Debug)]
pub enum SourceSpec {
    Live,
    Fixture(PathBuf),
}

impl SourceSpec {
    pub fn build(&self) -> Res<Arc<dyn Source>> {
        Ok(match self {
            SourceSpec::Live => Arc::new(LiveSource::new()),
            SourceSpec::Fixture(d) => Arc::new(FixtureSource::new(d)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_extrapolation() {
        let frames = ["a 1 x 10", "a 2 x 5", "a 4 x 7"];
        let series: Vec<_> = frames.iter().map(|s| numeric_tokens(s)).collect();
        let d = counter_deltas(&series);
        assert_eq!(d, vec![None, Some(4), None, None]); // 3 growth + 1 interval's worth; the gauge is left alone
        assert_eq!(extrapolate("a 1 x 10\n", &d, 2), "a 9 x 10\n");
    }

    #[test]
    fn run_cmd_timeout_and_errors() {
        assert!(run_cmd("sleep", &["5"], Duration::from_millis(100)).unwrap_err().contains("timed out"));
        assert_eq!(run_cmd("echo", &["hi"], Duration::from_secs(5)).unwrap(), "hi\n");
        assert!(run_cmd("false", &[], Duration::from_secs(5)).is_err());
        assert!(run_cmd("/nonexistent-zfscope", &[], Duration::from_secs(1)).is_err());
    }
}
