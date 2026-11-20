//! Colours. xterm-256 indices so it looks the same in every terminal; colour never carries meaning alone
//! (states and warnings are also spelled out).

use ratatui::style::{Color, Modifier, Style};

pub const READ: Color = Color::Indexed(75);
pub const WRITE: Color = Color::Indexed(212);
pub const READ_DIM: Color = Color::Indexed(32);
pub const WRITE_DIM: Color = Color::Indexed(169);
pub const OK: Color = Color::Indexed(114);
pub const WARN: Color = Color::Indexed(214);
pub const CRIT: Color = Color::Indexed(203);
pub const DIM: Color = Color::Indexed(244);
pub const FAINT: Color = Color::Indexed(240);
pub const TRACK: Color = Color::Indexed(238);
pub const ACCENT: Color = Color::Indexed(110);
pub const ACCENT2: Color = Color::Indexed(180);
pub const HEADER_BG: Color = Color::Indexed(236);
pub const SEL_BG: Color = Color::Indexed(239);
/// selection in a panel that doesn't have keyboard focus: clearly visible, but quieter than SEL_BG
pub const SEL_DIM_BG: Color = Color::Indexed(236);
pub const BG_DIM: Color = Color::Indexed(234);
pub const BORDER: Color = Color::Indexed(240);

/// Space parts: dataset, snapshots, children, refreservation.
pub const SP_DS: Color = Color::Indexed(75);
pub const SP_SNAP: Color = Color::Indexed(179);
pub const SP_CHILD: Color = Color::Indexed(109);
pub const SP_REFRES: Color = Color::Indexed(139);

/// ARC parts.
pub const ARC_MRU: Color = Color::Indexed(110);
pub const ARC_MFU: Color = Color::Indexed(150);
pub const ARC_META: Color = Color::Indexed(180);

pub fn header() -> Style {
    Style::new().bg(HEADER_BG).add_modifier(Modifier::BOLD)
}

pub fn dim() -> Style {
    Style::new().fg(DIM)
}

pub fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

pub fn sel() -> Style {
    Style::new().bg(SEL_BG).add_modifier(Modifier::BOLD)
}

pub fn sel_dim() -> Style {
    Style::new().bg(SEL_DIM_BG)
}

/// Pool/vdev state colour.
pub fn state(s: &str) -> Color {
    match s {
        "ONLINE" | "AVAIL" => OK,
        "DEGRADED" | "REMOVED" | "OFFLINE" | "INUSE" => WARN,
        "" => DIM,
        _ => CRIT,
    }
}

/// Utilisation-ish colour: green, orange at `warn`, red at `crit`.
pub fn level(v: f64, warn: f64, crit: f64) -> Color {
    if !v.is_finite() {
        DIM
    } else if v >= crit {
        CRIT
    } else if v >= warn {
        WARN
    } else {
        OK
    }
}

/// Latency colour (ns) for spinning disks and SSDs alike: < 10ms fine, < 100ms slow, else bad.
pub fn latency(ns: f64) -> Color {
    if !ns.is_finite() || ns <= 0.0 {
        DIM
    } else if ns >= 100e6 {
        CRIT
    } else if ns >= 20e6 {
        WARN
    } else {
        Color::Reset
    }
}
