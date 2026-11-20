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
    let mut r = Replay::new(std::path::Path::new("tests/fixtures/demo")).unwrap();
    let mut app = App::new("zfs-demo".into(), "fixture".into(), Duration::from_secs(1));
    r.init(&mut app);
    r.run(&mut app, ticks);
    let mut term = Terminal::new(TestBackend::new(COLS, ROWS)).unwrap();
    for k in keys.split_whitespace() {
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

#[test]
fn renders_at_small_sizes_without_panicking() {
    for (c, r) in [(80, 24), (100, 30), (60, 20)] {
        for tab in ["1", "2", "3", "4", "5", "6", "7"] {
            let mut rp = Replay::new(std::path::Path::new("tests/fixtures/demo")).unwrap();
            let mut app = App::new("zfs-demo".into(), "fixture".into(), Duration::from_secs(1));
            rp.init(&mut app);
            rp.run(&mut app, 8);
            app.handle_key(key(tab));
            let mut term = Terminal::new(TestBackend::new(c, r)).unwrap();
            term.draw(|f| zfscope::ui::draw(f, &mut app)).unwrap();
        }
    }
}
