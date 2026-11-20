//! Gathering raw samples from a `Source`. Runs on worker threads; returns plain data for the model.

use std::collections::HashMap;
use std::time::Duration;

use crate::parse::disk::{DiskCounters, Smart, enclosure_slot, parse_diskstats, parse_smart, parse_vdev_id};
use crate::parse::iostat::{Hist, parse_hists};
use crate::parse::kstat::{Kstat, Txg, parse_meminfo, parse_named, parse_txgs};
use crate::parse::zjson::{self, Dataset, PoolList, PoolStatus, Props};
use crate::source::{KSTAT, Res, Source};

pub const CMD_TIMEOUT: Duration = Duration::from_secs(20);

/// Global (not per-pool) kstats we read every tick.
const GLOBAL: [&str; 7] = ["arcstats", "zil", "dmu_tx", "zfetchstats", "dbufstats", "abdstats", "dnodestats"];

#[derive(Debug, Clone, Default)]
pub struct PoolKstats {
    pub name: String,
    pub state: String,
    pub iostats: Option<Kstat>,
    pub txgs: Vec<Txg>,
    pub tx_assign: Option<Kstat>,
    /// objset id (hex, from the file name) -> kstat (has `dataset_name`)
    pub objsets: Vec<(String, Kstat)>,
}

/// One tick of fast-moving data: kstats + /proc.
#[derive(Debug, Clone)]
pub struct KSample {
    pub t: f64,
    pub global: HashMap<String, Res<Kstat>>,
    pub pools: Vec<PoolKstats>,
    pub diskstats: Res<HashMap<String, DiskCounters>>,
    pub meminfo: HashMap<String, u64>,
}

impl KSample {
    pub fn k(&self, name: &str) -> Option<&Kstat> {
        self.global.get(name).and_then(|r| r.as_ref().ok())
    }
}

pub fn kstats(src: &dyn Source) -> KSample {
    let t = src.now();
    let mut global = HashMap::new();
    for g in GLOBAL {
        global.insert(g.to_string(), src.read(&format!("{KSTAT}/{g}")).and_then(|s| parse_named(&s)));
    }
    let mut pools = Vec::new();
    if let Ok(entries) = src.list(KSTAT) {
        for e in entries {
            let dir = format!("{KSTAT}/{e}");
            // Pool directories are the ones with a `state` file.
            let Ok(state) = src.read(&format!("{dir}/state")) else { continue };
            let mut p = PoolKstats { name: e.clone(), state: state.trim().to_string(), ..Default::default() };
            p.iostats = src.read(&format!("{dir}/iostats")).ok().and_then(|s| parse_named(&s).ok());
            p.txgs = src.read(&format!("{dir}/txgs")).ok().and_then(|s| parse_txgs(&s).ok()).unwrap_or_default();
            p.tx_assign = src.read(&format!("{dir}/dmu_tx_assign")).ok().and_then(|s| parse_named(&s).ok());
            for f in src.list(&dir).unwrap_or_default() {
                if let Some(id) = f.strip_prefix("objset-")
                    && let Ok(k) = src.read(&format!("{dir}/{f}")).and_then(|s| parse_named(&s))
                {
                    p.objsets.push((id.to_string(), k));
                }
            }
            pools.push(p);
        }
    }
    let diskstats = src.read("/proc/diskstats").map(|s| parse_diskstats(&s));
    let meminfo = src.read("/proc/meminfo").map(|s| parse_meminfo(&s)).unwrap_or_default();
    KSample { t, global, pools, diskstats, meminfo }
}

/// Slow-moving configuration: topology, space, properties.
#[derive(Debug, Clone)]
pub struct Config {
    pub t: f64,
    pub status: Res<Vec<PoolStatus>>,
    pub list: Res<Vec<PoolList>>,
    pub pool_props: Res<Vec<(String, Props)>>,
    pub datasets: Res<Vec<Dataset>>,
    pub params: Vec<(String, String)>,
    pub version: Option<(String, String)>,
}

pub fn config(src: &dyn Source) -> Config {
    let run = |args: &[&str], prog: &str| src.run(prog, args, CMD_TIMEOUT);
    let status = run(&["status", "-j", "--json-int"], "zpool").and_then(|s| zjson::parse_status(&s));
    let list = run(&["list", "-v", "-j", "--json-int"], "zpool").and_then(|s| zjson::parse_list(&s));
    let pool_props = run(&["get", "-j", "-p", "all"], "zpool").and_then(|s| zjson::parse_pool_get(&s));
    let datasets = run(&["list", "-j", "--json-int", "-t", "filesystem,volume", "-o", "all"], "zfs")
        .and_then(|s| zjson::parse_zfs_list(&s));
    let version = run(&["version", "-j"], "zfs").ok().and_then(|s| zjson::parse_version(&s));
    let pdir = "/sys/module/zfs/parameters";
    let params = src
        .list(pdir)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| src.read(&format!("{pdir}/{p}")).ok().map(|v| (p, v.trim().to_string())))
        .collect();
    Config { t: src.now(), status, list, pool_props, datasets, params, version }
}

/// Snapshots of one dataset (on demand: hosts can have 100k snapshots, so they are never listed wholesale).
pub fn snapshots(src: &dyn Source, ds: &str) -> Res<Vec<Dataset>> {
    let o = "name,used,referenced,written,logicalreferenced,compressratio,creation,userrefs,clones,defer_destroy";
    src.run("zfs", &["list", "-j", "--json-int", "-t", "snapshot", "-d", "1", "-o", o, ds], CMD_TIMEOUT)
        .and_then(|s| zjson::parse_zfs_list(&s))
}

/// Pool history (`zpool history -il`), newest `keep` entries.
pub fn history(src: &dyn Source, pool: &str) -> Res<Vec<crate::parse::history::Entry>> {
    let t = src.run("zpool", &["history", "-il", pool], Duration::from_secs(60))?;
    Ok(crate::parse::history::parse(&t, 20_000))
}

/// Cumulative latency (`w`) and request-size (`r`) histograms.
pub fn hists(src: &dyn Source) -> Res<(Vec<Hist>, Vec<Hist>)> {
    let w = src.run("zpool", &["iostat", "-Hpvw"], CMD_TIMEOUT)?;
    let r = src.run("zpool", &["iostat", "-Hpvr"], CMD_TIMEOUT)?;
    Ok((parse_hists(&w), parse_hists(&r)))
}

// ------------------------------------------------------------------------------------------------ disks

/// One path (leg) to a disk: the SCSI device under a multipath map, or the device itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Leg {
    pub dev: String,
    pub state: String,
    pub vendor: String,
    pub model: String,
    pub rev: String,
    pub wwid: String,
    pub sas_address: String,
    pub slot: Option<String>,
    pub ioerr: Option<u64>,
    pub iotmo: Option<u64>,
}

/// A leaf vdev resolved to block devices.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiskInfo {
    pub pool: String,
    /// leaf vdev name as zpool shows it (alias, by-id name, sdX)
    pub vdev: String,
    /// its parent vdev (raidz2-0, mirror-6) or "-" for a top-level disk
    pub parent: String,
    pub class: String,
    pub path: String,
    /// device the vdev path resolves to (dm-3, nvme3n1p1, sdb1)
    pub dev: String,
    /// whole device (dm-3, nvme3n1, sdb) — its diskstats are what we show
    pub whole: String,
    pub dm_name: Option<String>,
    pub legs: Vec<Leg>,
    pub size: u64,
    pub rotational: Option<bool>,
    pub scheduler: Option<String>,
    pub logical_bs: Option<u64>,
    pub physical_bs: Option<u64>,
    pub nvme_model: Option<String>,
    pub nvme_serial: Option<String>,
    pub nvme_fw: Option<String>,
    /// vdev_id.conf alias target and comment
    pub alias: Option<(String, String)>,
    /// device to run smartctl against (first multipath leg, or the whole device)
    pub smart_dev: String,
}

impl DiskInfo {
    pub fn is_nvme(&self) -> bool {
        self.whole.starts_with("nvme")
    }
    pub fn slot(&self) -> Option<&str> {
        self.legs.iter().find_map(|l| l.slot.as_deref())
    }
    pub fn model(&self) -> String {
        if let Some(m) = &self.nvme_model {
            return m.clone();
        }
        self.legs.first().map(|l| format!("{} {}", l.vendor, l.model).trim().to_string()).unwrap_or_default()
    }
    pub fn kind(&self) -> &'static str {
        match (self.is_nvme(), self.rotational) {
            (true, _) => "nvme",
            (_, Some(true)) => "hdd",
            (_, Some(false)) => "ssd",
            _ => "?",
        }
    }
}

fn join(base: &str, rel: &str) -> String {
    if rel.starts_with('/') {
        return rel.to_string();
    }
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    parts.pop(); // the link itself
    for c in rel.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Follow symlinks (by-vdev, by-id) to the kernel device name.
fn resolve(src: &dyn Source, path: &str) -> String {
    let mut cur = path.to_string();
    for _ in 0..8 {
        match src.read_link(&cur) {
            Ok(t) => cur = join(&cur, &t),
            Err(_) => break,
        }
    }
    cur.rsplit('/').next().unwrap_or(&cur).to_string()
}

fn attr(src: &dyn Source, dev: &str, a: &str) -> Option<String> {
    src.read(&format!("/sys/class/block/{dev}/{a}")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn parent_of(src: &dyn Source, dev: &str) -> Option<String> {
    attr(src, dev, "partition")?;
    if let Some(p) = attr(src, dev, "parent") {
        return Some(p); // fixture captures record it explicitly
    }
    let link = src.read_link(&format!("/sys/class/block/{dev}")).ok()?;
    let mut it = link.rsplit('/');
    it.next();
    it.next().map(str::to_string)
}

pub fn inventory(src: &dyn Source, pools: &[PoolStatus]) -> Vec<DiskInfo> {
    let aliases = src.read("/etc/zfs/vdev_id.conf").map(|s| parse_vdev_id(&s)).unwrap_or_default();
    let mut out = Vec::new();
    for p in pools {
        let mut parents: HashMap<String, String> = HashMap::new();
        p.root.walk(0, &mut |v, _| {
            for c in &v.children {
                if v.vtype != "class" && v.vtype != "root" {
                    parents.insert(c.name.clone(), v.name.clone());
                }
            }
        });
        for leaf in p.root.leaves() {
            let Some(path) = leaf.path.clone() else { continue };
            let mut d = DiskInfo {
                pool: p.name.clone(),
                vdev: leaf.name.clone(),
                parent: parents.get(&leaf.name).cloned().unwrap_or_else(|| "-".into()),
                class: leaf.class.clone(),
                path: path.clone(),
                alias: aliases.get(&leaf.name).cloned(),
                ..Default::default()
            };
            d.dev = resolve(src, &path);
            d.whole = parent_of(src, &d.dev).unwrap_or_else(|| d.dev.clone());
            // dm partitions (kpartx) sit on the whole multipath map
            if d.whole.starts_with("dm-")
                && attr(src, &d.whole, "dm/uuid").is_some_and(|u| u.starts_with("part"))
                && let Some(s) =
                    src.list(&format!("/sys/class/block/{}/slaves", d.whole)).ok().and_then(|v| v.into_iter().next())
            {
                d.whole = s;
            }
            let w = d.whole.clone();
            d.dm_name = attr(src, &w, "dm/name");
            d.size = attr(src, &w, "size").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) * 512;
            d.rotational = attr(src, &w, "queue/rotational").map(|r| r == "1");
            d.scheduler = attr(src, &w, "queue/scheduler").map(|s| {
                s.split_whitespace().find(|t| t.starts_with('[')).unwrap_or(&s).trim_matches(['[', ']']).to_string()
            });
            d.logical_bs = attr(src, &w, "queue/logical_block_size").and_then(|s| s.parse().ok());
            d.physical_bs = attr(src, &w, "queue/physical_block_size").and_then(|s| s.parse().ok());
            if w.starts_with("nvme") {
                d.nvme_model = attr(src, &w, "device/model");
                d.nvme_serial = attr(src, &w, "device/serial");
                d.nvme_fw = attr(src, &w, "device/firmware_rev");
            }
            let legs = if w.starts_with("dm-") {
                src.list(&format!("/sys/class/block/{w}/slaves")).unwrap_or_default()
            } else {
                vec![w.clone()]
            };
            for l in legs {
                let dir = format!("/sys/class/block/{l}/device");
                let slot = src.list(&dir).ok().and_then(|es| es.iter().find_map(|e| enclosure_slot(e)));
                d.legs.push(Leg {
                    state: attr(src, &l, "device/state").unwrap_or_default(),
                    vendor: attr(src, &l, "device/vendor").unwrap_or_default(),
                    model: attr(src, &l, "device/model").unwrap_or_default(),
                    rev: attr(src, &l, "device/rev").unwrap_or_default(),
                    wwid: attr(src, &l, "device/wwid").or_else(|| attr(src, &l, "wwid")).unwrap_or_default(),
                    sas_address: attr(src, &l, "device/sas_address").unwrap_or_default(),
                    ioerr: attr(src, &l, "device/ioerr_cnt")
                        .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()),
                    iotmo: attr(src, &l, "device/iotmo_cnt")
                        .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()),
                    slot,
                    dev: l,
                });
            }
            d.smart_dev = d.legs.first().map(|l| l.dev.clone()).unwrap_or_else(|| w.clone());
            out.push(d);
        }
    }
    out
}

pub fn smart(src: &dyn Source, dev: &str) -> Res<Smart> {
    let path = format!("/dev/{dev}");
    parse_smart(&src.run_lenient("smartctl", &["-j", "-a", &path], Duration::from_secs(30))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins() {
        assert_eq!(join("/dev/disk/by-vdev/e00s00", "../../dm-3"), "/dev/dm-3");
        assert_eq!(join("/dev/disk/by-id/x", "/dev/sda"), "/dev/sda");
        assert_eq!(
            join("/sys/class/block/nvme3n1p1", "../../devices/pci/nvme3n1/nvme3n1p1"),
            "/sys/devices/pci/nvme3n1/nvme3n1p1"
        );
    }
}
