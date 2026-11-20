//! Write-mode dialogs: the unlock prompt, the property and tunable editors, and the confirmation that shows exactly
//! what will run.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{BorderType, Clear, Paragraph, Wrap};

use super::{panel, params, props, theme};
use crate::app::{App, ConfirmState, EditState, Modal, Scope, TuneState, prop_options};
use crate::help::{self, Kind};
use crate::write::{Boot, Change, ParamChange, Target};

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let Some(m) = &app.modal else { return };
    let w = 96.min(area.width.saturating_sub(4)).max(20);
    let iw = w.saturating_sub(2).max(1) as usize;
    let (title, lines, border) = match m {
        Modal::Unlock => unlock(app),
        Modal::Edit(e) => edit(e, app, iw),
        Modal::Tune(t) => tune(t, app, iw),
        Modal::Confirm(ConfirmState { change: Change::Param(p), .. }) => confirm_param(p, app, iw),
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
        Line::from("On the ARC and ZIL·TXG tabs, e changes a tunable (now and/or at boot), i resets it."),
        Line::from("Changes run zfs set / zpool set / zfs inherit, or write /sys/module/zfs/parameters and"),
        Line::from("/etc/modprobe.d, as root, only after you confirm exactly what happens. All are logged (l)."),
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
        let cur = app.target_props(&e.target).and_then(|ps| ps.get(&e.key));
        options_block(
            &mut l,
            opts,
            e.opt_sel,
            |v| cur.is_some_and(|p| help::option_matches(v, &p.value, p.num)),
            |v| props::value_style(&e.key, v),
            iw,
        );
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

/// The "possible values" list: values in their own column, descriptions wrapped beside them; ▸ marks the picked
/// entry and ● the current value.
fn options_block<S: AsRef<str>>(
    l: &mut Vec<Line<'static>>,
    opts: &[(S, S)],
    picked: Option<usize>,
    is_now: impl Fn(&str) -> bool,
    style: impl Fn(&str) -> Style,
    iw: usize,
) {
    l.push(Line::from(""));
    l.push(Line::from(Span::styled("possible values (↑↓ pick)", theme::dim().add_modifier(Modifier::UNDERLINED))));
    let ow = opts.iter().map(|(v, _)| v.as_ref().chars().count()).max().unwrap_or(4).min(14);
    for (i, (v, d)) in opts.iter().enumerate() {
        let (v, d) = (v.as_ref(), d.as_ref());
        let sel = picked == Some(i);
        let mark = match (sel, is_now(v)) {
            (true, _) => "▸ ",
            (false, true) => "● ",
            _ => "  ",
        };
        let mut vs = style(v);
        if sel {
            vs = vs.add_modifier(Modifier::BOLD | Modifier::REVERSED);
        }
        // descriptions wrap with a hanging indent so the values stay in their own column
        let ds = if sel { Style::new() } else { theme::dim() };
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
}

/// `label  text`, the text wrapped with a hanging indent under itself.
fn field(l: &mut Vec<Line<'static>>, name: &str, text: &str, st: Style, iw: usize) {
    for (j, chunk) in props::wrap(text, iw.saturating_sub(8).max(12)).into_iter().enumerate() {
        let lead = if j == 0 { label(name) } else { Span::raw(" ".repeat(8)) };
        l.push(Line::from(vec![lead, Span::styled(chunk, st)]));
    }
}

fn warn_line(l: &mut Vec<Line<'static>>, text: &str, iw: usize) {
    field(l, "⚠", text, Style::new().fg(theme::WARN), iw);
}

/// A tunable value for people: sizes humanised, 0 = auto where that's what it means.
fn param_value(name: &str, raw: &str) -> String {
    params::display(name, raw)
}

/// What a tunable accepts, in words.
fn range_text(t: &help::Tunable) -> String {
    let n = |v: u64| match t.kind {
        Kind::Bytes if v >= 1024 => crate::fmt::size(v),
        _ => v.to_string(),
    };
    let open = t.max == u64::MAX || t.max == u32::MAX as u64 || t.max == i64::MAX as u64;
    let span = match (t.kind, open) {
        (Kind::Bool, _) => String::new(),
        (_, true) if t.min == 0 => String::new(),
        (_, true) => format!(", at least {}", n(t.min)),
        (_, false) => format!(", {}–{}", n(t.min), n(t.max)),
    };
    let zero = if t.zero_auto { " (0 = automatic)" } else { "" };
    format!("{}{span}{zero}", t.kind.unit())
}

/// Warnings that depend on the value being set (risky choices, an ARC limit below what it holds now).
fn value_warnings(name: &str, v: u64, app: &App) -> Vec<String> {
    let mut w = Vec::new();
    if let Some(r) = help::param_risk(name, v) {
        w.push(format!("{name}={v}: {r}"));
    }
    let size = app.model.val("arcstats", "size");
    if name == "zfs_arc_max" && v != 0 && size > v {
        w.push(format!(
            "the ARC holds {} now: it shrinks to the new limit only as it evicts blocks, and more reads go to disk",
            crate::fmt::size(size)
        ));
    }
    w
}

fn tune(t: &TuneState, app: &App, iw: usize) -> (String, Vec<Line<'static>>, ratatui::style::Color) {
    let name = t.name.as_str();
    let title = format!("set {name} · ZFS module parameter");
    let tun = help::tunable(name);
    let mut l = Vec::new();
    if let Some((a, b)) = help::param(name) {
        l.push(Line::from(Span::styled(help::words(a), theme::bold())));
        for para in help::paragraphs(b).into_iter().filter(|p| !p.is_empty()) {
            l.push(Line::from(Span::styled(para, theme::dim())));
        }
        l.push(Line::from(""));
    }
    let mut now = vec![label("now"), Span::styled(param_value(name, &t.orig), theme::bold())];
    if t.load_only {
        now.push(Span::styled("  read-only while the module is loaded", theme::dim()));
    }
    l.push(Line::from(now));
    match app.model.boot_param(name) {
        Some((v, f)) => l.push(Line::from(vec![
            label("boot"),
            Span::styled(param_value(name, &v), Style::new().fg(theme::ACCENT)),
            Span::styled(format!("  from {f}"), theme::dim()),
        ])),
        None => l.push(Line::from(vec![label("boot"), Span::styled("not set: the default applies", theme::dim())])),
    }
    if let Some(c) = app.model.cmdline_param(name) {
        warn_line(&mut l, &format!("the kernel command line sets zfs.{name}={c}, which overrides modprobe.d"), iw);
    }
    if let Some(tun) = &tun {
        let default = match (tun.default, app.model.param_default(name)) {
            ("", Some(d)) => format!("{} ({})", tun.auto, param_value(name, &d)),
            ("", None) => tun.auto.to_string(),
            (d, _) if tun.zero_auto && d == "0" => format!("0 = automatic: {}", tun.auto),
            (d, _) => param_value(name, d),
        };
        field(&mut l, "default", &default, Style::new(), iw);
        field(&mut l, "accepts", &range_text(tun), theme::dim(), iw);
        field(
            &mut l,
            "applies",
            if t.load_only { "at the next module load (reboot)" } else { tun.applies },
            theme::dim(),
            iw,
        );
        field(&mut l, "raise", tun.raise, Style::new(), iw);
        field(&mut l, "lower", tun.lower, Style::new(), iw);
        if !tun.note.is_empty() {
            field(&mut l, "note", tun.note, Style::new().fg(theme::WARN), iw);
        }
    }
    l.push(Line::from(""));
    let cursor = Span::styled("▏", Style::new().fg(theme::ACCENT));
    let mut new = vec![label("new"), Span::styled(t.input.clone(), theme::bold()), cursor];
    let parsed = tun.as_ref().and_then(|tun| crate::write::parse_value(tun.kind, &t.input).ok());
    if let (Some(v), Some(tun)) = (parsed, &tun)
        && tun.kind == Kind::Bytes
    {
        if t.input.trim() != v.to_string() {
            new.push(Span::styled(format!("  = {v} ({})", crate::fmt::size(v)), theme::dim()));
        } else if v >= 1024 {
            new.push(Span::styled(format!("  = {}", crate::fmt::size(v)), theme::dim()));
        }
    }
    l.push(Line::from(new));
    if let Some(v) = parsed {
        for w in value_warnings(name, v, app) {
            warn_line(&mut l, &w, iw);
        }
    }
    let mut sc = vec![label("apply")];
    for s in Scope::ALL {
        if t.load_only && s != Scope::BootOnly {
            continue;
        }
        let st = if s == t.scope {
            Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            theme::dim()
        };
        sc.push(Span::styled(format!(" {} ", s.label()), st));
        sc.push(Span::raw(" "));
    }
    if !t.load_only {
        sc.push(Span::styled("(Tab)", theme::dim()));
    }
    l.push(Line::from(sc));
    if let Some(err) = &t.error {
        l.push(Line::from(Span::styled(format!("✗ {err}"), Style::new().fg(theme::CRIT))));
    }
    let opts = app.tune_options(name);
    if !opts.is_empty() {
        options_block(&mut l, &opts, t.opt_sel, |v| v == t.orig, |_| Style::new(), iw);
    }
    l.push(Line::from(""));
    l.push(hint("Enter review the change · Tab now / at boot · ↑↓ values · Ctrl-U clear · Esc cancel"));
    (title, l, theme::ACCENT)
}

fn confirm_param(p: &ParamChange, app: &App, iw: usize) -> (String, Vec<Line<'static>>, ratatui::style::Color) {
    let name = p.name.as_str();
    let accent = Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD);
    let mut l = Vec::new();
    if let Some(v) = &p.value {
        l.push(Line::from(vec![label("run"), Span::styled(format!("echo {v} > {}", p.path()), accent)]));
        l.push(Line::from(vec![
            label("now"),
            Span::styled(name.to_string(), theme::bold()),
            Span::raw("  "),
            Span::raw(param_value(name, &p.was)),
            Span::styled(" → ", theme::dim()),
            Span::styled(param_value(name, v), theme::bold()),
        ]));
        field(&mut l, "applies", help::tunable(name).map_or("immediately", |t| t.applies), theme::dim(), iw);
    }
    if p.boot != Boot::Keep {
        let text = app.model.modprobe_text(&p.file);
        let value = match &p.boot {
            Boot::Set(v) => Some(v.as_str()),
            _ => None,
        };
        let after = crate::write::modprobe_set(text, name, value);
        let (old, new) = (crate::write::modprobe_line(text, name), crate::write::modprobe_line(&after, name));
        let verb = if text.is_empty() && !app.model.boot_param_files(name).contains(&p.file.as_str()) {
            "create"
        } else {
            "edit"
        };
        l.push(Line::from(vec![label(verb), Span::styled(p.file.clone(), accent)]));
        if let Some(o) = old {
            l.push(Line::from(vec![label(""), Span::styled(format!("- {o}"), Style::new().fg(theme::CRIT))]));
        }
        match new {
            Some(n) => l.push(Line::from(vec![label(""), Span::styled(format!("+ {n}"), Style::new().fg(theme::OK))])),
            None => l.push(Line::from(vec![label(""), Span::styled("  (line removed)", theme::dim())])),
        }
        let was = p.boot_was.as_deref().map_or("not set".into(), |b| param_value(name, b));
        let will = match &p.boot {
            Boot::Set(v) => param_value(name, v),
            _ => "not set (the default)".into(),
        };
        l.push(Line::from(vec![
            label("boot"),
            Span::styled(name.to_string(), theme::bold()),
            Span::raw("  "),
            Span::raw(was),
            Span::styled(" → ", theme::dim()),
            Span::styled(will, theme::bold()),
            Span::styled("  at next boot", theme::dim()),
        ]));
        if p.value.is_none() {
            field(
                &mut l,
                "note",
                &format!("the running value stays {} until then", param_value(name, &p.was)),
                theme::dim(),
                iw,
            );
        }
        field(&mut l, "backup", &format!("the old file is kept as {}.zfscope-bak", p.file), theme::dim(), iw);
        if let Some(c) = app.model.cmdline_param(name) {
            warn_line(
                &mut l,
                &format!("the kernel command line sets zfs.{name}={c}: it wins over modprobe.d until you remove it"),
                iw,
            );
        }
        if app.model.root_on_zfs() {
            warn_line(
                &mut l,
                "/ is on ZFS, so the module loads from the initramfs, which has its own copy of modprobe.d: run \
                 update-initramfs -u (Debian/Ubuntu) or dracut -f (RHEL/Fedora) afterwards",
                iw,
            );
        }
    } else if p.value.is_some() {
        field(&mut l, "note", "until reboot: the boot setting is unchanged", theme::dim(), iw);
    }
    // the value being set (the boot one when only that changes)
    let shown = match &p.boot {
        Boot::Set(v) => Some(v),
        _ => p.value.as_ref(),
    };
    let v = shown.and_then(|v| v.parse::<u64>().ok());
    for w in v.map(|v| value_warnings(name, v, app)).unwrap_or_default() {
        warn_line(&mut l, &w, iw);
    }
    if dry_run(app) {
        l.push(Line::from(Span::styled("fixture replay: dry run, nothing is executed", Style::new().fg(theme::WARN))));
    }
    l.push(Line::from(""));
    l.push(Line::from(vec![
        Span::styled("y", Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(" run it · any other key cancels", theme::dim()),
    ]));
    let border = if v.and_then(|v| help::param_risk(name, v)).is_some() { theme::CRIT } else { theme::WARN };
    (format!("confirm · {name}"), l, border)
}

fn confirm(c: &ConfirmState, app: &App) -> (String, Vec<Line<'static>>, ratatui::style::Color) {
    let one_way = c.change.one_way();
    let key = c.change.key().to_string();
    let Some(target) = c.change.target() else { return ("confirm".into(), Vec::new(), theme::WARN) };
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
        (Change::Param(_), _) => {}
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
