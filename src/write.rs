//! Write mode: the only changes zfscope can make. Every change is one of three fixed command shapes (`zfs set`,
//! `zpool set`, `zfs inherit`), run as an argument vector (never through a shell) after the user confirmed the exact
//! command line. Nothing here runs anything; see `Source::write` and the `writer` worker.

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
}

impl Change {
    pub fn target(&self) -> Target {
        match self {
            Change::Set { target, .. } => target.clone(),
            Change::Inherit { dataset, .. } => Target::Dataset(dataset.clone()),
        }
    }

    pub fn key(&self) -> &str {
        match self {
            Change::Set { key, .. } | Change::Inherit { key, .. } => key,
        }
    }

    /// Program and arguments.
    pub fn argv(&self) -> (&'static str, Vec<String>) {
        match self {
            Change::Set { target: Target::Pool(p), key, value } => {
                ("zpool", vec!["set".into(), format!("{key}={value}"), p.clone()])
            }
            Change::Set { target: Target::Dataset(d), key, value } => {
                ("zfs", vec!["set".into(), format!("{key}={value}"), d.clone()])
            }
            Change::Inherit { dataset, key } => ("zfs", vec!["inherit".into(), key.clone(), dataset.clone()]),
        }
    }

    /// The command as a shell line, for showing and logging (quoted where a shell would need it).
    pub fn command_line(&self) -> String {
        let (prog, args) = self.argv();
        std::iter::once(prog.to_string()).chain(args.iter().map(|a| quote(a))).collect::<Vec<_>>().join(" ")
    }

    /// Can't be undone: enabling a pool feature flag.
    pub fn one_way(&self) -> bool {
        matches!(self, Change::Set { target: Target::Pool(_), key, .. } if key.starts_with("feature@"))
    }

    /// Reject anything that isn't a plain property change of an existing-looking target.
    pub fn validate(&self) -> Result<(), String> {
        let target = self.target();
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
        assert_eq!(c.argv(), ("zfs", vec!["set".into(), "compression=zstd".into(), "pool0/ds01".into()]));
        assert_eq!(c.command_line(), "zfs set compression=zstd pool0/ds01");
        let c = set(Target::Pool("pool0".into()), "feature@head_errlog", "enabled");
        assert_eq!(c.command_line(), "zpool set feature@head_errlog=enabled pool0");
        assert!(c.one_way());
        let c = Change::Inherit { dataset: "pool0/ds01".into(), key: "org.x:note".into() };
        assert_eq!(c.argv(), ("zfs", vec!["inherit".into(), "org.x:note".into(), "pool0/ds01".into()]));
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

    #[test]
    fn target_pool() {
        assert_eq!(Target::Dataset("pool0/a/b".into()).pool(), "pool0");
        assert_eq!(Target::Pool("pool0".into()).pool(), "pool0");
    }
}
