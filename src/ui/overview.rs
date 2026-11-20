//! Overview: per pool, a health/capacity card beside IOPS, bandwidth and latency graphs; then ARC, ZIL, TXG and
//! the busiest datasets.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::widgets::{self, Dir};
use super::{graph_title, inner, kv, panel, theme};
use crate::app::App;
use crate::fmt;
use crate::model::{Model, vkey};
use crate::parse::zjson::{PoolStatus, PropsExt};

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let m = &app.model;
    let pools = m.pools();
    if pools.is_empty() {
        let msg = if m.config.is_none() { "reading pool configuration…" } else { "no pools imported" };
        f.render_widget(Paragraph::new(msg).style(theme::dim()).block(panel("pools")), area);
        return;
    }
    let n = pools.len().min(3) as u32;
    let bottom_h = if area.height >= 30 { 12 } else { 9 };
    let [top, bottom] = Layout::vertical([Constraint::Min(10), Constraint::Length(bottom_h)]).areas(area);
    let rows = Layout::vertical((0..n).map(|_| Constraint::Ratio(1, n))).split(top);
    for (p, r) in pools.iter().zip(rows.iter()) {
        pool_row(f, m, p, *r);
    }
    let [arc, zil, txg, top_ds] = Layout::horizontal([
        Constraint::Percentage(25),
        Constraint::Percentage(22),
        Constraint::Percentage(22),
        Constraint::Percentage(31),
    ])
    .areas(bottom);
    arc_card(f, m, arc);
    zil_card(f, m, zil);
    txg_card(f, m, &pools[0].name, txg);
    top_datasets(f, m, top_ds);
}

fn pool_row(f: &mut Frame, m: &Model, p: &PoolStatus, r: Rect) {
    let [card, graphs] = Layout::horizontal([Constraint::Length(46.min(r.width / 2)), Constraint::Min(20)]).areas(r);
    pool_card(f, m, p, card);
    let k = vkey(&p.name, &p.name);
    let h = graphs.height;
    let (gh, lat_h) = if h >= 14 { ((h - 5) / 2, h - 2 * ((h - 5) / 2)) } else { (h / 2, h - h / 2) };
    let show_lat = h >= 14;
    let cons = if show_lat {
        vec![Constraint::Length(gh), Constraint::Length(gh), Constraint::Length(lat_h)]
    } else {
        vec![Constraint::Length(gh), Constraint::Length(h - gh)]
    };
    let g = Layout::vertical(cons).split(graphs);
    io_graph(f, m, &k, g[0], "IOPS", "rops", "wops", fmt::count);
    io_graph(f, m, &k, g[1], "bandwidth", "rbw", "wbw", fmt::rate);
    if show_lat {
        latency_graph(f, m, &k, g[2]);
    }
}

/// Health, capacity per class, scan state, errors.
fn pool_card(f: &mut Frame, m: &Model, p: &PoolStatus, r: Rect) {
    let w = inner(r).width as usize;
    let mut lines = Vec::new();
    let props = m.pool_list(&p.name).map(|l| &l.props);
    let size = props.map(|p| p.n("size")).unwrap_or(p.root.size);
    let alloc = props.map(|p| p.n("allocated")).unwrap_or(p.root.alloc);
    let free = props.map(|p| p.n("free")).unwrap_or(size.saturating_sub(alloc));
    lines.push(Line::from(
        [
            kv("size", fmt::size(size), None),
            vec![Span::raw("  ")],
            kv("alloc", fmt::size(alloc), None),
            vec![Span::raw("  ")],
            kv("free", fmt::size(free), None),
        ]
        .concat(),
    ));
    let cap = if size > 0 { alloc as f64 / size as f64 } else { 0.0 };
    let bw = w.saturating_sub(6).max(4);
    let mut cap_line = widgets::meter(bw, cap, 0.8, 0.9);
    cap_line.push(Span::styled(format!(" {:>4}", fmt::pct(cap * 100.0)), Style::new().fg(theme::level(cap, 0.8, 0.9))));
    lines.push(Line::from(cap_line));
    // per-class capacity (data vdevs aggregated, then special/dedup/logs)
    let mut classes: Vec<(String, u64, u64, Option<u64>)> = Vec::new();
    let data: Vec<_> = p.root.children.iter().filter(|v| v.vtype != "class").collect();
    if !data.is_empty() {
        let (a, s) = data.iter().fold((0, 0), |(a, s), v| (a + v.alloc, s + v.size));
        let frag = props.and_then(|pp| pp.opt("fragmentation"));
        classes.push(("data".into(), a, s, frag));
    }
    for c in p.root.children.iter().filter(|v| v.vtype == "class" && !matches!(v.name.as_str(), "spares" | "cache")) {
        let frag = c.children.iter().filter_map(|v| v.frag).max();
        classes.push((c.name.clone(), c.alloc, c.size, frag));
    }
    if classes.len() > 1 || classes.iter().any(|c| c.0 != "data") {
        let mw = w.saturating_sub(8 + 6 + 10).max(4);
        for (name, a, s, frag) in &classes {
            let frac = if *s > 0 { *a as f64 / *s as f64 } else { 0.0 };
            let mut l = vec![Span::styled(format!("{:<8}", fmt::fit(name, 8)), theme::dim())];
            l.extend(widgets::meter(mw, frac, 0.8, 0.9));
            l.push(Span::raw(format!(" {:>4}", fmt::pct(frac * 100.0))));
            if let Some(fr) = frag {
                l.push(Span::styled(format!(" frag {fr:>2}%"), Style::new().fg(theme::level(*fr as f64, 50.0, 75.0))));
            }
            lines.push(Line::from(l));
        }
    } else if let Some(fr) = props.and_then(|pp| pp.opt("fragmentation")) {
        lines.push(Line::from(kv("fragmentation", format!("{fr}%"), Some(theme::level(fr as f64, 50.0, 75.0)))));
    }
    // layout summary
    let leaves = p.root.leaves();
    let mut kinds: Vec<(String, usize)> = Vec::new();
    for v in data.iter() {
        let label = if v.children.is_empty() {
            "disk".to_string()
        } else {
            format!("{}×{}", v.name.rsplit_once('-').map(|x| x.0).unwrap_or(&v.name), v.children.len())
        };
        match kinds.iter_mut().find(|(k, _)| *k == label) {
            Some(e) => e.1 += 1,
            None => kinds.push((label, 1)),
        }
    }
    let layout = kinds.iter().map(|(k, n)| format!("{n}× {k}")).collect::<Vec<_>>().join(" + ");
    let dedup = props.map(|p| p.str("dedupratio").to_string()).unwrap_or_default();
    lines.push(Line::from(vec![
        Span::styled(format!("{} devices", leaves.len()), theme::dim()),
        Span::styled(format!(" · {layout}"), theme::dim()),
        Span::styled(
            if dedup.is_empty() || dedup == "1.00" || dedup == "1.00x" {
                String::new()
            } else {
                format!(" · dedup {dedup}")
            },
            theme::dim(),
        ),
    ]));
    // scan
    if let Some(s) = &p.scan {
        lines.push(scan_line(s, w));
    }
    for n in &p.notes {
        lines.push(Line::from(Span::styled(n.clone(), Style::new().fg(theme::WARN))));
    }
    // errors
    let mut err_devs = 0;
    let mut slow = 0;
    p.root.walk(0, &mut |v, _| {
        if v.is_leaf() {
            if v.errors() > 0 {
                err_devs += 1;
            }
            slow += v.slow_ios;
        }
    });
    let bad: Vec<String> = leaves
        .iter()
        .filter(|v| v.state != "ONLINE" && !v.state.is_empty())
        .map(|v| format!("{} {}", v.name, v.state))
        .collect();
    if p.error_count > 0 {
        lines.push(Line::from(Span::styled(
            format!("✗ {} data errors (zpool status -v)", p.error_count),
            Style::new().fg(theme::CRIT),
        )));
    }
    if !bad.is_empty() {
        lines.push(Line::from(Span::styled(format!("✗ {}", bad.join(", ")), Style::new().fg(theme::CRIT))));
    }
    if err_devs > 0 {
        lines.push(Line::from(Span::styled(
            format!("⚠ {err_devs} devices with R/W/CKSUM errors"),
            Style::new().fg(theme::WARN),
        )));
    }
    if slow > 0 {
        lines.push(Line::from(Span::styled(format!("⚠ {slow} slow I/Os"), Style::new().fg(theme::WARN))));
    }
    if p.error_count == 0 && bad.is_empty() && err_devs == 0 && slow == 0 {
        lines.push(Line::from(Span::styled("✓ no errors", Style::new().fg(theme::OK))));
    }
    if let Some(st) = &p.status {
        lines.push(Line::from(Span::styled(st.clone(), Style::new().fg(theme::WARN))));
    }
    // top-level vdevs: capacity and live ops, as far as the card has room
    let room = (inner(r).height as usize).saturating_sub(lines.len() + 1);
    if room >= 2 {
        let mw = w.saturating_sub(12 + 6 + 16 + 2).max(3);
        lines.push(Line::from(Span::styled(
            format!("{:<13}{:>cw$}{:>7}{:>7}", "vdev", "cap", "r/s", "w/s", cw = mw + 5),
            theme::dim().add_modifier(Modifier::UNDERLINED),
        )));
        let tops: Vec<&crate::parse::zjson::Vdev> = p
            .root
            .children
            .iter()
            .flat_map(|v| if v.vtype == "class" { v.children.iter().collect::<Vec<_>>() } else { vec![v] })
            .collect();
        for v in tops.iter().take(room - 1) {
            let io = m.io(&p.name, &v.name);
            let cap = if v.size > 0 { v.alloc as f64 / v.size as f64 } else { 0.0 };
            let mut l = vec![Span::raw(format!("{:<12} ", fmt::fit_left(&v.name, 12)))];
            l.extend(widgets::meter(mw, cap, 0.8, 0.9));
            l.push(Span::raw(format!("{:>5}", fmt::pct(cap * 100.0))));
            l.push(Span::styled(
                format!("{:>7}", io.map(|i| fmt::count(i.rops)).unwrap_or("…".into())),
                Style::new().fg(theme::READ),
            ));
            l.push(Span::styled(
                format!("{:>7}", io.map(|i| fmt::count(i.wops)).unwrap_or("…".into())),
                Style::new().fg(theme::WRITE),
            ));
            lines.push(Line::from(l));
        }
        if tops.len() > room - 1 {
            lines.push(Line::from(Span::styled(format!("… {} more (2 Pool)", tops.len() - (room - 1)), theme::dim())));
        }
    }
    let title = Line::from(vec![
        Span::styled(format!(" {} ", p.name), Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{} ", p.state), Style::new().fg(theme::state(&p.state)).add_modifier(Modifier::BOLD)),
    ]);
    f.render_widget(Paragraph::new(lines).block(panel("").title(title)), r);
}

pub fn scan_line(s: &crate::parse::zjson::Scan, w: usize) -> Line<'static> {
    let now = jiff::Timestamp::now().as_second();
    let what = s.function.to_lowercase();
    if s.state == "SCANNING" {
        let total = s.to_examine.max(1) as f64;
        let frac = s.issued as f64 / total;
        let elapsed = (now - s.pass_start).max(1) as f64;
        let rate = s.pass_issued as f64 / elapsed;
        let eta =
            if rate > 0.0 { fmt::duration((s.to_examine.saturating_sub(s.issued)) as f64 / rate) } else { "-".into() };
        let mut l = vec![Span::styled(
            format!("{what}{} ", if s.paused { " (paused)" } else { "" }),
            Style::new().fg(theme::WARN),
        )];
        l.extend(widgets::gauge(w.saturating_sub(what.len() + 24).max(4), frac, theme::WARN));
        l.push(Span::raw(format!(" {} {}/s eta {eta}", fmt::pct(frac * 100.0), fmt::size(rate as u64))));
        Line::from(l)
    } else {
        let errs = if s.errors > 0 { format!(", {} errors", s.errors) } else { String::new() };
        let c = if s.errors > 0 { theme::WARN } else { theme::DIM };
        let ago = if s.end_time > 0 { fmt::age(s.end_time, now) } else { "-".into() };
        Line::from(Span::styled(format!("last {what}: {} {ago}{errs}", s.state.to_lowercase()), Style::new().fg(c)))
    }
}

/// Read-above/write-below graph of a vdev series pair, titled with current and peak values.
#[allow(clippy::too_many_arguments)]
pub fn io_graph(
    f: &mut Frame,
    m: &Model,
    key: &str,
    r: Rect,
    name: &str,
    rs: &str,
    ws: &str,
    fmtv: impl Fn(f64) -> String,
) {
    let gw = widgets::samples(inner(r).width);
    let rd: Vec<f64> = m.series.get(&format!("io:{key}:{rs}")).map(|s| s.tail(gw).collect()).unwrap_or_default();
    let wr: Vec<f64> = m.series.get(&format!("io:{key}:{ws}")).map(|s| s.tail(gw).collect()).unwrap_or_default();
    let peak = |v: &[f64]| v.iter().copied().filter(|x| x.is_finite()).fold(0.0, f64::max);
    let (rp, wp) = (peak(&rd), peak(&wr));
    let now_r = rd.last().copied().unwrap_or(0.0);
    let now_w = wr.last().copied().unwrap_or(0.0);
    let title = Line::from(vec![
        Span::styled(format!(" {name} "), Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("r {} ", fmtv(now_r)), Style::new().fg(theme::READ).add_modifier(Modifier::BOLD)),
        Span::styled(format!("w {} ", fmtv(now_w)), Style::new().fg(theme::WRITE).add_modifier(Modifier::BOLD)),
    ]);
    let block = panel("").title(title).title_bottom(
        Line::from(Span::styled(
            format!(" ◀ {} ", fmt::duration(rd.len().max(wr.len()) as f64 * m.dt.max(1.0))),
            theme::dim(),
        ))
        .right_aligned(),
    );
    let ir = inner(r);
    f.render_widget(block, r);
    if rd.is_empty() && !m.iostat_seen {
        f.render_widget(Paragraph::new("waiting for zpool iostat…").style(theme::dim()), ir);
        return;
    }
    // A shared scale keeps read and write comparable; each half still shows its own shape.
    let max = rp.max(wp);
    mirror_labeled(
        f.buffer_mut(),
        ir,
        (&rd, theme::READ, format!("▲ read · top {}", fmtv(max))),
        (&wr, theme::WRITE, format!("▼ write · bottom {}", fmtv(max))),
        max,
        max,
    );
}

/// Mirror graph with a scale label in each half (top-left for the upper series, bottom-left for the lower one).
pub fn mirror_labeled(
    buf: &mut ratatui::buffer::Buffer,
    r: Rect,
    up: (&[f64], ratatui::style::Color, String),
    down: (&[f64], ratatui::style::Color, String),
    umax: f64,
    dmax: f64,
) {
    widgets::mirror_c(buf, r, up.0, down.0, umax, dmax, up.1, down.1);
    if r.height >= 4 && r.width > 20 {
        widgets::label(buf, r.x, r.y, &up.2, up.1);
        widgets::label(buf, r.x, r.y + r.height - 1, &down.2, down.1);
    }
}

/// Total wait, read above / write below, on a shared scale; disk wait (the device's share) in the title.
pub fn latency_graph(f: &mut Frame, m: &Model, key: &str, r: Rect) {
    let ir = inner(r);
    let gw = widgets::samples(ir.width);
    let get = |s: &str| -> Vec<f64> {
        m.series.get(&format!("io:{key}:{s}")).map(|x| x.tail(gw).collect()).unwrap_or_default()
    };
    let (lr, lw, dr, dw) = (get("lr"), get("lw"), get("dr"), get("dw"));
    let peak = [&lr, &lw].iter().flat_map(|v| v.iter()).copied().filter(|x| x.is_finite()).fold(0.0, f64::max);
    let last = |v: &[f64]| v.last().copied().unwrap_or(f64::NAN);
    let title = Line::from(vec![
        Span::styled(" latency ", Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("r {} ", fmt::ns(last(&lr))), Style::new().fg(theme::READ).add_modifier(Modifier::BOLD)),
        Span::styled(format!("(disk {}) ", fmt::ns(last(&dr))), Style::new().fg(theme::READ_DIM)),
        Span::styled(format!("w {} ", fmt::ns(last(&lw))), Style::new().fg(theme::WRITE).add_modifier(Modifier::BOLD)),
        Span::styled(format!("(disk {}) ", fmt::ns(last(&dw))), Style::new().fg(theme::WRITE_DIM)),
    ]);
    f.render_widget(
        panel("")
            .title(title)
            .title_bottom(Line::from(Span::styled(" total wait = queue + disk ", theme::dim())).right_aligned()),
        r,
    );
    mirror_labeled(
        f.buffer_mut(),
        ir,
        (&lr, theme::READ, format!("▲ read wait · top {}", fmt::ns(peak))),
        (&lw, theme::WRITE, format!("▼ write wait · bottom {}", fmt::ns(peak))),
        peak,
        peak,
    );
}

fn arc_card(f: &mut Frame, m: &Model, r: Rect) {
    let ir = inner(r);
    let w = ir.width as usize;
    let v = |k: &str| m.val("arcstats", k) as f64;
    let (size, c, cmax) = (v("size"), v("c"), v("c_max"));
    let hit = m.series.last("arc.demand_hit");
    let title = graph_title("ARC", fmt::size(size as u64), format!("demand hit {}", fmt::pct(hit)));
    let mut lines = Vec::new();
    let mut g = vec![Span::styled("size ", theme::dim())];
    g.extend(widgets::gauge(w.saturating_sub(12).max(4), if cmax > 0.0 { size / cmax } else { 0.0 }, theme::ACCENT));
    g.push(Span::raw(format!(" {}", fmt::size(cmax as u64))));
    lines.push(Line::from(g));
    lines.push(Line::from(
        [
            kv("target", fmt::size(c as u64), None),
            vec![Span::raw("  ")],
            kv("free mem", fmt::size(v("memory_available_bytes") as u64), None),
        ]
        .concat(),
    ));
    let (mru, mfu) = (v("mru_size"), v("mfu_size"));
    let mut st = vec![Span::styled("mru/mfu ", theme::dim())];
    st.extend(widgets::stacked(
        w.saturating_sub(8).max(4),
        &[(mru, '█', theme::ARC_MRU), (mfu, '▓', theme::ARC_MFU)],
        mru + mfu,
    ));
    lines.push(Line::from(st));
    let ratio = if v("compressed_size") > 0.0 { v("uncompressed_size") / v("compressed_size") } else { 1.0 };
    lines.push(Line::from(
        [
            kv("meta", fmt::size(v("metadata_size") as u64), None),
            vec![Span::raw("  ")],
            kv("compr", format!("{ratio:.2}x"), None),
        ]
        .concat(),
    ));
    lines.push(Line::from(
        [
            kv("▲ reads/s", fmt::count(m.series.last("arc.reads")), Some(theme::OK)),
            vec![Span::raw("  ")],
            kv("▼ misses/s", fmt::count(m.series.last("arc.misses")), Some(theme::WARN)),
        ]
        .concat(),
    ));
    let n = lines.len() as u16;
    f.render_widget(Paragraph::new(lines).block(panel("").title(title)), r);
    // reads (up) vs misses (down), each on its own scale: misses are usually tiny next to hits
    if ir.height > n + 1 {
        let sr = Rect { y: ir.y + n, height: ir.height - n, ..ir };
        let gw = widgets::samples(sr.width);
        let rd: Vec<f64> = m.series.get("arc.reads").map(|s| s.tail(gw).collect()).unwrap_or_default();
        let ms: Vec<f64> = m.series.get("arc.misses").map(|s| s.tail(gw).collect()).unwrap_or_default();
        let mx = |v: &[f64]| v.iter().copied().fold(0.0, f64::max);
        let (rmx, mmx) = (mx(&rd), mx(&ms));
        mirror_labeled(
            f.buffer_mut(),
            sr,
            (&rd, theme::OK, format!("▲ reads · top {}/s", fmt::count(rmx))),
            (&ms, theme::WARN, format!("▼ misses · bottom {}/s", fmt::count(mmx))),
            rmx,
            mmx,
        );
    }
}

fn zil_card(f: &mut Frame, m: &Model, r: Rect) {
    let ir = inner(r);
    let commits = m.series.last("zil.commits");
    let title = graph_title("ZIL", format!("{} commits/s", fmt::count(commits)), String::new());
    let lines = vec![
        Line::from(
            [
                kv("itx/s", fmt::count(m.series.last("zil.itx")), None),
                vec![Span::raw("  ")],
                kv("bytes", fmt::rate(m.series.last("zil.bytes")), None),
            ]
            .concat(),
        ),
        Line::from([kv("to slog", fmt::rate(m.series.last("zil.slog")), Some(theme::ACCENT2))].concat()),
        Line::from([kv("to pool", fmt::rate(m.series.last("zil.normal")), None)].concat()),
        Line::from(kv(
            "stalls/s",
            fmt::count(m.series.last("zil.stalls")),
            Some(if m.series.last("zil.stalls") > 0.0 { theme::WARN } else { theme::DIM }),
        )),
    ];
    f.render_widget(Paragraph::new(lines).block(panel("").title(title)), r);
    if ir.height > 4 {
        let sr = Rect { y: ir.y + 4, height: ir.height - 4, ..ir };
        let d: Vec<f64> =
            m.series.get("zil.commits").map(|s| s.tail(widgets::samples(sr.width)).collect()).unwrap_or_default();
        let max = d.iter().copied().fold(0.0, f64::max);
        let buf = f.buffer_mut();
        widgets::area(buf, sr, &d, max, theme::ACCENT2, Dir::Up);
        widgets::label(buf, sr.x, sr.y, &format!("commits/s · top {}", fmt::count(max)), theme::ACCENT2);
    }
}

fn txg_card(f: &mut Frame, m: &Model, pool: &str, r: Rect) {
    let ir = inner(r);
    let txgs = m.txgs.get(pool);
    let last = txgs.and_then(|t| t.back());
    let sync = last.map(|t| t.stime as f64).unwrap_or(f64::NAN);
    let title = graph_title("TXG", format!("sync {}", fmt::ns(sync)), pool.to_string());
    let dirty = last.map(|t| t.ndirty as f64).unwrap_or(0.0);
    let dirty_max = m.param("zfs_dirty_data_max").and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    let timeout = m.param("zfs_txg_timeout").unwrap_or("?");
    let per_min = txgs
        .filter(|t| t.len() >= 2)
        .map(|t| {
            let (a, b) = (t.front().unwrap(), t.back().unwrap());
            let span = (b.birth.saturating_sub(a.birth)) as f64 / 1e9;
            if span > 0.0 { (t.len() - 1) as f64 * 60.0 / span } else { 0.0 }
        })
        .unwrap_or(0.0);
    let mut dl = vec![Span::styled("dirty/txg ", theme::dim())];
    dl.extend(widgets::meter(
        ir.width.saturating_sub(18).max(4) as usize,
        if dirty_max > 0.0 { dirty / dirty_max } else { 0.0 },
        0.6,
        0.8,
    ));
    dl.push(Span::raw(format!(" {}", fmt::size(dirty.max(0.0) as u64))));
    let delay = m.series.last("tx.delay");
    let lines = vec![
        Line::from(dl),
        Line::from(
            [
                kv("txg/min", format!("{per_min:.1}"), None),
                vec![Span::raw("  ")],
                kv("timeout", format!("{timeout}s"), None),
            ]
            .concat(),
        ),
        Line::from(kv("delays/s", fmt::count(delay), Some(if delay > 0.0 { theme::WARN } else { theme::DIM }))),
        Line::from(kv("written", last.map(|t| fmt::size(t.nwritten)).unwrap_or("-".into()), None)),
    ];
    f.render_widget(Paragraph::new(lines).block(panel("").title(title)), r);
    if ir.height > 4 {
        let sr = Rect { y: ir.y + 4, height: ir.height - 4, ..ir };
        let d: Vec<f64> = txgs.map(|t| t.iter().map(|x| x.stime as f64).collect()).unwrap_or_default();
        let max = d.iter().copied().fold(0.0, f64::max);
        let timeout_ns = m.param("zfs_txg_timeout").and_then(|s| s.parse::<f64>().ok()).unwrap_or(5.0) * 1e9;
        let buf = f.buffer_mut();
        widgets::bars(buf, sr, &d, max, |_, v| theme::level(v, timeout_ns * 0.5, timeout_ns));
        widgets::label(buf, sr.x, sr.y, &format!("sync time per txg · top {}", fmt::ns(max)), theme::DIM);
    }
}

fn top_datasets(f: &mut Frame, m: &Model, r: Rect) {
    let ir = inner(r);
    let mut v: Vec<(&String, &crate::model::DsIo)> = m.ds_io.iter().collect();
    v.sort_by(|a, b| (b.1.bw() + b.1.ops()).total_cmp(&(a.1.bw() + a.1.ops())).then(a.0.cmp(b.0)));
    let nw = (ir.width as usize).saturating_sub(30).max(8);
    let mut lines = vec![Line::from(Span::styled(
        format!("{:<nw$} {:>6} {:>6} {:>7} {:>7}", "dataset", "r/s", "w/s", "read", "write"),
        theme::dim(),
    ))];
    for (name, io) in v.iter().take(ir.height.saturating_sub(1) as usize) {
        if io.ops() <= 0.0 && io.bw() <= 0.0 {
            break;
        }
        lines.push(Line::from(vec![
            Span::raw(format!("{} ", fmt::fit_left(name, nw))),
            Span::styled(format!("{:>6} ", fmt::count(io.rops)), Style::new().fg(theme::READ)),
            Span::styled(format!("{:>6} ", fmt::count(io.wops)), Style::new().fg(theme::WRITE)),
            Span::styled(format!("{:>7} ", fmt::rate(io.rbw)), Style::new().fg(theme::READ)),
            Span::styled(format!("{:>7}", fmt::rate(io.wbw)), Style::new().fg(theme::WRITE)),
        ]));
    }
    if lines.len() == 1 {
        lines.push(Line::from(Span::styled(if m.samples == 0 { "measuring…" } else { "idle" }, theme::dim())));
    }
    f.render_widget(Paragraph::new(lines).block(panel("busiest datasets")), r);
}
