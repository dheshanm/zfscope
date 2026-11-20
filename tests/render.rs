//! Render every view from the demo fixture and compare with stored snapshots (tests/snapshots/).
//!
//! Deterministic: the fixture replays a fixed number of ticks, the clock is pinned to the capture time (UTC), and
//! the header line (which carries the build number) is left out. Review changes with `cargo insta review`, or accept
//! them all with `INSTA_UPDATE=always cargo test --test render`.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use zfscope::app::App;
use zfscope::replay::Replay;

const COLS: u16 = 160;
const ROWS: u16 = 48;

fn key(name: &str) -> KeyEvent {
    if name == "Ctrl-U" {
        return KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
    }
    KeyEvent::from(match name {
        "Enter" => KeyCode::Enter,
        "Esc" => KeyCode::Esc,
        "Backspace" => KeyCode::Backspace,
        "Tab" => KeyCode::Tab,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        s => KeyCode::Char(s.chars().next().unwrap()),
    })
}

/// Replay `ticks` samples, press `keys` (space-separated; `click:X,Y` clicks), render, return the screen as text
/// without the header line.
fn render(keys: &str, ticks: usize) -> String {
    render_with(keys, ticks, false, |_| {})
}

/// `render` with write mode available (fixture: dry run). Draws before every key, like the real loop does, since
/// write mode acts on the property the last frame showed as selected. `setup` runs before the keys.
fn render_rw(keys: &str, setup: impl FnOnce(&mut App)) -> String {
    render_with(keys, 10, true, setup)
}

fn render_with(keys: &str, ticks: usize, can_write: bool, setup: impl FnOnce(&mut App)) -> String {
    let mut r = Replay::new(std::path::Path::new("tests/fixtures/demo")).unwrap();
    let mut app = App::new("zfs-demo".into(), "fixture".into(), Duration::from_secs(1));
    app.can_write = can_write;
    r.init(&mut app);
    r.run(&mut app, ticks);
    setup(&mut app);
    let mut term = Terminal::new(TestBackend::new(COLS, ROWS)).unwrap();
    for k in keys.split_whitespace() {
        if can_write {
            term.draw(|f| zfscope::ui::draw(f, &mut app)).unwrap();
        }
        if let Some(xy) = k.strip_prefix("click:") {
            term.draw(|f| zfscope::ui::draw(f, &mut app)).unwrap();
            let (x, y) = xy.split_once(',').unwrap();
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x.parse().unwrap(),
                row: y.parse().unwrap(),
                modifiers: KeyModifiers::NONE,
            });
        } else {
            app.handle_key(key(k));
        }
        r.serve(&mut app);
    }
    term.draw(|f| zfscope::ui::draw(f, &mut app)).unwrap();
    let buf = term.backend().buffer().clone();
    (1..ROWS)
        .map(|y| (0..COLS).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

macro_rules! view {
    ($name:ident, $keys:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!(render($keys, 30));
        }
    };
}

view!(overview, "1");
view!(pool_io, "2 j");
view!(pool_latency_sorted, "2 m click:62,2");
view!(pool_queues, "2 m m");
view!(pool_space, "2 m m m");
view!(pool_disk_grid, "2 d");
view!(pool_disk_grid_util, &format!("2 d{}", " Down".repeat(9)));
view!(pool_disk_grid_temp, &format!("2 d{}", " Down".repeat(12)));

#[test]
fn disk_grid_metric_panel() {
    // every metric is listed, under its section; arrows clamp at the ends; a click picks one
    let s = render("2 d", 30);
    for m in [
        "latency",
        "IO",
        "load",
        "health",
        "disk wait read",
        "disk wait write",
        "total wait",
        "IOPS",
        "read ops/s",
        "write ops/s",
        "read bandwidth",
        "write bandwidth",
        "util %",
        "active queue",
        "errors",
        "temperature",
        "defects",
        "SSD wear",
    ] {
        assert!(s.contains(m), "{m} missing:\n{s}");
    }
    assert!(render("2 d Up", 30).contains("disk grid · disk wait ("));
    assert!(render(&format!("2 d{}", " Down".repeat(20)), 30).contains("disk grid · SSD wear"));
    assert!(render("2 d c", 30).contains("disk grid · disk wait read"));
    // metric panel (right column): screen row 1 is its top border, 2 the column header, 3 the "latency" heading
    let s = render("2 d click:140,5", 30);
    assert!(s.contains("disk grid · disk wait read"), "{s}");
    // …4 latency rows, IO heading + 5, load heading + 2, health heading, errors, then temperature on row 19
    let s = render("2 d click:140,19", 30);
    assert!(s.contains("disk grid · temperature"), "{s}");
    // a click on a section heading changes nothing
    assert!(render("2 d click:140,8", 30).contains("disk grid · disk wait ("));
}
view!(pool_history, "2 H");
view!(datasets_tree, "3");
view!(datasets_ztop, "3 t");
view!(datasets_space, "3 j Enter");
view!(datasets_snapshots, "3 / b u i l d Enter Enter ] ]");
view!(datasets_properties_focus, "3 j Enter ] ] ] Tab j j j");
view!(arc, "4");
view!(zil_txg, "5");
view!(disks, "6");
view!(disks_sorted_by_util, "6 s");
view!(disk_detail, "6 j Enter");
view!(props_pool, "7");
view!(props_dataset_about, "7 j j Enter j j j j j j j j j Enter");
view!(props_feature_flag, "7 Enter / h e a d _ Enter Enter");
view!(help_overlay, "2 ?");

/// The property named on the first line of the "about this property" pane.
fn about_name(screen: &str) -> String {
    let lines: Vec<&str> = screen.lines().collect();
    let i = lines.iter().position(|l| l.contains("about this property")).expect("about pane");
    // the about pane is the right-most bordered column: last non-empty cell between borders
    lines[i + 1].split(['│', '┃']).map(str::trim).rfind(|s| !s.is_empty()).unwrap_or("").to_string()
}

#[test]
fn same_property_follows_dataset_switch() {
    // pick a property on one dataset, go back to the dataset list, move to the next dataset: same property
    let before = about_name(&render("7 j Enter j j j j j j j j j j j", 10));
    let after = about_name(&render("7 j Enter j j j j j j j j j j j Backspace j", 10));
    assert!(!before.is_empty());
    assert_eq!(before, after);
}

// ---------------------------------------------------------------- write mode (fixture: dry run)

/// Props tab, dataset pool0/data/home, filtered to its compression property.
const HOME_COMPRESSION: &str = "7 / h o m e Enter Enter / c o m p r e s s i o n Enter";

#[test]
fn rw_unlock_prompt() {
    insta::assert_snapshot!(render_rw("W", |_| {}));
}

#[test]
fn rw_edit_compression() {
    insta::assert_snapshot!(render_rw(&format!("W y {HOME_COMPRESSION} e Down Down Down Down"), |_| {}));
}

#[test]
fn rw_confirm_compression() {
    insta::assert_snapshot!(render_rw(&format!("W y {HOME_COMPRESSION} e Down Down Down Enter"), |_| {}));
}

#[test]
fn rw_dry_run_result() {
    let s = render_rw(&format!("W y {HOME_COMPRESSION} e Down Down Down Enter y"), |_| {});
    assert!(s.contains("✓ zfs set compression=lz4 pool0/data/home · dry run (fixture)"), "{s}");
}

#[test]
fn rw_locked_does_nothing() {
    // without W, e only says how to unlock; nothing is queued
    let s = render_rw(&format!("{HOME_COMPRESSION} e"), |_| {});
    assert!(s.contains("write mode is off: W unlocks it"), "{s}");
    assert!(!s.contains("possible values (↑↓ pick)"));
}

#[test]
fn rw_needs_root() {
    let s = render("W", 5);
    assert!(s.contains("write mode needs root"), "{s}");
}

#[test]
fn rw_unchanged_and_inherit_refusals() {
    // submitting the current value does nothing
    let s = render_rw(&format!("W y {HOME_COMPRESSION} e Enter"), |_| {});
    assert!(s.contains("compression unchanged"), "{s}");
    // an inherited property can't be "reset"
    let s = render_rw("W y 7 / p r o j e c t s Enter Enter / c o m p r e s s i o n Enter i", |_| {});
    assert!(s.contains("compression is already inherited from pool0/data/home"), "{s}");
    // read-only statistics can't be set
    let s = render_rw("W y 7 / h o m e Enter Enter / u s e d b y s n a p Enter e", |_| {});
    assert!(s.contains("usedbysnapshots is read-only"), "{s}");
}

#[test]
fn rw_add_user_property() {
    let s = render_rw("W y 7 / h o m e Enter Enter a o r g . x : n o t e Enter h i Enter", |_| {});
    insta::assert_snapshot!(s);
    assert!(s.contains("zfs set org.x:note=hi pool0/data/home"));
}

#[test]
fn rw_feature_flag_needs_pool_name() {
    use zfscope::app::{ConfirmState, Modal};
    use zfscope::write::{Change, Target};
    let open = |app: &mut App| {
        app.rw = true;
        app.modal = Some(Modal::Confirm(ConfirmState {
            change: Change::Set {
                target: Target::Pool("pool0".into()),
                key: "feature@fast_dedup".into(),
                value: "enabled".into(),
            },
            old: None,
            typed: String::new(),
            error: None,
        }));
    };
    // y alone doesn't confirm a one-way change
    let s = render_rw("y Enter", open);
    assert!(s.contains("type the pool name exactly"), "{s}");
    insta::assert_snapshot!(s);
    let s = render_rw("p o o l 0 Enter", open);
    assert!(s.contains("✓ zpool set feature@fast_dedup=enabled pool0"), "{s}");
}

#[test]
fn renders_at_small_sizes_without_panicking() {
    for (c, r) in [(80, 24), (100, 30), (60, 20)] {
        for keys in ["1", "2", "2 d", "3", "4", "5", "6", "7"] {
            let mut rp = Replay::new(std::path::Path::new("tests/fixtures/demo")).unwrap();
            let mut app = App::new("zfs-demo".into(), "fixture".into(), Duration::from_secs(1));
            rp.init(&mut app);
            rp.run(&mut app, 8);
            for k in keys.split_whitespace() {
                app.handle_key(key(k));
            }
            let mut term = Terminal::new(TestBackend::new(c, r)).unwrap();
            term.draw(|f| zfscope::ui::draw(f, &mut app)).unwrap();
        }
    }
}

#[test]
fn rw_from_datasets_properties_pane() {
    // Datasets ▸ detail ▸ Properties pane, focused: e edits the highlighted property of the selected dataset
    let s = render_rw("W y 3 j Enter ] ] ] Tab j j j e", |_| {});
    assert!(s.contains("· dataset pool0/data"), "{s}");
    assert!(s.contains("Enter review the command"), "{s}");
}

// ---------------------------------------------------------------- write mode: tunables

#[test]
fn rw_tune_edit_arc_max() {
    // ARC tab (4), zfs_arc_max (first entry): description, default, range, effects, and the size typed with a suffix
    let s = render_rw("W y 4 e Ctrl-U 2 5 6 G", |_| {});
    insta::assert_snapshot!(s);
    assert!(s.contains("set zfs_arc_max · ZFS module parameter"), "{s}");
    assert!(s.contains("= 274877906944 (256G)"), "{s}");
    assert!(s.contains("from /etc/modprobe.d/zfs.conf"), "{s}");
}

#[test]
fn rw_tune_scope_and_confirm() {
    // now + at boot: the sysfs write and the modprobe.d line, old → new
    let s = render_rw("W y 4 e Ctrl-U 2 5 6 G Tab Enter", |_| {});
    insta::assert_snapshot!(s);
    assert!(s.contains("echo 274877906944 > /sys/module/zfs/parameters/zfs_arc_max"), "{s}");
    assert!(s.contains("- options zfs zfs_arc_max=343597383680 zfs_arc_sys_free=34359738368"), "{s}");
    assert!(s.contains("+ options zfs zfs_arc_max=274877906944 zfs_arc_sys_free=34359738368"), "{s}");
    // at boot only: nothing written to sysfs
    let s = render_rw("W y 4 e Ctrl-U 2 5 6 G Tab Tab Enter", |_| {});
    assert!(!s.contains("echo 274877906944"), "{s}");
    assert!(s.contains("the running value stays"), "{s}");
}

#[test]
fn rw_tune_dry_run_result() {
    let s = render_rw("W y 5 e Ctrl-U 1 0 Enter y", |_| {});
    assert!(s.contains("✓ zfs_txg_timeout = 10 now (until reboot) · dry run (fixture)"), "{s}");
}

#[test]
fn rw_tune_validation() {
    // below the 64 MiB minimum
    let s = render_rw("W y 4 e Ctrl-U 1 M Enter", |_| {});
    assert!(s.contains("must be at least 64.0M"), "{s}");
    // not a number
    let s = render_rw("W y 5 e Ctrl-U x Enter", |_| {});
    assert!(s.contains("is not a whole number"), "{s}");
    // the current value, now only: nothing to do
    let s = render_rw("W y 5 e Enter", |_| {});
    assert!(s.contains("zfs_txg_timeout unchanged"), "{s}");
}

#[test]
fn rw_tune_reset_and_locked() {
    // zfs_arc_meta_balance (3rd ARC entry) is 2000 now and at boot; reset → 500 now, boot setting removed
    let s = render_rw("W y 4 j j i", |_| {});
    assert!(s.contains("echo 500 > /sys/module/zfs/parameters/zfs_arc_meta_balance"), "{s}");
    assert!(s.contains("- options zfs zfs_arc_meta_balance=2000"), "{s}");
    assert!(s.contains("(line removed)"), "{s}");
    // already at its default
    let s = render_rw("W y 5 i", |_| {});
    assert!(s.contains("zfs_txg_timeout is already at its default"), "{s}");
    // locked: e only says how to unlock
    let s = render_rw("4 e", |_| {});
    assert!(s.contains("write mode is off: W unlocks it"), "{s}");
}

#[test]
fn rw_tune_risky_value() {
    // zil_nocacheflush: pick 1 from the list; the confirm dialog warns
    let s = render_rw(&format!("W y 5{} e Down Down Enter", " j".repeat(11)), |_| {});
    assert!(s.contains("zil_nocacheflush=1: a power loss can lose or corrupt"), "{s}");
}
