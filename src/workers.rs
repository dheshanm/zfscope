//! Worker threads. Each owns one kind of I/O and sends results to the UI thread; nothing here blocks the UI.
//!
//! * sampler — kstats + /proc every interval (cheap file reads, never shells out)
//! * iostat  — a long-lived `zpool iostat -Hpvlq -y N`, restarted when the interval changes
//! * slow    — histograms every 5s, config (`zpool status/list/get`, `zfs list`) every 15s, disk inventory
//! * jobs    — on-demand requests (snapshots of a dataset, smartctl), plus a slow background SMART sweep

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, after, select, unbounded};

use crate::collect::{self, Config, DiskInfo, KSample};
use crate::parse::disk::Smart;
use crate::parse::iostat::Hist;
use crate::parse::zjson::Dataset;
use crate::source::{Res, Source};

pub enum Msg {
    Kstats(KSample),
    Iostat(Res<String>, f64),
    Hists(Res<(Vec<Hist>, Vec<Hist>)>, f64),
    Config(Box<Config>),
    Disks(Res<Vec<DiskInfo>>),
    Smart(String, Res<Smart>),
    Snapshots(String, Res<Vec<Dataset>>),
    History(String, Res<Vec<crate::parse::history::Entry>>),
}

pub enum Job {
    Snapshots(String),
    History(String),
    Smart(String),
}

pub const CONFIG_EVERY: Duration = Duration::from_secs(15);
pub const HIST_EVERY: Duration = Duration::from_secs(5);
pub const INVENTORY_EVERY: Duration = Duration::from_secs(300);
pub const SMART_EVERY: Duration = Duration::from_secs(600);

/// Handles for talking to the workers.
pub struct Workers {
    interval_txs: Vec<Sender<Duration>>,
    refresh_tx: Sender<()>,
    job_tx: Sender<Job>,
    smart_list_tx: Sender<Vec<String>>,
}

impl Workers {
    pub fn set_interval(&self, d: Duration) {
        for tx in &self.interval_txs {
            let _ = tx.send(d);
        }
    }
    pub fn refresh(&self) {
        let _ = self.refresh_tx.send(());
    }
    pub fn job(&self, j: Job) {
        let _ = self.job_tx.send(j);
    }
    /// Devices for the background SMART sweep (sent whenever the inventory changes).
    pub fn smart_devices(&self, devs: Vec<String>) {
        let _ = self.smart_list_tx.send(devs);
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new().name(name.into()).spawn(f).expect("spawn worker thread");
}

pub fn start(src: Arc<dyn Source>, interval: Duration, tx: Sender<Msg>, smart: bool) -> Workers {
    let (i1_tx, i1_rx) = unbounded();
    let (i2_tx, i2_rx) = unbounded();
    let (refresh_tx, refresh_rx) = unbounded();
    let (job_tx, job_rx) = unbounded();
    let (smart_list_tx, smart_list_rx) = unbounded();

    {
        let (src, tx) = (src.clone(), tx.clone());
        spawn("sampler", move || sampler(src, interval, tx, i1_rx));
    }
    {
        let (src, tx) = (src.clone(), tx.clone());
        spawn("iostat", move || iostat(src, interval, tx, i2_rx));
    }
    {
        let (src, tx) = (src.clone(), tx.clone());
        spawn("slow", move || slow(src, tx, refresh_rx));
    }
    {
        let (src, tx) = (src.clone(), tx.clone());
        spawn("jobs", move || jobs(src, tx, job_rx, smart_list_rx, smart));
    }
    Workers { interval_txs: vec![i1_tx, i2_tx], refresh_tx, job_tx, smart_list_tx }
}

/// Deadline loop: a sample every `interval`; changing the interval re-arms the deadline without an early sample.
fn sampler(src: Arc<dyn Source>, mut interval: Duration, tx: Sender<Msg>, cmd: Receiver<Duration>) {
    // First sample right away (baseline), the next one interval later.
    let mut next = Instant::now();
    loop {
        let wait = next.saturating_duration_since(Instant::now());
        select! {
            recv(cmd) -> c => match c {
                Ok(d) => { next = next.checked_sub(interval).unwrap_or(next) + d; interval = d; continue; }
                Err(_) => return,
            },
            recv(after(wait)) -> _ => {}
        }
        src.tick();
        if tx.send(Msg::Kstats(collect::kstats(&*src))).is_err() {
            return;
        }
        next += interval;
        if next < Instant::now() {
            next = Instant::now() + interval;
        }
    }
}

fn iostat(src: Arc<dyn Source>, mut interval: Duration, tx: Sender<Msg>, cmd: Receiver<Duration>) {
    let mut backoff = Duration::from_secs(2);
    loop {
        let stop = Arc::new(AtomicBool::new(false));
        let (btx, brx) = unbounded();
        {
            let (src, stop) = (src.clone(), stop.clone());
            spawn("iostat-stream", move || src.iostat_stream(interval, stop, btx));
        }
        let mut restart_after: Option<Duration> = None;
        loop {
            select! {
                recv(cmd) -> c => match c {
                    Ok(d) => { interval = d; break; }
                    Err(_) => { stop.store(true, Ordering::Relaxed); return; }
                },
                recv(brx) -> b => match b {
                    Ok(block) => {
                        let failed = block.is_err();
                        if tx.send(Msg::Iostat(block, src.now())).is_err() {
                            stop.store(true, Ordering::Relaxed);
                            return;
                        }
                        if !failed { backoff = Duration::from_secs(2); }
                    }
                    Err(_) => { restart_after = Some(backoff); break; }
                },
            }
        }
        stop.store(true, Ordering::Relaxed);
        if let Some(d) = restart_after {
            // The stream died (zpool missing, not root, pool exported): retry slowly, but follow interval changes.
            backoff = (backoff * 2).min(Duration::from_secs(60));
            select! {
                recv(cmd) -> c => match c { Ok(n) => interval = n, Err(_) => return },
                recv(after(d)) -> _ => {}
            }
        }
    }
}

fn slow(src: Arc<dyn Source>, tx: Sender<Msg>, refresh: Receiver<()>) {
    let mut next_hist = Instant::now();
    let mut next_cfg = Instant::now();
    let mut next_inv = Instant::now();
    let mut hist_fail = 0u32;
    let mut topo_sig = String::new();
    loop {
        let now = Instant::now();
        if now >= next_cfg {
            let t = Instant::now();
            let cfg = collect::config(&*src);
            // inventory when the topology changes, and every few minutes (paths fail over, disks get replaced)
            let sig: String = match &cfg.status {
                Ok(p) => p
                    .iter()
                    .flat_map(|p| p.root.leaves().into_iter().map(|l| l.path.clone().unwrap_or_default()))
                    .collect(),
                Err(_) => String::new(),
            };
            let inv = (sig != topo_sig || now >= next_inv).then(|| cfg.status.as_ref().ok().cloned()).flatten();
            if tx.send(Msg::Config(Box::new(cfg))).is_err() {
                return;
            }
            if let Some(pools) = inv {
                topo_sig = sig;
                next_inv = now + INVENTORY_EVERY;
                let d = collect::inventory(&*src, &pools);
                if tx.send(Msg::Disks(Ok(d))).is_err() {
                    return;
                }
            }
            // A slow `zpool` (busy pool) shouldn't be hammered: wait at least 4x as long as it took.
            next_cfg = Instant::now() + CONFIG_EVERY.max(t.elapsed() * 4);
        }
        if now >= next_hist {
            let t = Instant::now();
            let h = collect::hists(&*src);
            hist_fail = if h.is_err() { (hist_fail + 1).min(6) } else { 0 };
            if tx.send(Msg::Hists(h, src.now())).is_err() {
                return;
            }
            next_hist = Instant::now() + (HIST_EVERY * (1 << hist_fail)).max(t.elapsed() * 4);
        }
        let wait = next_cfg.min(next_hist).saturating_duration_since(Instant::now());
        select! {
            recv(refresh) -> r => match r {
                Ok(()) => { next_cfg = Instant::now(); next_inv = Instant::now(); }
                Err(_) => return,
            },
            recv(after(wait)) -> _ => {}
        }
    }
}

fn jobs(src: Arc<dyn Source>, tx: Sender<Msg>, jobs: Receiver<Job>, smart_list: Receiver<Vec<String>>, smart: bool) {
    let mut sweep: Vec<String> = Vec::new();
    let mut sweep_pos = 0usize;
    let mut next_sweep = Instant::now();
    let mut smart_ok = smart;
    loop {
        // Background SMART sweep: one device per wake-up so on-demand jobs never wait long.
        let wait = if smart_ok && sweep_pos < sweep.len() {
            Duration::from_millis(200)
        } else {
            next_sweep.saturating_duration_since(Instant::now()).max(Duration::from_millis(200))
        };
        select! {
            recv(jobs) -> j => match j {
                Ok(Job::Snapshots(ds)) => {
                    let r = collect::snapshots(&*src, &ds);
                    if tx.send(Msg::Snapshots(ds, r)).is_err() { return; }
                }
                Ok(Job::History(pool)) => {
                    let r = collect::history(&*src, &pool);
                    if tx.send(Msg::History(pool, r)).is_err() { return; }
                }
                Ok(Job::Smart(dev)) => {
                    let r = collect::smart(&*src, &dev);
                    if tx.send(Msg::Smart(dev, r)).is_err() { return; }
                }
                Err(_) => return,
            },
            recv(smart_list) -> l => match l {
                Ok(l) => { sweep = l; sweep_pos = 0; }
                Err(_) => return,
            },
            recv(after(wait)) -> _ => {
                if !smart_ok { continue; }
                if sweep_pos < sweep.len() {
                    let dev = sweep[sweep_pos].clone();
                    sweep_pos += 1;
                    let r = collect::smart(&*src, &dev);
                    // smartctl missing entirely: stop sweeping (the Disks view says why)
                    if let Err(e) = &r
                        && (e.contains("No such file") || e.contains("not found")) {
                            smart_ok = false;
                        }
                    if tx.send(Msg::Smart(dev, r)).is_err() { return; }
                    if sweep_pos == sweep.len() { next_sweep = Instant::now() + SMART_EVERY; }
                } else if Instant::now() >= next_sweep && !sweep.is_empty() {
                    sweep_pos = 0;
                }
            }
        }
    }
}
