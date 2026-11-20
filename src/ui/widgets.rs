//! Hand-rolled drawing primitives: braille line graphs (mirrored read/write), meters, stacked bars, heatmaps.
//! All draw straight into the buffer so they compose inside any layout.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;

use super::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// filled from the bottom up
    Up,
    /// filled from the top down (the lower half of a mirrored graph)
    Down,
}

/// Samples of history a graph `width` cells wide shows (braille: two per cell).
pub fn samples(width: u16) -> usize {
    width as usize * 2
}

/// Braille dot bit for (x in 0..2, y in 0..4), y = 0 at the top of the cell.
const DOT: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// Dots of one braille line series over `r` (one u8 per cell). Two samples per cell, consecutive samples joined
/// by a vertical run so steps stay connected; zero sits on the baseline (bottom for Up, top for Down); NaN breaks.
fn line_bits(r: Rect, data: &[f64], max: f64, dir: Dir) -> Vec<u8> {
    let cols = r.width as usize * 2;
    let rows = r.height as usize * 4;
    let w = r.width as usize;
    let mut cells = vec![0u8; w * r.height as usize];
    if max <= 0.0 || rows == 0 {
        return cells;
    }
    let start = data.len().saturating_sub(cols);
    let pts = &data[start..];
    let off = cols - pts.len();
    let level = |v: f64| ((v.max(0.0) / max) * (rows - 1) as f64).round().clamp(0.0, (rows - 1) as f64) as usize;
    let mut prev: Option<usize> = None;
    for (i, &v) in pts.iter().enumerate() {
        if !v.is_finite() {
            prev = None;
            continue;
        }
        // any non-zero value lifts off the baseline by at least one dot
        let lv = if v > 0.0 { level(v).max(1).min(rows - 1) } else { 0 };
        let y = match dir {
            Dir::Up => rows - 1 - lv,
            Dir::Down => lv,
        };
        let x = off + i;
        let (lo, hi) = match prev {
            Some(p) => (p.min(y), p.max(y)),
            None => (y, y),
        };
        for yy in lo..=hi {
            cells[(yy / 4) * w + x / 2] |= DOT[x % 2][yy % 4];
        }
        prev = Some(y);
    }
    cells
}

fn braille_line(buf: &mut Buffer, r: Rect, data: &[f64], max: f64, color: &dyn Fn(usize, f64) -> Color, dir: Dir) {
    let c = data.iter().rev().find(|v| v.is_finite()).map(|&v| color(data.len() - 1, v)).unwrap_or(Color::Reset);
    let bits = line_bits(r, data, max, dir);
    let w = r.width as usize;
    for cy in 0..r.height as usize {
        for cx in 0..w {
            let b = bits[cy * w + cx];
            if b == 0 {
                continue;
            }
            if let Some(cell) = buf.cell_mut((r.x + cx as u16, r.y + cy as u16)) {
                cell.set_char(char::from_u32(0x2800 + b as u32).unwrap_or(' ')).set_fg(c);
            }
        }
    }
}

/// Braille line graph in one colour, rising from the bottom (Up) or falling from the top (Down).
pub fn area(buf: &mut Buffer, r: Rect, data: &[f64], max: f64, color: Color, dir: Dir) {
    if r.width > 0 && r.height > 0 && max > 0.0 {
        braille_line(buf, r, data, max, &|_, _| color, dir);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn mirror_c(buf: &mut Buffer, r: Rect, up: &[f64], down: &[f64], umax: f64, dmax: f64, uc: Color, dc: Color) {
    if r.height < 2 {
        area(buf, r, up, umax, uc, Dir::Up);
        return;
    }
    let top = r.height.div_ceil(2);
    let a = Rect { height: top, ..r };
    let b = Rect { y: r.y + top, height: r.height - top, ..r };
    area(buf, a, up, umax, uc, Dir::Up);
    area(buf, b, down, dmax, dc, Dir::Down);
}

/// Braille line graph whose colour follows the newest sample (`color(index, value)`), e.g. TXG sync times.
pub fn bars(buf: &mut Buffer, r: Rect, data: &[f64], max: f64, color: impl Fn(usize, f64) -> Color) {
    if r.width > 0 && r.height > 0 && max > 0.0 {
        braille_line(buf, r, data, max, &color, Dir::Up);
    }
}

/// Small text label drawn on top of a graph (on the terminal background, so it stays legible over filled columns).
pub fn label(buf: &mut Buffer, x: u16, y: u16, text: &str, color: Color) {
    for (i, ch) in text.chars().enumerate() {
        if let Some(c) = buf.cell_mut((x + i as u16, y)) {
            c.set_char(ch).set_fg(color).set_bg(Color::Reset);
        }
    }
}

/// Thin meter: `━━━━────` coloured by where each cell sits against warn/crit (a fuller pool turns red at the end).
pub fn meter(w: usize, frac: f64, warn: f64, crit: f64) -> Vec<Span<'static>> {
    let frac = if frac.is_finite() { frac.clamp(0.0, 1.0) } else { 0.0 };
    let filled = (frac * w as f64).round() as usize;
    let filled = if frac > 0.0 { filled.max(1) } else { 0 };
    let mut out = Vec::new();
    for i in 0..w {
        let pos = (i as f64 + 0.5) / w as f64;
        if i < filled {
            let c = if pos >= crit {
                theme::CRIT
            } else if pos >= warn {
                theme::WARN
            } else {
                theme::OK
            };
            out.push(Span::styled("━", Style::new().fg(c)));
        } else {
            out.push(Span::styled("─", Style::new().fg(theme::TRACK)));
        }
    }
    merge(out)
}

/// Single-colour gauge (for amounts where "full" is fine, like the ARC at its target).
pub fn gauge(w: usize, frac: f64, color: Color) -> Vec<Span<'static>> {
    let frac = if frac.is_finite() { frac.clamp(0.0, 1.0) } else { 0.0 };
    let filled = (frac * w as f64).round() as usize;
    vec![
        Span::styled("━".repeat(filled), Style::new().fg(color)),
        Span::styled("─".repeat(w - filled), Style::new().fg(theme::TRACK)),
    ]
}

/// Stacked bar of parts scaled to `total`; each part has its own glyph so it reads without colour.
pub fn stacked(w: usize, parts: &[(f64, char, Color)], total: f64) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    if total <= 0.0 || w == 0 {
        return vec![Span::styled("·".repeat(w), Style::new().fg(theme::TRACK))];
    }
    let mut used = 0usize;
    let mut acc = 0.0;
    for &(v, ch, color) in parts {
        if v <= 0.0 {
            continue;
        }
        acc += v;
        let end = ((acc / total) * w as f64).round().min(w as f64) as usize;
        let end = end.max(used + 1).min(w); // every non-zero part shows at least one cell
        if end > used {
            out.push(Span::styled(ch.to_string().repeat(end - used), Style::new().fg(color)));
            used = end;
        }
    }
    if used < w {
        out.push(Span::styled("·".repeat(w - used), Style::new().fg(theme::TRACK)));
    }
    out
}

fn merge(spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    for s in spans {
        match out.last_mut() {
            Some(l) if l.style == s.style => l.content = format!("{}{}", l.content, s.content).into(),
            _ => out.push(s),
        }
    }
    out
}

/// Heat colour for an intensity in 0..=1 (dark blue → cyan → yellow → red), from the xterm-256 palette.
pub fn heat(x: f64) -> Color {
    const RAMP: [u8; 10] = [17, 18, 25, 31, 37, 79, 149, 220, 208, 196];
    if !x.is_finite() || x <= 0.0 {
        return theme::BG_DIM;
    }
    let i = ((x.clamp(0.0, 1.0)) * (RAMP.len() - 1) as f64).round() as usize;
    Color::Indexed(RAMP[i.max(1)])
}

/// Heatmap: columns = time (newest right), rows = buckets (highest bucket at the top), two buckets per cell via `▀`.
/// Intensity is log-scaled per column max, so both a busy and a quiet interval show their shape.
pub fn heatmap(buf: &mut Buffer, r: Rect, cols: &[Vec<u32>], lo: usize, hi: usize) {
    if r.width == 0 || r.height == 0 || hi <= lo {
        return;
    }
    let nb = hi - lo;
    let rows = r.height as usize * 2;
    let w = r.width as usize;
    let start = cols.len().saturating_sub(w);
    let pts = &cols[start..];
    let off = w - pts.len();
    // global max over the window, log scale
    let gmax = pts.iter().flat_map(|c| c.iter().skip(lo).take(nb)).copied().max().unwrap_or(0) as f64;
    let lmax = (gmax + 1.0).ln();
    for cy in 0..r.height as usize {
        for cx in 0..w {
            let cell = buf.cell_mut((r.x + cx as u16, r.y + cy as u16));
            let Some(cell) = cell else { continue };
            let val = |sub: usize| -> f64 {
                let Some(col) = cx.checked_sub(off).and_then(|i| pts.get(i)) else { return 0.0 };
                // row 0 = top = highest bucket
                let ry = cy * 2 + sub;
                let b0 = lo + nb - 1 - (ry * nb / rows).min(nb - 1);
                let b1 = lo + nb - 1 - (((ry + 1) * nb).div_ceil(rows) - 1).min(nb - 1);
                let (a, b) = (b0.min(b1), b0.max(b1));
                let s: u64 = (a..=b).map(|i| col.get(i).copied().unwrap_or(0) as u64).sum();
                if s == 0 || lmax <= 0.0 { 0.0 } else { (s as f64 + 1.0).ln() / lmax }
            };
            let (t, b) = (val(0), val(1));
            cell.set_char('▀').set_fg(heat(t)).set_bg(heat(b));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(buf: &Buffer) -> String {
        let a = buf.area;
        (0..a.height)
            .map(|y| (0..a.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn line_connects_steps() {
        let r = Rect::new(0, 0, 2, 1);
        let mut b = Buffer::empty(r);
        // rows = 4 → levels 0..3; 0 on the baseline, 3 at the top; the jump 0→3 is joined vertically
        braille_line(&mut b, r, &[0.0, 0.0, 3.0, 3.0], 3.0, &|_, _| Color::Blue, Dir::Up);
        assert_eq!(text(&b), "⣀⡏".replace('⡏', &char::from_u32(0x2800 + 0x47 + 0x08).unwrap().to_string()));
    }

    #[test]
    fn meter_and_stack() {
        let m: String = meter(10, 0.5, 0.8, 0.9).iter().map(|s| s.content.to_string()).collect();
        assert_eq!(m, "━━━━━─────");
        let s: String = stacked(10, &[(1.0, '█', Color::Blue), (0.001, '▓', Color::Red)], 2.0)
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(s, "█████▓····");
    }
}
