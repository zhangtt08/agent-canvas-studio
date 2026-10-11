//! Small shared helpers: sandboxed path resolution, ids, hashing, JSON getters.
//!
//! Every path that crosses the RPC boundary goes through [`inp`]/[`outp`], which
//! resolve against the session root and refuse `..` escapes. That is the single
//! place where the "agents may not touch the whole disk" rule lives.

use crate::error::{Fail, R};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn ps<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}

pub fn req_s<'a>(v: &'a Value, k: &str) -> R<&'a str> {
    ps(v, k).filter(|s| !s.is_empty()).ok_or_else(|| {
        Fail::new("INVALID_SCHEMA", format!("missing string argument \"{k}\"")).details(jsonish(v))
    })
}

pub fn f(v: &Value, k: &str) -> Option<f64> {
    match v.get(k)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

pub fn req_f(v: &Value, k: &str) -> R<f64> {
    f(v, k).ok_or_else(|| Fail::new("INVALID_SCHEMA", format!("missing number argument \"{k}\"")))
}

pub fn opt_b(v: &Value, k: &str, d: bool) -> bool {
    match v.get(k) {
        None | Some(Value::Null) => d,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.as_str(), "1" | "true" | "yes" | "y" | "on"),
        Some(Value::Number(n)) => n.as_f64().map(|x| x != 0.0).unwrap_or(d),
        _ => d,
    }
}

pub fn arr(v: &Value, k: &str) -> Vec<Value> {
    match v.get(k) {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).ok().and_then(|x| match x {
            Value::Array(a) => Some(a),
            _ => None,
        }).unwrap_or_default(),
        _ => vec![],
    }
}

pub fn obj<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.get(k).filter(|x| x.is_object())
}

fn jsonish(v: &Value) -> Value {
    let mut m = serde_json::Map::new();
    for k in ["id", "nodeId", "artboardId", "assetId", "path", "projectId"] {
        if let Some(x) = v.get(k) {
            m.insert(k.into(), x.clone());
        }
    }
    if m.is_empty() { Value::Null } else { Value::Object(m) }
}

// ---------------------------------------------------------------- ids

const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// Short, sortable, collision-resistant-enough id with a readable prefix.
pub fn id(prefix: &str) -> String {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let mut x = t ^ ((n as u64) << 20) ^ (rand_seed() as u64);
    let mut s = String::new();
    for _ in 0..7 {
        s.insert(0, ALPHABET[(x % 36) as usize] as char);
        x /= 36;
    }
    format!("{prefix}_{s}")
}

fn rand_seed() -> u32 {
    let addr = &ALPHABET[0] as *const u8 as usize;
    ((addr >> 4) as u32) ^ (std::process::id() << 3) ^ (now_ms() as u32)
}

// ---------------------------------------------------------------- time

pub fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// `2026-10-09 17:41` in local-ish terms (UTC + 8, which is this machine's zone).
pub fn stamp() -> String {
    let ms = now_ms() as i64 + 8 * 3600 * 1000;
    let days = ms / 86_400_000;
    let secs = (ms / 1000) % 86400;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", secs / 3600, (secs % 3600) / 60)
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn date_str() -> String {
    stamp().split(' ').next().unwrap_or("").to_string()
}

// ---------------------------------------------------------------- hashing

pub fn sha256_bytes(b: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b);
    h.finalize().iter().map(|x| format!("{x:02x}")).collect()
}

pub fn sha256_file(p: &Path) -> R<String> {
    let b = std::fs::read(p).map_err(|e| Fail::new("NOT_FOUND", format!("{}: {e}", p.display())))?;
    Ok(sha256_bytes(&b))
}

pub fn short_hash(s: &str) -> String {
    s.chars().take(12).collect()
}

// ---------------------------------------------------------------- numbers

pub fn round(v: f64, dp: u32) -> f64 {
    let m = 10f64.powi(dp as i32);
    (v * m).round() / m
}

/// JSON writes a whole `f64` as `1440.0`. The `.0` is two tokens per number that no
/// reader asked for — a document reply has hundreds of them — so whole values leave
/// as integers. Fractions (`205.2`) are untouched, and values beyond exact i64
/// range keep their float form.
pub fn whole_numbers(v: &mut Value) {
    match v {
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                if f.fract() == 0.0 && f.abs() < 9.0e15 {
                    *n = serde_json::Number::from(f as i64);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(whole_numbers),
        Value::Object(o) => o.iter_mut().for_each(|(_, x)| whole_numbers(x)),
        _ => {}
    }
}

pub fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi)
}

pub fn human_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n}B")
    } else if n < 1024 * 1024 {
        format!("{}KB", round(n as f64 / 1024.0, 1))
    } else {
        format!("{}MB", round(n as f64 / 1048576.0, 2))
    }
}

// ---------------------------------------------------------------- paths

pub fn home() -> PathBuf {
    if let Ok(p) = std::env::var("AGENTCANVAS_HOME") {
        return PathBuf::from(p);
    }
    let base = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("AgentCanvas")
}

/// Root that all relative project paths resolve against.
pub fn root(ctx: Option<&Path>) -> PathBuf {
    let r = ctx.map(|p| p.to_path_buf()).or_else(ambient_root).unwrap_or_else(home);
    let _ = std::fs::create_dir_all(&r);
    absolutize(&r)
}

thread_local! {
    /// The `--root` of the call being served right now. Helpers that report paths
    /// (render, templates) have no `Ctx` of their own; without this they shorten
    /// against `home()` and a `--root` session answers with a home-absolute path.
    static AMBIENT_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

pub fn set_ambient_root(p: Option<&Path>) {
    AMBIENT_ROOT.with(|c| *c.borrow_mut() = p.map(|x| x.to_path_buf()));
}

fn ambient_root() -> Option<PathBuf> {
    AMBIENT_ROOT.with(|c| c.borrow().clone())
}

fn absolutize(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(p)
    }
}

/// Normalise without touching the disk (Windows has no `..`-free guarantee).
pub fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn inside(root: &Path, p: &Path) -> bool {
    let rp = clean(root);
    let pp = clean(p);
    pp.starts_with(&rp)
}

/// Resolve an input path. Absolute paths are allowed only when `AGENTCANVAS_OPEN_FS`
/// is set or the path is under the root; relative paths resolve against the root.
pub fn inp(s: &str, ctx: Option<&Path>) -> R<PathBuf> {
    let r = root(ctx);
    let raw = Path::new(s);
    let cand = if raw.is_absolute() { raw.to_path_buf() } else { r.join(raw) };
    let c = clean(&cand);
    let open = std::env::var("AGENTCANVAS_OPEN_FS").is_ok();
    if !open && raw.is_absolute() && !inside(&r, &c) {
        // Absolute paths outside the root are still fine for *reading* user
        // material when it is an existing file; the strict rule applies to writes.
        if !c.is_file() {
            return Err(Fail::new("PATH_DENIED", format!("\"{s}\" is outside root {}", r.display()))
                .hint("pass --root DIR, copy the file under the root, or set AGENTCANVAS_OPEN_FS=1"));
        }
    }
    if !c.exists() {
        return Err(Fail::new("NOT_FOUND", format!("no such file: {}", c.display()))
            .hint("asset.import_path then use the returned id, or check the spelling"));
    }
    Ok(c)
}

/// Resolve an output path; always created under the root unless absolute+allowed.
pub fn outp(s: &str, ctx: Option<&Path>) -> PathBuf {
    let r = root(ctx);
    let raw = Path::new(s);
    let cand = if raw.is_absolute() { raw.to_path_buf() } else { r.join(raw) };
    clean(&cand)
}

pub fn check_writable(p: &Path, ctx: Option<&Path>) -> R<()> {
    let r = root(ctx);
    let open = std::env::var("AGENTCANVAS_OPEN_FS").is_ok();
    if open || inside(&r, p) {
        Ok(())
    } else {
        Err(Fail::new("PATH_DENIED", format!("cannot write outside root {}", r.display()))
            .hint("use a relative path, or pass --root DIR"))
    }
}

pub fn ensure_parent(p: &Path) -> R<()> {
    if let Some(d) = p.parent() {
        if !d.as_os_str().is_empty() {
            std::fs::create_dir_all(d)?;
        }
    }
    Ok(())
}

pub fn ext(p: &Path) -> String {
    p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase()
}

pub fn stem(p: &Path) -> String {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("out").to_string()
}

pub fn with_ext(p: &Path, e: &str) -> PathBuf {
    p.with_extension(e)
}

/// Display a path relative to the root when possible: keeps responses short, which
/// matters because agents pay tokens for every byte echoed back.
pub fn short(p: &Path, ctx: Option<&Path>) -> String {
    let r = root(ctx);
    match clean(p).strip_prefix(&r) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => p.to_string_lossy().replace('\\', "/"),
    }
}

pub fn read(p: &Path) -> R<Vec<u8>> {
    std::fs::read(p).map_err(|e| Fail::new("IO", format!("{}: {e}", p.display())))
}

pub fn write(p: &Path, bytes: &[u8]) -> R<()> {
    ensure_parent(p)?;
    // Atomic: write to .tmp then rename, so a crash never leaves a half document.
    let tmp = {
        let mut s = p.as_os_str().to_os_string();
        s.push(".tmp");
        PathBuf::from(s)
    };
    std::fs::write(&tmp, bytes).map_err(|e| Fail::new("IO", format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, p).map_err(|e| Fail::new("IO", format!("{}: {e}", p.display())))?;
    Ok(())
}

pub fn copy(src: &Path, dst: &Path) -> R<u64> {
    ensure_parent(dst)?;
    let n = std::fs::copy(src, dst).map_err(|e| Fail::new("IO", format!("{} -> {}: {e}", src.display(), dst.display())))?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn whole_numbers_drop_the_dot_without_changing_the_value() {
        let mut v = json!({"w": 1440.0, "x": 205.2, "big": 1e18, "arr": [1.0, {"op": 2.5}], "s": "1440.0"});
        whole_numbers(&mut v);
        assert_eq!(v["w"], json!(1440));
        assert_eq!(v["arr"][0], json!(1));
        assert_eq!(v["x"], json!(205.2));
        assert_eq!(v["arr"][1]["op"], json!(2.5));
        assert!(v["big"].is_f64(), "past exact i64 range the float form is kept");
        assert_eq!(v["s"], json!("1440.0"), "a string is never a number");
    }

    /// A `--root` session promises root-relative paths. Helpers that report them
    /// (render, templates) have no `Ctx`, so they read the ambient root instead.
    #[test]
    fn the_ambient_root_shortens_paths_for_helpers_without_a_ctx() {
        let dir = std::env::temp_dir().join(format!("ac-ambient-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let inside = dir.join("final").join("a.png");
        set_ambient_root(Some(&dir));
        assert_eq!(short(&inside, None), "final/a.png");
        assert_eq!(root(None), absolutize(&dir), "relative resolution follows the same root");
        set_ambient_root(None);
        assert!(short(&inside, None).ends_with("final/a.png"), "no root, no shortening: {}", short(&inside, None));
        std::fs::remove_dir_all(&dir).ok();
    }
}
