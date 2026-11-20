//! `--dump`: run every collector once and print what came back, without the TUI. For checking a new host (or a
//! fixture) quickly and for bug reports.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use color_eyre::eyre::Result;

use crate::collect;
use crate::fmt;
use crate::model::{Model, vkey};
use crate::parse::zjson::PropsExt;
use crate::source::Source;

pub fn dump(src: &dyn Source, interval: Duration) -> Result<()> {
    let mut m = Model::new();
    println!("zfscope {} · source: {}", crate::VERSION, src.describe());

    let cfg = collect::config(src);
    match &cfg.version {
        Some((u, k)) => println!("zfs: {u} / kmod {k}"),
        None => println!("zfs: version unknown"),
    }
    let pools = cfg.status.clone().unwrap_or_default();
    for e in
        [cfg.status.as_ref().err(), cfg.list.as_ref().err(), cfg.pool_props.as_ref().err(), cfg.datasets.as_ref().err()]
            .into_iter()
            .flatten()
    {
        println!("ERROR {e}");
    }
    println!("params: {} read from /sys/module/zfs/parameters", cfg.params.len());
    m.on_config(cfg);

    // two kstat samples an interval apart, so rates exist
    src.tick();
    m.on_kstats(collect::kstats(src), true);
    std::thread::sleep(if src.is_fixture() { Duration::ZERO } else { interval });
    src.tick();
    m.on_kstats(collect::kstats(src), true);

    // one iostat block
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = crossbeam_channel::unbounded();
    {
        let stop = stop.clone();
        // SAFETY of lifetimes: the stream runs on this thread's borrowed source via scope
        std::thread::scope(|s| {
            s.spawn(|| src.iostat_stream(interval, stop.clone(), tx));
            match rx.recv_timeout(interval + Duration::from_secs(10)) {
                Ok(b) => m.on_iostat(b, src.now(), true),
                Err(_) => println!("ERROR zpool iostat: no output within {:?}", interval + Duration::from_secs(10)),
            }
            stop.store(true, Ordering::Relaxed);
        });
    }
    m.on_hists(collect::hists(src), src.now(), true);
    if let Some(e) = &m.hist_err {
        println!("ERROR histograms: {e}");
    }

    for p in m.pools() {
        println!();
        let l = m.pool_list(&p.name);
        println!(
            "pool {} {} size {} alloc {} frag {}% cap {}%  errors {}",
            p.name,
            p.state,
            fmt::size(l.map(|l| l.props.n("size")).unwrap_or(0)),
            fmt::size(l.map(|l| l.props.n("allocated")).unwrap_or(0)),
            l.map(|l| l.props.n("fragmentation")).unwrap_or(0),
            l.map(|l| l.props.n("capacity")).unwrap_or(0),
            p.error_count,
        );
        if let Some(s) = &p.scan {
            println!(
                "  scan: {} {} examined {} issued {} of {}",
                s.function,
                s.state,
                fmt::size(s.examined),
                fmt::size(s.issued),
                fmt::size(s.to_examine)
            );
        }
        println!(
            "  {:<44} {:<9} {:>7} {:>7} {:>8} {:>8} {:>7} {:>7} {:>5} {:>5}",
            "vdev", "state", "r/s", "w/s", "read", "write", "lat r", "lat w", "frag", "hist"
        );
        p.root.walk(0, &mut |v, d| {
            let io = m.io(&p.name, &v.name);
            let k = vkey(&p.name, &v.name);
            println!(
                "  {:<44} {:<9} {:>7} {:>7} {:>8} {:>8} {:>7} {:>7} {:>5} {:>5}",
                fmt::fit_left(&format!("{}{}", "  ".repeat(d), v.name), 44),
                v.state,
                io.map(|i| fmt::count(i.rops)).unwrap_or("-".into()),
                io.map(|i| fmt::count(i.wops)).unwrap_or("-".into()),
                io.map(|i| fmt::rate(i.rbw)).unwrap_or("-".into()),
                io.map(|i| fmt::rate(i.wbw)).unwrap_or("-".into()),
                io.map(|i| fmt::ns(i.wait[0].unwrap_or(0.0))).unwrap_or("-".into()),
                io.map(|i| fmt::ns(i.wait[1].unwrap_or(0.0))).unwrap_or("-".into()),
                v.frag.map(|f| format!("{f}%")).unwrap_or("-".into()),
                if m.hist.lat.contains_key(&k) { "yes" } else { "-" },
            );
        });
    }

    println!();
    let r = |k: &str, f: &str| m.rate(k, f);
    println!(
        "ARC size {} target {} max {}  hits/s {} misses/s {}  demand hit {}",
        fmt::size(m.val("arcstats", "size")),
        fmt::size(m.val("arcstats", "c")),
        fmt::size(m.val("arcstats", "c_max")),
        fmt::count(r("arcstats", "hits")),
        fmt::count(r("arcstats", "misses")),
        fmt::pct(m.series.last("arc.demand_hit")),
    );
    println!(
        "ZIL commits/s {} itx/s {} slog {} normal {}",
        fmt::count(r("zil", "zil_commit_count")),
        fmt::count(r("zil", "zil_itx_count")),
        fmt::rate(r("zil", "zil_itx_metaslab_slog_bytes")),
        fmt::rate(r("zil", "zil_itx_metaslab_normal_bytes")),
    );
    for (pool, t) in &m.txgs {
        if let Some(last) = t.back() {
            println!(
                "TXG {pool}: last committed {} sync {} wrote {}",
                last.txg,
                fmt::ns(last.stime as f64),
                fmt::size(last.nwritten)
            );
        }
    }
    let objsets: usize = m.ks.as_ref().map(|k| k.pools.iter().map(|p| p.objsets.len()).sum()).unwrap_or(0);
    println!(
        "datasets: {} from zfs list, {} objset kstats, {} with IO rates",
        m.datasets().len(),
        objsets,
        m.ds_io.len()
    );
    let mut busiest: Vec<_> = m.ds_io.iter().collect();
    busiest.sort_by(|a, b| b.1.bw().total_cmp(&a.1.bw()));
    for (n, io) in busiest.iter().take(5) {
        println!(
            "  {:<50} r {:>7}/s w {:>7}/s  {} {}",
            fmt::fit_left(n, 50),
            fmt::count(io.rops),
            fmt::count(io.wops),
            fmt::rate(io.rbw),
            fmt::rate(io.wbw)
        );
    }

    println!();
    let inv = collect::inventory(src, &pools);
    println!("disks: {} leaf vdevs resolved", inv.len());
    for d in inv.iter().take(4).chain(inv.iter().rev().take(2)) {
        println!(
            "  {:<20} -> {:<10} whole {:<10} legs [{}] slot {:<8} {} {} {}",
            fmt::fit_left(&d.vdev, 20),
            d.dev,
            d.whole,
            d.legs.iter().map(|l| l.dev.as_str()).collect::<Vec<_>>().join(","),
            d.slot().unwrap_or("-"),
            d.kind(),
            d.model(),
            fmt::size(d.size)
        );
    }
    if let Some(d) = inv.first() {
        match collect::smart(src, &d.smart_dev) {
            Ok(s) => println!(
                "  smart {}: model {:?} serial {:?} temp {:?} hours {:?} passed {:?}",
                d.smart_dev, s.model, s.serial, s.temp_c, s.power_on_hours, s.passed
            ),
            Err(e) => println!("  smart {}: ERROR {e}", d.smart_dev),
        }
    }
    Ok(())
}
