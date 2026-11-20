//! Pool tab: topology tree with live per-vdev IO / latency / queue / space columns; the selected row's graphs,
//! latency heatmap and request-size histogram; and a disk grid for spotting the one slow disk among many.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::overview::{io_graph, latency_graph};
use super::widgets;
use super::{Col, header_line, inner, panel, theme};
use crate::app::{App, GridMetric, Hit, IoMode, TopoRow, TopoSort};
use crate::fmt;
use crate::model::Model;
use crate::parse::iostat::{QUEUE_NAMES, Row, w};
use crate::parse::zjson::Vdev;

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if app.model.pools().is_empty() {
        f.render_widget(
            Paragraph::new("reading pool configuration…").style(theme::dim()).block(panel("topology")),
            area,
        );
        return;
    }
    if app.pool_history {
        history(f, app, area);
        return;
    }
    if app.grid {
        grid(f, app, area);
        return;
    }
    let rows = app.topo_rows();
    let wide = area.width >= 230;
    let (table_r, detail_r) = if wide {
        let [a, b] = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(area);
        (a, b)
    } else {
        let th = ((rows.len() as u16 + 3).min(area.height * 45 / 100)).max(8);
        let [a, b] = Layout::vertical([Constraint::Length(th), Constraint::Min(8)]).areas(area);
        (a, b)
    };
    table(f, app, &rows, table_r);
    detail(f, app, &rows, detail_r);
}

fn find<'a>(app: &'a App, pool: &str, name: &str) -> Option<&'a Vdev> {
    let p = app.model.pool(pool)?;
    let mut hit = None;
    p.root.walk(0, &mut |v, _| {
        if v.name == name && hit.is_none() {
            hit = Some(v);
        }
    });
    hit
}

/// Tree indent, same style as the Datasets tree: indentation and fold markers, no connecting lines.
fn tree_prefix(r: &TopoRow) -> String {
    "  ".repeat(r.depth)
}

fn errs(v: &Vdev) -> Vec<Span<'static>> {
    let c = |n: u64| {
        if n > 0 {
            Span::styled(format!("{n:>4}"), Style::new().fg(theme::CRIT).add_modifier(Modifier::BOLD))
        } else {
            Span::styled(format!("{:>4}", 0), theme::dim())
        }
    };
    vec![c(v.read_errors), c(v.write_errors), c(v.checksum_errors)]
}

fn table(f: &mut Frame, app: &mut App, rows: &[TopoRow], r: Rect) {
    let ir = inner(r);
    let m = &app.model;
    let sel = rows.iter().position(|x| Some(&x.key) == app.topo_sel.as_ref()).unwrap_or(0);
    let name_w = rows
        .iter()
        .map(|x| tree_prefix(x).chars().count() + x.name.chars().count() + 2)
        .max()
        .unwrap_or(10)
        .clamp(12, 32);
    let mode = app.io_mode;
    let (ts, td) = (app.topo_sort, app.topo_desc);
    let c = |t: &'static str, w: usize, right: bool, k: TopoSort| {
        Col::new(t, w, right).sort(Hit::TopoSort(k), (ts == k).then_some(td))
    };
    let mut cols = vec![
        c("NAME", name_w + 1, false, TopoSort::Name),
        c("STATE", 9, false, TopoSort::State),
        c("R", 4, true, TopoSort::Errors),
        c("W", 4, true, TopoSort::Errors),
        c("CK", 4, true, TopoSort::Errors),
        Col::new("", 2, false),
    ];
    match mode {
        IoMode::Io => cols.extend([
            c("r/s ", 8, true, TopoSort::Rops).color(theme::READ),
            c("w/s ", 8, true, TopoSort::Wops).color(theme::WRITE),
            c("read ", 9, true, TopoSort::Rbw).color(theme::READ),
            c("write  ", 10, true, TopoSort::Wbw).color(theme::WRITE),
            Col::new("trend", 16, false),
        ]),
        IoMode::Latency => {
            for (i, t) in ["tot r ", "tot w ", "disk r ", "disk w ", "syncq r ", "syncq w ", "asyq r ", "asyq w "]
                .into_iter()
                .enumerate()
            {
                let idx =
                    [w::TOTAL_R, w::TOTAL_W, w::DISK_R, w::DISK_W, w::SYNCQ_R, w::SYNCQ_W, w::ASYNCQ_R, w::ASYNCQ_W][i];
                cols.push(c(t, 8, true, TopoSort::Wait(idx)));
            }
        }
        IoMode::Queue => {
            cols.push(c("act ", 6, true, TopoSort::Active));
            cols.push(c("pend ", 6, true, TopoSort::Pending));
            for (i, q) in QUEUE_NAMES.iter().take(5).enumerate() {
                let t: &'static str = match i {
                    0 => "sync rd ",
                    1 => "sync wr ",
                    2 => "async rd ",
                    3 => "async wr ",
                    _ => "scrub ",
                };
                let _ = q;
                cols.push(c(t, 10, true, TopoSort::Queue(i)));
            }
        }
        IoMode::Space => cols.extend([
            c("size ", 8, true, TopoSort::Size),
            c("alloc ", 8, true, TopoSort::Alloc),
            c("free ", 8, true, TopoSort::Free),
            c("cap ", 6, true, TopoSort::Cap),
            c("frag  ", 7, true, TopoSort::Frag),
            Col::new("used", 16, false),
        ]),
    }
    let mut lines = vec![header_line(&cols, ir.x, ir.y, ir.width as usize, &mut app.hits)];
    let body_h = ir.height.saturating_sub(1) as usize;
    // keep the selection visible
    let top = if sel >= app.scroll + body_h { sel + 1 - body_h } else { app.scroll.min(sel) };
    app.scroll = top;
    for (i, row) in rows.iter().enumerate().skip(top).take(body_h) {
        let y = ir.y + 1 + (i - top) as u16;
        app.hits.add(Rect::new(ir.x, y, ir.width, 1), Hit::Row(i));
        if row.has_children {
            app.hits.add(Rect::new(ir.x + tree_prefix(row).chars().count() as u16, y, 2, 1), Hit::Fold(i));
        }
        let v = find(app, &row.pool, &row.name);
        let io = m.io(&row.pool, &row.name);
        let fold = if row.has_children { if row.collapsed { "▸ " } else { "▾ " } } else { "  " };
        // truncate the name, never the tree prefix
        let indent = format!("{}{fold}", tree_prefix(row));
        let label =
            format!("{indent}{}", fmt::fit_left(&row.name, name_w.saturating_sub(indent.chars().count()).max(4)));
        let name_style = match row.vtype.as_str() {
            "class" => Style::new().fg(theme::ACCENT2).add_modifier(Modifier::BOLD),
            "root" => Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
            t if t != "disk" && t != "file" => theme::bold(),
            _ => Style::new(),
        };
        let mut spans = vec![Span::styled(format!("{} ", fmt::fit(&label, name_w)), name_style)];
        if row.vtype == "class" {
            spans.push(Span::raw(" ".repeat(9 + 12 + 2)));
        } else if let Some(v) = v {
            spans.push(Span::styled(format!("{:<8} ", fmt::fit(&v.state, 8)), Style::new().fg(theme::state(&v.state))));
            spans.extend(errs(v));
            spans.push(Span::raw("  "));
        }
        match (mode, io) {
            (IoMode::Space, _) => spans.extend(space_cols(v, ir.width as usize)),
            (_, None) if row.vtype == "class" => {}
            (_, None) => spans.push(Span::styled("  …", theme::dim())),
            (IoMode::Io, Some(io)) => {
                spans.push(Span::styled(format!("{:>7} ", fmt::count(io.rops)), Style::new().fg(theme::READ)));
                spans.push(Span::styled(format!("{:>7} ", fmt::count(io.wops)), Style::new().fg(theme::WRITE)));
                spans.push(Span::styled(format!("{:>8} ", fmt::rate(io.rbw)), Style::new().fg(theme::READ)));
                spans.push(Span::styled(format!("{:>8}  ", fmt::rate(io.wbw)), Style::new().fg(theme::WRITE)));
                let ops: Vec<f64> = {
                    let a = m.series.get(&format!("io:{}:rops", row.key));
                    let b = m.series.get(&format!("io:{}:wops", row.key));
                    match (a, b) {
                        (Some(a), Some(b)) => a.tail(16).zip(b.tail(16)).map(|(x, y)| x + y).collect(),
                        _ => Vec::new(),
                    }
                };
                spans.push(Span::styled(fmt::spark(ops, 16, 0.0), Style::new().fg(theme::ACCENT)));
            }
            (IoMode::Latency, Some(io)) => {
                for i in
                    [w::TOTAL_R, w::TOTAL_W, w::DISK_R, w::DISK_W, w::SYNCQ_R, w::SYNCQ_W, w::ASYNCQ_R, w::ASYNCQ_W]
                {
                    let ns = io.wait[i].unwrap_or(f64::NAN);
                    spans.push(Span::styled(format!("{:>7} ", fmt::ns(ns)), Style::new().fg(theme::latency(ns))));
                }
            }
            (IoMode::Queue, Some(io)) => queue_cols(io, &mut spans),
        }
        let style = if i == sel { theme::sel() } else { Style::new() };
        lines.push(super::full(Line::from(spans), ir.width as usize).style(style));
    }
    let n = rows.len();
    let sort = if ts == TopoSort::Order {
        String::new()
    } else {
        format!(" · sort {} {}", ts.title(), if td { "▼" } else { "▲" })
    };
    let title = format!("topology · {}{sort} · {}/{}", mode.title(), sel + 1, n);
    f.render_widget(Paragraph::new(lines).block(panel(&title)), r);
}

fn queue_cols(io: &Row, spans: &mut Vec<Span<'static>>) {
    let a = io.active();
    let p = io.pending();
    spans.push(Span::styled(format!("{a:>5} "), if a > 0 { Style::new().fg(theme::ACCENT) } else { theme::dim() }));
    spans.push(Span::styled(format!("{p:>5} "), if p > 0 { Style::new().fg(theme::WARN) } else { theme::dim() }));
    for q in 0..5 {
        let (pe, ac) = (io.queue[q * 2], io.queue[q * 2 + 1]);
        let s = if pe == 0 && ac == 0 { format!("{:>9} ", "·") } else { format!("{:>9} ", format!("{ac}/{pe}")) };
        spans.push(Span::styled(s, if pe + ac > 0 { Style::new() } else { theme::dim() }));
    }
}

fn space_cols(v: Option<&Vdev>, _w: usize) -> Vec<Span<'static>> {
    let Some(v) = v else { return vec![] };
    if v.size == 0 {
        return vec![Span::styled(format!("{:>7}", "-"), theme::dim())];
    }
    let leafish = v.is_leaf() && v.alloc == 0;
    let cap = v.alloc as f64 / v.size as f64;
    let mut s = vec![Span::raw(format!("{:>7} ", fmt::size(v.size)))];
    if leafish {
        return s;
    }
    s.push(Span::raw(format!("{:>7} {:>7} ", fmt::size(v.alloc), fmt::size(v.size.saturating_sub(v.alloc)))));
    s.push(Span::styled(format!("{:>5} ", fmt::pct(cap * 100.0)), Style::new().fg(theme::level(cap, 0.8, 0.9))));
    s.push(Span::raw(format!("{:>5}  ", v.frag.map(|f| format!("{f}%")).unwrap_or("-".into()))));
    s.extend(widgets::meter(16, cap, 0.8, 0.9));
    s
}

fn detail(f: &mut Frame, app: &App, rows: &[TopoRow], r: Rect) {
    let Some(row) = rows.iter().find(|x| Some(&x.key) == app.topo_sel.as_ref()).or(rows.first()) else { return };
    let m = &app.model;
    if row.vtype == "class" {
        let v = find(app, &row.pool, &row.name);
        let n = v.map(|v| v.children.len()).unwrap_or(0);
        f.render_widget(
            Paragraph::new(format!(
                "{} vdevs in the {} class. Select one to see its IO.\n\nspecial: metadata and small blocks (special_small_blocks)\nlogs: ZIL (sync writes)\ncache: L2ARC\nspares: hot spares",
                n, row.name
            ))
            .style(theme::dim())
            .block(panel(&row.name)),
            r,
        );
        return;
    }
    let key = &row.key;
    let [info_r, graphs_r] = Layout::vertical([Constraint::Length(3), Constraint::Min(6)]).areas(r);
    info(f, app, row, info_r);
    let wide = graphs_r.width >= 110;
    if wide {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(graphs_r);
        let g =
            Layout::vertical([Constraint::Ratio(1, 3), Constraint::Ratio(1, 3), Constraint::Ratio(1, 3)]).split(left);
        io_graph(f, m, key, g[0], "IOPS", "rops", "wops", fmt::count);
        io_graph(f, m, key, g[1], "bandwidth", "rbw", "wbw", fmt::rate);
        latency_graph(f, m, key, g[2]);
        let [hm, rs] = Layout::vertical([Constraint::Percentage(62), Constraint::Percentage(38)]).areas(right);
        heatmap(f, app, key, hm);
        size_hist(f, m, key, rs);
    } else {
        let g = Layout::vertical([Constraint::Ratio(1, 4); 4]).split(graphs_r);
        io_graph(f, m, key, g[0], "IOPS", "rops", "wops", fmt::count);
        io_graph(f, m, key, g[1], "bandwidth", "rbw", "wbw", fmt::rate);
        latency_graph(f, m, key, g[2]);
        heatmap(f, app, key, g[3]);
    }
}

fn info(f: &mut Frame, app: &App, row: &TopoRow, r: Rect) {
    let m = &app.model;
    let v = find(app, &row.pool, &row.name);
    let mut l1 = vec![
        Span::styled(format!("{} ", row.name), theme::bold()),
        Span::styled(format!("{} ", if row.vtype == "root" { "pool" } else { &row.vtype }), theme::dim()),
    ];
    if let Some(v) = v {
        l1.push(Span::styled(format!("{} ", v.state), Style::new().fg(theme::state(&v.state))));
        if v.class != "normal" && !v.class.is_empty() {
            l1.push(Span::styled(format!("class {} ", v.class), Style::new().fg(theme::ACCENT2)));
        }
        if v.slow_ios > 0 {
            l1.push(Span::styled(format!("slow I/Os {} ", v.slow_ios), Style::new().fg(theme::WARN)));
        }
        if let Some(a) = &v.activity {
            l1.push(Span::styled(format!("{a} "), Style::new().fg(theme::WARN)));
        }
    }
    let mut l2 = Vec::new();
    if let Some(d) = m.disk_for_vdev(&row.pool, &row.name) {
        l2.push(Span::styled(format!("{} ", d.whole), Style::new().fg(theme::ACCENT)));
        if d.legs.len() > 1 {
            l2.push(Span::styled(
                format!("paths {} ", d.legs.iter().map(|l| l.dev.as_str()).collect::<Vec<_>>().join(",")),
                theme::dim(),
            ));
        }
        if let Some(s) = d.slot() {
            l2.push(Span::raw(format!("{s} ")));
        }
        l2.push(Span::raw(format!("{} {} ", d.model(), fmt::size(d.size))));
        if let Some(Ok(s)) = m.smart.get(&d.smart_dev) {
            if let Some(t) = s.temp_c {
                l2.push(Span::styled(format!("{t}°C "), Style::new().fg(theme::level(t as f64, 45.0, 55.0))));
            }
            if let Some(sn) = &s.serial {
                l2.push(Span::styled(format!("SN {sn} "), theme::dim()));
            }
        }
        if let Some(dr) = m.disk_rates.get(&d.whole) {
            l2.push(Span::styled(
                format!("util {} ", fmt::pct(dr.util)),
                Style::new().fg(theme::level(dr.util, 70.0, 90.0)),
            ));
        }
    } else if let Some(v) = v {
        if !v.children.is_empty() {
            l2.push(Span::styled(format!("{} children  ", v.children.len()), theme::dim()));
        }
        if v.size > 0 {
            l2.push(Span::raw(format!("{} / {} allocated", fmt::size(v.alloc), fmt::size(v.size))));
        }
        if let Some(fr) = v.frag {
            l2.push(Span::styled(format!("  frag {fr}%"), theme::dim()));
        }
    }
    f.render_widget(
        Paragraph::new(vec![Line::from(l1), Line::from(l2)]).block(
            ratatui::widgets::Block::new()
                .borders(ratatui::widgets::Borders::BOTTOM)
                .border_style(Style::new().fg(theme::BORDER)),
        ),
        r,
    );
}

/// Latency bucket label from the bucket's upper bound in ns.
fn lat_label(ns: u64) -> String {
    let n = ns as f64;
    match n {
        n if n < 1e3 => format!("{}ns", ns + 1),
        n if n < 1e6 => format!("{}us", ((n + 1.0) / 1e3).round()),
        n if n < 1e9 => format!("{}ms", ((n + 1.0) / 1e6).round()),
        n => format!("{}s", ((n + 1.0) / 1e9).round()),
    }
}

fn heatmap(f: &mut Frame, app: &App, key: &str, r: Rect) {
    let m = &app.model;
    let which = if app.heat_disk { "disk wait" } else { "total wait" };
    let title = Line::from(vec![
        Span::styled(" latency heatmap ", Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{which} (w) "), theme::dim()),
        Span::styled("read ", Style::new().fg(theme::READ)),
        Span::styled("▲ write ▼ ", Style::new().fg(theme::WRITE)),
    ]);
    let block = panel("").title(title).title_bottom(
        Line::from(Span::styled(
            format!(" ◀ {} per column ", fmt::duration(crate::workers::HIST_EVERY.as_secs_f64())),
            theme::dim(),
        ))
        .right_aligned(),
    );
    let ir = inner(r);
    f.render_widget(block, r);
    let (Some(cols), Some(h)) = (m.hist.heat.get(key), m.hist.lat.get(key)) else {
        let msg = m.hist_err.clone().unwrap_or_else(|| "collecting latency histograms (zpool iostat -w)…".into());
        f.render_widget(Paragraph::new(msg).style(theme::dim()), ir);
        return;
    };
    let pick = |c: &crate::model::HeatCol, write: bool| -> Vec<u32> {
        match (app.heat_disk, write) {
            (false, false) => c.read.clone(),
            (false, true) => c.write.clone(),
            (true, false) => c.disk_read.clone(),
            (true, true) => c.disk_write.clone(),
        }
    };
    let reads: Vec<Vec<u32>> = cols.iter().map(|c| pick(c, false)).collect();
    let writes: Vec<Vec<u32>> = cols.iter().map(|c| pick(c, true)).collect();
    // bucket range with data in the window shown (else since import), padded by one
    let col_idx = if app.heat_disk { [2, 3] } else { [0, 1] };
    let mut used: Vec<usize> = (0..h.buckets.len())
        .filter(|&i| reads.iter().chain(writes.iter()).any(|c| c.get(i).copied().unwrap_or(0) > 0))
        .collect();
    if used.is_empty() {
        used = (0..h.buckets.len())
            .filter(|&i| col_idx.iter().any(|&c| h.counts[i].get(c).copied().unwrap_or(0) > 0))
            .collect();
    }
    let (lo, hi) = match (used.first(), used.last()) {
        (Some(&a), Some(&b)) => (a.saturating_sub(1), (b + 2).min(h.buckets.len())),
        _ => (0, h.buckets.len().min(1)),
    };
    if ir.height < 4 || hi <= lo {
        return;
    }
    let label_w = 6u16;
    let half = ir.height / 2;
    let rr = Rect { x: ir.x + label_w, y: ir.y, width: ir.width.saturating_sub(label_w), height: half };
    let wr =
        Rect { x: ir.x + label_w, y: ir.y + half, width: ir.width.saturating_sub(label_w), height: ir.height - half };
    let buf = f.buffer_mut();
    widgets::heatmap(buf, rr, &reads, lo, hi);
    widgets::heatmap(buf, wr, &writes, lo, hi);
    // labels: top bucket and bottom bucket of each half, plus the middle
    let lab = |buf: &mut ratatui::buffer::Buffer, y: u16, s: String, c: Color| {
        buf.set_string(ir.x, y, fmt::rpad(&s, label_w as usize - 1), Style::new().fg(c));
    };
    lab(buf, rr.y, lat_label(h.buckets[hi - 1]), theme::READ);
    lab(buf, rr.y + rr.height - 1, lat_label(h.buckets[lo]), theme::READ);
    lab(buf, wr.y, lat_label(h.buckets[hi - 1]), theme::WRITE);
    lab(buf, wr.y + wr.height - 1, lat_label(h.buckets[lo]), theme::WRITE);
    if rr.height >= 5 {
        lab(buf, rr.y + rr.height / 2, lat_label(h.buckets[(lo + hi) / 2]), theme::DIM);
        lab(buf, wr.y + wr.height / 2, lat_label(h.buckets[(lo + hi) / 2]), theme::DIM);
    }
}

fn size_label(b: u64) -> String {
    match b {
        b if b >= 1 << 20 => format!("{}M", b >> 20),
        b if b >= 1 << 10 => format!("{}K", b >> 10),
        b => format!("{b}"),
    }
}

/// Request sizes over the last histogram interval (or since import if idle): reads and writes, individual vs
/// aggregated, as horizontal bars.
fn size_hist(f: &mut Frame, m: &Model, key: &str, r: Rect) {
    let ir = inner(r);
    let delta = m.hist.size_delta.get(key);
    let total = |h: &crate::parse::iostat::Hist| h.counts.iter().flatten().sum::<u64>();
    let (h, scope) = match (delta, m.hist.size.get(key)) {
        (Some(d), _) if total(d) > 0 => (d, "last interval"),
        (_, Some(c)) => (c, "since import"),
        _ => {
            f.render_widget(
                Paragraph::new("collecting request sizes…").style(theme::dim()).block(panel("request sizes")),
                r,
            );
            return;
        }
    };
    // reads = sync_read + async_read (+scrub), writes = sync_write + async_write; ind and agg summed
    let rd: Vec<u64> =
        h.counts.iter().map(|c| [0, 1, 4, 5].iter().map(|&i| c.get(i).copied().unwrap_or(0)).sum()).collect();
    let wr: Vec<u64> =
        h.counts.iter().map(|c| [2, 3, 6, 7].iter().map(|&i| c.get(i).copied().unwrap_or(0)).sum()).collect();
    let used: Vec<usize> = (0..h.buckets.len()).filter(|&i| rd[i] + wr[i] > 0).collect();
    f.render_widget(panel(&format!("request sizes · {scope}")), r);
    let (Some(&lo), Some(&hi)) = (used.first(), used.last()) else { return };
    let max = rd.iter().chain(wr.iter()).copied().max().unwrap_or(1).max(1) as f64;
    let bar_w = (ir.width as usize).saturating_sub(8) / 2;
    let mut lines = Vec::new();
    for i in lo..=hi {
        if lines.len() >= ir.height as usize {
            break;
        }
        let rb = ((rd[i] as f64 / max) * bar_w as f64).ceil() as usize;
        let wb = ((wr[i] as f64 / max) * bar_w as f64).ceil() as usize;
        lines.push(Line::from(vec![
            Span::styled(format!("{:>bar_w$}", "█".repeat(rb.min(bar_w))), Style::new().fg(theme::READ)),
            Span::styled(format!(" {:^6} ", size_label(h.buckets[i])), theme::dim()),
            Span::styled("█".repeat(wb.min(bar_w)), Style::new().fg(theme::WRITE)),
        ]));
    }
    f.render_widget(Paragraph::new(lines), ir);
}

// ------------------------------------------------------------------------------------------------ disk grid

/// Tile colour for a disk that doesn't report the metric (no SMART, not an SSD, no diskstats match).
const NO_DATA: Color = Color::Indexed(238);

/// How a metric's values map to tile colours.
enum Scale {
    /// 0 → the busiest disk of the pool; the formatter labels the legend's top end
    Peak(fn(f64) -> String),
    /// fixed range, whatever the pool does (a cool pool's hottest disk shouldn't look alarming)
    Fixed(f64, f64, &'static str),
    /// red when > 0, otherwise calm: counts where any is bad
    Flag(&'static str),
}

fn grid_scale(g: GridMetric) -> Scale {
    use GridMetric as G;
    match g {
        G::Latency | G::DiskRead | G::DiskWrite | G::TotalWait => Scale::Peak(fmt::ns),
        G::Ops | G::ReadOps | G::WriteOps | G::Queue => Scale::Peak(fmt::count),
        G::ReadBw | G::WriteBw => Scale::Peak(fmt::rate),
        G::Util | G::Wear => Scale::Fixed(0.0, 100.0, "0 → 100%"),
        G::Temp => Scale::Fixed(25.0, 60.0, "25 → 60 °C"),
        G::Errors => Scale::Flag("red = errors / slow I/Os / not ONLINE"),
        G::Defects => Scale::Flag("red = grown defects / reallocated sectors"),
    }
}

/// A leaf vdev's value for `metric` (NaN when unknown), and how a tile shows it.
fn grid_value(m: &Model, metric: GridMetric, pool: &str, v: &Vdev) -> (f64, String) {
    use GridMetric as G;
    let io = || m.io(pool, &v.name);
    let wait = |i: usize| io().map(|r| r.wait[i].unwrap_or(0.0)).unwrap_or(0.0);
    let disk = || m.disk_for_vdev(pool, &v.name);
    let smart = || disk().and_then(|d| m.smart.get(&d.smart_dev)).and_then(|r| r.as_ref().ok());
    let smart_num =
        |f: fn(&crate::parse::disk::Smart) -> Option<u64>| smart().and_then(f).map(|x| x as f64).unwrap_or(f64::NAN);
    let ops = |f: fn(&Row) -> f64| io().map(f).unwrap_or(0.0);
    match metric {
        G::Latency | G::DiskRead | G::DiskWrite | G::TotalWait => {
            let x = match metric {
                G::DiskRead => wait(w::DISK_R),
                G::DiskWrite => wait(w::DISK_W),
                G::TotalWait => wait(w::TOTAL_R).max(wait(w::TOTAL_W)),
                _ => wait(w::DISK_R).max(wait(w::DISK_W)),
            };
            (x, fmt::ns(x))
        }
        G::Ops | G::ReadOps | G::WriteOps | G::Queue => {
            let x = match metric {
                G::ReadOps => ops(|r| r.rops),
                G::WriteOps => ops(|r| r.wops),
                G::Queue => ops(|r| r.active() as f64),
                _ => ops(|r| r.rops + r.wops),
            };
            (x, fmt::count(x))
        }
        G::ReadBw | G::WriteBw => {
            let x = if metric == G::ReadBw { ops(|r| r.rbw) } else { ops(|r| r.wbw) };
            (x, fmt::rate(x))
        }
        G::Util => {
            let x = disk().and_then(|d| m.disk_rates.get(&d.whole)).map(|r| r.util).unwrap_or(f64::NAN);
            (x, fmt::pct(x))
        }
        G::Errors => {
            let x = (v.errors() + v.slow_ios) as f64;
            (x, if x > 0.0 { format!("{x}") } else { "ok".into() })
        }
        G::Temp => {
            let x = smart_num(|s| s.temp_c);
            (x, if x.is_finite() { format!("{x}°C") } else { "-".into() })
        }
        G::Defects => {
            let x = smart_num(|s| s.defects);
            (x, if x.is_finite() { format!("{x}") } else { "-".into() })
        }
        G::Wear => {
            let x = smart_num(|s| s.wear_pct);
            (x, fmt::pct(x))
        }
    }
}

/// Disk grid plus a panel listing every metric it can colour by (with the current worst disk's value), so the
/// choices are visible at a glance and ↑↓ / a click picks one.
fn grid(f: &mut Frame, app: &mut App, area: Rect) {
    let (grid_r, metrics_r) = if area.width >= 100 {
        let [a, b] = Layout::horizontal([Constraint::Min(40), Constraint::Length(34)]).areas(area);
        (a, b)
    } else {
        // narrow: a strip above the grid, tall enough for the largest section
        let h = sections().iter().map(|s| s.len()).max().unwrap_or(1) as u16 + 2;
        let [a, b] = Layout::vertical([Constraint::Length(h), Constraint::Min(4)]).areas(area);
        (b, a)
    };
    grid_tiles(f, app, grid_r);
    grid_metrics(f, app, metrics_r);
}

/// `GridMetric::ALL` split into its consecutive sections.
fn sections() -> Vec<&'static [GridMetric]> {
    GridMetric::ALL.chunk_by(|a, b| a.section() == b.section()).collect()
}

fn grid_metrics(f: &mut Frame, app: &mut App, area: Rect) {
    let m = &app.model;
    let ir = inner(area);
    let leaves: Vec<(&str, &Vdev)> =
        m.pools().iter().flat_map(|p| p.root.leaves().into_iter().map(move |v| (p.name.as_str(), v))).collect();
    let secs = sections();
    // everything (column header + a heading per section + every metric) if it fits, else the selected section
    let full = ir.height as usize >= 1 + secs.len() + GridMetric::ALL.len();
    let cur = app.grid_metric;
    let name_w = (ir.width as usize).saturating_sub(10);
    let mut lines = Vec::new();
    if full {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<name_w$}", "metric"), theme::dim()),
            Span::styled(format!("{:>7} ", "worst"), theme::dim()),
        ]));
    }
    for sec in secs {
        if !full && !sec.contains(&cur) {
            continue;
        }
        if full {
            lines.push(Line::from(Span::styled(sec[0].section(), theme::dim().add_modifier(Modifier::BOLD))));
        }
        for &g in sec {
            let vals = leaves.iter().map(|(p, v)| grid_value(m, g, p, v));
            // errors: the total across disks; the rest: the worst single disk
            let worst = if g == GridMetric::Errors {
                let n: f64 = vals.map(|(x, _)| x).sum();
                if n > 0.0 { format!("{n}") } else { "ok".into() }
            } else {
                vals.filter(|(x, _)| x.is_finite())
                    .max_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, t)| t)
                    .unwrap_or("-".into())
            };
            let on = g == cur;
            let style = if on { theme::sel() } else { Style::new() };
            let y = ir.y + lines.len() as u16;
            if y < ir.bottom() {
                app.hits.add(Rect::new(ir.x, y, ir.width, 1), Hit::GridMetric(g));
            }
            lines.push(Line::from(vec![
                Span::styled(if on { "▸ " } else { "  " }, style.fg(theme::ACCENT)),
                Span::styled(format!("{:<name_w$}", fmt::fit(g.title(), name_w)), style),
                Span::styled(format!("{worst:>7} "), style),
            ]));
        }
    }
    if full {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(cur.about(), theme::dim())));
    }
    let title = if full {
        "colour by · ↑↓ / c".to_string()
    } else {
        format!("colour by · {} · ↑↓ / c", cur.section())
    };
    f.render_widget(Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }).block(panel(&title)), area);
}

fn grid_tiles(f: &mut Frame, app: &mut App, area: Rect) {
    let m = &app.model;
    let metric = app.grid_metric;
    let ir = inner(area);
    let mut lines: Vec<Line> = Vec::new();
    let label_w = 12usize;
    // Two-line tiles: disk name on top, value below, both on the tile's colour.
    let short = |n: &str| if n.chars().count() <= 8 { n.to_string() } else { fmt::fit_left(n, 7) };
    let mut legend_max: f64 = 0.0;
    let scale = grid_scale(metric);
    let value = |pool: &str, v: &Vdev| grid_value(m, metric, pool, v);
    let missing = m.pools().iter().any(|p| p.root.leaves().iter().any(|v| !value(&p.name, v).0.is_finite()));
    for p in m.pools() {
        let leaves = p.root.leaves();
        let tile_w = leaves.iter().map(|v| short(&v.name).chars().count()).max().unwrap_or(6).max(7) + 2;
        let per_line = ((ir.width as usize).saturating_sub(label_w) / (tile_w + 1)).max(1);
        let peak = leaves.iter().map(|v| value(&p.name, v).0).filter(|x| x.is_finite()).fold(0.0, f64::max);
        legend_max = legend_max.max(peak);
        lines.push(Line::from(Span::styled(
            format!("{} ", p.name),
            Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
        )));
        let mut groups: Vec<(String, Vec<&Vdev>)> = Vec::new();
        for top in &p.root.children {
            let (label, tops): (String, Vec<&Vdev>) = if top.vtype == "class" {
                (top.name.clone(), top.children.iter().collect())
            } else {
                ("-".into(), vec![top])
            };
            for t in tops {
                if t.children.is_empty() {
                    groups.push((label.clone(), vec![t]));
                } else {
                    groups.push((t.name.clone(), t.leaves()));
                }
            }
        }
        for (gname, members) in groups {
            for (ci, chunk) in members.chunks(per_line).enumerate() {
                let head = if ci == 0 { fmt::fit(&gname, label_w - 1) } else { String::new() };
                let mut top = vec![Span::styled(format!("{head:<label_w$}"), theme::dim())];
                let mut bot = vec![Span::raw(" ".repeat(label_w))];
                for v in chunk {
                    let (x, txt) = value(&p.name, v);
                    let bg = if !x.is_finite() {
                        NO_DATA
                    } else {
                        match scale {
                            Scale::Flag(_) if x > 0.0 => theme::CRIT,
                            Scale::Flag(_) => Color::Indexed(23),
                            Scale::Fixed(lo, hi, _) => widgets::heat(((x - lo) / (hi - lo)).clamp(0.0, 1.0)),
                            Scale::Peak(_) => widgets::heat(if peak > 0.0 { x / peak } else { 0.0 }),
                        }
                    };
                    let bg = if v.state != "ONLINE" { theme::CRIT } else { bg };
                    let light = matches!(bg, Color::Indexed(n) if [79, 149, 220, 208, 196, 203].contains(&n));
                    let fg = if light { Color::Black } else { Color::Indexed(255) };
                    let name_fg = if light { Color::Indexed(236) } else { Color::Indexed(250) };
                    top.push(Span::styled(
                        format!(" {:<w$} ", short(&v.name), w = tile_w - 2),
                        Style::new().bg(bg).fg(name_fg),
                    ));
                    bot.push(Span::styled(
                        format!(" {:>w$} ", fmt::fit(&txt, tile_w - 2), w = tile_w - 2),
                        Style::new().bg(bg).fg(fg).add_modifier(Modifier::BOLD),
                    ));
                    top.push(Span::raw(" "));
                    bot.push(Span::raw(" "));
                }
                lines.push(Line::from(top));
                lines.push(Line::from(bot));
            }
            lines.push(Line::from(""));
        }
    }
    let mut legend = vec![Span::styled(format!("colour: {} ", metric.title()), theme::dim())];
    match scale {
        Scale::Flag(what) => {
            legend.push(Span::styled("  ", Style::new().bg(Color::Indexed(23))));
            legend.push(Span::styled(" none  ", theme::dim()));
            legend.push(Span::styled("  ", Style::new().bg(theme::CRIT)));
            legend.push(Span::styled(format!(" {what}"), theme::dim()));
        }
        _ => {
            for i in 0..=9 {
                legend.push(Span::styled("  ", Style::new().bg(widgets::heat(i as f64 / 9.0))));
            }
            let range = match scale {
                Scale::Peak(fmt) => format!("0 → {} (pool peak)", fmt(legend_max)),
                Scale::Fixed(_, _, label) => label.to_string(),
                Scale::Flag(_) => unreachable!(),
            };
            legend.push(Span::styled(format!(" {range}"), theme::dim()));
        }
    }
    if missing {
        legend.push(Span::styled("   ", theme::dim()));
        legend.push(Span::styled("  ", Style::new().bg(NO_DATA)));
        legend.push(Span::styled(" not reported", theme::dim()));
    }
    lines.push(Line::from(legend));
    f.render_widget(Paragraph::new(lines).block(panel(&format!("disk grid · {} (d/Esc back)", metric.title()))), area);
}

// ------------------------------------------------------------------------------------------------ history

fn history(f: &mut Frame, app: &mut App, area: Rect) {
    use crate::parse::history::Kind;
    let pool = app.current_pool().unwrap_or_default();
    let title = format!("history · {pool} · newest first (zpool history -il)");
    let entries = match app.model.history.get(&pool) {
        None => {
            f.render_widget(Paragraph::new("loading history…").style(theme::dim()).block(panel(&title)), area);
            return;
        }
        Some(Err(e)) => {
            f.render_widget(Paragraph::new(e.clone()).style(Style::new().fg(theme::WARN)).block(panel(&title)), area);
            return;
        }
        Some(Ok(v)) => v,
    };
    let filt = app.filter.to_lowercase();
    let rows: Vec<&crate::parse::history::Entry> = entries
        .iter()
        .rev()
        .filter(|e| filt.is_empty() || e.text.to_lowercase().contains(&filt) || e.time.contains(&filt))
        .collect();
    let sel = app.hist_sel.min(rows.len().saturating_sub(1));
    app.hist_sel = sel;
    let detail_h = rows.get(sel).map(|e| e.detail.len() as u16 + 3).unwrap_or(3).clamp(3, 12);
    let [lr, dr] = Layout::vertical([Constraint::Min(5), Constraint::Length(detail_h)]).areas(area);
    let ir = inner(lr);
    let body = ir.height.saturating_sub(1) as usize;
    let top = sel.saturating_sub(body.saturating_sub(1));
    let mut lines = vec![Line::from(Span::styled(
        format!("{:<20}{:>10}  {:<w$}", "TIME", "TXG", "EVENT", w = (ir.width as usize).saturating_sub(32)),
        theme::header(),
    ))];
    let ew = (ir.width as usize).saturating_sub(32);
    for (i, e) in rows.iter().enumerate().skip(top).take(body) {
        let (st, mark) = match e.kind {
            Kind::Command => (Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD), "$ "),
            Kind::Internal => (theme::dim(), "  "),
            Kind::Ioctl => (Style::new().fg(theme::ACCENT2), "⚙ "),
        };
        let txg = e.txg.map(|t| t.to_string()).unwrap_or_default();
        let l = Line::from(vec![
            Span::styled(format!("{:<20}", e.time.replace('.', " ")), theme::dim()),
            Span::styled(format!("{txg:>10}  "), Style::new().fg(theme::FAINT)),
            Span::styled(fmt::fit(&format!("{mark}{}", e.text), ew), st),
        ]);
        let y = ir.y + 1 + (i - top) as u16;
        app.hits.add(Rect::new(ir.x, y, ir.width, 1), Hit::Row(i));
        let l = super::full(l, ir.width as usize);
        lines.push(if i == sel { l.style(theme::sel()) } else { l });
    }
    let cmds = entries.iter().filter(|e| e.kind == Kind::Command).count();
    let t = format!(
        "{title} · {} entries ({cmds} commands){}",
        rows.len(),
        if filt.is_empty() { String::new() } else { format!(" matching '{filt}'") }
    );
    f.render_widget(Paragraph::new(lines).block(panel(&t)), lr);
    let mut dl = Vec::new();
    if let Some(e) = rows.get(sel) {
        dl.push(Line::from(Span::styled(e.text.clone(), theme::bold())));
        for d in &e.detail {
            dl.push(Line::from(Span::styled(d.clone(), theme::dim())));
        }
        if let Some(w) = &e.who {
            dl.push(Line::from(Span::styled(w.clone(), Style::new().fg(theme::FAINT))));
        }
    }
    f.render_widget(Paragraph::new(dl).wrap(ratatui::widgets::Wrap { trim: false }).block(panel("entry")), dr);
}
