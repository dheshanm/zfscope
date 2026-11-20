//! `zpool history -il <pool>`: commands, internal events (`[txg:N] ...`) and ioctls, some spanning several lines
//! (nvlist dumps), each optionally ending in ` [user 0 (root) on host:linux]` / ` [on host]`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `zfs ...` / `zpool ...` as typed
    Command,
    /// `[txg:N] create pool0/x (123)`
    Internal,
    /// `ioctl snapshot` with an nvlist
    Ioctl,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// `2026-10-02.16:16:15`
    pub time: String,
    pub kind: Kind,
    pub text: String,
    pub txg: Option<u64>,
    pub who: Option<String>,
    pub detail: Vec<String>,
}

fn is_stamp(l: &str) -> bool {
    let b = l.as_bytes();
    b.len() >= 20
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'.'
        && b[13] == b':'
        && b[16] == b':'
        && b[0].is_ascii_digit()
}

/// Split a trailing ` [user ... ]` / ` [on host]` off.
fn split_who(s: &str) -> (&str, Option<String>) {
    let t = s.trim_end();
    if t.ends_with(']')
        && let Some(i) =
            t.rfind(" [user ").or_else(|| t.rfind(" [on ")).or_else(|| t.starts_with("[user ").then_some(0))
    {
        let who = t[i..].trim().trim_start_matches('[').trim_end_matches(']').to_string();
        return (t[..i].trim_end(), Some(who));
    }
    (t, None)
}

/// Parse; keeps only the newest `keep` entries (histories can hold years of auto-snapshots).
pub fn parse(text: &str, keep: usize) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for line in text.lines() {
        if is_stamp(line) {
            let (time, rest) = line.split_at(19);
            let (body, who) = split_who(rest.trim_start());
            let (kind, txg, text) = if let Some(r) = body.strip_prefix("[txg:") {
                let (n, t) = r.split_once(']').unwrap_or((r, ""));
                (Kind::Internal, n.parse().ok(), t.trim().to_string())
            } else if body.starts_with("zfs ") || body.starts_with("zpool ") {
                (Kind::Command, None, body.to_string())
            } else {
                (Kind::Ioctl, None, body.to_string())
            };
            out.push(Entry { time: time.to_string(), kind, text, txg, who, detail: Vec::new() });
        } else if let Some(e) = out.last_mut() {
            let (body, who) = split_who(line);
            if who.is_some() {
                e.who = who;
            }
            if !body.trim().is_empty() {
                e.detail.push(body.trim_end().to_string());
            }
        }
    }
    let n = out.len();
    if n > keep {
        out.drain(..n - keep);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "History for 'pool0':
2026-10-02.13:51:59 [txg:4] create pool version 5000; software version zfs-2.4.4
2026-10-02.13:51:59 [txg:4] set ashift=12 [on host1]
2026-10-02.16:16:15 zfs create -o recordsize=1M pool0/a [user 0 (root) on host1:linux]
2026-10-02.16:16:30 ioctl snapshot
    input:
        snaps:
            pool0/a@s1
 [user 0 (root) on host1:linux]
";

    #[test]
    fn entries() {
        let e = parse(H, 100);
        assert_eq!(e.len(), 4);
        assert_eq!((e[0].kind, e[0].txg), (Kind::Internal, Some(4)));
        assert_eq!(e[1].text, "set ashift=12");
        assert_eq!(e[1].who.as_deref(), Some("on host1"));
        assert_eq!(e[2].kind, Kind::Command);
        assert_eq!(e[2].text, "zfs create -o recordsize=1M pool0/a");
        assert_eq!(e[2].who.as_deref(), Some("user 0 (root) on host1:linux"));
        assert_eq!(e[3].kind, Kind::Ioctl);
        assert_eq!(e[3].detail.len(), 3);
        assert_eq!(e[3].who.as_deref(), Some("user 0 (root) on host1:linux"));
        assert_eq!(parse(H, 2).len(), 2);
    }
}
