//! Write-mode dialogs: the unlock prompt, the property editor and the confirmation that shows the exact command.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{BorderType, Clear, Paragraph, Wrap};

use super::{panel, props, theme};
use crate::app::{App, ConfirmState, EditState, Modal, prop_options};
use crate::help;
use crate::write::{Change, Target};

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let Some(m) = &app.modal else { return };
    let w = 96.min(area.width.saturating_sub(4)).max(20);
    let iw = w.saturating_sub(2).max(1) as usize;
    let (title, lines, border) = match m {
        Modal::Unlock => unlock(app),
        Modal::Edit(e) => edit(e, app, iw),
        Modal::Confirm(c) => confirm(c, app),
    };
    let rows: usize = lines.iter().map(|l| l.width().div_ceil(iw).max(1)).sum();
    let h = (rows as u16 + 2).min(area.height.saturating_sub(2)).max(3);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height.saturating_sub(h)) / 2, w, h);
    f.render_widget(Clear, r);
    let block = panel(&title).border_style(Style::new().fg(border)).border_type(BorderType::Thick);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), r);
}

fn dry_run(app: &App) -> bool {
    app.source.starts_with("fixture")
}

fn hint(s: &str) -> Line<'static> {
    Line::from(Span::styled(s.to_string(), theme::dim()))
}

fn label(s: &str) -> Span<'static> {
    Span::styled(format!("{s:<8}"), theme::dim())
}

fn target_desc(t: &Target) -> String {
    match t {
        Target::Pool(p) => format!("pool {p}"),
        Target::Dataset(d) => format!("dataset {d}"),
    }
}

fn unlock(app: &App) -> (String, Vec<Line<'static>>, ratatui::style::Color) {
    let mut l = vec![
        Line::from(Span::styled(format!("Enable write mode on {}?", app.host), theme::bold())),
        Line::from(""),
        Line::from("Lets you change properties from the Props views: e sets a value (or enables a feature flag),"),
        Line::from("i resets a dataset property to inherited, a adds a user property."),
        Line::from("Each change runs zfs set / zpool set / zfs inherit as root, only after you confirm the exact"),
        Line::from("command. Every change is logged (l, and the log file)."),
    ];
    if dry_run(app) {
        l.push(Line::from(""));
        l.push(Line::from(Span::styled(
            "Fixture replay: commands are shown but never run.",
            Style::new().fg(theme::WARN),
        )));
    }
    l.push(Line::from(""));
    l.push(Line::from(vec![
        Span::styled("y", Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(" enable · any other key cancels · W locks it again", theme::dim()),
    ]));
    ("write mode".into(), l, theme::WARN)
}

fn edit(e: &EditState, app: &App, iw: usize) -> (String, Vec<Line<'static>>, ratatui::style::Color) {
    let target = target_desc(&e.target);
    let title = if e.naming {
        format!("add a user property · {target}")
    } else if e.new_prop {
        format!("add {} · {target}", e.key)
    } else {
        format!("set {} · {target}", e.key)
    };
    let mut l = Vec::new();
    let cursor = Span::styled("▏", Style::new().fg(theme::ACCENT));
    if e.naming {
        l.push(Line::from(vec![label("name"), Span::styled(e.input.clone(), theme::bold()), cursor]));
        l.push(hint("namespace:name in lowercase, e.g. org.example:backup (ZFS itself ignores user properties)"));
    } else {
        if let Some(p) = app.target_props(&e.target).and_then(|ps| ps.get(&e.key)) {
            let (src, sst) = props::source(p);
            l.push(Line::from(vec![
                label("now"),
                Span::styled(props::display(&e.key, p), props::value_style(&e.key, &p.value)),
                Span::raw("  "),
                Span::styled(src, sst),
            ]));
        }
        l.push(Line::from(vec![
            label("new"),
            Span::styled(e.input.clone(), props::value_style(&e.key, &e.input).add_modifier(Modifier::BOLD)),
            cursor,
        ]));
    }
    if let Some(err) = &e.error {
        l.push(Line::from(Span::styled(format!("✗ {err}"), Style::new().fg(theme::CRIT))));
    }
    let opts = e.options();
    if !opts.is_empty() {
        l.push(Line::from(""));
        l.push(Line::from(Span::styled("possible values (↑↓ pick)", theme::dim().add_modifier(Modifier::UNDERLINED))));
        let ow = opts.iter().map(|(v, _)| v.chars().count()).max().unwrap_or(4).min(14);
        let cur = app.target_props(&e.target).and_then(|ps| ps.get(&e.key));
        for (i, (v, d)) in opts.iter().enumerate() {
            let now = cur.is_some_and(|p| help::option_matches(v, &p.value, p.num));
            let picked = e.opt_sel == Some(i);
            let mark = match (picked, now) {
                (true, _) => "▸ ",
                (false, true) => "● ",
                _ => "  ",
            };
            let mut vs = props::value_style(&e.key, v);
            if picked {
                vs = vs.add_modifier(Modifier::BOLD | Modifier::REVERSED);
            }
            // descriptions wrap with a hanging indent so the values stay in their own column
            let ds = if picked { Style::new() } else { theme::dim() };
            for (j, chunk) in props::wrap(d, iw.saturating_sub(2 + ow + 2).max(12)).into_iter().enumerate() {
                l.push(Line::from(if j == 0 {
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
    } else if !e.naming {
        let man = if e.target.is_pool() { "zpoolprops" } else { "zfsprops" };
        l.push(hint(&format!("free-form value; ZFS checks it (see man {man})")));
    }
    l.push(Line::from(""));
    l.push(hint(if e.naming {
        "Enter next: the value · Backspace edit · Esc cancel"
    } else {
        "Enter review the command · ↑↓ values · Ctrl-U clear · Esc cancel"
    }));
    (title, l, theme::ACCENT)
}

fn confirm(c: &ConfirmState, app: &App) -> (String, Vec<Line<'static>>, ratatui::style::Color) {
    let one_way = c.change.one_way();
    let key = c.change.key().to_string();
    let target = c.change.target();
    let mut l = vec![Line::from(vec![
        label("run"),
        Span::styled(c.change.command_line(), Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
    ])];
    l.push(Line::from(vec![label("on"), Span::raw(target_desc(&target))]));
    match (&c.change, &c.old) {
        (Change::Set { value, .. }, Some(old)) => l.push(Line::from(vec![
            label("change"),
            Span::styled(key.clone(), theme::bold()),
            Span::raw("  "),
            Span::styled(old.value.clone(), props::value_style(&key, &old.value)),
            Span::styled(" → ", theme::dim()),
            Span::styled(value.clone(), props::value_style(&key, value).add_modifier(Modifier::BOLD)),
        ])),
        (Change::Set { value, .. }, None) => l.push(Line::from(vec![
            label("add"),
            Span::styled(key.clone(), theme::bold()),
            Span::styled(" = ", theme::dim()),
            Span::styled(value.clone(), theme::bold()),
        ])),
        (Change::Inherit { .. }, old) => {
            let now = old.as_ref().map(|p| p.value.clone()).unwrap_or_default();
            let what =
                if key.contains(':') { "removed (user property)" } else { "inherited from the parent, or the default" };
            l.push(Line::from(vec![
                label("reset"),
                Span::styled(key.clone(), theme::bold()),
                Span::raw("  "),
                Span::styled(now, theme::dim()),
                Span::styled(format!(" → {what}"), theme::dim()),
            ]));
        }
    }
    // a risky choice: say what it does, in the words of the possible-values list
    if let Change::Set { value, .. } = &c.change {
        let st = props::value_style(&key, value);
        if matches!(st.fg, Some(c) if c == theme::WARN || c == theme::CRIT)
            && let Some((_, d)) = prop_options(&target, &key).iter().find(|(o, _)| help::option_matches(o, value, None))
        {
            l.push(Line::from(Span::styled(format!("⚠ {value}: {d}"), st)));
        }
    }
    if let Some(note) = crate::write::side_effect(&key) {
        l.push(Line::from(vec![label("note"), Span::styled(note, theme::dim())]));
    }
    if dry_run(app) {
        l.push(Line::from(Span::styled("fixture replay: dry run, nothing is executed", Style::new().fg(theme::WARN))));
    }
    l.push(Line::from(""));
    if one_way {
        let pool = target.pool().to_string();
        let since = key.strip_prefix("feature@").map(|f| help::feature_info(f).0).unwrap_or("?");
        l.push(Line::from(Span::styled(
            "This cannot be undone: a feature flag can never be disabled again.",
            Style::new().fg(theme::CRIT).add_modifier(Modifier::BOLD),
        )));
        l.push(Line::from(Span::styled(
            format!("Once the feature is active, {pool} can only be imported by OpenZFS {since} or newer."),
            Style::new().fg(theme::CRIT),
        )));
        l.push(Line::from(""));
        l.push(Line::from(vec![
            Span::styled(format!("type {pool} and press Enter to confirm: "), theme::dim()),
            Span::styled(c.typed.clone(), theme::bold()),
            Span::styled("▏", Style::new().fg(theme::ACCENT)),
        ]));
        if let Some(err) = &c.error {
            l.push(Line::from(Span::styled(format!("✗ {err}"), Style::new().fg(theme::CRIT))));
        }
        l.push(hint("Esc cancels"));
        ("confirm a one-way change".into(), l, theme::CRIT)
    } else {
        l.push(Line::from(vec![
            Span::styled("y", Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" run it · any other key cancels", theme::dim()),
        ]));
        ("confirm".into(), l, theme::WARN)
    }
}
