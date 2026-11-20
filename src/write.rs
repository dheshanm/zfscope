//! Write mode: the only changes zfscope can make. Every change is one of three fixed command shapes (`zfs set`,
//! `zpool set`, `zfs inherit`), run as an argument vector (never through a shell), or a module-parameter change
//! (a write to `/sys/module/zfs/parameters/<name>` and/or an `options zfs name=value` line in modprobe.d), each
//! only after the user confirmed exactly what will happen. Nothing here runs anything; see `Source::write`,
//! `Source::write_file` and the `writer` worker.

use crate::help::Kind;

/// Where the running module's tunables live.
pub const PARAM_DIR: &str = "/sys/module/zfs/parameters";
/// Module options read by modprobe when the module loads (at boot).
pub const MODPROBE_DIR: &str = "/etc/modprobe.d";
/// Where a new boot setting goes when no file sets it yet.
pub const MODPROBE_FILE: &str = "/etc/modprobe.d/zfs.conf";

/// What a property belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Pool(String),
    Dataset(String),
}

impl Target {
    pub fn name(&self) -> &str {
        match self {
            Target::Pool(n) | Target::Dataset(n) => n,
        }
    }
    pub fn is_pool(&self) -> bool {
        matches!(self, Target::Pool(_))
    }
    /// The pool this target lives in.
    pub fn pool(&self) -> &str {
        let n = self.name();
        n.split(['/', '@']).next().unwrap_or(n)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// `zfs set key=value ds` / `zpool set key=value pool`
    Set { target: Target, key: String, value: String },
    /// `zfs inherit key ds`: back to the parent's / default value (removes a user property)
    Inherit { dataset: String, key: String },
    /// a ZFS module parameter: now (sysfs) and/or at boot (modprobe.d)
    Param(ParamChange),
}

/// What happens to a tunable's boot setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Boot {
    Keep,
    Set(String),
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamChange {
    pub name: String,
    /// write this to the running module (None: leave it)
    pub value: Option<String>,
    pub boot: Boot,
    /// the modprobe.d file the boot setting is in / goes to
    pub file: String,
    /// runtime value before the change (for the log)
    pub was: String,
    /// boot value before the change
    pub boot_was: Option<String>,
}

impl ParamChange {
    pub fn path(&self) -> String {
        format!("{PARAM_DIR}/{}", self.name)
    }

    /// What will be done, one shell-equivalent step per entry.
    pub fn steps(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(val) = &self.value {
            v.push(format!("echo {} > {}", quote(val), self.path()));
        }
        match &self.boot {
            Boot::Keep => {}
            Boot::Set(val) => v.push(format!("set 'options zfs {}={val}' in {}", self.name, self.file)),
            Boot::Remove => v.push(format!("remove {} from the options zfs line in {}", self.name, self.file)),
        }
        v
    }

    /// Short description of the outcome, for the notice.
    pub fn summary(&self) -> String {
        match (&self.value, &self.boot) {
            (Some(v), Boot::Set(b)) if v == b => format!("{} = {v} now and at boot", self.name),
            (Some(v), Boot::Set(b)) => format!("{} = {v} now, {b} at boot", self.name),
            (Some(v), Boot::Remove) => format!("{} = {v} now; boot setting removed", self.name),
            (Some(v), Boot::Keep) => format!("{} = {v} now (until reboot)", self.name),
            (None, Boot::Set(b)) => format!("{} = {b} from the next boot / module load", self.name),
            (None, Boot::Remove) => format!("{}: boot setting removed", self.name),
            (None, Boot::Keep) => format!("{}: nothing to do", self.name),
        }
    }

    fn validate(&self) -> Result<(), String> {
        let n = &self.name;
        if n.is_empty() || !n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return Err(format!("invalid parameter name {n:?}"));
        }
        if !crate::help::is_curated_param(n) {
            return Err(format!("{n} can't be changed from zfscope"));
        }
        let num = |v: &str| {
            if v.is_empty() || v.len() > 20 || !v.chars().all(|c| c.is_ascii_digit()) {
                Err(format!("{n}: {v:?} is not a whole number"))
            } else {
                Ok(())
            }
        };
        if let Some(v) = &self.value {
            num(v)?;
        }
        if let Boot::Set(v) = &self.boot {
            num(v)?;
        }
        if self.value.is_none() && self.boot == Boot::Keep {
            return Err("nothing to change".into());
        }
        if self.boot != Boot::Keep {
            valid_modprobe_file(&self.file)?;
        }
        Ok(())
    }
}

/// Only `*.conf` files directly in /etc/modprobe.d.
pub fn valid_modprobe_file(f: &str) -> Result<(), String> {
    let name = f.strip_prefix(MODPROBE_DIR).and_then(|r| r.strip_prefix('/')).unwrap_or("");
    let ok_chars = name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if name.is_empty() || name.starts_with('.') || !name.ends_with(".conf") || !ok_chars {
        return Err(format!("{f}: not a modprobe.d .conf file"));
    }
    Ok(())
}

impl Change {
    /// The pool or dataset changed (None for a module parameter).
    pub fn target(&self) -> Option<Target> {
        match self {
            Change::Set { target, .. } => Some(target.clone()),
            Change::Inherit { dataset, .. } => Some(Target::Dataset(dataset.clone())),
            Change::Param(_) => None,
        }
    }

    pub fn key(&self) -> &str {
        match self {
            Change::Set { key, .. } | Change::Inherit { key, .. } => key,
            Change::Param(p) => &p.name,
        }
    }

    /// Program and arguments (None for a module parameter, which is a file write).
    pub fn argv(&self) -> Option<(&'static str, Vec<String>)> {
        Some(match self {
            Change::Set { target: Target::Pool(p), key, value } => {
                ("zpool", vec!["set".into(), format!("{key}={value}"), p.clone()])
            }
            Change::Set { target: Target::Dataset(d), key, value } => {
                ("zfs", vec!["set".into(), format!("{key}={value}"), d.clone()])
            }
            Change::Inherit { dataset, key } => ("zfs", vec!["inherit".into(), key.clone(), dataset.clone()]),
            Change::Param(_) => return None,
        })
    }

    /// The command as a shell line, for showing and logging (quoted where a shell would need it).
    pub fn command_line(&self) -> String {
        match (self, self.argv()) {
            (Change::Param(p), _) => p.steps().join("; "),
            (_, Some((prog, args))) => {
                std::iter::once(prog.to_string()).chain(args.iter().map(|a| quote(a))).collect::<Vec<_>>().join(" ")
            }
            (_, None) => String::new(),
        }
    }

    /// Can't be undone: enabling a pool feature flag.
    pub fn one_way(&self) -> bool {
        matches!(self, Change::Set { target: Target::Pool(_), key, .. } if key.starts_with("feature@"))
    }

    /// Reject anything that isn't a plain property change of an existing-looking target.
    pub fn validate(&self) -> Result<(), String> {
        let target = match self {
            Change::Param(p) => return p.validate(),
            _ => self.target().expect("property change"),
        };
        let t = target.name();
        if t.is_empty() || t.starts_with('-') || t.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(format!("invalid target {t:?}"));
        }
        valid_key(self.key())?;
        if let Change::Set { value, .. } = self {
            if value.is_empty() {
                return Err("empty value (use inherit to clear a dataset property)".into());
            }
            if value.chars().any(char::is_control) {
                return Err("the value contains control characters".into());
            }
            if value.len() > 8192 {
                return Err("the value is too long".into());
            }
        }
        Ok(())
    }
}

/// Property names: native (`compression`), user (`org.example:backup`), or `feature@name`.
pub fn valid_key(k: &str) -> Result<(), String> {
    let ok_chars = k.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '@' | '-' | '+'));
    if k.is_empty() || k.len() > 256 || !k.starts_with(|c: char| c.is_ascii_alphabetic()) || !ok_chars {
        return Err(format!("invalid property name {k:?}"));
    }
    Ok(())
}

/// A new user property needs `namespace:name` (lowercase, so it can't clash with native properties).
pub fn valid_user_key(k: &str) -> Result<(), String> {
    valid_key(k)?;
    if !k.contains(':') || k.starts_with('@') || k.contains('@') {
        return Err("user properties are named namespace:name (e.g. org.example:backup)".into());
    }
    if k.chars().any(|c| c.is_ascii_uppercase()) {
        return Err("user property names are lowercase".into());
    }
    Ok(())
}

/// Parse a tunable value as typed: a whole number, for bytes optionally with a binary suffix (`8G`, `512MiB`,
/// `1.5T`).
pub fn parse_value(kind: Kind, input: &str) -> Result<u64, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("type a value".into());
    }
    if let Ok(n) = s.parse::<u64>() {
        return Ok(n);
    }
    if kind != Kind::Bytes {
        return Err(format!("{s:?} is not a whole number"));
    }
    let lower = s.to_ascii_lowercase();
    let t = lower.trim_end_matches('b').trim_end_matches('i');
    let (num, shift) = match t.chars().last() {
        Some('k') => (&t[..t.len() - 1], 10),
        Some('m') => (&t[..t.len() - 1], 20),
        Some('g') => (&t[..t.len() - 1], 30),
        Some('t') => (&t[..t.len() - 1], 40),
        Some('p') => (&t[..t.len() - 1], 50),
        _ if t.len() < lower.len() => (t, 0),
        _ => return Err(format!("{s:?} is not a size (e.g. 8G, 512M, 1048576)")),
    };
    let num = num.trim();
    let bad = || format!("{s:?} is not a size (e.g. 8G, 512M, 1048576)");
    if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err(bad());
    }
    let mult = 1u64 << shift;
    if let Ok(n) = num.parse::<u64>() {
        return n.checked_mul(mult).ok_or_else(|| format!("{s:?} is too large"));
    }
    let f: f64 = num.parse().map_err(|_| bad())?;
    let v = f * mult as f64;
    if v.fract() != 0.0 {
        return Err(format!("{s:?} is not a whole number of bytes"));
    }
    if v >= u64::MAX as f64 {
        return Err(format!("{s:?} is too large"));
    }
    Ok(v as u64)
}

/// The parameter tokens of an `options zfs ...` line (None for any other line).
fn zfs_options(line: &str) -> Option<Vec<&str>> {
    let mut it = line.split_whitespace();
    (it.next() == Some("options") && it.next() == Some("zfs")).then(|| it.collect())
}

fn token_value<'a>(tok: &'a str, name: &str) -> Option<&'a str> {
    tok.strip_prefix(name)?.strip_prefix('=').map(|v| v.trim_matches('"'))
}

/// The value modprobe would pass for `name` (the last setting wins), from one modprobe.d file.
pub fn modprobe_get(text: &str, name: &str) -> Option<String> {
    text.lines()
        .filter_map(zfs_options)
        .flat_map(|toks| toks.into_iter().filter_map(|t| token_value(t, name)))
        .last()
        .map(str::to_string)
}

/// The `options zfs` line that sets `name`, as written.
pub fn modprobe_line(text: &str, name: &str) -> Option<String> {
    text.lines()
        .rfind(|l| zfs_options(l).is_some_and(|t| t.iter().any(|t| token_value(t, name).is_some())))
        .map(|l| l.trim().to_string())
}

/// `text` with `name` set to `value` (None: removed). The first setting is updated in place and any others are
/// dropped; a line left with no parameters goes; a new setting is appended as its own line. Everything else
/// (comments, other modules, other parameters) is kept as is.
pub fn modprobe_set(text: &str, name: &str, value: Option<&str>) -> String {
    let mut out = Vec::new();
    let mut done = false;
    for line in text.lines() {
        match zfs_options(line) {
            Some(toks) if toks.iter().any(|t| token_value(t, name).is_some()) => {
                let mut kept: Vec<String> = Vec::new();
                for t in toks {
                    if token_value(t, name).is_some() {
                        if let (Some(v), false) = (value, done) {
                            kept.push(format!("{name}={v}"));
                        }
                        done = true;
                    } else {
                        kept.push(t.to_string());
                    }
                }
                if !kept.is_empty() {
                    out.push(format!("options zfs {}", kept.join(" ")));
                }
            }
            _ => out.push(line.to_string()),
        }
    }
    if let (Some(v), false) = (value, done) {
        out.push(format!("options zfs {name}={v}"));
    }
    let mut s = out.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

/// `zfs.<name>=value` on the kernel command line (it overrides modprobe.d).
pub fn cmdline_param(cmdline: &str, name: &str) -> Option<String> {
    let key = format!("zfs.{name}=");
    cmdline.split_whitespace().filter_map(|t| t.strip_prefix(&key)).next_back().map(str::to_string)
}

fn quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@,+%".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Effective uid from the text of `/proc/self/status` (`Uid: real effective saved fs`).
pub fn effective_uid(status: &str) -> Option<u32> {
    status.lines().find_map(|l| l.strip_prefix("Uid:")).and_then(|r| r.split_whitespace().nth(1)?.parse().ok())
}

/// Are we root? (Linux; elsewhere there's no /proc and the answer is no.)
pub fn running_as_root() -> bool {
    std::fs::read_to_string("/proc/self/status").ok().as_deref().and_then(effective_uid) == Some(0)
}

/// Side effects worth a line in the confirm dialog.
pub fn side_effect(key: &str) -> Option<&'static str> {
    Some(match key {
        "mountpoint" => "remounts the dataset (and its children) at the new place right away",
        "canmount" => "may unmount the dataset right away",
        "sharenfs" | "sharesmb" => "re-exports the share right away; clients may see a brief interruption",
        "readonly" => "takes effect immediately for every writer",
        "volsize" => "resizes the volume; shrinking it destroys data beyond the new end",
        "quota" | "refquota" => "writers get 'disk quota exceeded' if usage is already above the new limit",
        "recordsize" | "compression" | "checksum" | "dedup" | "copies" | "special_small_blocks" => {
            "applies to newly written blocks only; existing data keeps its old layout"
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(t: Target, k: &str, v: &str) -> Change {
        Change::Set { target: t, key: k.into(), value: v.into() }
    }

    #[test]
    fn argv_and_command_line() {
        let c = set(Target::Dataset("pool0/ds01".into()), "compression", "zstd");
        assert_eq!(c.argv().unwrap(), ("zfs", vec!["set".into(), "compression=zstd".into(), "pool0/ds01".into()]));
        assert_eq!(c.command_line(), "zfs set compression=zstd pool0/ds01");
        let c = set(Target::Pool("pool0".into()), "feature@head_errlog", "enabled");
        assert_eq!(c.command_line(), "zpool set feature@head_errlog=enabled pool0");
        assert!(c.one_way());
        let c = Change::Inherit { dataset: "pool0/ds01".into(), key: "org.x:note".into() };
        assert_eq!(c.argv().unwrap(), ("zfs", vec!["inherit".into(), "org.x:note".into(), "pool0/ds01".into()]));
        assert!(!c.one_way());
        let c = set(Target::Dataset("pool0/ds01".into()), "org.x:note", "it's here");
        assert_eq!(c.command_line(), r"zfs set 'org.x:note=it'\''s here' pool0/ds01");
        assert!(!set(Target::Pool("pool0".into()), "autotrim", "on").one_way());
        assert!(!set(Target::Dataset("pool0".into()), "feature@x", "enabled").one_way());
    }

    #[test]
    fn validation() {
        let ds = || Target::Dataset("pool0/ds01".into());
        assert!(set(ds(), "compression", "zstd").validate().is_ok());
        assert!(set(ds(), "compression", "").validate().is_err());
        assert!(set(ds(), "compression", "a\nb").validate().is_err());
        assert!(set(ds(), "-o", "x").validate().is_err());
        assert!(set(ds(), "", "x").validate().is_err());
        assert!(set(ds(), "comp ression", "x").validate().is_err());
        assert!(set(Target::Dataset("-r".into()), "compression", "lz4").validate().is_err());
        assert!(set(Target::Dataset("pool0/a b".into()), "compression", "lz4").validate().is_err());
        assert!(Change::Inherit { dataset: "pool0".into(), key: "--all".into() }.validate().is_err());
        assert!(valid_user_key("org.example:backup").is_ok());
        assert!(valid_user_key("backup").is_err());
        assert!(valid_user_key("Org:x").is_err());
        assert!(valid_user_key("feature@x").is_err());
    }

    #[test]
    fn uid_from_proc_status() {
        let s = "Name:\tzfscope\nUid:\t1000\t0\t0\t0\nGid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(effective_uid(s), Some(0));
        assert_eq!(effective_uid("Uid:\t1000\t1000\t1000\t1000\n"), Some(1000));
        assert_eq!(effective_uid("Name: x\n"), None);
    }

    fn param(value: Option<&str>, boot: Boot) -> Change {
        Change::Param(ParamChange {
            name: "zfs_arc_max".into(),
            value: value.map(Into::into),
            boot,
            file: MODPROBE_FILE.into(),
            was: "0".into(),
            boot_was: None,
        })
    }

    #[test]
    fn param_change() {
        let c = param(Some("8589934592"), Boot::Set("8589934592".into()));
        assert!(c.validate().is_ok());
        assert!(c.argv().is_none() && c.target().is_none() && !c.one_way());
        assert_eq!(
            c.command_line(),
            "echo 8589934592 > /sys/module/zfs/parameters/zfs_arc_max; \
             set 'options zfs zfs_arc_max=8589934592' in /etc/modprobe.d/zfs.conf"
        );
        let Change::Param(p) = &c else { unreachable!() };
        assert_eq!(p.summary(), "zfs_arc_max = 8589934592 now and at boot");
        assert!(param(None, Boot::Remove).validate().is_ok());
        assert!(param(None, Boot::Keep).validate().is_err());
        assert!(param(Some("8G"), Boot::Keep).validate().is_err());
        assert!(param(Some("1\n2"), Boot::Keep).validate().is_err());
        assert!(param(Some(""), Boot::Keep).validate().is_err());
        let mut bad = ParamChange {
            name: "zfs_vdev_raidz_impl".into(),
            value: Some("1".into()),
            boot: Boot::Keep,
            file: MODPROBE_FILE.into(),
            was: String::new(),
            boot_was: None,
        };
        assert!(Change::Param(bad.clone()).validate().is_err(), "not curated");
        bad.name = "zfs_txg_timeout".into();
        assert!(Change::Param(bad.clone()).validate().is_ok());
        bad.boot = Boot::Remove;
        for f in ["/etc/modprobe.d/../passwd", "/etc/zfs.conf", "/etc/modprobe.d/zfs", "/etc/modprobe.d/.x.conf"] {
            bad.file = f.into();
            assert!(Change::Param(bad.clone()).validate().is_err(), "{f}");
        }
    }

    #[test]
    fn values() {
        assert_eq!(parse_value(Kind::Bytes, "8G"), Ok(8 << 30));
        assert_eq!(parse_value(Kind::Bytes, " 512MiB "), Ok(512 << 20));
        assert_eq!(parse_value(Kind::Bytes, "1.5k"), Ok(1536));
        assert_eq!(parse_value(Kind::Bytes, "4096"), Ok(4096));
        assert_eq!(parse_value(Kind::Bytes, "4096B"), Ok(4096));
        assert!(parse_value(Kind::Bytes, "1.3k").is_err());
        assert!(parse_value(Kind::Bytes, "99999999P").is_err());
        assert!(parse_value(Kind::Bytes, "lots").is_err());
        assert!(parse_value(Kind::Bytes, "G").is_err());
        assert!(parse_value(Kind::Bytes, "-1").is_err());
        assert!(parse_value(Kind::Secs, "8G").is_err());
        assert_eq!(parse_value(Kind::Percent, "60"), Ok(60));
    }

    #[test]
    fn modprobe_edits() {
        let f = "# tuning\noptions zfs zfs_arc_max=1073741824 zfs_txg_timeout=10\noptions spl spl_hostid=1\n";
        assert_eq!(modprobe_get(f, "zfs_arc_max").as_deref(), Some("1073741824"));
        assert_eq!(modprobe_get(f, "zfs_arc_min"), None);
        assert_eq!(modprobe_get(f, "spl_hostid"), None, "other modules don't count");
        assert_eq!(
            modprobe_line(f, "zfs_txg_timeout").as_deref(),
            Some("options zfs zfs_arc_max=1073741824 zfs_txg_timeout=10")
        );
        let set = modprobe_set(f, "zfs_txg_timeout", Some("5"));
        assert_eq!(set, "# tuning\noptions zfs zfs_arc_max=1073741824 zfs_txg_timeout=5\noptions spl spl_hostid=1\n");
        assert_eq!(modprobe_set(&set, "zfs_txg_timeout", Some("5")), set, "idempotent");
        let rm = modprobe_set(f, "zfs_arc_max", None);
        assert_eq!(rm, "# tuning\noptions zfs zfs_txg_timeout=10\noptions spl spl_hostid=1\n");
        let rm2 = modprobe_set(&rm, "zfs_txg_timeout", None);
        assert_eq!(rm2, "# tuning\noptions spl spl_hostid=1\n", "an empty line goes");
        assert_eq!(modprobe_set("", "zil_slog_bulk", Some("1")), "options zfs zil_slog_bulk=1\n");
        assert_eq!(modprobe_set("# x", "zil_slog_bulk", Some("1")), "# x\noptions zfs zil_slog_bulk=1\n");
        // duplicates collapse into the first; the last one was the effective value
        let dup = "options zfs zfs_arc_max=1\noptions zfs zfs_arc_max=2\n";
        assert_eq!(modprobe_get(dup, "zfs_arc_max").as_deref(), Some("2"));
        assert_eq!(modprobe_set(dup, "zfs_arc_max", Some("3")), "options zfs zfs_arc_max=3\n");
        assert_eq!(modprobe_set("#options zfs zfs_arc_max=1\n", "zfs_arc_max", None), "#options zfs zfs_arc_max=1\n");
        assert_eq!(modprobe_get("options zfs zfs_arc_max_x=1", "zfs_arc_max"), None);
    }

    #[test]
    fn cmdline() {
        let c = "BOOT_IMAGE=/vmlinuz root=ZFS=rpool/root zfs.zfs_arc_max=4294967296 quiet";
        assert_eq!(cmdline_param(c, "zfs_arc_max").as_deref(), Some("4294967296"));
        assert_eq!(cmdline_param(c, "zfs_arc_min"), None);
    }

    #[test]
    fn target_pool() {
        assert_eq!(Target::Dataset("pool0/a/b".into()).pool(), "pool0");
        assert_eq!(Target::Pool("pool0".into()).pool(), "pool0");
    }
}
