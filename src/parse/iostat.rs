//! `zpool iostat -Hp` parsers (zpool iostat has no JSON output, even in 2.4).
//!
//! `-Hpvlq` rows are tab-separated, 32 columns, no indentation; class sections (special/logs/cache/spares) show
//! up as rows with an empty name. Rows are matched to the topology by name later (see `model::pool`).
//!
//! `-Hpvw` / `-Hpvr` histograms are blocks: a blank line, the vdev name, then `bucket\tcount...` rows.

/// One `zpool iostat -Hpvlq` row. Rates are per-second averages over the interval; waits are ns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    pub name: String,
    pub alloc: Option<u64>,
    pub free: Option<u64>,
    pub rops: f64,
    pub wops: f64,
    pub rbw: f64,
    pub wbw: f64,
    /// total_wait r/w, disk_wait r/w, syncq_wait r/w, asyncq_wait r/w, scrub, trim, rebuild (ns)
    pub wait: [Option<f64>; 11],
    /// pend/activ for syncq_read, syncq_write, asyncq_read, asyncq_write, scrubq_read, trimq_write, rebuildq_write
    pub queue: [u64; 14],
}

pub mod w {
    pub const TOTAL_R: usize = 0;
    pub const TOTAL_W: usize = 1;
    pub const DISK_R: usize = 2;
    pub const DISK_W: usize = 3;
    pub const SYNCQ_R: usize = 4;
    pub const SYNCQ_W: usize = 5;
    pub const ASYNCQ_R: usize = 6;
    pub const ASYNCQ_W: usize = 7;
    pub const SCRUB: usize = 8;
    pub const TRIM: usize = 9;
    pub const REBUILD: usize = 10;
    pub const NAMES: [&str; 11] = [
        "total r", "total w", "disk r", "disk w", "syncq r", "syncq w", "asyncq r", "asyncq w", "scrub", "trim",
        "rebuild",
    ];
}

pub const QUEUE_NAMES: [&str; 7] = ["sync rd", "sync wr", "async rd", "async wr", "scrub", "trim", "rebuild"];

impl Row {
    /// Active (issued to the disk) and pending (waiting in ZFS) I/Os summed over all queues.
    pub fn active(&self) -> u64 {
        self.queue.iter().skip(1).step_by(2).sum()
    }
    pub fn pending(&self) -> u64 {
        self.queue.iter().step_by(2).sum()
    }
}

fn num(s: &str) -> Option<u64> {
    s.parse().ok()
}

pub fn parse_row(line: &str) -> Option<Row> {
    let c: Vec<&str> = line.split('\t').collect();
    if c.len() < 7 {
        return None;
    }
    let f = |i: usize| c.get(i).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    let mut r = Row {
        name: c[0].trim().to_string(),
        alloc: num(c[1]),
        free: num(c[2]),
        rops: f(3),
        wops: f(4),
        rbw: f(5),
        wbw: f(6),
        ..Default::default()
    };
    for i in 0..11 {
        r.wait[i] = c.get(7 + i).and_then(|s| s.parse::<f64>().ok());
    }
    for i in 0..14 {
        r.queue[i] = c.get(18 + i).and_then(|s| s.parse().ok()).unwrap_or(0);
    }
    Some(r)
}

/// Parse one interval's block of rows.
pub fn parse_block(text: &str) -> Vec<Row> {
    text.lines().filter(|l| !l.is_empty()).filter_map(parse_row).collect()
}

/// Split a captured `-y N count` stream into per-interval blocks: every block starts with the same first pool row.
pub fn split_stream(text: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut first: Option<String> = None;
    for line in text.lines() {
        let name = line.split('\t').next().unwrap_or("");
        if first.is_none() && !name.is_empty() {
            first = Some(name.to_string());
        }
        if first.as_deref() == Some(name) || blocks.is_empty() {
            blocks.push(String::new());
        }
        let b = blocks.last_mut().expect("pushed above");
        b.push_str(line);
        b.push('\n');
    }
    blocks
}

/// A cumulative histogram for one vdev: `buckets[i]` is the upper bound (ns for latency, bytes for request size),
/// `counts[i][col]` the count since import.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hist {
    pub name: String,
    pub buckets: Vec<u64>,
    pub counts: Vec<Vec<u64>>,
}

// Histogram columns: `-w` latency = total r/w, disk r/w, syncq r/w, asyncq r/w, scrub, trim, rebuild;
// `-r` request size = sync r/w, async r/w, scrub, trim, rebuild, each as individual + aggregated.

pub fn parse_hists(text: &str) -> Vec<Hist> {
    let mut out: Vec<Hist> = Vec::new();
    let mut cur: Option<Hist> = None;
    for line in text.lines() {
        if line.trim().is_empty() {
            if let Some(h) = cur.take() {
                out.push(h);
            }
            continue;
        }
        let toks: Vec<&str> = line.split('\t').collect();
        let bucket = toks[0].trim().parse::<u64>();
        match (&mut cur, bucket) {
            (Some(h), Ok(b)) if toks.len() > 1 => {
                h.buckets.push(b);
                h.counts.push(toks[1..].iter().map(|t| t.trim().parse().unwrap_or(0)).collect());
            }
            _ => {
                if let Some(h) = cur.take() {
                    out.push(h);
                }
                cur = Some(Hist { name: line.trim().to_string(), ..Default::default() });
            }
        }
    }
    if let Some(h) = cur {
        out.push(h);
    }
    // The class separator rows come out as headers with no buckets.
    out.retain(|h| !h.buckets.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: &str = "pool0\t539082605805568\t667253448556544\t867\t1250\t29279612\t253427232\t8704858\t36722607\t5561245\t14465279\t572536\t1328369\t11022504\t28996282\t-\t3417023\t-\t0\t6\t0\t0\t0\t22\t4806\t120\t0\t0\t0\t0\t0\t0";

    #[test]
    fn row() {
        let r = parse_row(ROW).unwrap();
        assert_eq!(r.name, "pool0");
        assert_eq!(r.alloc, Some(539082605805568));
        assert_eq!(r.rops, 867.0);
        assert_eq!(r.wbw, 253427232.0);
        assert_eq!(r.wait[w::TOTAL_R], Some(8704858.0));
        assert_eq!(r.wait[w::SCRUB], None);
        assert_eq!(r.wait[w::TRIM], Some(3417023.0));
        assert_eq!(r.queue[1], 6);
        assert_eq!(r.active(), 6 + 22 + 120);
        assert_eq!(r.pending(), 4806);
    }

    #[test]
    fn idle_dashes() {
        let r = parse_row(
            "e0s00\t0\t0\t0\t0\t0\t0\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0",
        )
        .unwrap();
        assert!(r.wait.iter().all(Option::is_none));
    }

    #[test]
    fn class_separator_row() {
        let r =
            parse_row("\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-\t-")
                .unwrap();
        assert_eq!(r.name, "");
    }

    #[test]
    fn stream_split() {
        let s = format!("{ROW}\nraidz2-0\t1\t2\t0\t0\t0\t0\n{ROW}\nraidz2-0\t1\t2\t0\t0\t0\t0\n");
        let b = split_stream(&s);
        assert_eq!(b.len(), 2);
        assert_eq!(parse_block(&b[1]).len(), 2);
    }

    #[test]
    fn hists() {
        let h = parse_hists("\npool0\n1\t0\t0\n3\t5\t6\n\n\t\nraidz2-0\n1\t1\t2\n");
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].name, "pool0");
        assert_eq!(h[0].buckets, vec![1, 3]);
        assert_eq!(h[0].counts[1], vec![5, 6]);
        assert_eq!(h[1].name, "raidz2-0");
    }
}
