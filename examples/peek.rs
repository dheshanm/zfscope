//! Render one frame from a fixture, for eyeballing layouts and for screenshots.
//!
//!   FIXTURE=tests/fixtures/local/<host> TICKS=60 KEYS="2" COLS=160 ROWS=48 cargo run --example peek
//!   FORMAT=html ... > frame.html
//!
//! KEYS is a space-separated list of key names: single characters, Enter Esc Backspace Tab Up Down Left Right PgDn
//! PgUp Space, or `click:X,Y` for a left click at that cell.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};

use zfscope::app::App;
use zfscope::replay::Replay;

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
        "PgDn" => KeyCode::PageDown,
        "PgUp" => KeyCode::PageUp,
        "Space" => KeyCode::Char(' '),
        s => KeyCode::Char(s.chars().next().unwrap_or(' ')),
    })
}

fn env(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

fn main() {
    let Ok(dir) = std::env::var("FIXTURE") else {
        eprintln!("set FIXTURE=<capture dir> (see scripts/capture-fixture.sh)");
        std::process::exit(2);
    };
    let ticks: usize = env("TICKS", "60").parse().unwrap();
    let cols: u16 = env("COLS", "160").parse().unwrap();
    let rows: u16 = env("ROWS", "48").parse().unwrap();
    let mut r = Replay::new(std::path::Path::new(&dir)).expect("fixture");
    let mut app = App::new(env("NAME", "demo-host"), "fixture".into(), Duration::from_secs(1));
    r.init(&mut app);
    r.run(&mut app, ticks);
    let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
    for k in env("KEYS", "").split_whitespace() {
        if let Some(xy) = k.strip_prefix("click:") {
            // draw first so the click targets exist, then click (left button) at x,y
            term.draw(|f| zfscope::ui::draw(f, &mut app)).unwrap();
            let (x, y) = xy.split_once(',').unwrap();
            app.handle_mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: x.parse().unwrap(),
                row: y.parse().unwrap(),
                modifiers: crossterm::event::KeyModifiers::NONE,
            });
        } else {
            app.handle_key(key(k));
        }
        r.serve(&mut app);
    }
    term.draw(|f| zfscope::ui::draw(f, &mut app)).unwrap();
    let buf = term.backend().buffer().clone();
    if env("FORMAT", "text") == "html" {
        print!("{}", html(&buf));
    } else {
        for y in 0..rows {
            let line: String = (0..cols).map(|x| buf[(x, y)].symbol().to_string()).collect();
            println!("{}", line.trim_end());
        }
    }
}

fn rgb(c: Color, fg: bool) -> String {
    let xterm = |i: u8| -> (u8, u8, u8) {
        const BASE: [(u8, u8, u8); 16] = [
            (0, 0, 0),
            (205, 49, 49),
            (13, 188, 121),
            (229, 229, 16),
            (36, 114, 200),
            (188, 63, 188),
            (17, 168, 205),
            (229, 229, 229),
            (102, 102, 102),
            (241, 76, 76),
            (35, 209, 139),
            (245, 245, 67),
            (59, 142, 234),
            (214, 112, 214),
            (41, 184, 219),
            (255, 255, 255),
        ];
        match i {
            0..=15 => BASE[i as usize],
            16..=231 => {
                let i = i - 16;
                let s = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                (s(i / 36), s((i / 6) % 6), s(i % 6))
            }
            _ => {
                let g = 8 + (i - 232) * 10;
                (g, g, g)
            }
        }
    };
    let (r, g, b) = match c {
        Color::Reset => return if fg { "#d4d4d4".into() } else { "#1a1a1a".into() },
        Color::Black => (0, 0, 0),
        Color::White => (255, 255, 255),
        Color::DarkGray => (102, 102, 102),
        Color::Gray => (188, 188, 188),
        Color::Indexed(i) => xterm(i),
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (212, 212, 212),
    };
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// SVG for a braille cell: dot bits 1-8 laid out as a 2×4 grid, drawn in the current text colour.
fn braille_svg(bits: u32) -> String {
    const POS: [(u32, u32); 8] = [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (0, 3), (1, 3)];
    let mut dots = String::new();
    for (i, (x, y)) in POS.iter().enumerate() {
        if bits & (1 << i) != 0 {
            dots.push_str(&format!("<circle cx='{}' cy='{}' r='0.36'/>", 0.5 + *x as f32, 0.5 + *y as f32));
        }
    }
    format!(
        "<svg viewBox='0 0 2 4' width='100%' height='100%' preserveAspectRatio='none' fill='currentColor' style='display:block'>{dots}</svg>"
    )
}

fn html(buf: &ratatui::buffer::Buffer) -> String {
    let mut s = String::from(
        "<!doctype html><meta charset=utf-8><style>body{margin:0;background:#1a1a1a}pre{margin:0;padding:8px;font:13px/1.2 Menlo,'DejaVu Sans Mono',monospace;color:#d4d4d4}pre span{display:inline-block;width:1ch;height:1.2em;overflow:hidden;vertical-align:top}</style><pre>",
    );
    let a = buf.area;
    for y in 0..a.height {
        for x in 0..a.width {
            let c = &buf[(x, y)];
            let sym = match c.symbol() {
                "<" => "&lt;",
                ">" => "&gt;",
                "&" => "&amp;",
                s => s,
            };
            let bold = if c.modifier.contains(Modifier::BOLD) { "font-weight:bold;" } else { "" };
            let style = format!("color:{};background:{};{bold}", rgb(c.fg, true), rgb(c.bg, false));
            // Braille: browsers lack a monospace braille font (tiny fallback glyphs), so draw the dots ourselves
            match sym.chars().next().map(|ch| ch as u32) {
                Some(cp @ 0x2801..=0x28ff) => {
                    s.push_str(&format!("<span style=\"{style}\">{}</span>", braille_svg(cp - 0x2800)))
                }
                _ => s.push_str(&format!("<span style=\"{style}\">{sym}</span>")),
            }
        }
        s.push('\n');
    }
    s.push_str("</pre>");
    s
}
