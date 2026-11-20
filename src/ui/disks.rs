//! Disks tab: every leaf vdev resolved to its block device(s): enclosure slot, multipath legs, model/serial,
//! SMART health, temperature and wear, and live util/await from /proc/diskstats.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::overview::mirror_labeled;
use super::{Col, full, graph_title, header_line, inner, kv, panel, theme, widgets};
use crate::app::{App, DiskSort, Hit};
use crate::collect::DiskInfo;
use crate::fmt;
use crate::parse::disk::Smart;
use crate::parse::iostat::w;

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if app.model.disks.is_empty() {
        let msg = match (&app.model.disks_err, app.model.config.is_some()) {
            (Some(e), _) => e.clone(),
            (None, false) => "reading pool configuration…".into(),
            (None, true) => "resolving disks (sysfs)…".into(),
        };
        f.render_widget(Paragraph::new(msg).style(theme::dim()).block(panel("disks")), area);
        return;
    }
    if app.disk_detail {
        let [t, d] = Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(area);
        table(f, app, t);
        detail(f, app, d);
    } else {
        table(f, app, area);
    }
}

fn smart_of<'a>(app: &'a App, d: &DiskInfo) -> Option<&'a Result<Smart, String>> {
    app.model.smart.get(&d.smart_dev)
}

/// Compact age from power-on hours: `40d`, `1.4y`.
fn age(h: u64) -> String {
    let d = h as f64 / 24.0;
    if d < 365.0 { format!("{}d", d.round() as u64) } else { format!("{:.1}y", d / 365.25) }
}

fn temp_color(d: &DiskInfo, t: u64) -> Color {
    let (w, c) = if d.is_nvme() { (60.0, 70.0) } else { (45.0, 55.0) };
    theme::level(t as f64, w, c)
}

/// Health word + colour: FAIL (SMART says failing), WARN (smartctl flagged errors/prefail), ok, or `…` pending.
fn health(s: Option<&Result<Smart, String>>) -> (String, Style) {
    match s {
        None => ("…".into(), theme::dim()),
        Some(Err(_)) => ("n/a".into(), theme::dim()),
        Some(Ok(s)) => match s.passed {
            Some(false) => ("FAIL".into(), Style::new().fg(theme::CRIT).add_modifier(Modifier::BOLD)),
            _ if s.exit_status & 0b1111_0000 != 0 => ("warn".into(), Style::new().fg(theme::WARN)),
            Some(true) => ("ok".into(), Style::new().fg(theme::OK)),
            None => ("?".into(), theme::dim()),
        },
    }
}

fn legs_cell(d: &DiskInfo) -> (String, Style) {
    let n = d.legs.len();
    let bad = d.legs.iter().filter(|l| !l.state.is_empty() && l.state != "running" && l.state != "live").count();
    if n <= 1 && bad == 0 {
        return ("1".into(), theme::dim());
    }
    if bad > 0 {
        (format!("{}/{n}", n - bad), Style::new().fg(theme::CRIT).add_modifier(Modifier::BOLD))
    } else {
        (format!("{n}"), Style::new().fg(theme::OK))
    }
}

fn table(f: &mut Frame, app: &mut App, r: Rect) {
    let rows = app.disk_rows();
    let ir = inner(r);
    let w = ir.width as usize;
    app.disk_sel = app.disk_sel.min(rows.len().saturating_sub(1));
    let sel = app.disk_sel;
    let (ds, dd) = (app.disk_sort, app.disk_desc);
    let c = |t: &'static str, wd: usize, right: bool, k: DiskSort| {
        Col::new(t, wd, right).sort(Hit::DiskSort(k), (ds == k).then_some(dd))
    };
    let m = &app.model;
    let vw = rows.iter().map(|&i| m.disks[i].vdev.chars().count()).max().unwrap_or(8).clamp(8, 18) + 1;
    let mut cols = vec![c("SLOT", 9, false, DiskSort::Slot), c("VDEV", vw, false, DiskSort::Vdev)];
    if w >= 170 {
        cols.push(Col::new("IN", 11, false));
    }
    cols.extend([c("DEV", 9, false, DiskSort::Dev), Col::new("PATHS", 6, false)]);
    let model_w = if w >= 190 {
        26
    } else if w >= 150 {
        20
    } else {
        14
    };
    cols.push(c("MODEL", model_w, false, DiskSort::Model));
    if w >= 160 {
        cols.push(Col::new("SERIAL", 15, false));
    }
    cols.extend([
        c("SIZE ", 7, true, DiskSort::Size),
        Col::new("TYPE ", 6, true),
        c("TEMP ", 6, true, DiskSort::Temp),
        c("AGE ", 6, true, DiskSort::Hours),
        Col::new("SMART ", 7, true),
        c("DEFECT ", 8, true, DiskSort::Defects),
        c("ERR ", 6, true, DiskSort::Errors),
        c("UTIL ", 7, true, DiskSort::Util),
        c("AWAIT ", 8, true, DiskSort::Await),
        c("ZFS WAIT ", 10, true, DiskSort::Wait),
        c("r/s ", 7, true, DiskSort::Rops).color(theme::READ),
        c("w/s ", 7, true, DiskSort::Wops).color(theme::WRITE),
    ]);
    let used: usize = cols.iter().map(|c| c.w).sum();
    if w > used + 14 {
        cols.push(Col::new("util trend", 14, false));
    }
    let mut lines = vec![header_line(&cols, ir.x, ir.y, w, &mut app.hits)];
    let body = ir.height.saturating_sub(1) as usize;
    let top = if sel >= app.scroll + body { sel + 1 - body } else { app.scroll.min(sel) };
    app.scroll = top;
    let faint = Style::new().fg(theme::FAINT);
    for (ri, &i) in rows.iter().enumerate().skip(top).take(body) {
        let d = &m.disks[i];
        let y = ir.y + 1 + (ri - top) as u16;
        app.hits.add(Rect::new(ir.x, y, ir.width, 1), Hit::Row(ri));
        let smart = smart_of(app, d);
        let s = smart.and_then(|r| r.as_ref().ok());
        let rate = m.disk_rates.get(&d.whole);
        let io = m.io(&d.pool, &d.vdev);
        let mut spans = Vec::new();
        for col in &cols {
            let cell = |t: String| col.cell(&t);
            let sp = match col.title {
                "SLOT" => Span::styled(cell(d.slot().unwrap_or("-").to_string()), theme::dim()),
                "VDEV" => Span::styled(col.cell(&fmt::fit_left(&d.vdev, col.w - 1)), theme::bold()),
                "IN" => {
                    Span::styled(cell(if d.parent == "-" { d.class.clone() } else { d.parent.clone() }), theme::dim())
                }
                "DEV" => Span::styled(cell(d.whole.clone()), Style::new().fg(theme::ACCENT)),
                "PATHS" => {
                    let (t, st) = legs_cell(d);
                    Span::styled(col.cell(&format!(" {t}")), st)
                }
                "MODEL" => Span::raw(cell(s.and_then(|s| s.model.clone()).unwrap_or_else(|| d.model()))),
                "SERIAL" => Span::styled(
                    cell(
                        s.and_then(|s| s.serial.clone())
                            .or_else(|| d.nvme_serial.clone())
                            .unwrap_or_else(|| "-".into()),
                    ),
                    theme::dim(),
                ),
                "SIZE " => Span::raw(cell(format!("{} ", fmt::size(d.size)))),
                "TYPE " => Span::styled(cell(format!("{} ", d.kind())), theme::dim()),
                "TEMP " => match s.and_then(|s| s.temp_c) {
                    Some(t) => Span::styled(cell(format!("{t}°C ")), Style::new().fg(temp_color(d, t))),
                    None => Span::styled(cell("- ".into()), faint),
                },
                "AGE " => Span::styled(
                    cell(s.and_then(|s| s.power_on_hours).map(|h| format!("{} ", age(h))).unwrap_or("- ".into())),
                    theme::dim(),
                ),
                "SMART " => {
                    let (t, st) = health(smart);
                    Span::styled(cell(format!("{t} ")), st)
                }
                "DEFECT " => {
                    let v = s.and_then(|s| s.defects);
                    Span::styled(
                        cell(format!("{} ", v.map(|x| x.to_string()).unwrap_or("-".into()))),
                        if v.unwrap_or(0) > 0 { Style::new().fg(theme::WARN) } else { faint },
                    )
                }
                "ERR " => match s.and_then(|s| s.uncorrected) {
                    // SMART uncorrected / media errors only: the kernel's ioerr_cnt also counts benign command
                    // errors (probes, unit attentions) and is shown in the detail view instead
                    Some(v) => {
                        Span::styled(cell(format!("{v} ")), if v > 0 { Style::new().fg(theme::CRIT) } else { faint })
                    }
                    None => Span::styled(cell("- ".into()), faint),
                },
                "UTIL " => match rate {
                    Some(r) => Span::styled(
                        cell(format!("{} ", fmt::pct(r.util))),
                        Style::new().fg(theme::level(r.util, 70.0, 90.0)),
                    ),
                    None => Span::styled(cell("… ".into()), faint),
                },
                "AWAIT " => match rate {
                    Some(r) => {
                        let a = (r.r_await * r.rops + r.w_await * r.wops) / (r.rops + r.wops).max(1e-9);
                        Span::styled(cell(format!("{} ", fmt::ns(a * 1e6))), Style::new().fg(theme::latency(a * 1e6)))
                    }
                    None => Span::styled(cell("… ".into()), faint),
                },
                "ZFS WAIT " => {
                    let v = io
                        .map(|r| r.wait[w::DISK_R].unwrap_or(0.0).max(r.wait[w::DISK_W].unwrap_or(0.0)))
                        .unwrap_or(f64::NAN);
                    Span::styled(cell(format!("{} ", fmt::ns(v))), Style::new().fg(theme::latency(v)))
                }
                "r/s " => {
                    let v = io.map(|r| r.rops).or(rate.map(|r| r.rops)).unwrap_or(0.0);
                    Span::styled(
                        cell(format!("{} ", fmt::count(v))),
                        if v > 0.0 { Style::new().fg(theme::READ) } else { faint },
                    )
                }
                "w/s " => {
                    let v = io.map(|r| r.wops).or(rate.map(|r| r.wops)).unwrap_or(0.0);
                    Span::styled(
                        cell(format!("{} ", fmt::count(v))),
                        if v > 0.0 { Style::new().fg(theme::WRITE) } else { faint },
                    )
                }
                "util trend" => {
                    let t: Vec<f64> = m
                        .series
                        .get(&format!("dsk:{}:util", d.whole))
                        .map(|s| s.tail(13).collect())
                        .unwrap_or_default();
                    Span::styled(format!(" {}", fmt::spark(t, 13, 100.0)), Style::new().fg(theme::ACCENT))
                }
                _ => Span::raw(" ".repeat(col.w)),
            };
            spans.push(sp);
        }
        let l = full(Line::from(spans), w);
        lines.push(if ri == sel { l.style(theme::sel()) } else { l });
    }
    f.render_widget(Paragraph::new(lines).block(panel("").title(summary(app, rows.len()))), r);
}

/// Title: counts by type, temperature range, failing/erroring disks, SMART coverage.
fn summary(app: &App, shown: usize) -> Line<'static> {
    let m = &app.model;
    let n = m.disks.len();
    let mut kinds: Vec<(&str, usize)> = Vec::new();
    for d in &m.disks {
        match kinds.iter_mut().find(|k| k.0 == d.kind()) {
            Some(k) => k.1 += 1,
            None => kinds.push((d.kind(), 1)),
        }
    }
    let temps: Vec<u64> =
        m.disks.iter().filter_map(|d| smart_of(app, d).and_then(|r| r.as_ref().ok()).and_then(|s| s.temp_c)).collect();
    let fails = m.disks.iter().filter(|d| health(smart_of(app, d)).0 == "FAIL").count();
    let warns = m.disks.iter().filter(|d| health(smart_of(app, d)).0 == "warn").count();
    let smart_ok = m.disks.iter().filter(|d| matches!(smart_of(app, d), Some(Ok(_)))).count();
    let smart_err = m.disks.iter().find_map(|d| match smart_of(app, d) {
        Some(Err(e)) => Some(e.clone()),
        _ => None,
    });
    let degraded = m.disks.iter().filter(|d| legs_cell(d).1.fg == Some(theme::CRIT)).count();
    let mut spans = vec![
        Span::styled(" disks ", Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!(
                "{n} ({}){} ",
                kinds.iter().map(|(k, c)| format!("{c} {k}")).collect::<Vec<_>>().join(", "),
                if shown != n { format!(" · {shown} shown") } else { String::new() }
            ),
            theme::bold(),
        ),
    ];
    if let (Some(lo), Some(hi)) = (temps.iter().min(), temps.iter().max()) {
        spans.push(Span::styled(format!("· temp {lo}–{hi}°C "), theme::dim()));
    }
    spans.push(Span::styled(
        format!("· SMART {smart_ok}/{n} "),
        if smart_ok == n { theme::dim() } else { Style::new().fg(theme::WARN) },
    ));
    if fails > 0 {
        spans.push(Span::styled(
            format!("· {fails} FAILING "),
            Style::new().fg(theme::CRIT).add_modifier(Modifier::BOLD),
        ));
    }
    if warns > 0 {
        spans.push(Span::styled(format!("· {warns} with SMART warnings "), Style::new().fg(theme::WARN)));
    }
    if degraded > 0 {
        spans.push(Span::styled(format!("· {degraded} with failed paths "), Style::new().fg(theme::CRIT)));
    }
    if smart_ok == 0
        && let Some(e) = smart_err
    {
        spans.push(Span::styled(format!("· {} ", fmt::fit(&e, 60)), Style::new().fg(theme::WARN)));
    }
    Line::from(spans)
}

fn detail(f: &mut Frame, app: &mut App, r: Rect) {
    let rows = app.disk_rows();
    let Some(d) = rows.get(app.disk_sel).map(|&i| app.model.disks[i].clone()) else { return };
    let [info_r, right] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(r);
    let [g1, g2] = Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(right);
    info(f, app, &d, info_r);
    let m = &app.model;
    // kernel view: util ▲ (0-100%) / await ▼
    let ir = inner(g1);
    let n = widgets::samples(ir.width);
    let get = |k: String| -> Vec<f64> { m.series.get(&k).map(|s| s.tail(n).collect()).unwrap_or_default() };
    let (u, aw) = (get(format!("dsk:{}:util", d.whole)), get(format!("dsk:{}:await", d.whole)));
    let amax = aw.iter().copied().filter(|x| x.is_finite()).fold(0.0, f64::max);
    f.render_widget(
        panel("").title(graph_title(
            &format!("{} (kernel)", d.whole),
            format!("util {}", fmt::pct(m.series.last(&format!("dsk:{}:util", d.whole)))),
            format!("await {}", fmt::ns(m.series.last(&format!("dsk:{}:await", d.whole)) * 1e6)),
        )),
        g1,
    );
    mirror_labeled(
        f.buffer_mut(),
        ir,
        (&u, theme::ACCENT, "▲ util · top 100%".into()),
        (&aw, theme::WARN, format!("▼ await · bottom {}", fmt::ns(amax * 1e6))),
        100.0,
        amax,
    );
    // ZFS view: ops from zpool iostat
    let ir = inner(g2);
    let k = crate::model::vkey(&d.pool, &d.vdev);
    let (ro, wo) = (get(format!("io:{k}:rops")), get(format!("io:{k}:wops")));
    let mx = ro.iter().chain(wo.iter()).copied().filter(|x| x.is_finite()).fold(0.0, f64::max);
    f.render_widget(
        panel("").title(graph_title(
            &format!("{} (zfs)", d.vdev),
            format!("r {}", fmt::count(ro.last().copied().unwrap_or(0.0))),
            format!("w {}", fmt::count(wo.last().copied().unwrap_or(0.0))),
        )),
        g2,
    );
    mirror_labeled(
        f.buffer_mut(),
        ir,
        (&ro, theme::READ, format!("▲ r/s · top {}", fmt::count(mx))),
        (&wo, theme::WRITE, format!("▼ w/s · bottom {}", fmt::count(mx))),
        mx,
        mx,
    );
}

fn info(f: &mut Frame, app: &mut App, d: &DiskInfo, r: Rect) {
    let m = &app.model;
    let row = |k: &str, v: String| Line::from(kv(&format!("{k:<12}"), v, None));
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{} ", d.vdev), Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(format!("in {} · {} · {}", d.parent, d.class, d.pool), theme::dim()),
        ]),
        row("path", d.path.clone()),
        row(
            "device",
            format!(
                "{}{}{}",
                d.dev,
                if d.dev != d.whole { format!(" on {}", d.whole) } else { String::new() },
                d.dm_name.as_ref().map(|n| format!(" (dm {n})")).unwrap_or_default()
            ),
        ),
        row(
            "queue",
            format!(
                "{} · logical {} / physical {} · {}",
                d.scheduler.clone().unwrap_or("-".into()),
                d.logical_bs.map(fmt::size).unwrap_or("-".into()),
                d.physical_bs.map(fmt::size).unwrap_or("-".into()),
                d.kind()
            ),
        ),
    ];
    if let Some((target, comment)) = &d.alias {
        lines.push(row("vdev_id", format!("{target} {comment}")));
    }
    for l in &d.legs {
        let ok = l.state.is_empty() || l.state == "running" || l.state == "live";
        let tmo = l.iotmo.unwrap_or(0);
        let mut sp = vec![
            Span::styled(format!("{:<12} ", if d.legs.len() > 1 { "path" } else { "device" }), theme::dim()),
            Span::styled(format!("{:<9} ", l.dev), Style::new().fg(theme::ACCENT)),
            Span::styled(
                format!("{:<8} ", if l.state.is_empty() { "-" } else { &l.state }),
                Style::new().fg(if ok { theme::OK } else { theme::CRIT }),
            ),
        ];
        if !l.sas_address.is_empty() {
            sp.push(Span::styled(format!("sas {} ", l.sas_address), theme::dim()));
        }
        if l.ioerr.is_some() || l.iotmo.is_some() {
            // ioerr_cnt includes benign command errors (probes, unit attentions); timeouts are the worrying part
            sp.push(Span::styled(format!("cmd errors {} ", l.ioerr.unwrap_or(0)), theme::dim()));
            sp.push(Span::styled(
                format!("timeouts {tmo}"),
                if tmo > 0 { Style::new().fg(theme::WARN) } else { theme::dim() },
            ));
        }
        lines.push(Line::from(sp));
    }
    lines.push(Line::from(""));
    match m.smart.get(&d.smart_dev) {
        None => lines.push(Line::from(Span::styled(format!("SMART: reading /dev/{}…", d.smart_dev), theme::dim()))),
        Some(Err(e)) => lines.push(Line::from(Span::styled(format!("SMART: {e}"), Style::new().fg(theme::WARN)))),
        Some(Ok(s)) => {
            let (h, hs) = health(Some(&Ok(s.clone())));
            lines.push(Line::from(vec![
                Span::styled(format!("{:<12} ", "SMART"), theme::dim()),
                Span::styled(format!("{h} "), hs),
                Span::raw(format!(
                    "{} · SN {} · fw {}",
                    s.model.clone().unwrap_or_default(),
                    s.serial.clone().unwrap_or("-".into()),
                    s.firmware.clone().unwrap_or("-".into())
                )),
            ]));
            let mut facts = Vec::new();
            if let Some(t) = s.temp_c {
                facts.push(format!("{t}°C"));
            }
            if let Some(h) = s.power_on_hours {
                facts.push(format!("{} h on ({})", fmt::thousands(h), age(h)));
            }
            if let Some(rpm) = s.rotation_rpm.filter(|r| *r > 0) {
                facts.push(format!("{rpm} rpm"));
            }
            if let Some(w) = s.wear_pct {
                facts.push(format!("{w}% endurance used"));
            }
            if let Some(sp) = s.nvme_spare_pct {
                facts.push(format!("{sp}% spare"));
            }
            if let (Some(rd), Some(wr)) = (s.read_tb, s.written_tb) {
                facts.push(format!("read {rd:.1} TB · written {wr:.1} TB"));
            }
            lines.push(row("", facts.join(" · ")));
            let skip = app.disk_scroll.min(s.details.len().saturating_sub(1));
            let room = (inner(r).height as usize).saturating_sub(lines.len() + 1);
            for (k, v) in s.details.iter().skip(skip).take(room) {
                lines.push(Line::from(vec![Span::styled(format!("  {k:<32}"), theme::dim()), Span::raw(v.clone())]));
            }
            if s.details.len() > room {
                lines.push(Line::from(Span::styled(
                    format!("  … {} attributes · J/K or PgUp/PgDn scroll", s.details.len()),
                    Style::new().fg(theme::FAINT),
                )));
            }
        }
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(panel("disk")), r);
}
