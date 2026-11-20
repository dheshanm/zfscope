//! Build number: every release build of changed sources gets the next number (BUILD file, committed), so test
//! binaries copied to servers can be told apart: `zfscope 0.1.0+b12`. Debug builds of sources newer than the last
//! release build show `+b12-dev`. Builds of identical sources (e.g. x86_64 and aarch64) share a number.

use std::path::Path;

/// FNV-1a: stable across Rust versions (DefaultHasher isn't).
fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= *b as u64;
        *h = h.wrapping_mul(0x100000001b3);
    }
}

fn hash_dir(dir: &Path, h: &mut u64) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            hash_dir(&p, h);
        } else if let Ok(b) = std::fs::read(&p) {
            fnv(h, p.to_string_lossy().as_bytes());
            fnv(h, &b);
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=BUILD");
    let mut h = 0xcbf29ce484222325u64;
    hash_dir(Path::new("src"), &mut h);
    fnv(&mut h, &std::fs::read("Cargo.toml").unwrap_or_default());
    let hash = format!("{h:016x}");

    let stored = std::fs::read_to_string("BUILD").unwrap_or_default();
    let mut it = stored.split_whitespace();
    let n: u64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let last = it.next().unwrap_or("").to_string();

    // Published releases (CI, tag builds) are just the crate version: ZFSCOPE_RELEASE=1 cargo build --release
    println!("cargo:rerun-if-env-changed=ZFSCOPE_RELEASE");
    if std::env::var("ZFSCOPE_RELEASE").is_ok_and(|v| v == "1") {
        println!("cargo:rustc-env=ZFSCOPE_VERSION={}", std::env::var("CARGO_PKG_VERSION").unwrap_or_default());
        return;
    }
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    let (n, suffix) = if hash == last {
        (n, "")
    } else if release {
        let _ = std::fs::write("BUILD", format!("{} {hash}\n", n + 1));
        (n + 1, "")
    } else {
        (n, "-dev")
    };
    let v = format!("{}+b{n}{suffix}", std::env::var("CARGO_PKG_VERSION").unwrap_or_default());
    println!("cargo:rustc-env=ZFSCOPE_VERSION={v}");
}
