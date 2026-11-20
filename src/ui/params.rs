//! Module-parameter (tunable) list with built-in help for the selected one. In write mode, `e` / `i` change the
//! selected one (see `ui::edit`).

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::{full, inner, panel, theme};
use crate::app::App;
use crate::fmt;
use crate::help;

/// Parameters whose values are byte counts.
fn is_bytes(name: &str) -> bool {
    help::tunable(name).is_some_and(|t| t.kind == help::Kind::Bytes)
        || name.contains("dirty_data_max")
        || matches!(name, "zfs_arc_max" | "zfs_arc_min" | "zfs_arc_sys_free" | "l2arc_write_max" | "zil_slog_bulk")
        || name.ends_with("_sz")
        || name.ends_with("aggregation_limit")
}

pub fn display(name: &str, raw: &str) -> String {
    match raw.parse::<u64>() {
        Ok(0) if help::tunable(name).is_some_and(|t| t.zero_auto) => "0 (auto)".into(),
        Ok(n) if is_bytes(name) && !fmt::exact() => format!("{} ({n})", fmt::size(n)),
        _ => raw.to_string(),
    }
}

pub fn draw(f: &mut Frame, app: &mut App, r: Rect, title: &str, names: &[&str]) {
    let sel = app.param_sel.min(names.len().saturating_sub(1));
    app.param_sel = sel;
    let help_h = if r.height >= 14 { 6 } else { 4 };
    let [list_r, help_r] = Layout::vertical([Constraint::Min(3), Constraint::Length(help_h)]).areas(r);
    let ir = inner(list_r);
    let w = ir.width as usize;
    let nw = names.iter().map(|n| n.len()).max().unwrap_or(10).min(w.saturating_sub(14));
    let body = ir.height as usize;
    let top = sel.saturating_sub(body.saturating_sub(1));
    let mut lines = Vec::new();
    for (i, n) in names.iter().enumerate().skip(top).take(body) {
        let raw = app.model.param(n);
        let v = raw.map(|v| display(n, v)).unwrap_or_else(|| "-".into());
        // ◆ = not the default (a fixed default that differs)
        let changed = raw.zip(help::tunable(n)).is_some_and(|(r, t)| !t.default.is_empty() && r != t.default);
        let mut spans = vec![
            Span::styled(if changed { "◆" } else { " " }, Style::new().fg(theme::ACCENT2)),
            Span::styled(format!("{:<nw$} ", fmt::fit(n, nw)), Style::new()),
            Span::styled(v, theme::bold()),
        ];
        if let Some((b, _)) = app.model.boot_param(n).filter(|(b, _)| Some(b.as_str()) != raw) {
            spans.push(Span::styled(format!("  boot {}", display(n, &b)), Style::new().fg(theme::FAINT)));
        }
        let l = full(Line::from(spans), w);
        lines.push(if i == sel { l.style(theme::sel()) } else { l });
    }
    let keys = if app.rw { "↑↓ for help · e edit · i reset" } else { "↑↓ for help" };
    f.render_widget(Paragraph::new(lines).block(panel(&format!("{title} · {keys}"))), list_r);
    let name = names.get(sel).copied().unwrap_or("");
    let mut hl =
        vec![Line::from(Span::styled(name.to_string(), Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)))];
    match help::param(name) {
        Some((a, b)) => {
            hl.push(Line::from(help::words(a)));
            for para in help::paragraphs(b).into_iter().filter(|p| !p.is_empty()) {
                hl.push(Line::from(Span::styled(para, theme::dim())));
            }
        }
        None => hl.push(Line::from(Span::styled("see `man 4 zfs`", theme::dim()))),
    }
    let mut foot = Vec::new();
    if let Some(t) = help::tunable(name) {
        let d = match t.default {
            "" => t.auto.to_string(),
            d => display(name, d),
        };
        foot.push(Span::styled(format!("default {d} · "), theme::dim()));
    }
    if let Some((b, file)) = app.model.boot_param(name) {
        foot.push(Span::styled(format!("boot {} ({file}) · ", display(name, &b)), theme::dim()));
    }
    foot.push(Span::styled(format!("/sys/module/zfs/parameters/{name}"), Style::new().fg(theme::FAINT)));
    hl.push(Line::from(foot));
    f.render_widget(Paragraph::new(hl).wrap(Wrap { trim: false }).block(panel("about")), help_r);
}
