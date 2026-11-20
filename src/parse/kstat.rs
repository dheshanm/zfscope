//! `/proc/spl/kstat/zfs/*` parsers.
//!
//! Named kstats look like:
//! ```text
//! 100 1 0x01 28 7872 875297016447292 2937458992061001
//! name                            type data
//! dataset_name                    7    pool0/fs
//! writes                          4    8
//! ```
//! Types: 3 = int64, 4 = uint64, 7 = string. Names may contain spaces (`1 ns` in `dmu_tx_assign`).

use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Kstat {
    /// Numeric entries in file order (order matters for histograms like `dmu_tx_assign`).
    pub nums: Vec<(String, u64)>,
    pub strs: HashMap<String, String>,
    idx: HashMap<String, usize>,
    /// kstat snaptime (ns, monotonic clock), from the header line.
    pub snaptime: u64,
}

impl Kstat {
    pub fn get(&self, name: &str) -> Option<u64> {
        self.idx.get(name).map(|&i| self.nums[i].1)
    }

    /// Missing entries read as 0 (fields come and go between ZFS versions).
    pub fn n(&self, name: &str) -> u64 {
        self.get(name).unwrap_or(0)
    }

    pub fn s(&self, name: &str) -> Option<&str> {
        self.strs.get(name).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.nums.is_empty() && self.strs.is_empty()
    }
}

pub fn parse_named(text: &str) -> Result<Kstat, String> {
    let mut k = Kstat::default();
    let mut lines = text.lines();
    let header = lines.next().ok_or("empty kstat")?;
    k.snaptime = header.split_whitespace().nth(6).and_then(|s| s.parse().ok()).unwrap_or(0);
    let cols = lines.next().ok_or("kstat without column header")?;
    if !cols.starts_with("name") {
        return Err(format!("not a named kstat: {cols:?}"));
    }
    // Column offsets from the header ("name   type data"): names and string data may contain spaces.
    let type_col = cols.find("type").unwrap_or(32);
    let data_col = cols.find("data").unwrap_or(37);
    for line in lines {
        let by_col = (line.len() > data_col && line.is_char_boundary(type_col) && line.is_char_boundary(data_col))
            .then(|| (line[..type_col].trim(), line[type_col..data_col].trim(), line[data_col..].trim()))
            .filter(|(n, t, _)| !n.is_empty() && !n.ends_with(' ') && t.parse::<u8>().is_ok());
        let (name, ty, data) = match by_col {
            Some((n, t, d)) => (n.to_string(), t.parse::<u8>().unwrap_or(0), d.to_string()),
            None => {
                // Overlong name pushed the columns right: numeric data is the last token, type the one before.
                let toks: Vec<&str> = line.split_whitespace().collect();
                if toks.len() < 3 {
                    continue;
                }
                let n = toks.len();
                (toks[..n - 2].join(" "), toks[n - 2].parse().unwrap_or(0), toks[n - 1].to_string())
            }
        };
        match ty {
            7 => {
                k.strs.insert(name, data);
            }
            3 => {
                let v = data.parse::<i64>().unwrap_or(0).max(0) as u64;
                k.idx.insert(name.clone(), k.nums.len());
                k.nums.push((name, v));
            }
            _ => {
                if let Ok(v) = data.parse::<u64>() {
                    k.idx.insert(name.clone(), k.nums.len());
                    k.nums.push((name, v));
                }
            }
        }
    }
    Ok(k)
}

/// One row of `<pool>/txgs`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Txg {
    pub txg: u64,
    /// hrtime ns
    pub birth: u64,
    /// O(pen), Q(uiescing), W(ait for sync), S(yncing), C(ommitted)
    pub state: char,
    pub ndirty: u64,
    pub nread: u64,
    pub nwritten: u64,
    pub reads: u64,
    pub writes: u64,
    /// ns spent open / quiescing / waiting to sync / syncing
    pub otime: u64,
    pub qtime: u64,
    pub wtime: u64,
    pub stime: u64,
}

pub fn parse_txgs(text: &str) -> Result<Vec<Txg>, String> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    // optional kstat header line (number soup), then the column header
    for l in lines.by_ref() {
        if l.starts_with("txg") {
            break;
        }
    }
    for l in lines {
        let t: Vec<&str> = l.split_whitespace().collect();
        if t.len() < 12 {
            continue;
        }
        let n = |i: usize| t[i].parse::<u64>().unwrap_or(0);
        out.push(Txg {
            txg: n(0),
            birth: n(1),
            state: t[2].chars().next().unwrap_or('?'),
            ndirty: n(3),
            nread: n(4),
            nwritten: n(5),
            reads: n(6),
            writes: n(7),
            otime: n(8),
            qtime: n(9),
            wtime: n(10),
            stime: n(11),
        });
    }
    Ok(out)
}

/// `/proc/meminfo` → bytes for the few fields we show.
pub fn parse_meminfo(text: &str) -> HashMap<String, u64> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            let mut it = v.split_whitespace();
            let n: u64 = it.next()?.parse().ok()?;
            let mult = if it.next() == Some("kB") { 1024 } else { 1 };
            Some((k.trim().to_string(), n * mult))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBJSET: &str = "100 1 0x01 28 7872 875297016447292 2937458992061001
name                            type data
dataset_name                    7    pool0/a/b c
writes                          4    8
nwritten                        4    25151
memory_available_bytes          3    -5
";

    #[test]
    fn named() {
        let k = parse_named(OBJSET).unwrap();
        assert_eq!(k.s("dataset_name"), Some("pool0/a/b c"));
        assert_eq!(k.n("writes"), 8);
        assert_eq!(k.n("nwritten"), 25151);
        assert_eq!(k.n("memory_available_bytes"), 0);
        assert_eq!(k.n("missing"), 0);
        assert_eq!(k.snaptime, 2937458992061001);
    }

    #[test]
    fn spaced_names() {
        let k = parse_named("1 1 0x01 2 3 4 5\nname type data\n1 ns 4 0\n32 ns 4 244\n").unwrap();
        assert_eq!(k.n("32 ns"), 244);
        assert_eq!(k.nums.len(), 2);
    }

    #[test]
    fn txgs() {
        let t = parse_txgs(
            "18 0 0x01 1 2 3 4\ntxg      birth            state ndirty       nread        nwritten     reads    writes   otime        qtime        wtime        stime\n\
             1106835  2937456440201552 C     560137728    22269952     860889088    332      5083     5047598471   2793         73832        281284334\n\
             1106837  2937466607834844 O     0            0            0            0        0        0            0            0            0\n",
        )
        .unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].state, 'C');
        assert_eq!(t[0].stime, 281284334);
        assert_eq!(t[1].state, 'O');
    }

    #[test]
    fn meminfo() {
        let m = parse_meminfo("MemTotal:       394691260 kB\nHugePages_Total:       0\n");
        assert_eq!(m["MemTotal"], 394691260 * 1024);
        assert_eq!(m["HugePages_Total"], 0);
    }
}
