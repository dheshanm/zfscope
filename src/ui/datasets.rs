//! Datasets tab: tree (or flat "ztop" list) with space breakdown and live IO; detail panes for space, IO/ZIL,
//! snapshots and properties.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::overview::mirror_labeled;
use super::widgets;
use super::{Col, header_line, inner, kv, panel, props, theme};
use crate::app::{App, DsPane, DsSort, Hit};
use crate::fmt;
use crate::parse::zjson::{Dataset, DsKind, PropsExt};

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if app.model.datasets().is_empty() {
        let msg = match app.model.config.as_ref().map(|c| &c.datasets) {
            None => "reading datasets (zfs list)…".to_string(),
            Some(Err(e)) => e.clone(),
            Some(Ok(_)) => "no datasets".into(),
        };
        f.render_widget(Paragraph::new(msg).style(theme::dim()).block(panel("datasets")), area);
        return;
    }
    if app.ds_detail {
        let [t, d] = Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area);
        table(f, app, t);
        detail(f, app, d);
    } else {
        table(f, app, area);
    }
}

/// Space bar parts for a dataset: (value, glyph, colour).
fn space_parts(d: &Dataset) -> [(f64, char, ratatui::style::Color); 4] {
    let p = &d.props;
    [
        (p.n("usedbydataset") as f64, '█', theme::SP_DS),
        (p.n("usedbysnapshots") as f64, '▓', theme::SP_SNAP),
        (p.n("usedbychildren") as f64, '▒', theme::SP_CHILD),
        (p.n("usedbyrefreservation") as f64, '░', theme::SP_REFRES),
    ]
}

/// Fill of the tightest quota: (fraction, label) — refquota vs referenced, quota vs used.
fn quota(d: &Dataset) -> Option<(f64, String)> {
    let p = &d.props;
    let rq = p.n("refquota");
    let q = p.n("quota");
    let a = (rq > 0).then(|| (p.n("referenced") as f64 / rq as f64, format!("refquota {}", fmt::size(rq))));
    let b = (q > 0).then(|| (p.n("used") as f64 / q as f64, format!("quota {}", fmt::size(q))));
    match (a, b) {
        (Some(a), Some(b)) => Some(if a.0 >= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

fn table(f: &mut Frame, app: &mut App, r: Rect) {
    let rows = app.ds_rows();
    let ir = inner(r);
    let m = &app.model;
    let sel = rows.iter().position(|x| Some(&x.name) == app.ds_sel.as_ref()).unwrap_or(0);
    let w = ir.width as usize;
    let flat = app.ds_flat;
    let name_w = rows
        .iter()
        .map(|x| {
            let leaf = if flat { x.name.as_str() } else { x.name.rsplit('/').next().unwrap_or(&x.name) };
            x.depth * 2 + leaf.chars().count() + 3
        })
        .max()
        .unwrap_or(12)
        .clamp(16, if flat { 48 } else { 40 });
    let (sort, desc) = (app.ds_sort, app.ds_desc);
    let c = |title: &'static str, wd: usize, right: bool, k: DsSort| {
        Col::new(title, wd, right).sort(Hit::DsSort(k), (sort == k).then_some(desc))
    };
    let rest = w.saturating_sub(name_w);
    let mut cols = vec![c(if flat { "DATASET" } else { "NAME" }, name_w, false, DsSort::Name)];
    if flat {
        cols.extend([
            c("r/s", 9, true, DsSort::Rops).color(theme::READ),
            c("w/s", 9, true, DsSort::Wops).color(theme::WRITE),
            c("read", 10, true, DsSort::Read).color(theme::READ),
            c("write", 10, true, DsSort::Write).color(theme::WRITE),
            c("unlink/s", 10, true, DsSort::Unlinks),
            c("zil c/s", 9, true, DsSort::ZilCommits).color(theme::ACCENT2),
            c("zil B/s", 10, true, DsSort::ZilBytes).color(theme::ACCENT2),
            Col::new("  trend", 18, false),
        ]);
    } else {
        cols.extend([
            c("USED", 8, true, DsSort::Used),
            c("AVAIL", 8, true, DsSort::Avail),
            c("REFER", 8, true, DsSort::Refer),
            c("SNAP", 8, true, DsSort::Snap).color(theme::SP_SNAP),
        ]);
        if rest >= 120 {
            cols.push(c("LUSED", 8, true, DsSort::Lused));
        }
        cols.push(c("RATIO", 7, true, DsSort::Ratio));
        if rest >= 110 {
            cols.push(c("  QUOTA", 16, false, DsSort::Quota));
        }
        if rest >= 90 {
            cols.push(Col::new("  SHARE OF PARENT", 18, false));
        }
        cols.push(c("read", 10, true, DsSort::Read).color(theme::READ));
        cols.push(c("write", 10, true, DsSort::Write).color(theme::WRITE));
    }
    let header = header_line(&cols, ir.x, ir.y, w, &mut app.hits);
    let mut lines = vec![header];
    let body_h = ir.height.saturating_sub(1) as usize;
    let top = if sel >= app.scroll + body_h { sel + 1 - body_h } else { app.scroll.min(sel) };
    app.scroll = top;
    let faint = Style::new().fg(theme::FAINT);
    for (i, row) in rows.iter().enumerate().skip(top).take(body_h) {
        let y = ir.y + 1 + (i - top) as u16;
        app.hits.add(Rect::new(ir.x, y, ir.width, 1), Hit::Row(i));
        let Some(d) = m.dataset(&row.name) else { continue };
        let io = m.ds_io.get(&row.name).cloned().unwrap_or_default();
        let label = if flat {
            row.name.clone()
        } else {
            let fold = if row.has_children { if row.collapsed { "▸ " } else { "▾ " } } else { "  " };
            if row.has_children {
                // the fold marker is its own click target
                app.hits.add(Rect::new(ir.x + (row.depth * 2) as u16, y, 2, 1), Hit::Fold(i));
            }
            format!("{}{}{}", "  ".repeat(row.depth), fold, row.name.rsplit('/').next().unwrap_or(&row.name))
        };
        let style = if d.kind == DsKind::Volume { Style::new().fg(theme::ACCENT2) } else { Style::new() };
        let mut spans = vec![Span::styled(format!("{:<name_w$}", fmt::fit_left(&label, name_w - 1)), style)];
        let p = &d.props;
        let num = |v: f64, txt: String, col: ratatui::style::Color, cw: usize| {
            Span::styled(fmt::rpad(&txt, cw), if v > 0.0 { Style::new().fg(col) } else { faint })
        };
        for col in cols.iter().skip(1) {
            let cw = col.w;
            let sp = match col.title {
                "r/s" => num(io.rops, fmt::count(io.rops), theme::READ, cw),
                "w/s" => num(io.wops, fmt::count(io.wops), theme::WRITE, cw),
                "read" => num(io.rbw, fmt::rate(io.rbw), theme::READ, cw),
                "write" => num(io.wbw, fmt::rate(io.wbw), theme::WRITE, cw),
                "unlink/s" => num(io.unlinks, fmt::count(io.unlinks), theme::ACCENT, cw),
                "zil c/s" => num(io.zil_commits, fmt::count(io.zil_commits), theme::ACCENT2, cw),
                "zil B/s" => num(io.zil_bytes(), fmt::rate(io.zil_bytes()), theme::ACCENT2, cw),
                "  trend" => {
                    let tr: Vec<f64> = match (
                        m.series.get(&format!("ds:{}:r", row.name)),
                        m.series.get(&format!("ds:{}:w", row.name)),
                    ) {
                        (Some(a), Some(b)) => a.tail(16).zip(b.tail(16)).map(|(x, y)| x + y).collect(),
                        _ => Vec::new(),
                    };
                    Span::styled(format!("  {}", fmt::spark(tr, 16, 0.0)), Style::new().fg(theme::ACCENT))
                }
                "USED" => Span::raw(col.cell(&fmt::size(p.n("used")))),
                "AVAIL" => Span::styled(col.cell(&fmt::size(p.n("available"))), theme::dim()),
                "REFER" => Span::raw(col.cell(&fmt::size(p.n("referenced")))),
                "SNAP" => {
                    let s = p.n("usedbysnapshots");
                    Span::styled(
                        col.cell(&fmt::size_or_dash(s)),
                        if s > 0 { Style::new().fg(theme::SP_SNAP) } else { faint },
                    )
                }
                "LUSED" => Span::styled(col.cell(&fmt::size(p.n("logicalused"))), theme::dim()),
                "RATIO" => Span::raw(col.cell(&format!("{}x", p.str("compressratio").trim_end_matches('x')))),
                "  QUOTA" => {
                    spans.push(Span::raw("  "));
                    match quota(d) {
                        Some((fr, _)) => {
                            spans.extend(widgets::meter(9, fr, 0.8, 0.95));
                            Span::styled(
                                format!("{:>5}", fmt::pct(fr * 100.0)),
                                Style::new().fg(theme::level(fr, 0.8, 0.95)),
                            )
                        }
                        None => Span::styled(format!("{:<14}", "·"), faint),
                    }
                }
                "  SHARE OF PARENT" => {
                    // Bar length = share of the parent's `used` (a root is full); segments = how it is made up.
                    spans.push(Span::raw("  "));
                    let parent_used = row
                        .name
                        .rsplit_once('/')
                        .and_then(|(par, _)| m.dataset(par))
                        .map(|pd| pd.props.n("used"))
                        .unwrap_or(p.n("used"))
                        .max(1) as f64;
                    let len = ((p.n("used") as f64 / parent_used) * 16.0).ceil().clamp(1.0, 16.0) as usize;
                    let mut bar = widgets::stacked(len, &space_parts(d), p.n("used") as f64);
                    bar.retain(|s| !s.content.starts_with('·'));
                    let bw: usize = bar.iter().map(|s| s.width()).sum();
                    spans.extend(bar);
                    Span::styled("·".repeat(16usize.saturating_sub(bw)), Style::new().fg(theme::TRACK))
                }
                _ => Span::raw(" ".repeat(cw)),
            };
            spans.push(sp);
        }
        let st = match (i == sel, app.ds_detail && app.ds_focus_pane) {
            (true, false) => theme::sel(),
            (true, true) => theme::sel_dim(),
            _ => Style::new(),
        };
        lines.push(super::full(Line::from(spans), w).style(st));
    }
    let mode = if flat { "ztop" } else { "tree" };
    let dir = if desc { "▼" } else { "▲" };
    let title = format!("datasets · {mode} · sort {} {dir} · {}/{}", sort.title(), sel + 1, rows.len());
    let block = if app.ds_detail { super::panel_focus(&title, !app.ds_focus_pane) } else { panel(&title) };
    f.render_widget(Paragraph::new(lines).block(block), r);
}

fn detail(f: &mut Frame, app: &mut App, r: Rect) {
    let Some(name) = app.ds_sel.clone() else { return };
    let Some(d) = app.model.dataset(&name).cloned() else { return };
    // sub-tab bar
    let mut spans = vec![Span::styled(format!(" {name} "), theme::bold())];
    let mut x = r.x + spans[0].width() as u16 + 1;
    for (i, p) in DsPane::ALL.iter().enumerate() {
        let label = format!(" {} ", p.title());
        let wdt = label.chars().count() as u16;
        app.hits.add(Rect::new(x, r.y, wdt, 1), Hit::SubTab(i));
        x += wdt + 1;
        let st = if *p == app.ds_pane {
            Style::new().bg(theme::ACCENT2).fg(ratatui::style::Color::Black).add_modifier(Modifier::BOLD)
        } else {
            theme::dim()
        };
        spans.push(Span::styled(label, st));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(" [ ] panes · Tab focus list ↔ pane · P props tab · Enter close", theme::dim()));
    let [bar, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).areas(r);
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::new().bg(theme::BG_DIM)), bar);
    match app.ds_pane {
        DsPane::Space => space(f, app, &d, body),
        DsPane::Io => io(f, app, &d, body),
        DsPane::Snapshots => snapshots(f, app, &d, body),
        DsPane::Props => {
            let sel = app.snap_sel;
            let focused = app.ds_focus_pane;
            app.snap_sel =
                props::table(f, app, &d.props, false, sel, body, "properties", Hit::PaneRow, focused, "", &d.name);
        }
    }
}

fn space(f: &mut Frame, _app: &App, d: &Dataset, r: Rect) {
    let p = &d.props;
    let ir = inner(r);
    let w = ir.width as usize;
    let used = p.n("used") as f64;
    let mut lines = Vec::new();
    let parts = space_parts(d);
    let mut bar = widgets::stacked(w.saturating_sub(2), &parts, used);
    bar.insert(0, Span::raw(" "));
    lines.push(Line::from(bar));
    let labels = [
        ("dataset", 0, "usedbydataset"),
        ("snapshots", 1, "usedbysnapshots"),
        ("children", 2, "usedbychildren"),
        ("refreserv", 3, "usedbyrefreservation"),
    ];
    let mut legend = vec![Span::raw(" ")];
    for (l, i, k) in labels {
        let v = p.n(k);
        legend.push(Span::styled(format!("{} ", parts[i].1), Style::new().fg(parts[i].2)));
        legend.push(Span::styled(format!("{l} "), theme::dim()));
        legend.push(Span::raw(format!("{}  ", fmt::size(v))));
    }
    lines.push(Line::from(legend));
    lines.push(Line::from(""));
    let row = |a: Vec<Span<'static>>| Line::from([vec![Span::raw(" ")], a].concat());
    lines.push(row([
        kv("used", fmt::size(p.n("used")), None),
        vec![Span::raw("   ")],
        kv("available", fmt::size(p.n("available")), None),
        vec![Span::raw("   ")],
        kv("referenced", fmt::size(p.n("referenced")), None),
        vec![Span::raw("   ")],
        kv("written since last snapshot", fmt::size(p.n("written")), None),
    ]
    .concat()));
    lines.push(row([
        kv("logical used", fmt::size(p.n("logicalused")), None),
        vec![Span::raw("   ")],
        kv("compression", format!("{} → {}", p.str("compression"), p.str("compressratio")), None),
        vec![Span::raw("   ")],
        kv(
            "recordsize",
            if d.kind == DsKind::Volume { fmt::size(p.n("volblocksize")) } else { fmt::size(p.n("recordsize")) },
            None,
        ),
        vec![Span::raw("   ")],
        kv("dedup", p.str("dedup").to_string(), None),
    ]
    .concat()));
    let meters = [
        ("quota", p.n("quota"), p.n("used")),
        ("refquota", p.n("refquota"), p.n("referenced")),
        ("reservation", p.n("reservation"), p.n("used")),
        ("refreservation", p.n("refreservation"), p.n("referenced")),
    ];
    lines.push(Line::from(""));
    for (name, lim, val) in meters {
        if lim == 0 {
            lines.push(row(kv(&format!("{name:<15}"), "none", Some(theme::DIM))));
            continue;
        }
        let fr = val as f64 / lim as f64;
        let mut l = vec![Span::raw(" "), Span::styled(format!("{name:<15} "), theme::dim())];
        l.extend(widgets::meter(30, fr, 0.8, 0.95));
        l.push(Span::raw(format!(" {} of {} ({})", fmt::size(val), fmt::size(lim), fmt::pct(fr * 100.0))));
        lines.push(Line::from(l));
    }
    lines.push(Line::from(""));
    lines.push(row([
        kv("mountpoint", p.str("mountpoint").to_string(), None),
        vec![Span::raw("   ")],
        kv("sync", p.str("sync").to_string(), Some(if p.str("sync") == "disabled" { theme::WARN } else { theme::DIM })),
        vec![Span::raw("   ")],
        kv("encryption", p.str("encryption").to_string(), None),
        vec![Span::raw("   ")],
        kv("special_small_blocks", fmt::size(p.n("special_small_blocks")), None),
    ]
    .concat()));
    if let Some(o) = p.get("origin").filter(|o| o.value != "-" && !o.value.is_empty()) {
        lines.push(row(kv("clone of", o.value.clone(), Some(theme::ACCENT2))));
    }
    f.render_widget(Paragraph::new(lines).block(panel("space")), r);
}

fn io(f: &mut Frame, app: &App, d: &Dataset, r: Rect) {
    let m = &app.model;
    let [g, z] = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).areas(r);
    let [ops_r, bw_r] = Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(g);
    let io = m.ds_io.get(&d.name).cloned().unwrap_or_default();
    for (rr, title, rk, wk, fm) in [
        (ops_r, "ops", "ro", "wo", &(|v: f64| fmt::count(v)) as &dyn Fn(f64) -> String),
        (bw_r, "bandwidth", "r", "w", &(fmt::rate as fn(f64) -> String) as &dyn Fn(f64) -> String),
    ] {
        let ir = inner(rr);
        let gw = widgets::samples(ir.width);
        let a: Vec<f64> =
            m.series.get(&format!("ds:{}:{rk}", d.name)).map(|s| s.tail(gw).collect()).unwrap_or_default();
        let b: Vec<f64> =
            m.series.get(&format!("ds:{}:{wk}", d.name)).map(|s| s.tail(gw).collect()).unwrap_or_default();
        let mx = a.iter().chain(b.iter()).copied().fold(0.0, f64::max);
        let t = Line::from(vec![
            Span::styled(format!(" {title} "), Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("r {} ", fm(a.last().copied().unwrap_or(0.0))),
                Style::new().fg(theme::READ).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("w {} ", fm(b.last().copied().unwrap_or(0.0))),
                Style::new().fg(theme::WRITE).add_modifier(Modifier::BOLD),
            ),
        ]);
        f.render_widget(panel("").title(t), rr);
        if a.is_empty() {
            f.render_widget(Paragraph::new("no objset kstat for this dataset (not mounted?)").style(theme::dim()), ir);
            continue;
        }
        mirror_labeled(
            f.buffer_mut(),
            ir,
            (&a, theme::READ, format!("▲ read · top {}", fm(mx))),
            (&b, theme::WRITE, format!("▼ write · bottom {}", fm(mx))),
            mx,
            mx,
        );
    }
    let rows = [
        ("commits/s", fmt::count(io.zil_commits)),
        ("commit writers/s", fmt::count(io.zil_writers)),
        ("itx/s", fmt::count(io.zil_itx)),
        ("log bytes/s", fmt::rate(io.zil_bytes())),
        ("  indirect", fmt::rate(io.zil_indirect_b)),
        ("  copied", fmt::rate(io.zil_copied_b)),
        ("  needcopy", fmt::rate(io.zil_needcopy_b)),
        ("to SLOG", fmt::rate(io.zil_slog_b)),
        ("to pool (no SLOG)", fmt::rate(io.zil_normal_b)),
        ("unlinks/s", fmt::count(io.unlinks)),
    ];
    let mut lines: Vec<Line> = rows
        .iter()
        .map(|(k, v)| Line::from(vec![Span::styled(format!(" {k:<20}"), theme::dim()), Span::raw(v.clone())]))
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(" sync={} logbias={}", d.props.str("sync"), d.props.str("logbias")),
        theme::dim(),
    )));
    lines.push(Line::from(Span::styled(
        " indirect = written to the pool, log holds a pointer; copied/needcopy = data in the log itself",
        theme::dim(),
    )));
    f.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }).block(panel("ZIL (sync writes)")),
        z,
    );
}

fn snapshots(f: &mut Frame, app: &mut App, d: &Dataset, r: Rect) {
    let ir = inner(r);
    let snaps = match app.model.snapshots.get(&d.name) {
        None => {
            f.render_widget(Paragraph::new("loading snapshots…").style(theme::dim()).block(panel("snapshots")), r);
            return;
        }
        Some(Err(e)) => {
            f.render_widget(Paragraph::new(e.clone()).style(Style::new().fg(theme::WARN)).block(panel("snapshots")), r);
            return;
        }
        Some(Ok(v)) => v,
    };
    app.hits.add(r, Hit::PaneArea);
    let mut v: Vec<&Dataset> = snaps.iter().collect();
    v.sort_by_key(|s| std::cmp::Reverse(s.createtxg));
    let now = jiff::Timestamp::now().as_second();
    let total_used: u64 = v.iter().map(|s| s.props.n("used")).sum();
    let max_used = v.iter().map(|s| s.props.n("used")).max().unwrap_or(0).max(1);
    let nw = (ir.width as usize).saturating_sub(70).clamp(16, 60);
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "{:<nw$}{:>9}{:>9}{:>9}  {:<17}{:>11}  {}",
            "SNAPSHOT", "USED", "REFER", "WRITTEN", "CREATED", "AGE", "unique"
        ),
        theme::header(),
    ))];
    let body = ir.height.saturating_sub(1) as usize;
    app.snap_sel = app.snap_sel.min(v.len().saturating_sub(1));
    let top = app.snap_sel.saturating_sub(body.saturating_sub(1));
    for (i, s) in v.iter().enumerate().skip(top).take(body) {
        let p = &s.props;
        let short = s.name.split_once('@').map(|x| x.1).unwrap_or(&s.name);
        let used = p.n("used");
        let created = p.opt("creation").unwrap_or(0) as i64;
        let mut l = vec![
            Span::raw(format!("{:<nw$}", fmt::fit(short, nw - 1))),
            Span::styled(format!("{:>9}", fmt::size(used)), Style::new().fg(theme::SP_SNAP)),
            Span::raw(format!("{:>9}", fmt::size(p.n("referenced")))),
            Span::styled(format!("{:>9}", fmt::size(p.n("written"))), theme::dim()),
            Span::raw(format!("  {:<17}", fmt::date(created))),
            Span::styled(format!("{:>11}  ", fmt::age(created, now)), theme::dim()),
        ];
        l.extend(widgets::gauge(12, used as f64 / max_used as f64, theme::SP_SNAP));
        if p.str("userrefs") != "0" && p.str("userrefs") != "-" {
            l.push(Span::styled(format!(" held×{}", p.str("userrefs")), Style::new().fg(theme::WARN)));
        }
        let y = ir.y + 1 + (i - top) as u16;
        app.hits.add(Rect::new(ir.x, y, ir.width, 1), Hit::PaneRow(i));
        let st = match (i == app.snap_sel, app.ds_focus_pane) {
            (true, true) => theme::sel(),
            (true, false) => theme::sel_dim(),
            _ => Style::new(),
        };
        lines.push(super::full(Line::from(l), ir.width as usize).style(st));
    }
    if v.is_empty() {
        lines.push(Line::from(Span::styled("no snapshots", theme::dim())));
    }
    let title = format!(
        "snapshots · {} · unique {} · all snapshots hold {}",
        v.len(),
        fmt::size(total_used),
        fmt::size(d.props.n("usedbysnapshots"))
    );
    f.render_widget(Paragraph::new(lines).block(super::panel_focus(&title, app.ds_focus_pane)), r);
}
