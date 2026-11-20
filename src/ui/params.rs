//! Module-parameter (tunable) list with built-in help for the selected one. Read-only.

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
    name.contains("dirty_data_max")
        || matches!(name, "zfs_arc_max" | "zfs_arc_min" | "zfs_arc_sys_free" | "l2arc_write_max" | "zil_slog_bulk")
        || name.ends_with("_sz")
        || name.ends_with("aggregation_limit")
}

pub fn display(name: &str, raw: &str) -> String {
    match raw.parse::<u64>() {
        Ok(0) if matches!(name, "zfs_arc_max" | "zfs_arc_min") => "0 (auto)".into(),
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
    let nw = names.iter().map(|n| n.len()).max().unwrap_or(10).min(w.saturating_sub(12));
    let body = ir.height as usize;
    let top = sel.saturating_sub(body.saturating_sub(1));
    let mut lines = Vec::new();
    for (i, n) in names.iter().enumerate().skip(top).take(body) {
        let v = app.model.param(n).map(|v| display(n, v)).unwrap_or_else(|| "-".into());
        let l = full(
            Line::from(vec![
                Span::styled(format!("{:<nw$}  ", fmt::fit(n, nw)), Style::new()),
                Span::styled(v, theme::bold()),
            ]),
            w,
        );
        lines.push(if i == sel { l.style(theme::sel()) } else { l });
    }
    f.render_widget(Paragraph::new(lines).block(panel(&format!("{title} · ↑↓ for help"))), list_r);
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
    hl.push(Line::from(Span::styled(format!("/sys/module/zfs/parameters/{name}"), Style::new().fg(theme::FAINT))));
    f.render_widget(Paragraph::new(hl).wrap(Wrap { trim: false }).block(panel("about")), help_r);
}
