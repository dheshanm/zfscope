//! Property browser: every property of a pool or dataset with where its value comes from, and built-in help for
//! the selected one.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::{inner, panel, theme};
use crate::app::{App, Hit, PropSort, PropsFocus};
use crate::fmt;
use crate::help;
use crate::parse::zjson::{Prop, Props};

const SIZE_PROPS: [&str; 26] = [
    "used",
    "available",
    "referenced",
    "quota",
    "refquota",
    "reservation",
    "refreservation",
    "recordsize",
    "volsize",
    "volblocksize",
    "usedbydataset",
    "usedbysnapshots",
    "usedbychildren",
    "usedbyrefreservation",
    "logicalused",
    "logicalreferenced",
    "written",
    "special_small_blocks",
    "size",
    "allocated",
    "free",
    "freeing",
    "leaked",
    "expandsize",
    "bcloneused",
    "bclonesaved",
];

/// Display value: sizes and dates humanised unless `b` (exact) is on.
pub fn display(name: &str, p: &Prop) -> String {
    if fmt::exact() {
        return p.value.clone();
    }
    match p.num {
        Some(n) if SIZE_PROPS.contains(&name) => {
            if n == 0 && matches!(name, "quota" | "refquota" | "reservation" | "refreservation") {
                "none".into()
            } else {
                fmt::size(n)
            }
        }
        Some(n) if name == "creation" || name == "snapshots_changed" => fmt::date(n as i64),
        Some(n) if matches!(name, "capacity" | "fragmentation") => format!("{n}%"),
        _ if matches!(name, "dedupratio" | "bcloneratio" | "compressratio" | "refcompressratio")
            && !p.value.ends_with('x') =>
        {
            format!("{}x", p.value)
        }
        _ => p.value.clone(),
    }
}

/// Word-wrap `s` to `w` columns.
pub fn wrap(s: &str, w: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > w {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
}

/// Colour for a property value: on/active green, enabled blue, off/none dim; risky choices amber/red.
pub fn value_style(k: &str, v: &str) -> Style {
    match (k, v) {
        ("sync", "disabled") | ("readonly", "on") | ("health", "DEGRADED") => Style::new().fg(theme::WARN),
        ("checksum", "off") | ("health", "FAULTED" | "UNAVAIL") => Style::new().fg(theme::CRIT),
        (_, "on" | "yes" | "active" | "available" | "ONLINE") => Style::new().fg(theme::OK),
        (_, "enabled") => Style::new().fg(theme::ACCENT),
        (_, "off" | "no" | "none" | "-" | "disabled" | "unavailable" | "") => Style::new().fg(theme::DIM),
        _ => Style::new(),
    }
}

/// ZFS reports no source (`-`) for properties that can't be set: statistics, identity, and settings fixed at
/// creation (casesensitivity, encryption, volblocksize...).
pub fn read_only(p: &Prop) -> bool {
    matches!(p.source.as_str(), "NONE" | "")
}

pub fn source(p: &Prop) -> (String, Style) {
    match p.source.as_str() {
        "LOCAL" => ("local".into(), Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
        "INHERITED" => (format!("inherited from {}", p.from), Style::new().fg(theme::ACCENT2)),
        "DEFAULT" => ("default".into(), theme::dim()),
        "RECEIVED" => ("received".into(), Style::new().fg(theme::WARN)),
        "TEMPORARY" => ("temporary".into(), Style::new().fg(theme::WARN)),
        "NONE" | "" => ("-".into(), Style::new().fg(theme::FAINT)),
        s => (s.to_lowercase(), theme::dim()),
    }
}

/// Property table with a help pane; returns the (clamped) selection.
#[allow(clippy::too_many_arguments)]
pub fn table(
    f: &mut Frame,
    app: &mut App,
    props: &Props,
    is_pool: bool,
    sel: usize,
    r: Rect,
    title: &str,
    row_hit: fn(usize) -> Hit,
    focused: bool,
    filter: &str,
    target: &str,
) -> usize {
    let filter = filter.to_lowercase();
    let keep = |k: &String, p: &Prop| {
        (!app.props_local || p.source == "LOCAL" || p.source == "RECEIVED")
            && (filter.is_empty() || k.contains(&filter))
    };
    // Sections, each sorted by name: settable native properties, user properties (namespace:name), pool feature
    // flags, then read-only ones (source `-`: statistics, identity, settings fixed at creation).
    let group = |k: &str, p: &Prop| -> usize {
        if read_only(p) {
            3
        } else if k.starts_with("feature@") {
            2
        } else if k.contains(':') {
            1
        } else {
            0
        }
    };
    // default: ZFS's own property order (as `zfs get` / `zpool get` list them); a header click sorts by that column
    let mut rows: Vec<(&String, &Prop)> = props.iter().filter(|(k, p)| keep(k, p)).collect();
    let (sort, desc) = (app.props_sort, app.props_desc);
    let src_rank = |p: &Prop| match p.source.as_str() {
        "LOCAL" => 0,
        "TEMPORARY" => 1,
        "RECEIVED" => 2,
        "INHERITED" => 3,
        "DEFAULT" => 4,
        _ => 5,
    };
    rows.sort_by(|a, b| {
        let g = group(a.0, a.1).cmp(&group(b.0, b.1));
        let o = match sort {
            PropSort::Order => std::cmp::Ordering::Equal, // stable sort keeps ZFS order within a section
            PropSort::Name => a.0.cmp(b.0),
            PropSort::Value => match (a.1.num, b.1.num) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                _ => crate::app::natural(&a.1.value, &b.1.value),
            }
            .then_with(|| a.0.cmp(b.0)),
            PropSort::Source => {
                src_rank(a.1).cmp(&src_rank(b.1)).then_with(|| a.1.from.cmp(&b.1.from)).then_with(|| a.0.cmp(b.0))
            }
        };
        g.then(if desc { o.reverse() } else { o })
    });
    let n_set = rows.iter().filter(|(k, p)| group(k, p) < 3).count();
    // display list: section headings interleaved with property rows (indices into `rows`)
    enum Item {
        Head(String),
        Prop(usize),
    }
    let count = |g: usize| rows.iter().filter(|(k, p)| group(k, p) == g).count();
    let heads = [
        format!("settable · {} ({} set)", count(0), if is_pool { "zpool" } else { "zfs" }),
        format!("user properties · {} (namespace:name, ignored by ZFS itself)", count(1)),
        format!("feature flags · {} (enabled = available, active = in use on disk)", count(2)),
        format!("read-only · {} (statistics, identity, set at creation)", count(3)),
    ];
    let mut items = Vec::new();
    let mut last_group = usize::MAX;
    for (i, (k, p)) in rows.iter().enumerate() {
        let g = group(k, p);
        if g != last_group {
            items.push(Item::Head(heads[g].clone()));
            last_group = g;
        }
        items.push(Item::Prop(i));
    }
    // after switching pool/dataset or re-sorting: keep the same property selected (compare it across datasets)
    let mut sel = sel;
    if app.props_relocate {
        if let Some(i) = rows.iter().position(|(k, _)| **k == app.about_key) {
            sel = i;
        }
        app.props_relocate = false;
    }
    let sel = sel.min(rows.len().saturating_sub(1));
    app.props_sel_key = rows.get(sel).map(|(k, _)| k.to_string()); // what write mode's e / i act on
    let wide = r.width >= 110;
    let (tr, hr) = if wide {
        let [a, b] = Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(r);
        (a, b)
    } else {
        let [a, b] = Layout::vertical([Constraint::Min(5), Constraint::Length(7)]).areas(r);
        (a, b)
    };
    let ir = inner(tr);
    if row_hit(0) != Hit::Row(0) {
        app.hits.add(r, Hit::PaneArea);
    }
    let kw = rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(10).clamp(10, 32);
    let vw = (ir.width as usize).saturating_sub(kw + 2).clamp(8, 40);
    let col = |t: &'static str, w: usize, k: PropSort| {
        super::Col::new(t, w, false).sort(Hit::PropSort(k), (sort == k).then_some(desc))
    };
    let cols = [
        col("PROPERTY", kw + 2, PropSort::Name),
        col("VALUE", vw + 2, PropSort::Value),
        col("SOURCE", (ir.width as usize).saturating_sub(kw + vw + 4), PropSort::Source),
    ];
    let mut lines = vec![super::header_line(&cols, ir.x, ir.y, ir.width as usize, &mut app.hits)];
    let body = ir.height.saturating_sub(1) as usize;
    let sel_line = items.iter().position(|it| matches!(it, Item::Prop(i) if *i == sel)).unwrap_or(0);
    // keep the selection (and the heading just above the first row) in view
    let top = sel_line.saturating_sub(body.saturating_sub(1)).min(if sel_line <= 1 { 0 } else { usize::MAX });
    for (vi, it) in items.iter().enumerate().skip(top).take(body) {
        let y = ir.y + 1 + (vi - top) as u16;
        let i = match it {
            Item::Head(t) => {
                lines.push(super::full(
                    Line::from(Span::styled(
                        format!("── {t} "),
                        Style::new().fg(theme::ACCENT2).add_modifier(Modifier::BOLD),
                    )),
                    ir.width as usize,
                ));
                continue;
            }
            Item::Prop(i) => *i,
        };
        let (k, p) = rows[i];
        let (src, sst) = source(p);
        let has_help = if is_pool { help::pool_prop(k).is_some() } else { help::dataset_prop(k).is_some() };
        let ro = i >= n_set;
        let kst = match (has_help, ro) {
            (_, true) => theme::dim(),
            (true, false) => Style::new(),
            (false, false) => Style::new().fg(theme::DIM),
        };
        let mut vst = value_style(k, &p.value);
        if p.source == "LOCAL" {
            vst = vst.add_modifier(Modifier::BOLD);
        }
        let l = Line::from(vec![
            Span::styled(format!("{:<kw$}  ", fmt::fit(k, kw)), kst),
            Span::styled(format!("{:<vw$}  ", fmt::fit(&display(k, p), vw)), vst),
            Span::styled(src, sst),
        ]);
        app.hits.add(Rect::new(ir.x, y, ir.width, 1), row_hit(i));
        // the row the about pane describes is always highlighted: bright when this list has focus, grey otherwise
        let st = if focused && !app.about_focus { theme::sel() } else { theme::sel_dim() };
        let l = super::full(l, ir.width as usize);
        lines.push(if i == sel { l.style(st) } else { l });
    }
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            if app.props_local { "no locally set properties (L shows all)" } else { "no properties match" },
            theme::dim(),
        )));
    }
    let local = props.values().filter(|p| p.source == "LOCAL").count();
    let order = match sort {
        PropSort::Order => "zfs order".to_string(),
        k => format!("by {} {}", format!("{k:?}").to_lowercase(), if desc { "▼" } else { "▲" }),
    };
    let t = format!(
        "{title} · {} shown · {local} local · {order}{}",
        rows.len(),
        if app.props_local { " (L: local only)" } else { "" }
    );
    let table_focus = focused && !app.about_focus;
    f.render_widget(Paragraph::new(lines).block(super::panel_focus(&t, table_focus)), tr);
    // help pane
    let mut hl: Vec<Line> = Vec::new();
    if let Some((k, _)) = rows.get(sel)
        && app.about_key != **k
    {
        if !app.about_key.is_empty() {
            app.about_scroll = 0; // a different property: start from the top
        }
        app.about_key = k.to_string();
    }
    // write mode: the how-to lines name the key that does it
    let rw = app.rw;
    let how = |key: &str, label: &str| {
        if rw { format!("{key} {label:<7}") } else { format!("{label:<9}") }
    };
    if let Some((k, p)) = rows.get(sel) {
        let cmd = if is_pool { "zpool" } else { "zfs" };
        hl.push(Line::from(Span::styled(k.to_string(), Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD))));
        hl.push(Line::from(vec![
            Span::styled("value ", theme::dim()),
            Span::styled(p.value.clone(), value_style(k, &p.value).add_modifier(Modifier::BOLD)),
            Span::styled(
                if display(k, p) != p.value { format!("  ({})", display(k, p)) } else { String::new() },
                theme::dim(),
            ),
        ]));
        let (src, sst) = source(p);
        hl.push(Line::from(vec![Span::styled("source ", theme::dim()), Span::styled(src, sst)]));
        let feature = k.strip_prefix("feature@");
        hl.push(Line::from(if read_only(p) {
            Span::styled("read-only: reported by ZFS", theme::dim())
        } else if feature.is_some() {
            Span::styled("feature flag: can be enabled, never disabled again", Style::new().fg(theme::ACCENT2))
        } else {
            Span::styled(format!("settable: {cmd} set {k}=…"), Style::new().fg(theme::ACCENT2))
        }));
        hl.push(Line::from(""));
        let h = if is_pool { help::pool_prop(k) } else { help::dataset_prop(k) };
        match h {
            Some((a, _)) => {
                hl.push(Line::from(Span::styled(help::words(a), Style::new().add_modifier(Modifier::BOLD))))
            }
            None => hl.push(Line::from(Span::styled(
                format!("No built-in help; see `man {}`.", if is_pool { "zpoolprops" } else { "zfsprops" }),
                theme::dim(),
            ))),
        }
        // possible values, current one highlighted
        let opts = if is_pool { help::pool_options(k) } else { help::dataset_options(k) };
        if let Some(opts) = opts {
            hl.push(Line::from(""));
            hl.push(Line::from(Span::styled("possible values", theme::dim().add_modifier(Modifier::UNDERLINED))));
            let ow = opts.iter().map(|(v, _)| v.chars().count()).max().unwrap_or(4).min(14);
            // descriptions wrap with a hanging indent so the values stay in their own column
            let dw = (hr.width as usize).saturating_sub(2 + 2 + ow + 2).max(12);
            for (v, d) in opts {
                let cur = help::option_matches(v, &p.value, p.num);
                let mark = if cur { "● " } else { "  " };
                let vs = if cur {
                    value_style(k, &p.value).add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    value_style(k, v)
                };
                let ds = if cur { Style::new() } else { theme::dim() };
                for (i, chunk) in wrap(d, dw).into_iter().enumerate() {
                    hl.push(Line::from(if i == 0 {
                        vec![
                            Span::styled(mark, Style::new().fg(theme::ACCENT2)),
                            Span::styled(format!("{v:<ow$}"), vs),
                            Span::styled(format!("  {chunk}"), ds),
                        ]
                    } else {
                        vec![Span::raw(" ".repeat(2 + ow + 2)), Span::styled(chunk, ds)]
                    }));
                }
            }
        }
        if let Some((_, b)) = h.filter(|(_, b)| !b.is_empty()) {
            hl.push(Line::from(""));
            for para in help::paragraphs(b) {
                hl.push(Line::from(para));
            }
        }
        if let Some(fname) = feature {
            let (since, when) = help::feature_info(fname);
            hl.push(Line::from(""));
            hl.push(Line::from(Span::styled("how to use it", theme::dim().add_modifier(Modifier::UNDERLINED))));
            hl.push(Line::from(vec![
                Span::styled("needs    ", theme::dim()),
                Span::raw(format!("OpenZFS {since} or newer")),
            ]));
            hl.push(Line::from(vec![
                Span::styled(how("e", "enable"), theme::dim()),
                Span::styled(format!("zpool set {k}=enabled {target}"), Style::new().fg(theme::ACCENT)),
            ]));
            hl.push(Line::from(vec![
                Span::styled("or all   ", theme::dim()),
                Span::styled(format!("zpool upgrade {target}"), Style::new().fg(theme::ACCENT)),
            ]));
            hl.push(Line::from(vec![Span::styled("active   ", theme::dim()), Span::raw(when.to_string())]));
            hl.push(Line::from(vec![
                Span::styled("check    ", theme::dim()),
                Span::styled(format!("zpool get {k} {target}"), Style::new().fg(theme::ACCENT)),
            ]));
            hl.push(Line::from(""));
            hl.push(Line::from(Span::styled(help::words(help::FEATURE_STATE), theme::dim())));
        } else if !read_only(p) {
            hl.push(Line::from(""));
            hl.push(Line::from(vec![
                Span::styled(how("e", "change"), theme::dim()),
                Span::styled(format!("{cmd} set {k}=<value> {target}"), Style::new().fg(theme::ACCENT)),
            ]));
            if !is_pool && p.source != "DEFAULT" {
                hl.push(Line::from(vec![
                    Span::styled(how("i", "reset"), theme::dim()),
                    Span::styled(format!("zfs inherit {k} {target}"), Style::new().fg(theme::ACCENT)),
                    Span::styled("  (back to the parent's / default value)", theme::dim()),
                ]));
            }
        }
    }
    // scrollable: clamp to the wrapped length, show a hint when there's more than fits
    let iw = inner(hr).width.max(1) as usize;
    let total: usize = hl.iter().map(|l| l.width().div_ceil(iw).max(1)).sum();
    let max_scroll = total.saturating_sub(inner(hr).height as usize);
    app.about_scroll = app.about_scroll.min(max_scroll);
    app.hits.add(hr, Hit::AboutArea);
    let about_focused = app.about_focus && focused;
    let title = match (max_scroll > 0, about_focused) {
        (false, _) => "about this property".to_string(),
        (true, true) => format!("about this property · ↑↓ scroll · {}/{}", app.about_scroll, max_scroll),
        (true, false) => "about this property · Tab, then ↑↓ to scroll".to_string(),
    };
    f.render_widget(
        Paragraph::new(hl)
            .wrap(Wrap { trim: false })
            .scroll((app.about_scroll as u16, 0))
            .block(super::panel_focus(&title, about_focused)),
        hr,
    );
    sel
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    // pools & datasets │ properties │ about — focus moves right with Enter, left with Esc
    let tw = (area.width / 4).clamp(22, 40);
    let [targets_r, rest] = Layout::horizontal([Constraint::Length(tw), Constraint::Min(30)]).areas(area);
    targets(f, app, targets_r);
    let Some(target) = app.props_target() else {
        f.render_widget(Paragraph::new("reading configuration…").style(theme::dim()).block(panel("properties")), rest);
        return;
    };
    let (props, is_pool) = match target.strip_prefix("pool:") {
        Some(p) => (app.model.pool_props(p).cloned(), true),
        None => (app.model.dataset(&target).map(|d| d.props.clone()), false),
    };
    let name = target.trim_start_matches("pool:").to_string();
    let title = format!("{} {name}", if is_pool { "pool" } else { "dataset" });
    match props {
        Some(p) => {
            let sel = app.props_sel;
            let filter = app.filter.clone();
            let focused = app.props_focus != PropsFocus::Targets;
            app.props_sel = table(f, app, &p, is_pool, sel, rest, &title, Hit::Row, focused, &filter, &name);
        }
        None => f.render_widget(
            Paragraph::new("not found (refreshing…)").style(theme::dim()).block(panel("properties")),
            rest,
        ),
    }
}

/// Left column: pools and datasets as a tree. Moving the selection previews its properties; Enter focuses them.
fn targets(f: &mut Frame, app: &mut App, r: Rect) {
    let shown = app.prop_targets_shown();
    let cur = app.props_target();
    // keep the selection on the current target (it can move when the list changes)
    if let Some(i) = shown.iter().position(|t| Some(t) == cur.as_ref()) {
        app.picker_sel = i;
    }
    let sel = app.picker_sel.min(shown.len().saturating_sub(1));
    let ir = inner(r);
    let body = ir.height as usize;
    let top = sel.saturating_sub(body.saturating_sub(1));
    let focused = app.props_focus == PropsFocus::Targets;
    let mut lines = Vec::new();
    for (i, t) in shown.iter().enumerate().skip(top).take(body) {
        // pool rows hold pool properties; datasets (root dataset included) are indented under them
        let (label, st) = match t.strip_prefix("pool:") {
            Some(p) => (format!("◆ {p} (pool)"), Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
            None => {
                let depth = t.matches('/').count() + 1;
                let leaf = if app.target_filter.is_empty() { t.rsplit('/').next().unwrap_or(t) } else { t.as_str() };
                (format!("{}{leaf}", "  ".repeat(depth)), Style::new())
            }
        };
        let y = ir.y + (i - top) as u16;
        app.hits.add(Rect::new(ir.x, y, ir.width, 1), Hit::TargetRow(i));
        let l = super::full(Line::from(Span::styled(fmt::fit_left(&label, ir.width as usize), st)), ir.width as usize);
        let hl = if focused { theme::sel() } else { theme::sel_dim() };
        lines.push(if i == sel { l.style(hl) } else { l });
    }
    if shown.is_empty() {
        lines.push(Line::from(Span::styled("nothing matches", theme::dim())));
    }
    let filt = if app.target_filter.is_empty() { String::new() } else { format!(" · /{}", app.target_filter) };
    let title = format!("pools & datasets{filt}");
    f.render_widget(Paragraph::new(lines).block(super::panel_focus(&title, focused)), r);
}
