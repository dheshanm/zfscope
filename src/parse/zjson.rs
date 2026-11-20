//! OpenZFS 2.3+ JSON output (`zpool status -j --json-int`, `zpool list -v -j`, `zpool get -j`, `zfs list -j`).
//!
//! Parsed leniently through `serde_json::Value` (field sets differ between versions; numbers are ints with
//! `--json-int` but strings in `zpool get`). Map order is preserved, so vdevs stay in pool order.

use indexmap::IndexMap;
use serde_json::Value;

pub type Res<T> = Result<T, String>;

fn doc(text: &str, what: &str) -> Res<Value> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("{what}: bad JSON: {e}"))?;
    match v["output_version"]["vers_major"].as_u64() {
        Some(0) | None => Ok(v),
        Some(m) => Err(format!("{what}: unsupported JSON output version {m} (zfscope knows 0.x)")),
    }
}

/// Number from an int, a numeric string, or `-`/missing (None).
pub fn num(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f.max(0.0) as u64)),
        Value::String(s) => s.trim_end_matches('%').parse().ok(),
        _ => None,
    }
}

fn s(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

// ------------------------------------------------------------------------------------------------ properties

/// One property: display value, numeric value if any, and where it came from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prop {
    pub value: String,
    pub num: Option<u64>,
    /// LOCAL, DEFAULT, INHERITED, TEMPORARY, RECEIVED, NONE
    pub source: String,
    /// for INHERITED: the dataset it comes from
    pub from: String,
}

pub type Props = IndexMap<String, Prop>;

fn props(v: &Value) -> Props {
    let mut out = Props::new();
    if let Some(m) = v.as_object() {
        for (k, p) in m {
            let value = &p["value"];
            out.insert(
                k.clone(),
                Prop {
                    value: s(value),
                    num: num(value),
                    source: p["source"]["type"].as_str().unwrap_or("").to_string(),
                    from: p["source"]["data"].as_str().unwrap_or("").to_string(),
                },
            );
        }
    }
    out
}

pub trait PropsExt {
    fn n(&self, k: &str) -> u64;
    fn opt(&self, k: &str) -> Option<u64>;
    fn str(&self, k: &str) -> &str;
}

impl PropsExt for Props {
    fn n(&self, k: &str) -> u64 {
        self.get(k).and_then(|p| p.num).unwrap_or(0)
    }
    fn opt(&self, k: &str) -> Option<u64> {
        self.get(k).and_then(|p| p.num)
    }
    fn str(&self, k: &str) -> &str {
        self.get(k).map(|p| p.value.as_str()).unwrap_or("-")
    }
}

// ------------------------------------------------------------------------------------------------ zpool status

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Vdev {
    pub name: String,
    /// root, raidz, mirror, draid, disk, file, replacing, spare, indirect, or "class" for a section header
    pub vtype: String,
    /// normal, special, dedup, log, l2cache/cache, spare
    pub class: String,
    pub state: String,
    pub path: Option<String>,
    pub devid: Option<String>,
    pub phys_path: Option<String>,
    pub alloc: u64,
    pub size: u64,
    pub read_errors: u64,
    pub write_errors: u64,
    pub checksum_errors: u64,
    pub slow_ios: u64,
    /// from `zpool list -v`
    pub frag: Option<u64>,
    pub cap: Option<u64>,
    /// resilver / trim / initialize activity on this vdev
    pub activity: Option<String>,
    pub children: Vec<Vdev>,
}

impl Vdev {
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty() && self.vtype != "class"
    }
    pub fn errors(&self) -> u64 {
        self.read_errors + self.write_errors + self.checksum_errors
    }
    /// Depth-first walk with depth.
    pub fn walk<'a>(&'a self, depth: usize, f: &mut impl FnMut(&'a Vdev, usize)) {
        f(self, depth);
        for c in &self.children {
            c.walk(depth + 1, f);
        }
    }
    pub fn leaves(&self) -> Vec<&Vdev> {
        let mut out = Vec::new();
        self.walk(0, &mut |v, _| {
            if v.is_leaf() {
                out.push(v)
            }
        });
        out
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scan {
    /// SCRUB, RESILVER, ERRORSCRUB
    pub function: String,
    /// SCANNING, FINISHED, CANCELED
    pub state: String,
    pub start_time: i64,
    pub end_time: i64,
    pub to_examine: u64,
    pub examined: u64,
    pub issued: u64,
    pub processed: u64,
    pub skipped: u64,
    pub errors: u64,
    pub pass_start: i64,
    pub pass_examined: u64,
    pub pass_issued: u64,
    pub paused: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PoolStatus {
    pub name: String,
    pub state: String,
    pub guid: u64,
    pub txg: u64,
    pub status: Option<String>,
    pub action: Option<String>,
    pub error_count: u64,
    pub scan: Option<Scan>,
    /// Extra progress lines (raidz expansion, removal, checkpoint).
    pub notes: Vec<String>,
    /// The pool's root vdev; class sections (special, logs, ...) are `vtype == "class"` children after the data vdevs.
    pub root: Vdev,
}

const CLASS_KEYS: [(&str, &str); 6] = [
    ("special", "special"),
    ("dedup", "dedup"),
    ("logs", "logs"),
    ("l2cache", "cache"),
    ("cache", "cache"),
    ("spares", "spares"),
];

fn vdev(name: &str, v: &Value) -> Vdev {
    let mut out = Vdev {
        name: v["name"].as_str().unwrap_or(name).to_string(),
        vtype: v["vdev_type"].as_str().unwrap_or("").to_string(),
        class: v["class"].as_str().unwrap_or("normal").to_string(),
        state: v["state"].as_str().unwrap_or("").to_string(),
        path: v["path"].as_str().map(str::to_string),
        devid: v["devid"].as_str().map(str::to_string),
        phys_path: v["phys_path"].as_str().map(str::to_string),
        alloc: num(&v["alloc_space"]).unwrap_or(0),
        size: num(&v["total_space"]).or_else(|| num(&v["phys_space"])).unwrap_or(0),
        read_errors: num(&v["read_errors"]).unwrap_or(0),
        write_errors: num(&v["write_errors"]).unwrap_or(0),
        checksum_errors: num(&v["checksum_errors"]).unwrap_or(0),
        slow_ios: num(&v["slow_ios"]).unwrap_or(0),
        ..Default::default()
    };
    let mut act = Vec::new();
    if v["resilver_repaired"].is_string() || v["scan_processed"].is_number() {
        // per-vdev resilver progress
        if let Some(p) = num(&v["scan_processed"]).filter(|&p| p > 0) {
            act.push(format!("resilvering ({} done)", crate::fmt::size(p)));
        }
    }
    if let Some(t) = v["trim_state"].as_str().filter(|t| *t == "ACTIVE" || *t == "VDEV_TRIM_ACTIVE") {
        act.push(format!("trimming {t}"));
    }
    if let Some(i) = v["initialize_state"].as_str().filter(|t| t.contains("ACTIVE")) {
        act.push(format!("initializing {i}"));
    }
    if !act.is_empty() {
        out.activity = Some(act.join(", "));
    }
    if let Some(ch) = v["vdevs"].as_object() {
        out.children = ch.iter().map(|(k, c)| vdev(k, c)).collect();
    }
    out
}

fn class_group(key: &str, label: &str, m: &serde_json::Map<String, Value>) -> Vdev {
    let children: Vec<Vdev> = m.iter().map(|(k, c)| vdev(k, c)).collect();
    let class = children.first().map(|c| c.class.clone()).unwrap_or_else(|| key.to_string());
    Vdev {
        name: label.to_string(),
        vtype: "class".into(),
        class,
        alloc: children.iter().map(|c| c.alloc).sum(),
        size: children.iter().map(|c| c.size).sum(),
        children,
        ..Default::default()
    }
}

fn scan(v: &Value) -> Option<Scan> {
    if !v.is_object() {
        return None;
    }
    let i = |k: &str| num(&v[k]).unwrap_or(0) as i64;
    Some(Scan {
        function: s(&v["function"]),
        state: s(&v["state"]),
        start_time: i("start_time"),
        end_time: i("end_time"),
        to_examine: num(&v["to_examine"]).unwrap_or(0),
        examined: num(&v["examined"]).unwrap_or(0),
        issued: num(&v["issued"]).unwrap_or(0),
        processed: num(&v["processed"]).unwrap_or(0),
        skipped: num(&v["skipped"]).unwrap_or(0),
        errors: num(&v["errors"]).unwrap_or(0),
        pass_start: i("pass_start"),
        pass_examined: num(&v["bytes_per_scan"]).unwrap_or(0),
        pass_issued: num(&v["issued_bytes_per_scan"]).unwrap_or(0),
        paused: i("scrub_pause") > 0,
    })
}

pub fn parse_status(text: &str) -> Res<Vec<PoolStatus>> {
    let d = doc(text, "zpool status")?;
    let mut out = Vec::new();
    for (name, p) in d["pools"].as_object().into_iter().flatten() {
        let mut root = p["vdevs"]
            .as_object()
            .and_then(|m| m.iter().next())
            .map(|(k, v)| vdev(k, v))
            .unwrap_or_else(|| Vdev { name: name.clone(), vtype: "root".into(), ..Default::default() });
        for (key, label) in CLASS_KEYS {
            if let Some(m) = p[key].as_object().filter(|m| !m.is_empty()) {
                root.children.push(class_group(key, label, m));
            }
        }
        let mut notes = Vec::new();
        if let Some(x) = p["raidz_expand_stats"].as_object() {
            let (done, total) = (num(&x["reflowed"]).unwrap_or(0), num(&x["to_reflow"]).unwrap_or(0));
            if x.get("state").and_then(Value::as_str).is_some_and(|s| s.contains("SCANNING")) && total > 0 {
                notes.push(format!(
                    "raidz expansion: {} / {} ({:.1}%)",
                    crate::fmt::size(done),
                    crate::fmt::size(total),
                    100.0 * done as f64 / total as f64
                ));
            }
        }
        if let Some(x) = p["removal_stats"].as_object() {
            let (done, total) = (num(&x["copied"]).unwrap_or(0), num(&x["to_copy"]).unwrap_or(0));
            if x.get("state").and_then(Value::as_str).is_some_and(|s| s.contains("SCANNING")) && total > 0 {
                notes.push(format!("device removal: {} / {}", crate::fmt::size(done), crate::fmt::size(total)));
            }
        }
        if p["checkpoint_stats"].is_object() {
            notes.push("checkpoint exists".into());
        }
        out.push(PoolStatus {
            name: name.clone(),
            state: s(&p["state"]),
            guid: num(&p["pool_guid"]).unwrap_or(0),
            txg: num(&p["txg"]).unwrap_or(0),
            status: p["status"].as_str().map(clean_msg),
            action: p["action"].as_str().map(clean_msg),
            error_count: num(&p["error_count"]).unwrap_or(0),
            scan: scan(&p["scan_stats"]),
            notes,
            root,
        });
    }
    Ok(out)
}

fn clean_msg(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ------------------------------------------------------------------------------------------------ zpool list -v

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PoolList {
    pub name: String,
    pub props: Props,
    /// (class, vdev name) -> (size, alloc, frag%, cap%) for every vdev that reports them
    pub vdevs: Vec<(String, String, Props)>,
}

pub fn parse_list(text: &str) -> Res<Vec<PoolList>> {
    let d = doc(text, "zpool list")?;
    let mut out = Vec::new();
    for (name, p) in d["pools"].as_object().into_iter().flatten() {
        let mut vdevs = Vec::new();
        fn walk(class: &str, m: &serde_json::Map<String, Value>, out: &mut Vec<(String, String, Props)>) {
            for (k, v) in m {
                if v.get("vdev_type").is_none() && v.is_object() {
                    // a class section: {"special": {"mirror-6": {...}}}
                    if let Some(sub) = v.as_object() {
                        walk(k, sub, out);
                    }
                    continue;
                }
                let c = v["class"].as_str().unwrap_or(class).to_string();
                out.push((c, v["name"].as_str().unwrap_or(k).to_string(), props(&v["properties"])));
                if let Some(ch) = v["vdevs"].as_object() {
                    walk(class, ch, out);
                }
            }
        }
        if let Some(m) = p["vdevs"].as_object() {
            walk("normal", m, &mut vdevs);
        }
        out.push(PoolList { name: name.clone(), props: props(&p["properties"]), vdevs });
    }
    Ok(out)
}

// ------------------------------------------------------------------------------------------------ zpool get

pub fn parse_pool_get(text: &str) -> Res<Vec<(String, Props)>> {
    let d = doc(text, "zpool get")?;
    Ok(d["pools"].as_object().into_iter().flatten().map(|(name, p)| (name.clone(), props(&p["properties"]))).collect())
}

// ------------------------------------------------------------------------------------------------ zfs list

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DsKind {
    #[default]
    Filesystem,
    Volume,
    Snapshot,
    Bookmark,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dataset {
    pub name: String,
    pub kind: DsKind,
    pub pool: String,
    pub createtxg: u64,
    pub props: Props,
}

pub fn parse_zfs_list(text: &str) -> Res<Vec<Dataset>> {
    let d = doc(text, "zfs list")?;
    Ok(d["datasets"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, v)| Dataset {
            name: name.clone(),
            kind: match v["type"].as_str().unwrap_or("") {
                "VOLUME" => DsKind::Volume,
                "SNAPSHOT" => DsKind::Snapshot,
                "BOOKMARK" => DsKind::Bookmark,
                _ => DsKind::Filesystem,
            },
            pool: v["pool"].as_str().map(str::to_string).unwrap_or_else(|| name.split('/').next().unwrap_or("").into()),
            createtxg: num(&v["createtxg"]).unwrap_or(0),
            props: props(&v["properties"]),
        })
        .collect())
}

/// `zfs version -j`: (userland, kernel module).
pub fn parse_version(text: &str) -> Option<(String, String)> {
    let d: Value = serde_json::from_str(text).ok()?;
    let z = &d["zfs_version"];
    Some((s(&z["userland"]), s(&z["kernel"])))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = r#"{"output_version":{"command":"zpool status","vers_major":0,"vers_minor":1},"pools":{"tank":{"name":"tank","state":"ONLINE","pool_guid":81,"txg":4,"spa_version":5000,"zpl_version":5,
      "vdevs":{"tank":{"name":"tank","vdev_type":"root","state":"ONLINE","alloc_space":10,"total_space":100,"read_errors":0,"write_errors":0,"checksum_errors":0,
        "vdevs":{"raidz2-0":{"name":"raidz2-0","vdev_type":"raidz","class":"normal","state":"ONLINE","alloc_space":10,"total_space":100,"read_errors":0,"write_errors":0,"checksum_errors":2,
          "vdevs":{"d1":{"name":"d1","vdev_type":"disk","path":"/dev/disk/by-vdev/d1","class":"normal","state":"ONLINE","read_errors":1,"write_errors":0,"checksum_errors":0,"slow_ios":3}}}}}},
      "special":{"mirror-1":{"name":"mirror-1","vdev_type":"mirror","class":"special","state":"ONLINE","alloc_space":1,"total_space":5,
          "vdevs":{"n1":{"name":"n1","vdev_type":"disk","class":"special","state":"ONLINE"}}}},
      "logs":{"l1":{"name":"l1","vdev_type":"disk","class":"log","state":"ONLINE"}},
      "scan_stats":{"function":"SCRUB","state":"SCANNING","start_time":1794548475,"end_time":0,"to_examine":1000,"examined":500,"issued":250,"processed":0,"errors":0,"scrub_pause":0},
      "error_count":0}}}"#;

    #[test]
    fn status() {
        let p = &parse_status(STATUS).unwrap()[0];
        assert_eq!(p.name, "tank");
        assert_eq!(p.root.children.len(), 3);
        assert_eq!(p.root.children[1].vtype, "class");
        assert_eq!(p.root.children[1].name, "special");
        assert_eq!(p.root.children[1].class, "special");
        assert_eq!(p.root.children[2].name, "logs");
        let leaves: Vec<_> = p.root.leaves().iter().map(|v| v.name.clone()).collect();
        assert_eq!(leaves, ["d1", "n1", "l1"]);
        assert_eq!(p.root.children[0].children[0].slow_ios, 3);
        let s = p.scan.as_ref().unwrap();
        assert_eq!((s.function.as_str(), s.examined, s.issued), ("SCRUB", 500, 250));
    }

    #[test]
    fn version_gate() {
        let e = parse_status(r#"{"output_version":{"vers_major":1},"pools":{}}"#).unwrap_err();
        assert!(e.contains("unsupported"), "{e}");
    }

    #[test]
    fn list_classes() {
        let l = parse_list(r#"{"output_version":{"vers_major":0},"pools":{"tank":{"name":"tank","properties":{"size":{"value":100,"source":{"type":"NONE","data":"-"}},"fragmentation":{"value":"7%","source":{"type":"NONE","data":"-"}}},
          "vdevs":{"raidz2-0":{"name":"raidz2-0","vdev_type":"raidz","class":"normal","properties":{"fragmentation":{"value":1,"source":{"type":"NONE","data":"-"}}},"vdevs":{"d1":{"name":"d1","vdev_type":"disk","properties":{}}}},
                   "special":{"mirror-6":{"name":"mirror-6","vdev_type":"mirror","class":"special","properties":{"fragmentation":{"value":39,"source":{"type":"NONE","data":"-"}}}}}}}}}"#).unwrap();
        let p = &l[0];
        assert_eq!(p.props.n("size"), 100);
        assert_eq!(p.props.n("fragmentation"), 7);
        let names: Vec<_> = p.vdevs.iter().map(|(c, n, _)| format!("{c}:{n}")).collect();
        assert_eq!(names, ["normal:raidz2-0", "normal:d1", "special:mirror-6"]);
        assert_eq!(p.vdevs[2].2.n("fragmentation"), 39);
    }

    #[test]
    fn zfs_list() {
        let d = parse_zfs_list(r#"{"output_version":{"vers_major":0},"datasets":{"tank/a":{"name":"tank/a","type":"FILESYSTEM","pool":"tank","createtxg":"9","properties":{"used":{"value":520192,"source":{"type":"NONE","data":"-"}},"compression":{"value":"zstd","source":{"type":"INHERITED","data":"tank"}}}},
          "tank/a@s":{"name":"tank/a@s","type":"SNAPSHOT","pool":"tank","createtxg":"10","properties":{}}}}"#).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].props.n("used"), 520192);
        assert_eq!(d[0].createtxg, 9);
        let c = &d[0].props["compression"];
        assert_eq!((c.value.as_str(), c.source.as_str(), c.from.as_str()), ("zstd", "INHERITED", "tank"));
        assert_eq!(d[1].kind, DsKind::Snapshot);
    }
}
