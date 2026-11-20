//! Every collector against the committed demo fixture (and any real captures under tests/fixtures/local/).

use std::path::Path;

use zfscope::collect;
use zfscope::model::Model;
use zfscope::source::FixtureSource;

const DEMO: &str = "tests/fixtures/demo";

/// What a healthy capture must yield. Returns a summary for the local-capture test's output.
fn check(dir: &Path) -> String {
    let src = FixtureSource::new(dir).expect("fixture");
    let mut m = Model::new();

    let cfg = collect::config(&src);
    let pools = cfg.status.clone().expect("zpool status");
    assert!(!pools.is_empty(), "{}: no pools", dir.display());
    assert!(cfg.list.is_ok() && cfg.pool_props.is_ok(), "{}: zpool list/get", dir.display());
    let datasets = cfg.datasets.clone().expect("zfs list");
    assert!(!datasets.is_empty());
    m.on_config(cfg);

    for t in 0..3 {
        src.set_tick(t);
        m.on_kstats(collect::kstats(&src), true);
    }
    assert!(m.has_kstats(), "arcstats");
    assert!(m.val("arcstats", "c_max") > 0);
    assert!(!m.ds_io.is_empty(), "per-dataset IO from objset kstats");
    let pool = &pools[0].name;
    assert!(!m.txgs.get(pool).map(|t| t.is_empty()).unwrap_or(true), "txgs");

    // two histogram polls give per-interval deltas for every vdev
    src.set_tick(0);
    m.on_hists(collect::hists(&src), 0.0, true);
    src.set_tick(5);
    m.on_hists(collect::hists(&src), 5.0, true);
    let leaves: usize = pools.iter().map(|p| p.root.leaves().len()).sum();
    assert!(m.hist.lat_delta.len() > leaves, "latency histograms for every vdev");

    let inv = collect::inventory(&src, &pools);
    assert_eq!(inv.len(), leaves, "every leaf vdev resolves to a block device");
    assert!(inv.iter().all(|d| !d.whole.is_empty() && !d.legs.is_empty()));
    let smart_ok = inv.iter().filter(|d| collect::smart(&src, &d.smart_dev).is_ok()).count();

    let hist = collect::history(&src, pool).expect("zpool history");
    assert!(!hist.is_empty());
    format!(
        "{}: {} pools, {} datasets, {} disks ({} with SMART), {} history entries",
        dir.display(),
        pools.len(),
        datasets.len(),
        inv.len(),
        smart_ok,
        hist.len()
    )
}

#[test]
fn demo_fixture_parses() {
    let s = check(Path::new(DEMO));
    assert!(s.contains("64 disks (64 with SMART)"), "{s}");
}

#[test]
fn demo_snapshots_on_demand() {
    let src = FixtureSource::new(Path::new(DEMO)).unwrap();
    let snaps = collect::snapshots(&src, "pool0/data/build").unwrap();
    assert!(snaps.iter().all(|s| s.name.starts_with("pool0/data/build@")), "only that dataset's snapshots");
    assert!(!snaps.is_empty());
}

/// Real captures (gitignored) parse too; skipped when there are none.
#[test]
fn local_captures_parse() {
    let Ok(rd) = std::fs::read_dir("tests/fixtures/local") else { return };
    for e in rd.flatten() {
        if e.path().join("meta.json").exists() {
            eprintln!("{}", check(&e.path()));
        }
    }
}
