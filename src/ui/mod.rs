//! Drawing. `draw` lays out the frame (header with tabs, status banners, body, footer) and dispatches per tab.

pub mod arc;
pub mod datasets;
pub mod disks;
pub mod edit;
pub mod overview;
pub mod params;
pub mod pool;
pub mod props;
pub mod theme;
pub mod widgets;
pub mod zil;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, Hit, Tab};
use crate::fmt;

pub fn draw(f: &mut Frame, app: &mut App) {
    app.hits.0.clear();
    let area = f.area();
    let banners = status_lines(app);
    let status = write_status(app);
    let [head, ban, body, stat, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(banners.len() as u16),
        Constraint::Min(1),
        Constraint::Length(status.is_some() as u16),
        Constraint::Length(1),
    ])
    .areas(area);
    header(f, app, head);
    f.render_widget(Paragraph::new(banners), ban);
    if let Some(l) = status {
        f.render_widget(Paragraph::new(l).style(Style::new().bg(theme::HEADER_BG)), stat);
    }
    let (body, log) = if app.show_log {
        let [b, l] = Layout::vertical([Constraint::Min(5), Constraint::Length(10)]).areas(body);
        (b, Some(l))
    } else {
        (body, None)
    };
    match app.tab {
        Tab::Overview => overview::draw(f, app, body),
        Tab::Pool => pool::draw(f, app, body),
        Tab::Datasets => datasets::draw(f, app, body),
        Tab::Arc => arc::draw(f, app, body),
        Tab::Zil => zil::draw(f, app, body),
        Tab::Disks => disks::draw(f, app, body),
        Tab::Props => props::draw(f, app, body),
    }
    if let Some(l) = log {
        let w = tui_logger::TuiLoggerWidget::default()
            .block(panel("log (l to close)"))
            .style_error(Style::new().fg(theme::CRIT))
            .style_warn(Style::new().fg(theme::WARN))
            .style_info(Style::new().fg(theme::DIM))
            .output_target(false)
            .output_file(false)
            .output_line(false);
        f.render_widget(w, l);
    }
    footer(f, app, foot);
    if app.show_help {
        help(f, app, area);
    }
    edit::draw(f, app, area);
}

/// Panel whose border shows keyboard focus (when a view has more than one scrollable part).
pub fn panel_focus(title: &str, focused: bool) -> Block<'_> {
    let b = panel(title);
    if focused { b.border_style(Style::new().fg(theme::ACCENT)).border_type(BorderType::Thick) } else { b }
}

/// Standard bordered panel with a title.
pub fn panel(title: &str) -> Block<'_> {
    let b = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::BORDER));
    if title.is_empty() {
        b
    } else {
        b.title(Span::styled(format!(" {title} "), Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)))
    }
}

fn header(f: &mut Frame, app: &mut App, r: Rect) {
    let mut spans = vec![
        Span::styled(" zfscope ", Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{} ", app.host), theme::bold()),
    ];
    let mut x = r.x + spans.iter().map(|s| s.width() as u16).sum::<u16>();
    for (i, t) in Tab::ALL.iter().enumerate() {
        let label = format!(" {} {} ", i + 1, t.title());
        let w = label.chars().count() as u16;
        let style = if *t == app.tab {
            Style::new().bg(theme::ACCENT).fg(ratatui::style::Color::Black).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme::DIM)
        };
        app.hits.add(Rect::new(x, r.y, w, 1), Hit::Tab(*t));
        x += w;
        spans.push(Span::styled(label, style));
    }
    let left_w: usize = spans.iter().map(|s| s.width()).sum();
    let state = if app.paused {
        Span::styled(
            " ⏸ paused · mouse released: drag to select & copy ",
            Style::new().fg(theme::WARN).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(format!(" ⟳ {}s ", app.interval.as_secs()), theme::dim())
    };
    let ver = app
        .model
        .config
        .as_ref()
        .and_then(|c| c.version.as_ref())
        .map(|(u, _)| u.trim_start_matches("zfs-").to_string())
        .unwrap_or_default();
    let right = vec![
        Span::styled(format!("zfs {ver} "), theme::dim()),
        Span::styled(format!("zfscope {} ", crate::VERSION), Style::new().fg(theme::FAINT)),
        if app.rw {
            Span::styled(
                " rw ",
                Style::new().fg(ratatui::style::Color::Black).bg(theme::CRIT).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                " ro ",
                Style::new().fg(ratatui::style::Color::Black).bg(theme::ACCENT2).add_modifier(Modifier::BOLD),
            )
        },
        Span::raw(" "),
        state,
    ];
    let rw: usize = right.iter().map(|s| s.width()).sum();
    let pad = (r.width as usize).saturating_sub(left_w + rw);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.extend(right);
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::new().bg(theme::HEADER_BG)), r);
}

/// Problems worth a line of screen: missing data sources, failed commands, stuck sampler.
pub fn status_lines(app: &App) -> Vec<Line<'static>> {
    let mut v = Vec::new();
    let warn = |s: String| Line::from(Span::styled(format!(" ⚠ {s}"), Style::new().fg(theme::WARN)));
    let m = &app.model;
    if app.last_kstat.is_none() && app.started.elapsed().as_secs() < 3 {
        // just started
    } else if let Some(Err(e)) = m.ks.as_ref().and_then(|k| k.global.get("arcstats")) {
        v.push(warn(format!("no ZFS kstats ({e}) — is the zfs module loaded?")));
    }
    if let Some(age) = app.stale_for() {
        v.push(warn(format!("no kstat sample for {} — sampler stuck?", fmt::duration(age.as_secs_f64()))));
    }
    for e in &m.config_errors {
        v.push(warn(e.clone()));
    }
    if let Some(e) = &m.iostat_err {
        v.push(warn(format!("vdev IO unavailable: {e}")));
    }
    v.truncate(3);
    v
}

/// Write-mode status, shown just above the key bar: a change running, or the outcome of the last action
/// (unlocked / locked, applied, failed, refused) for a few seconds.
pub fn write_status(app: &App) -> Option<Line<'static>> {
    if app.write_busy {
        return Some(Line::from(Span::styled(" ⟳ applying change…", Style::new().fg(theme::WARN))));
    }
    let (t, msg, err) = app.notice.as_ref()?;
    if t.elapsed() >= crate::app::NOTICE_FOR {
        return None;
    }
    let c = if *err { theme::CRIT } else { theme::OK };
    Some(Line::from(Span::styled(format!(" {msg}"), Style::new().fg(c).add_modifier(Modifier::BOLD))))
}

fn footer(f: &mut Frame, app: &App, r: Rect) {
    let mut hints: Vec<(&str, &str)> = vec![("q", "quit"), ("?", "help"), ("1-7", "tabs")];
    match app.tab {
        Tab::Pool => {
            if app.pool_history {
                hints.extend([("↑↓", "scroll"), ("/", "filter"), ("H/Esc", "back")]);
            } else if app.grid {
                hints.extend([("↑↓", "colour by"), ("c", "next colour"), ("d/Esc", "back"), ("H", "history")]);
            } else {
                hints.extend([
                    ("↑↓", "select"),
                    ("←→␣", "fold"),
                    ("m", app.io_mode.title()),
                    ("d", "disk grid"),
                    ("H", "history"),
                ]);
            }
            if !app.pool_history && !app.grid {
                hints.push(("w", if app.heat_disk { "heat: disk wait" } else { "heat: total wait" }));
            }
        }
        Tab::Datasets if app.ds_detail => hints.extend([
            ("Tab", "next focus"),
            (
                "↑↓",
                if app.about_focus {
                    "scroll help"
                } else if app.ds_focus_pane {
                    "move in pane"
                } else {
                    "select dataset"
                },
            ),
            ("[ ]", "panes"),
            ("Esc", if app.ds_focus_pane { "back to list" } else { "close" }),
            ("P", "props tab"),
        ]),
        Tab::Datasets => hints.extend([
            ("↑↓", "select"),
            ("←→␣", "fold"),
            ("Enter", "detail"),
            ("t", if app.ds_flat { "tree" } else { "ztop" }),
            ("s/S", "sort/reverse"),
            ("/", "filter"),
        ]),
        Tab::Disks => hints.extend([
            ("↑↓", "select"),
            ("Enter", if app.disk_detail { "close" } else { "detail" }),
            ("s/S", "sort/reverse"),
            ("/", "filter"),
        ]),
        Tab::Arc => {
            hints.push(("↑↓", "tunable help"));
            if app.rw {
                hints.extend([("e", "edit tunable"), ("i", "reset")]);
            }
        }
        Tab::Zil => {
            hints.extend([("↑↓", "tunable help"), ("n", "next pool")]);
            if app.rw {
                hints.extend([("e", "edit tunable"), ("i", "reset")]);
            }
        }
        Tab::Props => match app.props_focus {
            crate::app::PropsFocus::Targets => {
                hints.extend([("↑↓", "pick pool/dataset"), ("Enter →", "its properties"), ("/", "filter")])
            }
            crate::app::PropsFocus::Props => hints.extend([
                ("↑↓", "select"),
                ("Enter →", "about"),
                ("Esc ←", "pools & datasets"),
                ("s/S", "sort"),
                ("L", "local only"),
                ("/", "filter"),
            ]),
            crate::app::PropsFocus::About => hints.extend([("↑↓", "scroll"), ("Esc ←", "back to properties")]),
        },
        _ => {}
    }
    if app.can_write {
        let in_props = app.prop_view_target().is_some()
            && (app.tab != Tab::Props || app.props_focus != crate::app::PropsFocus::Targets)
            && (app.tab != Tab::Datasets || app.ds_focus_pane);
        if app.rw && in_props {
            hints.extend([("e", "set"), ("i", "inherit"), ("a", "add")]);
        }
        hints.push(("W", if app.rw { "lock (rw)" } else { "write mode" }));
    }
    hints.extend([
        ("p", if app.paused { "resume" } else { "pause" }),
        ("+/-", "interval"),
        ("b", "bytes"),
        ("l", "log"),
    ]);
    let mut spans = Vec::new();
    if app.editing || !app.filter.is_empty() {
        spans.push(Span::styled(" filter: ", Style::new().fg(theme::WARN)));
        spans.push(Span::styled(format!("{}{} ", app.filter, if app.editing { "▏" } else { "" }), theme::bold()));
    }
    for (k, d) in hints {
        spans.push(Span::styled(format!(" {k}"), Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!(" {d} "), theme::dim()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::new().bg(theme::BG_DIM)), r);
}

fn help(f: &mut Frame, app: &mut App, area: Rect) {
    let keys: &[(&str, &str)] = &[
        ("1 … 7, Tab", "switch view: Overview, Pool, Datasets, ARC, ZIL·TXG, Disks, Props"),
        ("↑↓ j k, PgUp PgDn, g G", "move the selection"),
        ("← → Space", "collapse / expand tree rows (or click ▾ ▸)"),
        ("click a header", "sort by that column; click again to reverse (s / S from the keyboard)"),
        ("Enter", "open / close detail"),
        ("/", "filter the list (Esc clears)"),
        ("p", "pause: freezes the screen and releases the mouse so you can select & copy text; p resumes"),
        ("+ -", "sampling interval 1s … 60s"),
        ("b", "exact byte counts instead of K/M/G/T"),
        ("r", "re-read configuration now (zpool status/list, zfs list)"),
        ("l", "log pane"),
        ("Pool:  m / d / c / w / H", "column set / disk grid (↑↓/c colour) / heatmap wait type / history"),
        ("Datasets:  t / Enter", "ztop mode / detail ([ ] panes, Tab focus list ↔ pane, P opens Props)"),
        ("ARC, ZIL·TXG:  ↑↓ / n", "pick a tunable to read its help / next pool"),
        ("Props:  Enter / Esc", "move focus right (pools → properties → about) / left"),
        ("       L", "show only locally set properties"),
        ("W", "write mode (root): unlock / lock changing properties"),
        (
            "  e / i / a",
            "in write mode, in Props or Datasets ▸ Properties: set / inherit (reset) / add a user property",
        ),
        ("q, Ctrl-C", "quit"),
    ];
    let head = |t: String| Line::from(Span::styled(t, Style::new().fg(theme::ACCENT2).add_modifier(Modifier::BOLD)));
    let mut lines = vec![head(format!("What am I looking at? — {}", app.tab.title()))];
    for (h, t) in crate::help::tab_guide(app.tab) {
        lines.push(Line::from(vec![Span::styled(format!("{h}: "), theme::bold()), Span::raw(crate::help::words(t))]));
    }
    lines.push(Line::from(""));
    lines.push(head("Terms".into()));
    for (k, d) in crate::help::GLOSSARY {
        lines.push(Line::from(vec![Span::styled(format!(" {k:<18}"), theme::bold()), Span::styled(*d, theme::dim())]));
    }
    lines.push(Line::from(""));
    lines.push(head("Keys".into()));
    for (k, d) in keys {
        lines.push(Line::from(vec![
            Span::styled(format!(" {k:<26}"), Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
            Span::raw(*d),
        ]));
    }
    let w = 110.min(area.width.saturating_sub(4));
    let h = area.height.saturating_sub(4).min(48);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    // clamp scrolling to the wrapped length (rough: count wrapped rows)
    let inner_w = w.saturating_sub(2).max(1) as usize;
    let total: usize = lines.iter().map(|l| l.width().div_ceil(inner_w).max(1)).sum();
    let max_scroll = total.saturating_sub(h.saturating_sub(2) as usize);
    app.help_scroll = app.help_scroll.min(max_scroll);
    f.render_widget(Clear, r);
    let more = if max_scroll > 0 { " · ↑↓ scroll" } else { "" };
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.help_scroll as u16, 0))
            .block(panel(&format!("help{more} · any other key closes"))),
        r,
    );
}

/// `label value` pairs as spans.
pub fn kv(label: &str, value: impl Into<String>, color: Option<ratatui::style::Color>) -> Vec<Span<'static>> {
    let v: String = value.into();
    vec![
        Span::styled(format!("{label} "), theme::dim()),
        match color {
            Some(c) => Span::styled(v, Style::new().fg(c)),
            None => Span::raw(v),
        },
    ]
}

/// Title for a card: "name  value  note".
pub fn graph_title(name: &str, now: String, note: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {name} "), Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{now} "), theme::bold()),
        Span::styled(if note.is_empty() { String::new() } else { format!("{note} ") }, theme::dim()),
    ])
}

/// Pad a table row to the full width, so a row highlight spans the whole table like the header does.
pub fn full(mut l: Line<'static>, w: usize) -> Line<'static> {
    let cur = l.width();
    if cur < w {
        l.spans.push(Span::raw(" ".repeat(w - cur)));
    }
    l
}

/// Inner area of a panel.
pub fn inner(r: Rect) -> Rect {
    Rect { x: r.x + 1, y: r.y + 1, width: r.width.saturating_sub(2), height: r.height.saturating_sub(2) }
}

/// A table column: header title, width, alignment, and (optionally) the sort it selects when clicked.
pub struct Col {
    pub title: &'static str,
    pub w: usize,
    pub right: bool,
    pub hit: Option<Hit>,
    /// Some(desc) when this column is the active sort
    pub active: Option<bool>,
    pub color: Option<ratatui::style::Color>,
}

impl Col {
    pub fn new(title: &'static str, w: usize, right: bool) -> Self {
        Col { title, w, right, hit: None, active: None, color: None }
    }
    pub fn sort(mut self, hit: Hit, active: Option<bool>) -> Self {
        self.hit = Some(hit);
        self.active = active;
        self
    }
    pub fn color(mut self, c: ratatui::style::Color) -> Self {
        self.color = Some(c);
        self
    }
    /// Pad/align a cell value to this column.
    pub fn cell(&self, s: &str) -> String {
        if self.right { fmt::rpad(fmt::fit(s, self.w).trim_end(), self.w) } else { fmt::fit(s, self.w) }
    }
}

/// Header line for `cols` starting at (x, y); sortable columns become click targets and the active one shows ▲/▼.
pub fn header_line(cols: &[Col], x: u16, y: u16, total_w: usize, hits: &mut crate::app::HitMap) -> Line<'static> {
    let mut spans = Vec::new();
    let mut cx = x;
    for c in cols {
        let arrow = match c.active {
            Some(true) => "▼",
            Some(false) => "▲",
            None => "",
        };
        // a trailing space in a right-aligned title is deliberate (lines it up with values that have one)
        let t = match (c.right, c.title.strip_suffix(' ')) {
            (true, Some(base)) if !arrow.is_empty() => format!("{base}{arrow} "),
            _ => format!("{}{arrow}", c.title),
        };
        let text = if c.right { fmt::rpad(&t, c.w) } else { fmt::fit(&t, c.w) };
        let mut st = theme::header();
        if let Some(col) = c.color {
            st = st.fg(col);
        }
        if c.active.is_some() {
            st = st.add_modifier(Modifier::UNDERLINED);
        }
        if let Some(h) = c.hit {
            hits.add(Rect::new(cx, y, c.w as u16, 1), h);
        }
        cx += c.w as u16;
        spans.push(Span::styled(text, st));
    }
    let used: usize = cols.iter().map(|c| c.w).sum();
    spans.push(Span::styled(" ".repeat(total_w.saturating_sub(used)), theme::header()));
    Line::from(spans)
}
