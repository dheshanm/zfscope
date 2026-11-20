//! Raw samples → current state, rates and history. Pure (no I/O): the app feeds it worker messages.

use std::collections::{HashMap, VecDeque};

use crate::collect::{Config, DiskInfo, KSample};
use crate::parse::disk::{DiskCounters, Smart};
use crate::parse::iostat::{Hist, Row, parse_block};
use crate::parse::kstat::{Kstat, Txg};
use crate::parse::zjson::{Dataset, PoolStatus, Props, PropsExt};
use crate::source::Res;

/// Samples kept per series (10 minutes at 1s).
pub const HISTORY: usize = 600;
/// Latency heatmap columns kept per vdev.
pub const HEAT_HISTORY: usize = 240;
/// Committed TXGs kept per pool.
pub const TXG_HISTORY: usize = 240;

// ------------------------------------------------------------------------------------------------ ring buffers

#[derive(Debug, Clone, Default)]
pub struct Ring {
    v: VecDeque<f64>,
}

impl Ring {
    pub fn push(&mut self, x: f64) {
        if self.v.len() == HISTORY {
            self.v.pop_front();
        }
        self.v.push_back(x);
    }
    pub fn last(&self) -> Option<f64> {
        self.v.back().copied()
    }
    pub fn len(&self) -> usize {
        self.v.len()
    }
    pub fn is_empty(&self) -> bool {
        self.v.is_empty()
    }
    /// The last `n` samples, oldest first.
    pub fn tail(&self, n: usize) -> impl Iterator<Item = f64> + '_ {
        self.v.iter().skip(self.v.len().saturating_sub(n)).copied()
    }
}

/// Named series: `arc.size`, `io:pool0:raidz2-0:rops`, `ds:pool0/a:wbw`, ...
#[derive(Debug, Clone, Default)]
pub struct Series(HashMap<String, Ring>);

impl Series {
    pub fn push(&mut self, key: impl Into<String>, x: f64) {
        self.0.entry(key.into()).or_default().push(x);
    }
    pub fn get(&self, key: &str) -> Option<&Ring> {
        self.0.get(key)
    }
    pub fn last(&self, key: &str) -> f64 {
        self.get(key).and_then(Ring::last).unwrap_or(f64::NAN)
    }
}

/// Counter growth per second; a counter that went backwards (module reload, pool re-import) reads as 0.
pub fn per_sec(prev: u64, cur: u64, dt: f64) -> f64 {
    if dt <= 0.0 { 0.0 } else { cur.checked_sub(prev).map(|d| d as f64 / dt).unwrap_or(0.0) }
}

pub fn ratio(hits: f64, misses: f64) -> f64 {
    let t = hits + misses;
    if t <= 0.0 { f64::NAN } else { 100.0 * hits / t }
}

pub fn vkey(pool: &str, name: &str) -> String {
    format!("{pool}:{name}")
}

// ------------------------------------------------------------------------------------------------ per-dataset IO

/// Per-second rates for one dataset, from its objset kstat.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DsIo {
    pub rops: f64,
    pub wops: f64,
    pub rbw: f64,
    pub wbw: f64,
    pub unlinks: f64,
    pub zil_commits: f64,
    pub zil_writers: f64,
    pub zil_itx: f64,
    pub zil_indirect_b: f64,
    pub zil_copied_b: f64,
    pub zil_needcopy_b: f64,
    pub zil_normal_b: f64,
    pub zil_slog_b: f64,
}

impl DsIo {
    fn from(prev: &Kstat, cur: &Kstat, dt: f64) -> Self {
        let r = |k: &str| per_sec(prev.n(k), cur.n(k), dt);
        DsIo {
            rops: r("reads"),
            wops: r("writes"),
            rbw: r("nread"),
            wbw: r("nwritten"),
            unlinks: r("nunlinked"),
            zil_commits: r("zil_commit_count"),
            zil_writers: r("zil_commit_writer_count"),
            zil_itx: r("zil_itx_count"),
            zil_indirect_b: r("zil_itx_indirect_bytes"),
            zil_copied_b: r("zil_itx_copied_bytes"),
            zil_needcopy_b: r("zil_itx_needcopy_bytes"),
            zil_normal_b: r("zil_itx_metaslab_normal_bytes"),
            zil_slog_b: r("zil_itx_metaslab_slog_bytes"),
        }
    }
    pub fn ops(&self) -> f64 {
        self.rops + self.wops
    }
    pub fn bw(&self) -> f64 {
        self.rbw + self.wbw
    }
    pub fn zil_bytes(&self) -> f64 {
        self.zil_indirect_b + self.zil_copied_b + self.zil_needcopy_b
    }
}

// ------------------------------------------------------------------------------------------------ disks

/// iostat -x style rates for one block device.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DiskRate {
    pub rops: f64,
    pub wops: f64,
    pub rbw: f64,
    pub wbw: f64,
    /// average ms per completed read / write
    pub r_await: f64,
    pub w_await: f64,
    /// % of time with I/O in flight
    pub util: f64,
    /// average queue length
    pub aqu: f64,
}

impl DiskRate {
    fn from(p: &DiskCounters, c: &DiskCounters, dt: f64) -> Self {
        let d = |a: u64, b: u64| b.saturating_sub(a) as f64;
        let (dr, dw) = (d(p.reads, c.reads), d(p.writes, c.writes));
        DiskRate {
            rops: dr / dt,
            wops: dw / dt,
            rbw: d(p.read_sectors, c.read_sectors) * 512.0 / dt,
            wbw: d(p.write_sectors, c.write_sectors) * 512.0 / dt,
            r_await: if dr > 0.0 { d(p.read_ms, c.read_ms) / dr } else { 0.0 },
            w_await: if dw > 0.0 { d(p.write_ms, c.write_ms) / dw } else { 0.0 },
            util: (d(p.io_ms, c.io_ms) / (dt * 10.0)).min(100.0),
            aqu: d(p.weighted_ms, c.weighted_ms) / (dt * 1000.0),
        }
    }
}

// ------------------------------------------------------------------------------------------------ histograms

/// One heatmap column: per-interval counts per latency bucket.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HeatCol {
    pub read: Vec<u32>,
    pub write: Vec<u32>,
    pub disk_read: Vec<u32>,
    pub disk_write: Vec<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct HistState {
    pub t: f64,
    /// cumulative, keyed by vkey
    pub lat: HashMap<String, Hist>,
    pub size: HashMap<String, Hist>,
    /// counts over the last poll interval
    pub lat_delta: HashMap<String, Hist>,
    pub size_delta: HashMap<String, Hist>,
    pub heat: HashMap<String, VecDeque<HeatCol>>,
}

fn hist_delta(prev: &Hist, cur: &Hist) -> Hist {
    let counts = cur
        .counts
        .iter()
        .enumerate()
        .map(|(i, row)| {
            row.iter()
                .enumerate()
                .map(|(j, &c)| c.saturating_sub(prev.counts.get(i).and_then(|r| r.get(j)).copied().unwrap_or(c)))
                .collect()
        })
        .collect();
    Hist { name: cur.name.clone(), buckets: cur.buckets.clone(), counts }
}

/// Assign rows (iostat block / histogram list) to pools: a row named after a known pool starts that pool's section.
fn section<'a, T>(items: &'a [T], name: impl Fn(&T) -> &str, pools: &[String]) -> Vec<(String, &'a T)> {
    let mut cur = String::new();
    let mut out = Vec::new();
    for it in items {
        let n = name(it);
        if n.is_empty() {
            continue;
        }
        if pools.iter().any(|p| p == n) || cur.is_empty() {
            cur = n.to_string();
        }
        out.push((vkey(&cur, n), it));
    }
    out
}

// ------------------------------------------------------------------------------------------------ the model

#[derive(Debug, Default)]
pub struct Model {
    pub series: Series,
    pub ks: Option<KSample>,
    pub ks_prev: Option<KSample>,
    pub dt: f64,
    /// pool names known from kstats / config, in order
    pub pool_names: Vec<String>,

    pub iostat: HashMap<String, Row>,
    pub iostat_err: Option<String>,
    pub iostat_t: f64,
    pub iostat_seen: bool,

    pub hist: HistState,
    pub hist_err: Option<String>,

    pub config: Option<Config>,
    pub config_errors: Vec<String>,

    /// committed TXGs per pool, oldest first
    pub txgs: HashMap<String, VecDeque<Txg>>,
    /// per-dataset IO rates for the last tick, by dataset name
    pub ds_io: HashMap<String, DsIo>,
    pub disk_rates: HashMap<String, DiskRate>,

    pub disks: Vec<DiskInfo>,
    pub disks_err: Option<String>,
    pub smart: HashMap<String, Res<Smart>>,
    pub snapshots: HashMap<String, Res<Vec<Dataset>>>,
    pub history: HashMap<String, Res<Vec<crate::parse::history::Entry>>>,
    /// dataset name -> its props from `zfs list -o all` (fast lookup)
    pub ds_index: HashMap<String, usize>,
    pub samples: u64,
}

impl Model {
    pub fn new() -> Self {
        Self::default()
    }

    // ---------------------------------------------------------------- kstats

    /// Apply one kstat sample. With `record == false` (paused) only the baseline moves, so rates are right on resume.
    pub fn on_kstats(&mut self, s: KSample, record: bool) {
        for p in &s.pools {
            if !self.pool_names.contains(&p.name) {
                self.pool_names.push(p.name.clone());
                self.pool_names.sort();
            }
        }
        let prev = self.ks.take();
        self.ks_prev = prev;
        self.dt = self.ks_prev.as_ref().map(|p| s.t - p.t).unwrap_or(0.0);
        self.ks = Some(s);
        if !record || self.dt <= 0.0 {
            return;
        }
        self.samples += 1;
        self.record_arc();
        self.record_zil();
        self.record_pools();
        self.record_datasets();
        self.record_disks();
    }

    /// Rate of a counter in a global kstat (`arcstats`, `zil`, `dmu_tx`, ...).
    pub fn rate(&self, kstat: &str, field: &str) -> f64 {
        match (&self.ks_prev, &self.ks) {
            (Some(p), Some(c)) => match (p.k(kstat), c.k(kstat)) {
                (Some(a), Some(b)) => per_sec(a.n(field), b.n(field), self.dt),
                _ => 0.0,
            },
            _ => 0.0,
        }
    }

    /// Current value of a global kstat field.
    pub fn val(&self, kstat: &str, field: &str) -> u64 {
        self.ks.as_ref().and_then(|s| s.k(kstat)).map(|k| k.n(field)).unwrap_or(0)
    }

    pub fn kstat(&self, kstat: &str) -> Option<&Kstat> {
        self.ks.as_ref().and_then(|s| s.k(kstat))
    }

    pub fn has_kstats(&self) -> bool {
        self.kstat("arcstats").is_some()
    }

    fn record_arc(&mut self) {
        let r = |f: &str| self.rate("arcstats", f);
        let dd = (r("demand_data_hits") + r("demand_data_iohits"), r("demand_data_misses"));
        let dm = (r("demand_metadata_hits") + r("demand_metadata_iohits"), r("demand_metadata_misses"));
        let pd = (r("prefetch_data_hits") + r("prefetch_data_iohits"), r("prefetch_data_misses"));
        let pm = (r("prefetch_metadata_hits") + r("prefetch_metadata_iohits"), r("prefetch_metadata_misses"));
        let hits = r("hits") + r("iohits");
        let misses = r("misses");
        let v = |f: &str| self.val("arcstats", f) as f64;
        let pushes = [
            ("arc.size", v("size")),
            ("arc.c", v("c")),
            ("arc.mru", v("mru_size")),
            ("arc.mfu", v("mfu_size")),
            ("arc.data", v("data_size")),
            ("arc.meta", v("metadata_size")),
            ("arc.avail", v("memory_available_bytes")),
            ("arc.reads", hits + misses),
            ("arc.misses", misses),
            ("arc.hit", ratio(hits, misses)),
            ("arc.dd_hit", ratio(dd.0, dd.1)),
            ("arc.dm_hit", ratio(dm.0, dm.1)),
            ("arc.pd_hit", ratio(pd.0, pd.1)),
            ("arc.pm_hit", ratio(pm.0, pm.1)),
            ("arc.pf_hit", ratio(pd.0 + pm.0, pd.1 + pm.1)),
            ("arc.demand_hit", ratio(dd.0 + dm.0, dd.1 + dm.1)),
            ("arc.ghost", r("mru_ghost_hits") + r("mfu_ghost_hits")),
            ("arc.evict", r("evict_l2_eligible") + r("evict_l2_ineligible")),
            ("arc.l2_hit", ratio(r("l2_hits"), r("l2_misses"))),
            ("arc.l2_read", r("l2_read_bytes")),
            ("arc.l2_write", r("l2_write_bytes")),
        ];
        for (k, x) in pushes {
            self.series.push(k, x);
        }
        let z = |f: &str| self.rate("zfetchstats", f);
        let zf = ratio(z("hits"), z("misses"));
        let d = |f: &str| self.rate("dbufstats", f);
        let db = ratio(d("hash_hits"), d("hash_misses"));
        self.series.push("zfetch.hit", zf);
        self.series.push("dbuf.hit", db);
    }

    fn record_zil(&mut self) {
        let r = |f: &str| self.rate("zil", f);
        let pushes = [
            ("zil.commits", r("zil_commit_count")),
            ("zil.writers", r("zil_commit_writer_count")),
            ("zil.itx", r("zil_itx_count")),
            ("zil.bytes", r("zil_itx_indirect_bytes") + r("zil_itx_copied_bytes") + r("zil_itx_needcopy_bytes")),
            ("zil.slog", r("zil_itx_metaslab_slog_bytes")),
            ("zil.normal", r("zil_itx_metaslab_normal_bytes")),
            ("zil.stalls", r("zil_commit_stall_count")),
        ];
        for (k, x) in pushes {
            self.series.push(k, x);
        }
        let t = |f: &str| self.rate("dmu_tx", f);
        let tx = [
            ("tx.delay", t("dmu_tx_dirty_delay")),
            ("tx.over_max", t("dmu_tx_dirty_over_max")),
            ("tx.throttle", t("dmu_tx_dirty_throttle") + t("dmu_tx_memory_reclaim")),
            ("tx.assigned", t("dmu_tx_assigned")),
        ];
        for (k, x) in tx {
            self.series.push(k, x);
        }
    }

    fn record_pools(&mut self) {
        let Some(s) = &self.ks else { return };
        for p in &s.pools {
            let hist = self.txgs.entry(p.name.clone()).or_default();
            let last = hist.back().map(|t| t.txg).unwrap_or(0);
            let mut new: Vec<Txg> = p.txgs.iter().filter(|t| t.state == 'C' && t.txg > last).copied().collect();
            new.sort_by_key(|t| t.txg);
            for t in new {
                if hist.len() == TXG_HISTORY {
                    hist.pop_front();
                }
                hist.push_back(t);
            }
            // dirty data in the open txg right now
            let open_dirty = p.txgs.iter().filter(|t| t.state != 'C').map(|t| t.ndirty).sum::<u64>();
            self.series.push(format!("txg:{}:dirty", p.name), open_dirty as f64);
        }
    }

    fn record_datasets(&mut self) {
        let (Some(prev), Some(cur)) = (&self.ks_prev, &self.ks) else { return };
        let mut out = HashMap::new();
        for p in &cur.pools {
            let Some(pp) = prev.pools.iter().find(|x| x.name == p.name) else { continue };
            let before: HashMap<&str, &Kstat> = pp.objsets.iter().map(|(id, k)| (id.as_str(), k)).collect();
            for (id, k) in &p.objsets {
                let Some(name) = k.s("dataset_name") else { continue };
                let Some(b) = before.get(id.as_str()) else { continue };
                out.insert(name.to_string(), DsIo::from(b, k, self.dt));
            }
        }
        for (name, io) in &out {
            self.series.push(format!("ds:{name}:r"), io.rbw);
            self.series.push(format!("ds:{name}:w"), io.wbw);
            self.series.push(format!("ds:{name}:ro"), io.rops);
            self.series.push(format!("ds:{name}:wo"), io.wops);
        }
        self.ds_io = out;
    }

    fn record_disks(&mut self) {
        let (Some(prev), Some(cur)) = (&self.ks_prev, &self.ks) else { return };
        let (Ok(p), Ok(c)) = (&prev.diskstats, &cur.diskstats) else { return };
        let mut want: Vec<&str> = Vec::new();
        for d in &self.disks {
            want.push(&d.whole);
            want.extend(d.legs.iter().map(|l| l.dev.as_str()));
        }
        let mut rates = HashMap::new();
        for dev in want {
            if let (Some(a), Some(b)) = (p.get(dev), c.get(dev)) {
                let r = DiskRate::from(a, b, self.dt);
                rates.insert(dev.to_string(), r);
            }
        }
        for (dev, r) in &rates {
            self.series.push(format!("dsk:{dev}:util"), r.util);
            self.series.push(
                format!("dsk:{dev}:await"),
                (r.r_await * r.rops + r.w_await * r.wops) / (r.rops + r.wops).max(1e-9),
            );
        }
        self.disk_rates = rates;
    }

    /// Per-interval deltas of the pool's `dmu_tx_assign` histogram (time to assign a tx to a txg), (bucket ns, count).
    pub fn tx_assign_delta(&self, pool: &str) -> Vec<(String, u64)> {
        let get = |s: &Option<KSample>| {
            s.as_ref().and_then(|s| s.pools.iter().find(|p| p.name == pool)).and_then(|p| p.tx_assign.clone())
        };
        match (get(&self.ks_prev), get(&self.ks)) {
            (Some(a), Some(b)) => {
                b.nums.iter().map(|(k, v)| (k.clone(), v.saturating_sub(a.get(k).unwrap_or(*v)))).collect()
            }
            (None, Some(b)) => b.nums.clone(),
            _ => Vec::new(),
        }
    }

    pub fn pool_kstats(&self, pool: &str) -> Option<&crate::collect::PoolKstats> {
        self.ks.as_ref().and_then(|s| s.pools.iter().find(|p| p.name == pool))
    }

    // ---------------------------------------------------------------- iostat

    pub fn on_iostat(&mut self, block: Res<String>, t: f64, record: bool) {
        let text = match block {
            Ok(t) => t,
            Err(e) => {
                self.iostat_err = Some(e);
                return;
            }
        };
        self.iostat_err = None;
        let rows = parse_block(&text);
        if rows.is_empty() {
            return;
        }
        self.iostat_seen = true;
        self.iostat_t = t;
        let pools = self.known_pools();
        let mut map = HashMap::new();
        for (k, r) in section(&rows, |r| r.name.as_str(), &pools) {
            map.insert(k, r.clone());
        }
        if record {
            for (k, r) in &map {
                let w = |i: usize| r.wait[i].unwrap_or(f64::NAN);
                use crate::parse::iostat::w::*;
                let pushes = [
                    ("rops", r.rops),
                    ("wops", r.wops),
                    ("rbw", r.rbw),
                    ("wbw", r.wbw),
                    ("lr", w(TOTAL_R)),
                    ("lw", w(TOTAL_W)),
                    ("dr", w(DISK_R)),
                    ("dw", w(DISK_W)),
                    ("act", r.active() as f64),
                    ("pend", r.pending() as f64),
                ];
                for (s, x) in pushes {
                    self.series.push(format!("io:{k}:{s}"), x);
                }
            }
        }
        self.iostat = map;
    }

    fn known_pools(&self) -> Vec<String> {
        let mut v = self.pool_names.clone();
        if let Some(Ok(st)) = self.config.as_ref().map(|c| &c.status) {
            for p in st {
                if !v.contains(&p.name) {
                    v.push(p.name.clone());
                }
            }
        }
        v
    }

    pub fn io(&self, pool: &str, name: &str) -> Option<&Row> {
        self.iostat.get(&vkey(pool, name))
    }

    // ---------------------------------------------------------------- histograms

    pub fn on_hists(&mut self, h: Res<(Vec<Hist>, Vec<Hist>)>, t: f64, record: bool) {
        let (lat, size) = match h {
            Ok(x) => x,
            Err(e) => {
                self.hist_err = Some(e);
                return;
            }
        };
        self.hist_err = None;
        let pools = self.known_pools();
        let lat: HashMap<String, Hist> =
            section(&lat, |h| h.name.as_str(), &pools).into_iter().map(|(k, h)| (k, h.clone())).collect();
        let size: HashMap<String, Hist> =
            section(&size, |h| h.name.as_str(), &pools).into_iter().map(|(k, h)| (k, h.clone())).collect();
        let hs = &mut self.hist;
        let had_prev = !hs.lat.is_empty();
        if had_prev {
            hs.lat_delta =
                lat.iter().filter_map(|(k, c)| hs.lat.get(k).map(|p| (k.clone(), hist_delta(p, c)))).collect();
            hs.size_delta =
                size.iter().filter_map(|(k, c)| hs.size.get(k).map(|p| (k.clone(), hist_delta(p, c)))).collect();
            if record {
                for (k, d) in &hs.lat_delta {
                    let col = |c: usize| {
                        d.counts.iter().map(|r| r.get(c).copied().unwrap_or(0).min(u32::MAX as u64) as u32).collect()
                    };
                    let q = hs.heat.entry(k.clone()).or_default();
                    if q.len() == HEAT_HISTORY {
                        q.pop_front();
                    }
                    q.push_back(HeatCol { read: col(0), write: col(1), disk_read: col(2), disk_write: col(3) });
                }
            }
        }
        hs.lat = lat;
        hs.size = size;
        hs.t = t;
    }

    // ---------------------------------------------------------------- config

    pub fn on_config(&mut self, c: Config) {
        let mut errs = Vec::new();
        for (what, e) in [
            ("zpool status", c.status.as_ref().err()),
            ("zpool list", c.list.as_ref().err()),
            ("zpool get", c.pool_props.as_ref().err()),
            ("zfs list", c.datasets.as_ref().err()),
        ] {
            if let Some(e) = e {
                errs.push(if e.starts_with(what) { e.clone() } else { format!("{what}: {e}") });
            }
        }
        self.config_errors = errs;
        // Keep the last good pieces when a refresh fails partway.
        let mut c = c;
        if let Some(old) = self.config.take() {
            if c.status.is_err() && old.status.is_ok() {
                c.status = old.status;
            }
            if c.list.is_err() && old.list.is_ok() {
                c.list = old.list;
            }
            if c.pool_props.is_err() && old.pool_props.is_ok() {
                c.pool_props = old.pool_props;
            }
            if c.datasets.is_err() && old.datasets.is_ok() {
                c.datasets = old.datasets;
            }
            if c.params.is_empty() {
                c.params = old.params;
            }
        }
        // Merge zpool list -v (frag/cap per vdev, pool size) into the status topology.
        if let (Ok(st), Ok(list)) = (&mut c.status, &c.list) {
            for p in st.iter_mut() {
                let Some(l) = list.iter().find(|l| l.name == p.name) else { continue };
                fn merge(v: &mut crate::parse::zjson::Vdev, l: &crate::parse::zjson::PoolList) {
                    if let Some((_, _, props)) = l.vdevs.iter().find(|(_, n, _)| *n == v.name) {
                        v.frag = props.opt("fragmentation");
                        v.cap = props.opt("capacity");
                        if v.size == 0 {
                            v.size = props.n("size");
                        }
                    }
                    for c in &mut v.children {
                        merge(c, l);
                    }
                }
                merge(&mut p.root, l);
                p.root.frag = l.props.opt("fragmentation");
                p.root.cap = l.props.opt("capacity");
            }
            for p in st.iter() {
                if !self.pool_names.contains(&p.name) {
                    self.pool_names.push(p.name.clone());
                }
            }
            self.pool_names.sort();
        }
        if let Ok(ds) = &mut c.datasets {
            ds.sort_by(|a, b| a.name.cmp(&b.name));
            self.ds_index = ds.iter().enumerate().map(|(i, d)| (d.name.clone(), i)).collect();
        }
        self.config = Some(c);
    }

    pub fn pools(&self) -> &[PoolStatus] {
        match self.config.as_ref().map(|c| &c.status) {
            Some(Ok(v)) => v,
            _ => &[],
        }
    }

    pub fn pool(&self, name: &str) -> Option<&PoolStatus> {
        self.pools().iter().find(|p| p.name == name)
    }

    pub fn pool_list(&self, name: &str) -> Option<&crate::parse::zjson::PoolList> {
        match self.config.as_ref().map(|c| &c.list) {
            Some(Ok(v)) => v.iter().find(|p| p.name == name),
            _ => None,
        }
    }

    pub fn pool_props(&self, name: &str) -> Option<&Props> {
        match self.config.as_ref().map(|c| &c.pool_props) {
            Some(Ok(v)) => v.iter().find(|(n, _)| n == name).map(|(_, p)| p),
            _ => None,
        }
    }

    pub fn datasets(&self) -> &[Dataset] {
        match self.config.as_ref().map(|c| &c.datasets) {
            Some(Ok(v)) => v,
            _ => &[],
        }
    }

    pub fn dataset(&self, name: &str) -> Option<&Dataset> {
        self.ds_index.get(name).and_then(|&i| self.datasets().get(i))
    }

    pub fn param(&self, name: &str) -> Option<&str> {
        self.config.as_ref()?.params.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// The boot setting of a tunable and the modprobe.d file it is in (the last one modprobe reads wins).
    pub fn boot_param(&self, name: &str) -> Option<(String, &str)> {
        let c = self.config.as_ref()?;
        c.modprobe.iter().rev().find_map(|(f, t)| crate::write::modprobe_get(t, name).map(|v| (v, f.as_str())))
    }

    /// Every modprobe.d file that sets this tunable.
    pub fn boot_param_files(&self, name: &str) -> Vec<&str> {
        let Some(c) = self.config.as_ref() else { return Vec::new() };
        c.modprobe
            .iter()
            .filter(|(_, t)| crate::write::modprobe_get(t, name).is_some())
            .map(|(f, _)| f.as_str())
            .collect()
    }

    /// Content of a modprobe.d file as last read ("" for one that doesn't exist or sets no ZFS options).
    pub fn modprobe_text(&self, file: &str) -> &str {
        self.config.as_ref().and_then(|c| c.modprobe.iter().find(|(f, _)| f == file)).map_or("", |(_, t)| t)
    }

    /// `zfs.<name>=` on the kernel command line.
    pub fn cmdline_param(&self, name: &str) -> Option<String> {
        crate::write::cmdline_param(&self.config.as_ref()?.cmdline, name)
    }

    /// Root filesystem on ZFS: the module loads from the initramfs.
    pub fn root_on_zfs(&self) -> bool {
        self.config.as_ref().and_then(|c| c.root_fs.as_deref()) == Some("zfs")
    }

    /// The tunable is read-only while the module is loaded.
    pub fn param_load_only(&self, name: &str) -> bool {
        self.config.as_ref().is_some_and(|c| c.param_ro.iter().any(|p| p == name))
    }

    /// The value a tunable has when nothing sets it, as a number to write (automatic defaults: 0 where 0 means
    /// automatic; zfs_dirty_data_max: worked out from RAM like the module does).
    pub fn param_default(&self, name: &str) -> Option<String> {
        let t = crate::help::tunable(name)?;
        if !t.default.is_empty() {
            return Some(t.default.to_string());
        }
        if name == "zfs_dirty_data_max" {
            let ram = self.val("arcstats", "memory_all_bytes");
            let pct: u64 = self.param("zfs_dirty_data_max_percent").and_then(|v| v.parse().ok()).unwrap_or(10);
            let cap: u64 = self.param("zfs_dirty_data_max_max").and_then(|v| v.parse().ok()).unwrap_or(u64::MAX);
            return (ram > 0).then(|| (ram / 100 * pct).min(cap).to_string());
        }
        None
    }

    // ---------------------------------------------------------------- disks

    pub fn on_disks(&mut self, d: Res<Vec<DiskInfo>>) {
        match d {
            Ok(v) => {
                self.disks = v;
                self.disks_err = None;
            }
            Err(e) => self.disks_err = Some(e),
        }
    }

    pub fn on_smart(&mut self, dev: String, s: Res<Smart>) {
        self.smart.insert(dev, s);
    }

    pub fn disk_for_vdev(&self, pool: &str, vdev: &str) -> Option<&DiskInfo> {
        self.disks.iter().find(|d| d.pool == pool && d.vdev == vdev)
    }

    pub fn on_history(&mut self, pool: String, h: Res<Vec<crate::parse::history::Entry>>) {
        self.history.insert(pool, h);
    }

    pub fn on_snapshots(&mut self, ds: String, s: Res<Vec<Dataset>>) {
        self.snapshots.insert(ds, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_caps() {
        let mut r = Ring::default();
        for i in 0..(HISTORY + 5) {
            r.push(i as f64);
        }
        assert_eq!(r.len(), HISTORY);
        assert_eq!(r.tail(2).collect::<Vec<_>>(), vec![(HISTORY + 3) as f64, (HISTORY + 4) as f64]);
    }

    #[test]
    fn counter_reset_is_zero() {
        assert_eq!(per_sec(100, 50, 1.0), 0.0);
        assert_eq!(per_sec(100, 150, 2.0), 25.0);
    }

    #[test]
    fn sectioning() {
        let names = ["p1", "raidz2-0", "d1", "p2", "raidz2-0", "d9"];
        let s = section(&names, |n| n, &["p1".into(), "p2".into()]);
        let keys: Vec<_> = s.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["p1:p1", "p1:raidz2-0", "p1:d1", "p2:p2", "p2:raidz2-0", "p2:d9"]);
    }

    #[test]
    fn hist_deltas() {
        let a = Hist { name: "x".into(), buckets: vec![1, 3], counts: vec![vec![1, 2], vec![3, 4]] };
        let b = Hist { name: "x".into(), buckets: vec![1, 3], counts: vec![vec![2, 2], vec![9, 4]] };
        assert_eq!(hist_delta(&a, &b).counts, vec![vec![1, 0], vec![6, 0]]);
    }

    #[test]
    fn disk_rate() {
        let a = DiskCounters { reads: 100, read_sectors: 0, read_ms: 0, io_ms: 0, ..Default::default() };
        let b = DiskCounters { reads: 200, read_sectors: 2048, read_ms: 500, io_ms: 500, ..Default::default() };
        let r = DiskRate::from(&a, &b, 1.0);
        assert_eq!((r.rops, r.rbw, r.r_await, r.util), (100.0, 1048576.0, 5.0, 50.0));
    }
}
