//! Live-sorted tables (Datasets, Pool topology, Disks): ties (idle rows) keep the previous update's order, so a row
//! that just went idle stays at the top of the idle ones instead of dropping back to name / pool order.

use std::time::Duration;

use zfscope::app::{App, DiskSort, DsSort, TopoSort};
use zfscope::model::{DiskRate, DsIo, vkey};
use zfscope::parse::iostat::Row;
use zfscope::replay::Replay;

fn app() -> App {
    let mut r = Replay::new(std::path::Path::new("tests/fixtures/demo")).unwrap();
    let mut app = App::new("zfs-demo".into(), "fixture".into(), Duration::from_secs(1));
    r.init(&mut app);
    r.run(&mut app, 3);
    app
}

// ------------------------------------------------------------------------------------------------ datasets

/// Set w/s for the named datasets, everything else idle, and record the update.
fn ds_tick(app: &mut App, active: &[(&str, f64)]) {
    let names: Vec<String> = app.model.datasets().iter().map(|d| d.name.clone()).collect();
    app.model.ds_io = names.into_iter().map(|n| (n, DsIo::default())).collect();
    for &(n, w) in active {
        app.model.ds_io.insert(n.to_string(), DsIo { wops: w, ..Default::default() });
    }
    app.record_sort_orders();
}

fn ds_order(app: &App) -> Vec<String> {
    app.ds_rows().into_iter().map(|r| r.name).collect()
}

fn ds_app() -> App {
    let mut app = app();
    app.ds_flat = true;
    app.ds_sort = DsSort::Wops;
    app.ds_desc = true;
    app
}

#[test]
fn datasets_idle_stays_on_top_of_idle_ones() {
    let mut app = ds_app();
    let mut names: Vec<String> = app.model.datasets().iter().map(|d| d.name.clone()).collect();
    names.sort();
    assert!(names.len() >= 4, "fixture has too few datasets");
    // alphabetically last, so name order alone would put them at the bottom
    let (x, y, z) = (names[names.len() - 1].clone(), names[names.len() - 2].clone(), names[0].clone());

    ds_tick(&mut app, &[(&x, 500.0), (&y, 100.0)]);
    assert_eq!(ds_order(&app)[..2], [x.clone(), y.clone()]);

    // both go idle: they stay on top, in the same order
    ds_tick(&mut app, &[]);
    assert_eq!(ds_order(&app)[..2], [x.clone(), y.clone()]);

    // another dataset becomes active: it goes above, the idle ones keep their order below it
    ds_tick(&mut app, &[(&z, 10.0)]);
    assert_eq!(ds_order(&app)[..3], [z.clone(), x.clone(), y.clone()]);

    // and when it goes idle too, it stays at the top of the idle ones
    ds_tick(&mut app, &[]);
    ds_tick(&mut app, &[]);
    assert_eq!(ds_order(&app)[..3], [z, x, y]);
}

#[test]
fn datasets_changing_the_sort_forgets_the_old_order() {
    let mut app = ds_app();
    let x = app.model.datasets().iter().map(|d| d.name.clone()).max().unwrap();
    ds_tick(&mut app, &[(&x, 500.0)]);
    ds_tick(&mut app, &[]);
    assert_eq!(ds_order(&app)[0], x);
    app.sort_datasets_by(DsSort::Rops);
    let mut by_name = ds_order(&app);
    by_name.sort();
    assert_eq!(ds_order(&app), by_name, "all idle: a fresh sort falls back to name");
}

// ------------------------------------------------------------------------------------------------ topology

/// The largest set of sibling leaf vdevs in the fixture: (pool, names in pool order).
fn topo_siblings(app: &App) -> (String, Vec<String>) {
    let rows = app.topo_rows();
    let mut best: (String, Vec<String>) = (String::new(), Vec::new());
    for (i, r) in rows.iter().enumerate() {
        if !r.has_children {
            continue;
        }
        let kids: Vec<String> = rows[i + 1..]
            .iter()
            .take_while(|c| c.depth > r.depth)
            .filter(|c| c.depth == r.depth + 1 && !c.has_children)
            .map(|c| c.name.clone())
            .collect();
        if kids.len() > best.1.len() {
            best = (r.pool.clone(), kids);
        }
    }
    best
}

fn topo_tick(app: &mut App, pool: &str, active: &[(&str, f64)]) {
    for row in app.model.iostat.values_mut() {
        row.wops = 0.0;
    }
    for &(n, w) in active {
        app.model.iostat.insert(vkey(pool, n), Row { name: n.into(), wops: w, ..Default::default() });
    }
    app.record_sort_orders();
}

/// Sibling order (among `sibs`) as the topology currently shows it.
fn topo_order(app: &App, sibs: &[String]) -> Vec<String> {
    app.topo_rows().into_iter().map(|r| r.name).filter(|n| sibs.contains(n)).collect()
}

#[test]
fn topology_idle_vdev_stays_on_top_of_idle_siblings() {
    let mut app = app();
    let (pool, sibs) = topo_siblings(&app);
    assert!(sibs.len() >= 3, "fixture has no vdev with 3+ leaves");
    app.topo_sort = TopoSort::Wops;
    app.topo_desc = true;
    let last = sibs.last().unwrap().clone();

    topo_tick(&mut app, &pool, &[(&last, 300.0)]);
    assert_eq!(topo_order(&app, &sibs)[0], last);

    // idle: stays first among its siblings instead of going back to its pool-order slot (last)
    topo_tick(&mut app, &pool, &[]);
    let o = topo_order(&app, &sibs);
    assert_eq!(o[0], last);
    assert_eq!(o[1..], sibs[..sibs.len() - 1], "the other idle siblings keep pool order");
}

// ------------------------------------------------------------------------------------------------ disks

fn disk_tick(app: &mut App, active: &[(&str, f64)]) {
    let wholes: Vec<String> = app.model.disks.iter().map(|d| d.whole.clone()).collect();
    app.model.disk_rates = wholes.into_iter().map(|w| (w, DiskRate::default())).collect();
    for &(w, util) in active {
        app.model.disk_rates.insert(w.to_string(), DiskRate { util, ..Default::default() });
    }
    app.record_sort_orders();
}

fn disk_order(app: &App) -> Vec<String> {
    app.disk_rows().into_iter().map(|i| app.model.disks[i].whole.clone()).collect()
}

#[test]
fn disks_idle_disk_stays_on_top_of_idle_ones() {
    let mut app = app();
    assert!(app.model.disks.len() >= 3, "fixture has too few disks");
    app.sort_disks_by(DiskSort::Util);
    // last in pool order, so it would drop to the bottom without the sticky order
    let last = app.model.disks.last().unwrap().whole.clone();

    disk_tick(&mut app, &[(&last, 90.0)]);
    assert_eq!(disk_order(&app)[0], last);

    disk_tick(&mut app, &[]);
    assert_eq!(disk_order(&app)[0], last);
}
