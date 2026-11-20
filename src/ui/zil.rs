//! ZIL·TXG tab: sync-write logging (zilstat) per pool and per dataset; transaction groups (txgs kstat) and the
//! write throttle (dmu_tx); the tunables that govern them.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::overview::mirror_labeled;
use super::{full, graph_title, inner, kv, panel, params, theme, widgets};
use crate::app::App;
use crate::fmt;
use crate::model::Model;

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if !app.model.has_kstats() {
        f.render_widget(Paragraph::new("no kstats yet").style(theme::dim()).block(panel("ZIL · TXG")), area);
        return;
    }
    let [left, right] = Layout::horizontal([Constraint::Percentage(52), Constraint::Percentage(48)]).areas(area);
    let [zs, zg, zt] =
        Layout::vertical([Constraint::Length(11), Constraint::Length(10), Constraint::Min(5)]).areas(left);
    zil_summary(f, &app.model, zs);
    zil_graph(f, &app.model, zg);
    zil_datasets(f, &app.model, zt);

    let pools: Vec<String> = app.model.pools().iter().map(|p| p.name.clone()).collect();
    let pool = if pools.is_empty() {
        app.model.pool_names.first().cloned().unwrap_or_default()
    } else {
        pools[app.txg_pool % pools.len()].clone()
    };
    let [tc, tg, tt, th, tp] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Length(10),
        Constraint::Min(6),
        Constraint::Length(12),
        Constraint::Length(13),
    ])
    .areas(right);
    txg_card(f, &app.model, &pool, pools.len(), tc);
    txg_graph(f, &app.model, &pool, tg);
    txg_table(f, &app.model, &pool, tt);
    throttle(f, &app.model, &pool, th);
    params::draw(f, app, tp, "TXG / write tunables", &crate::help::TXG_PARAMS);
}

fn zil_summary(f: &mut Frame, m: &Model, r: Rect) {
    let rr = |k: &str| m.rate("zil", k);
    let tot = |k: &str| m.val("zil", k);
    let commits = rr("zil_commit_count");
    let writers = rr("zil_commit_writer_count");
    let mut lines = vec![Line::from(
        [
            kv("commits/s", fmt::count(commits), Some(theme::ACCENT2)),
            vec![Span::raw("   ")],
            kv("as writer", fmt::count(writers), None),
            vec![Span::raw("   ")],
            kv("itx/s", fmt::count(rr("zil_itx_count")), None),
        ]
        .concat(),
    )];
    lines.push(Line::from(Span::styled(
        format!("{:<26}{:>10}{:>11}", "log records (itx) by kind", "count/s", "bytes/s"),
        theme::header(),
    )));
    for (label, k, note) in [
        ("indirect", "zil_itx_indirect", "data to pool, log keeps a pointer"),
        ("copied", "zil_itx_copied", "data copied into the log"),
        ("needcopy", "zil_itx_needcopy", "data copied at commit"),
    ] {
        lines.push(Line::from(vec![
            Span::raw(format!("{label:<26}")),
            Span::raw(format!("{:>10}", fmt::count(rr(&format!("{k}_count"))))),
            Span::raw(format!("{:>11}", fmt::rate(rr(&format!("{k}_bytes"))))),
            Span::styled(format!("  {note}"), theme::dim()),
        ]));
    }
    for (label, k) in [("→ SLOG", "zil_itx_metaslab_slog"), ("→ pool (no SLOG)", "zil_itx_metaslab_normal")] {
        let (b, wr, al) = (rr(&format!("{k}_bytes")), rr(&format!("{k}_write")), rr(&format!("{k}_alloc")));
        lines.push(Line::from(vec![
            Span::styled(format!("{label:<26}"), Style::new().fg(theme::ACCENT2)),
            Span::raw(format!("{:>10}", fmt::count(rr(&format!("{k}_count"))))),
            Span::raw(format!("{:>11}", fmt::rate(b))),
            Span::styled(format!("  written {} · allocated {}", fmt::rate(wr), fmt::rate(al)), theme::dim()),
        ]));
    }
    let bad = [
        ("stalls", "zil_commit_stall_count"),
        ("suspends", "zil_commit_suspend_count"),
        ("errors", "zil_commit_error_count"),
        ("crashes", "zil_commit_crash_count"),
    ];
    let mut l = vec![Span::styled("since boot: ", theme::dim())];
    for (label, k) in bad {
        let n = tot(k);
        l.push(Span::styled(
            format!("{label} {}  ", fmt::thousands(n)),
            if n > 0 { Style::new().fg(theme::WARN) } else { theme::dim() },
        ));
    }
    lines.push(Line::from(l));
    lines.push(Line::from(Span::styled(
        "commit = fsync/O_SYNC/NFS COMMIT; 'as writer' = commits that wrote the log themselves",
        theme::dim(),
    )));
    f.render_widget(Paragraph::new(lines).block(panel("ZIL · sync write log (zilstat)")), r);
}

fn zil_graph(f: &mut Frame, m: &Model, r: Rect) {
    let ir = inner(r);
    let n = widgets::samples(ir.width);
    let get = |k: &str| -> Vec<f64> { m.series.get(k).map(|s| s.tail(n).collect()).unwrap_or_default() };
    let (sl, no) = (get("zil.slog"), get("zil.normal"));
    let mx = sl.iter().chain(no.iter()).copied().filter(|x| x.is_finite()).fold(0.0, f64::max);
    f.render_widget(
        panel("").title(graph_title(
            "log bytes",
            format!("SLOG {}", fmt::rate(m.series.last("zil.slog"))),
            format!("pool {}", fmt::rate(m.series.last("zil.normal"))),
        )),
        r,
    );
    mirror_labeled(
        f.buffer_mut(),
        ir,
        (&sl, theme::ACCENT2, format!("▲ to SLOG · top {}", fmt::rate(mx))),
        (&no, theme::WRITE, format!("▼ to pool · bottom {}", fmt::rate(mx))),
        mx,
        mx,
    );
}

fn zil_datasets(f: &mut Frame, m: &Model, r: Rect) {
    let ir = inner(r);
    let mut v: Vec<(&String, &crate::model::DsIo)> =
        m.ds_io.iter().filter(|(_, io)| io.zil_commits > 0.0 || io.zil_itx > 0.0).collect();
    v.sort_by(|a, b| b.1.zil_commits.total_cmp(&a.1.zil_commits).then(a.0.cmp(b.0)));
    let nw = (ir.width as usize).saturating_sub(58).max(12);
    let mut lines = vec![full(
        Line::from(Span::styled(
            format!(
                "{:<nw$}{:>9}{:>8}{:>10}{:>10}{:>10}  {}",
                "dataset", "commit/s", "itx/s", "bytes/s", "SLOG/s", "pool/s", "sync"
            ),
            theme::header(),
        )),
        ir.width as usize,
    )];
    for (name, io) in v.iter().take(ir.height.saturating_sub(1) as usize) {
        let sync = m
            .dataset(name)
            .map(|d| d.props.get("sync").map(|p| p.value.clone()).unwrap_or_default())
            .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::raw(format!("{:<nw$}", fmt::fit_left(name, nw - 1))),
            Span::styled(format!("{:>9}", fmt::count(io.zil_commits)), Style::new().fg(theme::ACCENT2)),
            Span::raw(format!("{:>8}", fmt::count(io.zil_itx))),
            Span::raw(format!("{:>10}", fmt::rate(io.zil_bytes()))),
            Span::raw(format!("{:>10}", fmt::rate(io.zil_slog_b))),
            Span::raw(format!("{:>10}", fmt::rate(io.zil_normal_b))),
            Span::styled(
                format!("  {sync}"),
                if sync == "disabled" { Style::new().fg(theme::WARN) } else { theme::dim() },
            ),
        ]));
    }
    if v.is_empty() {
        lines.push(Line::from(Span::styled(
            if m.samples == 0 { "measuring…" } else { "no sync writes right now" },
            theme::dim(),
        )));
    }
    f.render_widget(Paragraph::new(lines).block(panel("sync writers by dataset")), r);
}

fn txg_card(f: &mut Frame, m: &Model, pool: &str, npools: usize, r: Rect) {
    let txgs = m.txgs.get(pool);
    let cur = m.pool_kstats(pool).map(|p| p.txgs.clone()).unwrap_or_default();
    let timeout = m.param("zfs_txg_timeout").and_then(|s| s.parse::<f64>().ok()).unwrap_or(5.0);
    let n = txgs.map(|t| t.len()).unwrap_or(0);
    let avg = |f: fn(&crate::parse::kstat::Txg) -> u64| -> f64 {
        txgs.map(|t| t.iter().rev().take(20).map(f).sum::<u64>() as f64 / t.len().clamp(1, 20) as f64).unwrap_or(0.0)
    };
    let per_min = txgs
        .filter(|t| t.len() >= 2)
        .map(|t| {
            let (a, b) = (t.front().unwrap(), t.back().unwrap());
            let span = (b.birth.saturating_sub(a.birth)) as f64 / 1e9;
            if span > 0.0 { (t.len() - 1) as f64 * 60.0 / span } else { 0.0 }
        })
        .unwrap_or(0.0);
    let mut states = Vec::new();
    for t in cur.iter().filter(|t| t.state != 'C') {
        let name = match t.state {
            'O' => "open",
            'Q' => "quiescing",
            'W' => "waiting to sync",
            'S' => "syncing",
            _ => "?",
        };
        states.push(Span::styled(
            format!("{} {name}  ", t.txg),
            Style::new().fg(if t.state == 'S' { theme::ACCENT } else { theme::DIM }),
        ));
    }
    let sync = avg(|t| t.stime);
    let lines = vec![
        Line::from(states),
        Line::from(
            [
                kv("txg/min", format!("{per_min:.1}"), None),
                vec![Span::raw("  ")],
                kv("avg sync (20)", fmt::ns(sync), Some(theme::level(sync, timeout * 0.5e9, timeout * 1e9))),
                vec![Span::raw("  ")],
                kv("avg dirty", fmt::size(avg(|t| t.ndirty) as u64), None),
                vec![Span::raw("  ")],
                kv("avg written", fmt::size(avg(|t| t.nwritten) as u64), None),
            ]
            .concat(),
        ),
        Line::from(Span::styled(
            format!("{n} committed txgs seen · timeout {timeout}s{}", if npools > 1 { " · n: next pool" } else { "" }),
            theme::dim(),
        )),
    ];
    f.render_widget(Paragraph::new(lines).block(panel(&format!("TXG · {pool}"))), r);
}

fn txg_graph(f: &mut Frame, m: &Model, pool: &str, r: Rect) {
    let ir = inner(r);
    let n = widgets::samples(ir.width);
    let t: Vec<&crate::parse::kstat::Txg> = m.txgs.get(pool).map(|t| t.iter().collect()).unwrap_or_default();
    let t = &t[t.len().saturating_sub(n)..];
    let st: Vec<f64> = t.iter().map(|x| x.stime as f64).collect();
    let wr: Vec<f64> = t.iter().map(|x| x.nwritten as f64).collect();
    let mx = |v: &[f64]| v.iter().copied().fold(0.0, f64::max);
    f.render_widget(
        panel("").title(graph_title(
            "per txg",
            format!("sync {}", fmt::ns(st.last().copied().unwrap_or(f64::NAN))),
            format!("wrote {}", fmt::size(wr.last().copied().unwrap_or(0.0) as u64)),
        )),
        r,
    );
    let (a, b) = (mx(&st), mx(&wr));
    mirror_labeled(
        f.buffer_mut(),
        ir,
        (&st, theme::ACCENT, format!("▲ sync time · top {}", fmt::ns(a))),
        (&wr, theme::WRITE, format!("▼ written · bottom {}", fmt::size(b as u64))),
        a,
        b,
    );
}

fn txg_table(f: &mut Frame, m: &Model, pool: &str, r: Rect) {
    let ir = inner(r);
    let mut lines = vec![full(
        Line::from(Span::styled(
            format!(
                "{:>9} {:>8}{:>9}{:>8}{:>8}{:>8}{:>9}{:>8}{:>8}",
                "txg", "dirty", "written", "reads", "writes", "open", "quiesce", "wait", "sync"
            ),
            theme::header(),
        )),
        ir.width as usize,
    )];
    let timeout_ns = m.param("zfs_txg_timeout").and_then(|s| s.parse::<f64>().ok()).unwrap_or(5.0) * 1e9;
    if let Some(t) = m.txgs.get(pool) {
        for x in t.iter().rev().take(ir.height.saturating_sub(1) as usize) {
            lines.push(Line::from(vec![
                Span::styled(format!("{:>9} ", x.txg), theme::dim()),
                Span::raw(format!("{:>8}", fmt::size(x.ndirty))),
                Span::styled(format!("{:>9}", fmt::size(x.nwritten)), Style::new().fg(theme::WRITE)),
                Span::raw(format!("{:>8}", fmt::count(x.reads as f64))),
                Span::raw(format!("{:>8}", fmt::count(x.writes as f64))),
                Span::styled(format!("{:>8}", fmt::ns(x.otime as f64)), theme::dim()),
                Span::styled(format!("{:>9}", fmt::ns(x.qtime as f64)), theme::dim()),
                Span::styled(format!("{:>8}", fmt::ns(x.wtime as f64)), theme::dim()),
                Span::styled(
                    format!("{:>8}", fmt::ns(x.stime as f64)),
                    Style::new()
                        .fg(theme::level(x.stime as f64, timeout_ns * 0.5, timeout_ns))
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines).block(panel("recent txgs (newest first)")), r);
}

/// Write throttle: dmu_tx counters and how long writers waited to get into a txg.
fn throttle(f: &mut Frame, m: &Model, pool: &str, r: Rect) {
    let ir = inner(r);
    let rr = |k: &str| m.rate("dmu_tx", k);
    let warn = |v: f64| if v > 0.0 { Some(theme::WARN) } else { Some(theme::DIM) };
    let mut lines = vec![
        Line::from(
            [
                kv("tx/s", fmt::count(rr("dmu_tx_assigned")), None),
                vec![Span::raw("  ")],
                kv("delayed/s", fmt::count(rr("dmu_tx_dirty_delay")), warn(rr("dmu_tx_dirty_delay"))),
                vec![Span::raw("  ")],
                kv("over dirty max/s", fmt::count(rr("dmu_tx_dirty_over_max")), warn(rr("dmu_tx_dirty_over_max"))),
                vec![Span::raw("  ")],
                kv("log delay/s", fmt::count(rr("dmu_tx_wrlog_delay")), warn(rr("dmu_tx_wrlog_delay"))),
            ]
            .concat(),
        ),
        Line::from(
            [
                kv("memory reclaim/s", fmt::count(rr("dmu_tx_memory_reclaim")), warn(rr("dmu_tx_memory_reclaim"))),
                vec![Span::raw("  ")],
                kv("quota/s", fmt::count(rr("dmu_tx_quota")), warn(rr("dmu_tx_quota"))),
                vec![Span::raw("  ")],
                kv("suspended/s", fmt::count(rr("dmu_tx_suspended")), warn(rr("dmu_tx_suspended"))),
            ]
            .concat(),
        ),
    ];
    // assign-wait distribution as horizontal bars: the last interval, or since boot when nobody waited lately
    // (the kstat only counts transactions that had to wait for a txg)
    let mut d = m.tx_assign_delta(pool);
    let mut scope = "last interval";
    if d.iter().all(|(_, c)| *c == 0) {
        d = m.pool_kstats(pool).and_then(|p| p.tx_assign.as_ref()).map(|k| k.nums.clone()).unwrap_or_default();
        scope = "since boot";
    }
    let used: Vec<&(String, u64)> = d.iter().filter(|(_, c)| *c > 0).collect();
    if let (Some(first), Some(last)) = (used.first(), used.last()) {
        let lo = d.iter().position(|x| x.0 == first.0).unwrap_or(0);
        let hi = d.iter().position(|x| x.0 == last.0).unwrap_or(0);
        let rows = (ir.height as usize).saturating_sub(lines.len() + 1);
        // merge adjacent buckets to fit
        let span = hi - lo + 1;
        let per = span.div_ceil(rows.max(1)).max(1);
        let groups: Vec<(String, u64)> = d[lo..=hi]
            .chunks(per)
            .map(|c| (c.last().map(|x| x.0.clone()).unwrap_or_default(), c.iter().map(|x| x.1).sum()))
            .collect();
        let max = groups.iter().map(|g| g.1).max().unwrap_or(1).max(1);
        let total: u64 = groups.iter().map(|g| g.1).sum();
        lines.push(Line::from(Span::styled(format!("wait to enter a txg, {scope}:"), theme::dim())));
        let bw = (ir.width as usize).saturating_sub(20);
        for (b, c) in groups {
            let ns: f64 = b.trim_end_matches(" ns").trim().parse().unwrap_or(0.0);
            let len = ((c as f64 / max as f64) * bw as f64).ceil() as usize;
            lines.push(Line::from(vec![
                Span::styled(format!("{:>7} ", fmt::ns(ns)), theme::dim()),
                Span::styled("━".repeat(len), Style::new().fg(theme::level(ns, 10e6, 100e6))),
                Span::styled(format!(" {}", fmt::pct(100.0 * c as f64 / total.max(1) as f64)), theme::dim()),
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines).block(panel("write throttle (dmu_tx)")), r);
}
