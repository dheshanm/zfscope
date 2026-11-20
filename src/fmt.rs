//! Number formatting helpers. Everything here is pure and width-conscious: table cells are narrow.

use std::sync::atomic::{AtomicBool, Ordering};

/// `b` toggles exact byte counts instead of IEC units, process-wide (render code has no context to thread it).
static EXACT: AtomicBool = AtomicBool::new(false);

pub fn set_exact(on: bool) {
    EXACT.store(on, Ordering::Relaxed);
}

pub fn exact() -> bool {
    EXACT.load(Ordering::Relaxed)
}

const UNITS: [&str; 7] = ["B", "K", "M", "G", "T", "P", "E"];

/// Compact IEC size, zfs-style: `512B`, `4.0K`, `18.2T`, `1.07P` (3 significant digits, max 6 chars).
pub fn size(n: u64) -> String {
    if exact() {
        return n.to_string();
    }
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        return format!("{n}B");
    }
    match v {
        v if v < 9.995 => format!("{v:.2}{}", UNITS[u]),
        v if v < 99.95 => format!("{v:.1}{}", UNITS[u]),
        v => format!("{:.0}{}", v, UNITS[u]),
    }
}

/// Size with a non-zero guard: `-` for zero (sparse tables read better).
pub fn size_or_dash(n: u64) -> String {
    if n == 0 { "-".into() } else { size(n) }
}

/// Bytes per second: `0`, `512B/s`, `146M/s`.
pub fn rate(bps: f64) -> String {
    let b = bps.max(0.0);
    if b < 0.5 {
        return "0".into();
    }
    if exact() {
        return format!("{}/s", b.round() as u64);
    }
    format!("{}/s", size(b.round() as u64))
}

/// Compact count: `0`, `950`, `12.3k`, `1.2M`, `4.5G`.
pub fn count(n: f64) -> String {
    let n = n.max(0.0);
    match n {
        n if n < 0.05 => "0".into(),
        n if n < 9.95 && n.fract() != 0.0 => format!("{n:.1}"),
        n if n < 999.5 => format!("{}", n.round() as u64),
        n if n < 9_950.0 => format!("{:.2}k", n / 1e3),
        n if n < 999_500.0 => format!("{:.1}k", n / 1e3),
        n if n < 999_500_000.0 => format!("{:.1}M", n / 1e6),
        n => format!("{:.1}G", n / 1e9),
    }
}

/// Integer count with thousands separators: `1,234,567`.
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Latency from nanoseconds: `-` (none), `850ns`, `26us`, `4.2ms`, `120ms`, `1.24s`.
pub fn ns(n: f64) -> String {
    if !n.is_finite() || n <= 0.0 {
        return "-".into();
    }
    match n {
        n if n < 999.5 => format!("{}ns", n.round() as u64),
        n if n < 9_950.0 => format!("{:.1}us", n / 1e3),
        n if n < 999_500.0 => format!("{}us", (n / 1e3).round() as u64),
        n if n < 9_950_000.0 => format!("{:.1}ms", n / 1e6),
        n if n < 999_500_000.0 => format!("{}ms", (n / 1e6).round() as u64),
        n if n < 59.95e9 => format!("{:.2}s", n / 1e9),
        n => duration(n / 1e9),
    }
}

/// Compact duration: `45s`, `2m30s`, `1h05m`, `3d04h`.
pub fn duration(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    match s {
        0..60 => format!("{s}s"),
        60..3600 if s.is_multiple_of(60) => format!("{}m", s / 60),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        3600..86400 => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
        _ => format!("{}d{:02}h", s / 86400, (s % 86400) / 3600),
    }
}

/// Percentage with one decimal below 10%: `0.4%`, `2.3%`, `45%`.
pub fn pct(p: f64) -> String {
    if !p.is_finite() {
        return "-".into();
    }
    let p = p.clamp(0.0, 100.0);
    if p > 0.0 && p < 9.95 { format!("{p:.1}%") } else { format!("{}%", p.round() as u64) }
}

/// Hit ratio from hits and misses (`-` when there was no traffic).
pub fn ratio_pct(hits: f64, misses: f64) -> String {
    let t = hits + misses;
    if t <= 0.0 { "-".into() } else { pct(100.0 * hits / t) }
}

/// Truncate to `w` display columns with a trailing `…`, or pad with spaces.
pub fn fit(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n <= w {
        let mut o = s.to_string();
        o.extend(std::iter::repeat_n(' ', w - n));
        o
    } else if w == 0 {
        String::new()
    } else {
        let mut o: String = s.chars().take(w - 1).collect();
        o.push('…');
        o
    }
}

/// Keep the end of a long name (device paths differ at the end): `…CM7_U.2_2GECT00704E3`.
pub fn fit_left(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n <= w {
        return fit(s, w);
    }
    if w == 0 {
        return String::new();
    }
    let mut o = String::from("…");
    o.extend(s.chars().skip(n - (w - 1)));
    o
}

/// Right-align in `w` columns (truncating from the left if needed).
pub fn rpad(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n >= w { s.chars().skip(n - w).collect() } else { format!("{}{s}", " ".repeat(w - n)) }
}

const BARS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// One-line sparkline of the last `w` values, scaled to `max` (or the window's max when `max` is 0).
/// Any non-zero value shows at least one eighth so "a little" never reads as "none".
pub fn spark<I: IntoIterator<Item = f64>>(values: I, w: usize, max: f64) -> String {
    let v: Vec<f64> = values.into_iter().collect();
    let start = v.len().saturating_sub(w);
    let pts = &v[start..];
    let max = if max > 0.0 { max } else { pts.iter().copied().filter(|x| x.is_finite()).fold(0.0, f64::max) };
    let mut s: String = std::iter::repeat_n(' ', w - pts.len()).collect();
    for &x in pts {
        let idx = if !x.is_finite() || x <= 0.0 || max <= 0.0 {
            0
        } else {
            ((x / max) * 8.0).ceil().clamp(1.0, 8.0) as usize
        };
        s.push(BARS[idx]);
    }
    s
}

/// Human age of a unix timestamp relative to `now` (`3d04h ago`).
pub fn age(ts: i64, now: i64) -> String {
    if ts <= 0 {
        return "-".into();
    }
    format!("{} ago", duration((now - ts).max(0) as f64))
}

/// Pinned wall clock (unix seconds) and UTC dates, for deterministic renders (tests, screenshots). 0 = real clock.
static PINNED_NOW: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

pub fn pin_clock(unix_secs: i64) {
    PINNED_NOW.store(unix_secs, Ordering::Relaxed);
}

/// Current unix time (or the pinned one).
pub fn now() -> i64 {
    match PINNED_NOW.load(Ordering::Relaxed) {
        0 => jiff::Timestamp::now().as_second(),
        t => t,
    }
}

/// Unix seconds to local `YYYY-MM-DD HH:MM` (UTC while the clock is pinned).
pub fn date(ts: i64) -> String {
    let tz =
        if PINNED_NOW.load(Ordering::Relaxed) != 0 { jiff::tz::TimeZone::UTC } else { jiff::tz::TimeZone::system() };
    match jiff::Timestamp::from_second(ts) {
        Ok(t) => jiff::ZonedDateTime::new(t, tz).strftime("%Y-%m-%d %H:%M").to_string(),
        Err(_) => "-".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0B");
        assert_eq!(size(512), "512B");
        assert_eq!(size(4096), "4.00K");
        assert_eq!(size(20_000_588_955_648), "18.2T");
        assert_eq!(size(1_206_336_054_362_112), "1.07P");
        assert_eq!(size(539_082_605_805_568), "490T");
    }

    #[test]
    fn rates() {
        assert_eq!(rate(0.2), "0");
        assert_eq!(rate(146.0 * 1048576.0), "146M/s");
    }

    #[test]
    fn counts() {
        assert_eq!(count(0.0), "0");
        assert_eq!(count(2.5), "2.5");
        assert_eq!(count(950.2), "950");
        assert_eq!(count(2750.0), "2.75k");
        assert_eq!(count(12_345.0), "12.3k");
        assert_eq!(count(1_234_567.0), "1.2M");
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(123), "123");
    }

    #[test]
    fn latencies() {
        assert_eq!(ns(0.0), "-");
        assert_eq!(ns(426.0), "426ns");
        assert_eq!(ns(26_000.0), "26us");
        assert_eq!(ns(6_200_000.0), "6.2ms");
        assert_eq!(ns(120_000_000.0), "120ms");
        assert_eq!(ns(1_240_000_000.0), "1.24s");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(4.6), "5s");
        assert_eq!(duration(150.0), "2m30s");
        assert_eq!(duration(3900.0), "1h05m");
        assert_eq!(duration(3.0 * 86400.0 + 4.0 * 3600.0), "3d04h");
    }

    #[test]
    fn fitting() {
        assert_eq!(fit("abc", 5), "abc  ");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit_left("nvme-Dell_ABCDEF", 6), "…BCDEF");
        assert_eq!(rpad("7", 3), "  7");
    }

    #[test]
    fn sparks() {
        assert_eq!(spark([0.0, 1.0, 2.0, 4.0], 6, 0.0), "   ▂▄█");
        assert_eq!(spark([0.0, 0.01, 8.0], 3, 8.0), " ▁█");
    }
}
