//! ARC tab: size and composition, hit ratios per access class (zarcstat/zarcsummary), list breakdown, graphs and
//! tunables.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::overview::mirror_labeled;
use super::{graph_title, inner, kv, panel, params, theme, widgets};
use crate::app::App;
use crate::fmt;
use crate::model::{Model, ratio};

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if !app.model.has_kstats() {
        f.render_widget(
            Paragraph::new("no arcstats yet (/proc/spl/kstat/zfs/arcstats)").style(theme::dim()).block(panel("ARC")),
            area,
        );
        return;
    }
    let [top, mid, bot] =
        Layout::vertical([Constraint::Length(13), Constraint::Percentage(40), Constraint::Min(8)]).areas(area);
    let [size_r, eff_r] = Layout::horizontal([Constraint::Percentage(48), Constraint::Percentage(52)]).areas(top);
    size_card(f, &app.model, size_r);
    efficiency(f, &app.model, eff_r);
    let g = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(mid);
    graphs(f, &app.model, &g);
    let [lists_r, params_r] = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(bot);
    lists(f, &app.model, lists_r);
    params::draw(f, app, params_r, "ARC tunables", &crate::help::ARC_PARAMS);
}

fn v(m: &Model, k: &str) -> f64 {
    m.val("arcstats", k) as f64
}

fn size_card(f: &mut Frame, m: &Model, r: Rect) {
    let w = inner(r).width as usize;
    let (size, c, cmin, cmax) = (v(m, "size"), v(m, "c"), v(m, "c_min"), v(m, "c_max"));
    let mut lines = Vec::new();
    let mut g = vec![Span::styled("size   ", theme::dim())];
    g.extend(widgets::gauge(w.saturating_sub(16).max(4), if cmax > 0.0 { size / cmax } else { 0.0 }, theme::ACCENT));
    g.push(Span::raw(format!(" {}", fmt::size(cmax as u64))));
    lines.push(Line::from(g));
    lines.push(Line::from(
        [
            kv("now", fmt::size(size as u64), None),
            vec![Span::raw("   ")],
            kv("target (c)", fmt::size(c as u64), None),
            vec![Span::raw("   ")],
            kv("min", fmt::size(cmin as u64), None),
            vec![Span::raw("   ")],
            kv("max", fmt::size(cmax as u64), None),
        ]
        .concat(),
    ));
    // composition of `size`
    let parts = [
        (v(m, "data_size"), '█', theme::READ, "data"),
        (v(m, "metadata_size"), '▓', theme::ARC_META, "metadata"),
        (v(m, "dnode_size"), '▒', theme::ACCENT2, "dnode"),
        (v(m, "dbuf_size"), '░', theme::ARC_MFU, "dbuf"),
        (
            v(m, "hdr_size") + v(m, "l2_hdr_size") + v(m, "bonus_size") + v(m, "abd_chunk_waste_size"),
            '·',
            theme::DIM,
            "hdr/other",
        ),
    ];
    let bar: Vec<(f64, char, ratatui::style::Color)> = parts.iter().map(|p| (p.0, p.1, p.2)).collect();
    let mut l = vec![Span::styled("made of ", theme::dim())];
    l.extend(widgets::stacked(w.saturating_sub(8).max(4), &bar, size));
    lines.push(Line::from(l));
    let mut leg = Vec::new();
    for (val, ch, col, name) in parts {
        leg.push(Span::styled(format!("{ch} "), Style::new().fg(col)));
        leg.push(Span::styled(format!("{name} "), theme::dim()));
        leg.push(Span::raw(format!("{}  ", fmt::size(val as u64))));
    }
    lines.push(Line::from(leg));
    let (mru, mfu) = (v(m, "mru_size"), v(m, "mfu_size"));
    let mut l = vec![Span::styled("mru|mfu ", theme::dim())];
    l.extend(widgets::stacked(
        w.saturating_sub(8).max(4),
        &[(mru, '█', theme::ARC_MRU), (mfu, '▓', theme::ARC_MFU)],
        mru + mfu,
    ));
    lines.push(Line::from(l));
    lines.push(Line::from(
        [
            kv("MRU (recent)", fmt::size(mru as u64), Some(theme::ARC_MRU)),
            vec![Span::raw("   ")],
            kv("MFU (frequent)", fmt::size(mfu as u64), Some(theme::ARC_MFU)),
            vec![Span::raw("   ")],
            kv(
                "ghosts",
                format!("{} / {}", fmt::size(v(m, "mru_ghost_size") as u64), fmt::size(v(m, "mfu_ghost_size") as u64)),
                None,
            ),
        ]
        .concat(),
    ));
    let (cs, us) = (v(m, "compressed_size"), v(m, "uncompressed_size"));
    lines.push(Line::from(
        [
            kv("compressed", format!("{} → {} logical", fmt::size(cs as u64), fmt::size(us as u64)), None),
            vec![Span::raw("  ")],
            kv("ratio", if cs > 0.0 { format!("{:.2}x", us / cs) } else { "-".into() }, Some(theme::OK)),
        ]
        .concat(),
    ));
    lines.push(Line::from(""));
    lines.push(Line::from(
        [
            kv("memory", fmt::size(v(m, "memory_all_bytes") as u64), None),
            vec![Span::raw("   ")],
            kv("free", fmt::size(v(m, "memory_free_bytes") as u64), None),
            vec![Span::raw("   ")],
            kv("available", fmt::size(v(m, "memory_available_bytes") as u64), None),
            vec![Span::raw("   ")],
            kv("sys_free", fmt::size(v(m, "arc_sys_free") as u64), None),
        ]
        .concat(),
    ));
    let no_grow = v(m, "arc_no_grow") > 0.0;
    let throttle = m.rate("arcstats", "memory_throttle_count");
    let mut flags = Vec::new();
    if no_grow {
        flags.push(Span::styled("⚠ arc_no_grow: memory is tight, ARC can't grow  ", Style::new().fg(theme::WARN)));
    }
    if throttle > 0.0 {
        flags.push(Span::styled(
            format!("⚠ memory throttle {}/s  ", fmt::count(throttle)),
            Style::new().fg(theme::CRIT),
        ));
    }
    let direct = m.rate("arcstats", "memory_direct_count");
    let indirect = m.rate("arcstats", "memory_indirect_count");
    flags.push(Span::styled(
        format!("reclaim: direct {}/s, indirect {}/s", fmt::count(direct), fmt::count(indirect)),
        if direct > 0.0 { Style::new().fg(theme::WARN) } else { theme::dim() },
    ));
    lines.push(Line::from(flags));
    let title =
        graph_title("ARC", fmt::size(size as u64), format!("{} of max", fmt::pct(100.0 * size / cmax.max(1.0))));
    f.render_widget(Paragraph::new(lines).block(panel("").title(title)), r);
}

/// Hit ratios per access class, now and since boot.
fn efficiency(f: &mut Frame, m: &Model, r: Rect) {
    let rr = |k: &str| m.rate("arcstats", k);
    let tot = |k: &str| v(m, k);
    let mut lines = vec![Line::from(Span::styled(
        format!("{:<18}{:>9}{:>9}{:>9}{:>8}{:>12}", "", "hits/s", "iohit/s", "miss/s", "hit%", "since boot"),
        theme::header(),
    ))];
    let classes = [
        ("demand data", "demand_data"),
        ("demand metadata", "demand_metadata"),
        ("prefetch data", "prefetch_data"),
        ("prefetch metadata", "prefetch_metadata"),
    ];
    let mut all = (0.0, 0.0, 0.0);
    for (label, k) in classes {
        let (h, io, mi) = (rr(&format!("{k}_hits")), rr(&format!("{k}_iohits")), rr(&format!("{k}_misses")));
        all = (all.0 + h, all.1 + io, all.2 + mi);
        let boot = ratio(tot(&format!("{k}_hits")) + tot(&format!("{k}_iohits")), tot(&format!("{k}_misses")));
        lines.push(eff_row(label, h, io, mi, boot));
    }
    let boot_all = ratio(tot("hits") + tot("iohits"), tot("misses"));
    lines.push(eff_row("all", all.0, all.1, all.2, boot_all).style(theme::bold()));
    lines.push(Line::from(""));
    lines.push(Line::from(
        [
            kv("MRU hits/s", fmt::count(rr("mru_hits")), Some(theme::ARC_MRU)),
            vec![Span::raw("  ")],
            kv("MFU hits/s", fmt::count(rr("mfu_hits")), Some(theme::ARC_MFU)),
            vec![Span::raw("  ")],
            kv("uncached/s", fmt::count(rr("uncached_hits")), None),
        ]
        .concat(),
    ));
    let ghost = rr("mru_ghost_hits") + rr("mfu_ghost_hits");
    lines.push(Line::from(
        [
            kv("ghost hits/s", fmt::count(ghost), Some(if ghost > 0.0 { theme::WARN } else { theme::DIM })),
            vec![Span::styled("  (misses a bigger ARC would have served)", theme::dim())],
        ]
        .concat(),
    ));
    lines.push(Line::from(
        [
            kv("evicted", fmt::rate(rr("evict_l2_eligible") + rr("evict_l2_ineligible")), None),
            vec![Span::raw("  ")],
            kv("deleted/s", fmt::count(rr("deleted")), None),
            vec![Span::raw("  ")],
            kv("evict skips/s", fmt::count(rr("evict_skip")), None),
        ]
        .concat(),
    ));
    let zf = |k: &str| m.rate("zfetchstats", k);
    let db = |k: &str| m.rate("dbufstats", k);
    lines.push(Line::from(
        [
            kv("prefetcher hit", fmt::ratio_pct(zf("hits"), zf("misses")), None),
            vec![Span::raw("  ")],
            kv("dbuf hash hit", fmt::ratio_pct(db("hash_hits"), db("hash_misses")), None),
        ]
        .concat(),
    ));
    if v(m, "l2_size") > 0.0 {
        lines.push(Line::from(
            [
                kv(
                    "L2ARC",
                    format!("{} ({} on disk)", fmt::size(v(m, "l2_size") as u64), fmt::size(v(m, "l2_asize") as u64)),
                    None,
                ),
                vec![Span::raw("  ")],
                kv("hit", fmt::ratio_pct(rr("l2_hits"), rr("l2_misses")), Some(theme::OK)),
                vec![Span::raw("  ")],
                kv("read", fmt::rate(rr("l2_read_bytes")), None),
                vec![Span::raw("  ")],
                kv("write", fmt::rate(rr("l2_write_bytes")), None),
            ]
            .concat(),
        ));
    } else {
        lines.push(Line::from(Span::styled("no L2ARC (cache devices)", theme::dim())));
    }
    f.render_widget(Paragraph::new(lines).block(panel("hit ratios · last interval")), r);
}

fn eff_row(label: &str, h: f64, io: f64, mi: f64, boot: f64) -> Line<'static> {
    let now = ratio(h + io, mi);
    let c = theme::level(100.0 - if now.is_finite() { now } else { 100.0 }, 10.0, 30.0);
    Line::from(vec![
        Span::raw(format!("{label:<18}")),
        Span::styled(format!("{:>9}", fmt::count(h)), Style::new().fg(theme::OK)),
        Span::styled(format!("{:>9}", fmt::count(io)), theme::dim()),
        Span::styled(format!("{:>9}", fmt::count(mi)), Style::new().fg(theme::WARN)),
        Span::styled(format!("{:>8}", fmt::pct(now)), Style::new().fg(c).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:>12}", fmt::pct(boot)), theme::dim()),
    ])
}

fn graphs(f: &mut Frame, m: &Model, g: &[Rect]) {
    let series = |k: &str, w: u16| -> Vec<f64> {
        m.series.get(k).map(|s| s.tail(widgets::samples(w)).collect()).unwrap_or_default()
    };
    let mx = |v: &[f64]| v.iter().copied().filter(|x| x.is_finite()).fold(0.0, f64::max);
    // 1: reads ▲ misses ▼ (own scales: misses are tiny next to reads)
    let ir = inner(g[0]);
    let (rd, ms) = (series("arc.reads", ir.width), series("arc.misses", ir.width));
    f.render_widget(
        panel("").title(graph_title(
            "accesses/s",
            fmt::count(m.series.last("arc.reads")),
            format!("{} misses/s", fmt::count(m.series.last("arc.misses"))),
        )),
        g[0],
    );
    let (a, b) = (mx(&rd), mx(&ms));
    mirror_labeled(
        f.buffer_mut(),
        ir,
        (&rd, theme::OK, format!("▲ reads · top {}/s", fmt::count(a))),
        (&ms, theme::WARN, format!("▼ misses · bottom {}/s", fmt::count(b))),
        a,
        b,
    );
    // 2: demand hit% ▲ prefetch hit% ▼ (0..100)
    let ir = inner(g[1]);
    let (dh, ph) = (series("arc.demand_hit", ir.width), series("arc.pf_hit", ir.width));
    f.render_widget(
        panel("").title(graph_title(
            "hit %",
            format!("demand {}", fmt::pct(m.series.last("arc.demand_hit"))),
            format!("prefetch {}", fmt::pct(m.series.last("arc.pf_hit"))),
        )),
        g[1],
    );
    mirror_labeled(
        f.buffer_mut(),
        ir,
        (&dh, theme::OK, "▲ demand · top 100%".into()),
        (&ph, theme::ACCENT2, "▼ prefetch · bottom 100%".into()),
        100.0,
        100.0,
    );
    // 3: size ▲ available memory ▼
    let ir = inner(g[2]);
    let (sz, av) = (series("arc.size", ir.width), series("arc.avail", ir.width));
    let cmax = v(m, "c_max");
    let mem = v(m, "memory_all_bytes");
    f.render_widget(
        panel("").title(graph_title(
            "size",
            fmt::size(m.series.last("arc.size").max(0.0) as u64),
            format!("avail mem {}", fmt::size(m.series.last("arc.avail").max(0.0) as u64)),
        )),
        g[2],
    );
    mirror_labeled(
        f.buffer_mut(),
        ir,
        (&sz, theme::ACCENT, format!("▲ ARC size · top {} (max)", fmt::size(cmax as u64))),
        (&av, theme::ACCENT2, format!("▼ available memory · bottom {}", fmt::size(mem as u64))),
        cmax.max(mx(&sz)),
        mem.max(mx(&av)),
    );
}

/// zarcsummary-style list table: anon / MRU / MFU / ghosts / uncached, data vs metadata and how much is evictable.
fn lists(f: &mut Frame, m: &Model, r: Rect) {
    let mut lines = vec![Line::from(Span::styled(
        format!("{:<12}{:>9}{:>10}{:>10}{:>11}{:>11}", "list", "size", "data", "metadata", "evict data", "evict meta"),
        theme::header(),
    ))];
    for (label, k, col) in [
        ("anon", "anon", theme::DIM),
        ("MRU", "mru", theme::ARC_MRU),
        ("MRU ghost", "mru_ghost", theme::FAINT),
        ("MFU", "mfu", theme::ARC_MFU),
        ("MFU ghost", "mfu_ghost", theme::FAINT),
        ("uncached", "uncached", theme::DIM),
    ] {
        let s = |x: &str| fmt::size(v(m, &format!("{k}_{x}")) as u64);
        lines.push(Line::from(vec![
            Span::styled(format!("{label:<12}"), Style::new().fg(col)),
            Span::raw(format!("{:>9}", s("size"))),
            Span::raw(format!("{:>10}", s("data"))),
            Span::raw(format!("{:>10}", s("metadata"))),
            Span::styled(format!("{:>11}", s("evictable_data")), theme::dim()),
            Span::styled(format!("{:>11}", s("evictable_metadata")), theme::dim()),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(
        [
            kv("hash elements", fmt::thousands(m.val("arcstats", "hash_elements")), None),
            vec![Span::raw("  ")],
            kv("max", fmt::thousands(m.val("arcstats", "hash_elements_max")), None),
            vec![Span::raw("  ")],
            kv("chains", fmt::thousands(m.val("arcstats", "hash_chains")), None),
        ]
        .concat(),
    ));
    lines.push(Line::from(
        [
            kv("dnode limit", fmt::size(m.val("arcstats", "arc_dnode_limit")), None),
            vec![Span::raw("  ")],
            kv("meta used", fmt::size(m.val("arcstats", "arc_meta_used")), None),
            vec![Span::raw("  ")],
            kv("prunes/s", fmt::count(m.rate("arcstats", "arc_prune")), None),
        ]
        .concat(),
    ));
    f.render_widget(Paragraph::new(lines).block(panel("ARC lists (ghosts: recently evicted, headers only)")), r);
}
