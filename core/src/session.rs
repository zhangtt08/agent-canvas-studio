//! Session layer: the workspace directory, the settings file, the call log and —
//! most importantly — the **process-global registry of open projects**.
//!
//! This is the file that makes AgentCanvas *one* application. A desktop window and
//! a remote MCP client do not each keep their own copy of a document: every
//! mutating RPC method goes through [`with_project`], which lends out the single
//! `Document` stored in the registry and writes it back when the closure returns.
//! There is therefore exactly one in-memory document per project id per process.
//!
//! ```text
//! REG : Mutex<HashMap<project_id, Arc<Shared>>>
//! Shared { core: RwLock<Project>,   // the document + dir/path, loaned by with_project
//!          meta: RwLock<Meta> }     // undo/redo stacks, dirty + loan + autosave flags
//! ```
//!
//! Two locks on purpose: a loan borrows `core`, while `history` and `project` still
//! need to touch `meta` *from inside* that loan (same thread, nested call).
//! One lock would fail with `WouldBlock`; two do not. `Arc`/`RwLock` rather than
//! `Rc`/`RefCell` because the HTTP layer serves requests on several threads, so
//! `REG` has to be `Sync`; every acquisition is a `try_*`, which keeps the failure
//! mode a retryable `PROJECT_BUSY` instead of a block or a panic.
//!
//! Concurrency, stated honestly:
//! * The `Mutex` is held only for map lookups and inserts, never across a closure,
//!   so two *different* projects edit fully in parallel (a `Mutex<Project>` here
//!   would deadlock instead, because Rust mutexes are not reentrant).
//! * One project is edited by at most one caller at a time. The `loaned` flag makes
//!   a second caller — same thread or another — get a retryable `PROJECT_BUSY`
//!   rather than deadlocking or watching a half-edited document.
//! * The document is moved out of `core` for the duration of the loan, so read-only
//!   helpers (`peek`, `read`, `registry_summary`) refuse to report a placeholder
//!   document while a loan is live.

use crate::commands::Cmd;
use crate::doc::Document;
use crate::error::{Fail, R};
use crate::project::Project;
use crate::util;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// Ring-buffer size of `<workspace>/log/calls.jsonl`.
pub const LOG_CAP: usize = 5000;
/// Let the log run this far past the cap before paying for a full rewrite;
/// trimming one line at a time would rewrite ~1 MB on every single call.
const LOG_TRIM_AT: usize = LOG_CAP + 1000;
/// Undo/redo entries kept per project.
pub const HISTORY_CAP: usize = 200;
/// Bounded per-job log lines (declared here because `Meta`/`jobs` share the cap).
pub const JOB_LOG_CAP: usize = 200;
/// The persisted undo trail of a project: `<project dir>/history.jsonl`.
///
/// One JSON object per **stack event** (`push` / `undo` / `redo` / `put_undo` /
/// `put_redo`), appended, never rewritten in place. A CLI or MCP call is its own
/// process, so without this file `history.undo` would answer "nothing to undo" to a
/// trail the desktop window can see — the exact asymmetry this product exists to
/// avoid. Replaying the events is what rebuilds *both* stacks, so an undo performed
/// by another process is not re-offered (and its redo still is).
pub const HISTORY_FILE: &str = "history.jsonl";
/// Let the trail run this far past the cap before paying for a full rewrite;
/// trimming one line at a time would rewrite the whole file on every edit.
const HISTORY_TRIM_AT: usize = HISTORY_CAP * 4;
/// Lines kept when the trail is rewritten (enough for [`HISTORY_CAP`] entries plus
/// the undo/redo markers that refer to them).
const HISTORY_KEEP: usize = HISTORY_CAP * 2;

// --------------------------------------------------------------- workspace

/// `%USERPROFILE%\AgentCanvas`, honouring `AGENTCANVAS_HOME` (see [`util::home`]).
/// The directory is created: a session that cannot write is useless.
pub fn workspace() -> PathBuf {
    let p = util::home();
    let _ = std::fs::create_dir_all(&p);
    util::clean(&p)
}

/// Hook registered by [`crate::project`] on first use: save a project if it is
/// stale. Keeping it a function pointer preserves the layering (`session` is
/// layer 1, `project` is layer 3).
type AutosaveFn = fn(&str) -> R<()>;
static AUTOSAVE: OnceLock<AutosaveFn> = OnceLock::new();

/// Install the autosave hook (idempotent; the first registration wins).
pub fn set_autosave(f: AutosaveFn) {
    let _ = AUTOSAVE.set(f);
}

thread_local! {
    /// Set while *this* thread is inside the autosave hook.
    ///
    /// The hook is `project::save_if_stale`, which opens its own loan through
    /// [`with_project`] — and `with_project` runs the autosave hook before it lends
    /// the document. Without this flag the two call each other until the stack gives
    /// out, because nothing clears `dirty` (or the forced "save now" marker) until
    /// the save actually runs. A thread-local rather than a `Meta` flag because the
    /// re-entry is always on the same thread, and another thread's autosave must not
    /// be suppressed by it.
    static IN_AUTOSAVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn run_autosave(id: &str) {
    if IN_AUTOSAVE.with(|c| c.get()) {
        return;
    }
    if let Some(f) = AUTOSAVE.get() {
        IN_AUTOSAVE.with(|c| c.set(true));
        // Best effort: a failed autosave must not break the edit that triggered it,
        // but it is recorded in the call log rather than swallowed.
        if let Err(e) = f(id) {
            if e.code != "PROJECT_BUSY" {
                log_call(&json!({"m": "session.autosave", "ok": false, "ms": 0, "p": id, "e": e.to_string()}));
            }
        }
        IN_AUTOSAVE.with(|c| c.set(false));
    }
}

/// Force the autosave hook to run for `id` *now*, bypassing the interval.
///
/// Used when an undo or redo closes its loan: the document moved, and in a one-shot
/// process there is no later call for the interval-based flush to land in. The
/// interval itself stays the only gate on every other path, and a workspace that
/// switched autosave off (`autosaveSecs: 0`) keeps refusing session-driven writes —
/// such a project is persisted by its next explicit `project.save`.
fn flush_now(id: &str) {
    let sh = match shared(id) {
        Some(sh) => sh,
        None => return,
    };
    if let Ok(mut m) = sh.meta.try_write() {
        // `last_save = 0` is what makes `save_due` report "overdue" for the hook.
        m.last_save = 0;
    }
    run_autosave(id);
}

static DEFAULT_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// The root every session file lives under: the registered override, else
/// [`workspace`].
pub fn root() -> PathBuf {
    match DEFAULT_ROOT.get() {
        Some(p) => p.clone(),
        None => workspace(),
    }
}

/// `rpc::Ctx.root` (`--root DIR`, a CLI job, an HTTP server with a custom root)
/// calls this once so [`log_call`], [`read_log`] and the default config path know
/// where the workspace is. First registration wins; the return value is the
/// *effective* root, so a second caller can detect that it was ignored.
pub fn set_default_root(p: &Path) -> PathBuf {
    let abs = util::clean(&util::root(Some(p)));
    let _ = std::fs::create_dir_all(&abs);
    let _ = DEFAULT_ROOT.set(abs);
    root()
}

/// `<workspace>/projects` — one directory per project (see [`crate::project`]).
pub fn projects_dir() -> PathBuf {
    root().join("projects")
}
/// `<workspace>/log/calls.jsonl`.
pub fn log_path() -> PathBuf {
    root().join("log").join("calls.jsonl")
}
/// `<workspace>/settings.json`.
pub fn settings_path() -> PathBuf {
    root().join("settings.json")
}
/// `<workspace>/recents.json`, owned by [`crate::project`].
pub fn recents_path() -> PathBuf {
    root().join("recents.json")
}
/// `<workspace>/bundles`, the default home of `export_bundle`.
pub fn bundles_dir() -> PathBuf {
    root().join("bundles")
}

/// Create the workspace skeleton. Idempotent; every writer calls it first.
pub fn ensure_dirs() -> R<()> {
    for d in [root(), projects_dir(), root().join("log"), root().join("exports")] {
        std::fs::create_dir_all(&d).map_err(|e| Fail::new("IO", format!("{}: {e}", d.display())))?;
    }
    Ok(())
}

/// Refuse a root-relative path that climbs out of `root` with `..`.
pub fn safe_rel(root: &Path, rel: &str) -> R<PathBuf> {
    let r = util::clean(&util::root(Some(root)));
    let raw = Path::new(rel);
    if raw.is_absolute() {
        let c = util::clean(raw);
        return if starts_with_posix(&c, &r) {
            Ok(c)
        } else {
            Err(Fail::new("PATH_DENIED", format!("\"{rel}\" is outside {}", r.display()))
                .hint("pass --root DIR or a path under the workspace"))
        };
    }
    let joined = util::clean(&r.join(rel));
    if !starts_with_posix(&joined, &r) {
        return Err(Fail::new("PATH_DENIED", format!("\"{rel}\" escapes {}", r.display()))
            .hint("paths are relative to the workspace root; `..` is not allowed"));
    }
    Ok(joined)
}

/// [`Path::starts_with`] is byte-exact, which would reject `AgentCanvas` when the
/// root is `AgentCanvas2`. Compare component-wise.
pub fn starts_with_posix(p: &Path, base: &Path) -> bool {
    let mut a = p.components();
    let mut b = base.components();
    loop {
        match (a.next(), b.next()) {
            (Some(x), Some(y)) => {
                if x.as_os_str() != y.as_os_str() {
                    return false;
                }
            }
            (None, None) => return true,
            (None, Some(_)) => return false,
            (Some(_), None) => return true,
        }
    }
}

/// Response-friendly path form: relative to the root when possible.
pub fn show(p: &Path) -> String {
    util::short(p, Some(&root()))
}

fn ensure_sub(root: &Path, rel: &Path) -> R<()> {
    if let Some(d) = rel.parent() {
        let dd = root.join(d);
        if !dd.as_os_str().is_empty() {
            std::fs::create_dir_all(&dd).map_err(|e| Fail::new("IO", format!("{}: {e}", dd.display())))?;
        }
    }
    Ok(())
}

// --------------------------------------------------------------- config

/// The documented default set. `provider.apiKeySet` / `baseUrlSet` are **presence
/// booleans**: a key value is never stored, never returned and never logged.
pub fn defaults() -> Value {
    let mut m = Map::new();
    m.insert("version".into(), json!(1));
    m.insert("exportDir".into(), json!("exports"));
    m.insert("autosaveSecs".into(), json!(30));
    m.insert("maxImageMp".into(), json!(40.0));
    m.insert("renderScaleMax".into(), json!(4.0));
    m.insert("logCalls".into(), json!(true));
    m.insert("logCapLines".into(), json!(LOG_CAP as u32));
    m.insert("historyCap".into(), json!(HISTORY_CAP as u32));
    m.insert("provider".into(), json!({"name": "", "model": "", "baseUrlSet": false, "apiKeySet": false}));
    m.insert("tokenSet".into(), json!(false));
    Value::Object(m)
}

const SECRET_KEYS: &[&str] = &["apikey", "api_key", "apiKey", "token", "secret", "password", "passwd", "auth", "key"];
/// Secret-shaped keys this product *does* store: the loopback access token, kept in
/// `settings.json` so the desktop app can protect the HTTP door without a terminal.
/// It is stored, used, and never returned — `config()` reports `tokenSet` instead.
const STORED_SECRETS: &[&str] = &["token"];
const ALLOWED_KEYS: &[&str] = &[
    "version", "exportDir", "autosaveSecs", "maxImageMp", "renderScaleMax", "logCalls", "logCapLines",
    "historyCap", "provider", "token",
];
const ALLOWED_PROVIDER_KEYS: &[&str] = &["name", "model", "baseUrl", "apiKeyEnv", "baseUrlSet", "apiKeySet"];

fn is_secret(k: &str) -> bool {
    SECRET_KEYS.iter().any(|s| s.eq_ignore_ascii_case(k))
}

fn to_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn to_u64(v: &Value) -> Option<u64> {
    to_f64(v).map(|x| x.max(0.0).round() as u64)
}

/// Walk a patch and flag any secret-shaped key that carries a value.
fn has_secret_value(v: &Value) -> bool {
    match v {
        Value::Object(o) => o.iter().any(|(k, x)| {
            (is_secret(k)
                && !STORED_SECRETS.contains(&k.as_str())
                && !matches!(x, Value::Bool(_))
                && !x.is_null()
                && x.as_str().map(|s| !s.trim().is_empty()).unwrap_or(!x.is_boolean()))
                || has_secret_value(x)
        }),
        Value::Array(a) => a.iter().any(has_secret_value),
        _ => false,
    }
}

/// The loopback access token, from the environment or from `settings.json`.
/// Reading it is deliberately a separate call: nothing that *lists* settings can
/// reach it, because `config()` strips the key and reports `tokenSet` instead.
pub fn access_token(root: &Path) -> Option<String> {
    if let Ok(t) = std::env::var("AGENTCANVAS_TOKEN") {
        if !t.trim().is_empty() {
            return Some(t.trim().to_string());
        }
    }
    std::fs::read(root.join("settings.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("token").and_then(|x| x.as_str()).map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
}

/// Deep merge where `null` deletes a key (RFC 7386 merge-patch semantics).
/// Used for both settings and document patches so the two behave identically.
pub fn merge_patch(base: &Value, patch: &Value) -> Value {
    if !patch.is_object() {
        return patch.clone();
    }
    let mut out = match base {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    for (k, v) in patch.as_object().into_iter().flatten() {
        if v.is_null() {
            out.remove(k);
        } else {
            let merged = match out.get(k) {
                Some(old) => merge_patch(old, v),
                None => v.clone(),
            };
            out.insert(k.clone(), merged);
        }
    }
    Value::Object(out)
}

/// Replace secret-shaped values with presence booleans before anything is stored.
/// `STORED_SECRETS` are the exception: they are kept in the file and removed again on
/// every read, which is what lets the desktop set the loopback token without a terminal
/// while `system.config` still cannot return it.
fn sanitize(v: &Value) -> Value {
    match v {
        Value::Object(o) => {
            let mut m = Map::new();
            for (k, x) in o {
                if is_secret(k) && !k.ends_with("Set") && !STORED_SECRETS.contains(&k.as_str()) {
                    let present = match x {
                        Value::Bool(b) => *b,
                        Value::Null => false,
                        Value::String(s) => !s.trim().is_empty(),
                        _ => true,
                    };
                    m.insert(format!("{k}Set"), json!(present));
                } else {
                    m.insert(k.clone(), sanitize(x));
                }
            }
            Value::Object(m)
        }
        Value::Array(a) => Value::Array(a.iter().map(sanitize).collect()),
        other => other.clone(),
    }
}

/// Read `<root>/settings.json` merged over [`defaults`], with presence-only
/// provider booleans derived from the file *and* the environment.
///
/// A missing file yields the defaults; a corrupt file yields the defaults plus an
/// `cfgError` key, so `system.config` reports the problem instead of hiding it.
pub fn config(root: &Path) -> Value {
    let mut out = defaults();
    let mut note: Option<String> = None;
    let path = root.join("settings.json");
    match std::fs::read(&path) {
        Ok(b) => match serde_json::from_slice::<Value>(&b) {
            Ok(v) => out = merge_patch(&out, &sanitize(&v)),
            Err(e) => note = Some(format!("settings.json ignored: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => note = Some(format!("settings.json unreadable: {e}")),
    }
    let mut prov = out.get("provider").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    let file_url = prov.get("baseUrl").and_then(|v| v.as_str()).map(|s| !s.trim().is_empty()).unwrap_or(false);
    let env_url = std::env::var("AGENTCANVAS_PROVIDER_BASE_URL").ok().filter(|s| !s.trim().is_empty());
    let env_key = std::env::var("AGENTCANVAS_API_KEY").ok().filter(|s| !s.trim().is_empty());
    prov.insert("baseUrlSet".into(), json!(file_url || env_url.is_some()));
    prov.insert("apiKeySet".into(), json!(env_key.is_some()));
    if let Some(u) = env_url {
        prov.insert("baseUrl".into(), json!(u));
    }
    if let Some(o) = out.as_object_mut() {
        o.insert("provider".into(), Value::Object(prov));
        o.insert("workspace".into(), json!(util::clean(root).to_string_lossy().replace('\\', "/")));
        // The stored token is dropped here, so no read path — `system.config`, the
        // settings reply, `caps`, the call log — can hand it back.
        let stored = o.remove("token").and_then(|t| t.as_str().map(|s| !s.trim().is_empty())).unwrap_or(false);
        let from_env = std::env::var("AGENTCANVAS_TOKEN").map(|s| !s.trim().is_empty()).unwrap_or(false);
        o.insert("tokenSet".into(), json!(stored || from_env));
        if let Some(n) = note {
            o.insert("cfgError".into(), json!(n));
        }
    }
    out
}

fn check_range(name: &str, v: &Value, lo: f64, hi: f64) -> R<()> {
    let x = to_f64(v).ok_or_else(|| {
        Fail::new("INVALID_VALUE", format!("{name} must be a number")).details(json!({"key": name, "got": v}))
    })?;
    if x.is_finite() && x >= lo && x <= hi {
        Ok(())
    } else {
        Err(Fail::new("INVALID_VALUE", format!("{name}={x} is out of range")).details(json!({"key": name, "range": [lo, hi]})))
    }
}

/// Validate, atomically write `<root>/settings.json`, then return the merged file.
///
/// Secrets are refused with `PERMISSION_DENIED` (presence booleans are stored
/// instead), unknown keys with `INVALID_VALUE`. The write is verified by reading
/// the bytes back before this reports success.
pub fn set_config(root: &Path, patch: &Value) -> R<Value> {
    if !patch.is_object() {
        return Err(Fail::new("INVALID_SCHEMA", "system.config set expects an object"));
    }
    if has_secret_value(patch) {
        return Err(Fail::new(
            "PERMISSION_DENIED",
            "secret values are not stored in settings.json; only presence is recorded",
        )
        .hint("set provider.baseUrl, and keep the key in AGENTCANVAS_API_KEY"));
    }
    let clean = sanitize(patch);
    if let Some(o) = clean.as_object() {
        if let Some(k) = o.keys().find(|k| !ALLOWED_KEYS.contains(&k.as_str())) {
            return Err(Fail::new("INVALID_VALUE", format!("unknown setting \"{k}\""))
                .details(json!({"unknown": k, "allowed": ALLOWED_KEYS}))
                .hint("see the documented default set in system.config"));
        }
    }
    if let Some(o) = clean.get("provider").and_then(|v| v.as_object()) {
        if let Some(k) = o.keys().find(|k| !ALLOWED_PROVIDER_KEYS.contains(&k.as_str())) {
            return Err(Fail::new("INVALID_VALUE", format!("unknown provider setting \"{k}\""))
                .details(json!({"unknown": k, "allowed": ALLOWED_PROVIDER_KEYS})));
        }
    }
    if let Some(v) = clean.get("autosaveSecs") {
        check_range("autosaveSecs", v, 0.0, 86_400.0)?;
    }
    if let Some(v) = clean.get("maxImageMp") {
        check_range("maxImageMp", v, 0.1, 200.0)?;
    }
    if let Some(v) = clean.get("renderScaleMax") {
        check_range("renderScaleMax", v, 1.0, 8.0)?;
    }
    if let Some(v) = clean.get("logCapLines") {
        check_range("logCapLines", v, 100.0, 100_000.0)?;
    }
    if let Some(v) = clean.get("historyCap") {
        check_range("historyCap", v, 1.0, 5_000.0)?;
    }
    if let Some(v) = clean.get("exportDir") {
        let s = v.as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| {
            Fail::new("INVALID_VALUE", "exportDir must be a non-empty relative path")
        })?;
        if s.contains("..") {
            return Err(Fail::new("INVALID_VALUE", "exportDir may not contain \"..\""));
        }
    }

    let merged = merge_patch(&config(root), &clean);
    // Reporting/derived keys are recomputed on read; do not persist them.
    let mut store = merged.as_object().cloned().unwrap_or_default();
    for k in ["cfgError", "workspace", "tokenSet"] {
        store.remove(k);
    }
    if let Some(p) = store.get_mut("provider").and_then(|v| v.as_object_mut()) {
        p.remove("baseUrlSet");
        p.remove("apiKeySet");
    }
    let path = root.join("settings.json");
    util::check_writable(&path, Some(root))?;
    ensure_sub(root, Path::new("settings.json"))?;
    let body = format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(store)).unwrap_or_else(|_| "{}".into())
    );
    util::write(&path, body.as_bytes())?;
    let back = std::fs::read(&path).map_err(|e| Fail::new("IO", format!("settings.json: {e}")))?;
    if back.as_slice() != body.as_bytes() {
        return Err(Fail::new("IO", "settings.json did not round-trip"));
    }
    Ok(config(root))
}

/// Configured export directory, resolved under `root`.
pub fn export_dir(root: &Path) -> PathBuf {
    let rel = config(root)
        .get("exportDir")
        .and_then(|v| v.as_str())
        .unwrap_or("exports")
        .to_string();
    util::clean(&root.join(rel))
}

/// Autosave interval in seconds; `0` disables autosave entirely.
pub fn autosave_secs(root: &Path) -> u64 {
    config(root).get("autosaveSecs").and_then(to_u64).unwrap_or(30)
}

/// Largest image the engine accepts, in megapixels (`TOO_LARGE` above this).
pub fn max_image_mp(root: &Path) -> f64 {
    config(root).get("maxImageMp").and_then(to_f64).filter(|x| *x > 0.0).unwrap_or(40.0)
}

/// Cap for undo/redo stacks, from config (falls back to [`HISTORY_CAP`]).
pub fn history_cap(root: &Path) -> usize {
    config(root).get("historyCap").and_then(to_u64).map(|x| x as usize).unwrap_or(HISTORY_CAP)
}

// --------------------------------------------------------------- call log

/// Shrink a value for the log: no secrets, no blobs, no full params.
fn redact(v: &Value, depth: usize) -> Value {
    match v {
        Value::Object(o) => {
            if depth > 3 {
                return json!("…");
            }
            let mut m = Map::new();
            for (k, x) in o.iter().take(12) {
                if is_secret(k) {
                    m.insert(k.clone(), json!(has_secret_value(&json!({k: x}))));
                } else {
                    m.insert(k.clone(), redact(x, depth + 1));
                }
            }
            if o.len() > 12 {
                m.insert("…".into(), json!(o.len() - 12));
            }
            Value::Object(m)
        }
        Value::Array(a) => {
            if a.len() > 8 {
                let mut head = vec![redact(&a[0], depth + 1)];
                head.push(json!(format!("+{} more", a.len() - 1)));
                Value::Array(head)
            } else {
                Value::Array(a.iter().map(|x| redact(x, depth + 1)).collect())
            }
        }
        Value::String(s) => {
            let t = s.trim_start();
            let blob = t.starts_with("data:")
                || (s.len() > 48 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)));
            if blob {
                json!(format!("<{}B>", s.len()))
            } else if s.chars().count() > 96 {
                let mut cut: String = s.chars().take(96).collect();
                cut.push('…');
                json!(cut)
            } else {
                json!(s)
            }
        }
        other => other.clone(),
    }
}

/// Append one JSON line to `<workspace>/log/calls.jsonl`, ring-capped at
/// [`LOG_CAP`] lines.
///
/// A line is `{t, m, ok, ms, p, changed, e?}` (`t` is stamped here when absent,
/// `slow` is added for calls over 2 s). Full params are **never** written: a
/// `params` object becomes a trimmed `args` summary, `p` becomes the bare project
/// id, secret-shaped keys collapse to booleans and long base64/data-URI strings
/// become `<nB>`. Logging cannot fail the caller: an unwritable log is recorded on
/// the next [`read_log`] instead of aborting an edit.
pub fn log_call(entry: &Value) {
    let mut line: Map<String, Value> = match entry {
        Value::Object(o) => o.clone(),
        other => {
            let mut m = Map::new();
            m.insert("p".into(), redact(other, 0));
            m
        }
    };
    if let Some(v) = line.remove("params") {
        line.insert("args".into(), redact(&v, 0));
    }
    match line.get("p") {
        Some(Value::String(s)) => {
            let id = short_id(s);
            line.insert("p".into(), json!(id));
        }
        Some(Value::Object(o)) => {
            let id = o
                .get("p")
                .or_else(|| o.get("project"))
                .and_then(|v| v.as_str())
                .map(short_id)
                .unwrap_or_default();
            line.insert("args".into(), redact(&Value::Object(o.clone()), 0));
            line.insert("p".into(), json!(id));
        }
        _ => {}
    }
    for k in ["changed", "ids", "created", "warnings", "deleted"] {
        if let Some(v) = line.get(k).cloned() {
            line.insert(k.into(), redact(&v, 2));
        }
    }
    let ms = line.get("ms").and_then(to_u64).unwrap_or(0);
    if ms > 2000 {
        line.insert("slow".into(), json!(true));
    }
    line.entry("t").or_insert_with(|| json!(util::now_ms()));
    line.entry("ok").or_insert_with(|| json!(true));
    let s = match serde_json::to_string(&Value::Object(line)) {
        Ok(s) => s,
        Err(_) => return,
    };
    let _ = append_ring(&log_path(), &s, LOG_CAP, LOG_TRIM_AT);
}

fn short_id(s: &str) -> String {
    let t = s.replace('\\', "/");
    match t.rsplit('/').next() {
        Some(last) if !last.is_empty() && !last.ends_with(".json") => last.to_string(),
        _ => t,
    }
}

/// Append, and rewrite the file when the ring overflows. `Ok(true)` = rewritten.
///
/// Shared by the call log and the per-project undo trail: both are append-only
/// jsonl with a line budget, and both would otherwise rewrite the whole file on
/// every single line if the cap were enforced one line at a time. The rewrite keeps
/// the **newest** `keep` lines *including the line being appended* — a cap that
/// swallowed the edit it was asked to record would silently eat history.
fn append_ring(path: &Path, line: &str, keep: usize, trim_at: usize) -> R<bool> {
    let mut buf = std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    if !buf.is_empty() && !buf.ends_with('\n') {
        buf.push('\n');
    }
    if buf.lines().count() + 1 > trim_at {
        let mut lines: Vec<&str> = buf.lines().filter(|l| !l.trim().is_empty()).collect();
        lines.push(line);
        let start = lines.len().saturating_sub(keep);
        let tail = &lines[start..];
        util::write(path, format!("{}\n", tail.join("\n")).as_bytes())?;
        return Ok(true);
    }
    use std::io::Write;
    util::ensure_parent(path)?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| Fail::new("IO", format!("{}: {e}", path.display())))?;
    writeln!(f, "{line}").map_err(|e| Fail::new("IO", format!("{}: {e}", path.display())))?;
    Ok(false)
}

/// The last `n` logged calls, **newest first** (deviation: `read_log` returns the
/// tail in reverse-chronological order, which is what `log` wants to show).
/// Unparsable lines are skipped rather than reported as garbage.
pub fn read_log(n: usize) -> Vec<Value> {
    let buf = std::fs::read(log_path()).unwrap_or_default();
    let s = String::from_utf8_lossy(&buf);
    s.lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .take(n.max(1))
        .collect()
}

/// Truncate the call log (`clear`). Returns how many lines were dropped.
pub fn clear_log(root: &Path) -> R<usize> {
    let path = root.join("log").join("calls.jsonl");
    let n = std::fs::read(&path)
        .map(|b| String::from_utf8_lossy(&b).lines().count())
        .unwrap_or(0);
    if path.exists() {
        util::check_writable(&path, Some(root))?;
        util::write(&path, b"")?;
    }
    Ok(n)
}

// --------------------------------------------------------------- registry

/// One undo/redo record.
///
/// The stacks in the registry are the live view; every record is also appended to
/// `<project dir>/history.jsonl` (see [`HISTORY_FILE`]) so a process that did not
/// make the edit — a one-shot CLI call, an MCP tool run — can still undo it.
/// [`crate::history::checkpoint`] is the crash-safe counterpart of the whole
/// document, this is the crash-safe counterpart of the trail.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HistoryEntry {
    /// Command that reverses the recorded edit.
    pub undo: Cmd,
    /// Command that replays it.
    pub redo: Cmd,
    /// Epoch milliseconds when the edit was applied.
    pub at: u64,
    /// `desktop` | `mcp` | `http` | `cli`.
    pub origin: String,
    /// Node/board/asset ids the edit touched.
    pub changed: Vec<String>,
    /// Short human label, e.g. `Transform n_1`.
    pub label: String,
    /// Revision the edit produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<u64>,
    /// Non-fatal notes from [`crate::commands::Applied::warnings`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Snapshot id auto-written before this edit, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snap: Option<String>,
}

struct Meta {
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    /// Has [`hydrate_history`] replayed `<project dir>/history.jsonl` into the two
    /// stacks in *this* process? Checked once per project, so a one-shot CLI call
    /// pays for the read exactly once and never re-reads a trail it already has.
    hydrated: bool,
    /// An undo/redo moved the document *without* going through the saving mutation
    /// path (`rpc::edit`), so the loan that did it must write the file when it
    /// closes: a one-shot CLI process that only keeps an undo in memory would hand
    /// the next process a document that never moved.
    pending_flush: bool,
    in_tx: u32,
    loaned: bool,
    /// Set by a mutating command *inside* the current loan; `with_project` folds it
    /// into `dirty` when the closure returns, so read-only loans do not schedule a
    /// pointless autosave.
    touched: bool,
    dirty: bool,
    last_save: u64,
    /// `(mtime-ms, size)` of `project.json` as last seen. Another door — the CLI, an
    /// HTTP call in a second process — writes the same file; when this process holds no
    /// unsaved work it follows the disk instead of serving a stale document.
    disk: (u64, u64),
    opened: u64,
    calls: u64,
    dir: PathBuf,
    path: PathBuf,
}

struct Shared {
    /// The live document holder, loaned out by [`with_project`].
    core: RwLock<Project>,
    meta: RwLock<Meta>,
}

type Reg = HashMap<String, Arc<Shared>>;
static REG: OnceLock<Mutex<Reg>> = OnceLock::new();

fn reg() -> &'static Mutex<Reg> {
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

fn locked() -> std::sync::MutexGuard<'static, Reg> {
    reg().lock().unwrap_or_else(|e| e.into_inner())
}

fn shared(id: &str) -> Option<Arc<Shared>> {
    locked().get(id).cloned()
}

/// The registry key of a project: its `project_id`, else its directory name.
pub fn key_of(p: &Project) -> String {
    if p.doc.project_id.is_empty() {
        util::stem(&p.dir)
    } else {
        p.doc.project_id.clone()
    }
}

/// Insert or replace a project under its id; returns the id.
///
/// Re-registering a project that is currently loaned updates only its directory
/// metadata — the live document wins, so a background re-open can never clobber an
/// edit in progress.
pub fn register(p: Project) -> String {
    let id = key_of(&p);
    let now = util::now_ms() as u64;
    let (dir, path) = (p.dir.clone(), p.path.clone());
    if let Some(sh) = shared(&id) {
        if let Ok(mut m) = sh.meta.try_write() {
            m.dir = dir;
            m.path = path;
        }
        if !loaned_of(&id) {
            if let Ok(mut c) = sh.core.try_write() {
                *c = p;
            }
        }
        return id;
    }
    let sh = Arc::new(Shared {
        core: RwLock::new(p),
        meta: RwLock::new(Meta {
            undo: Vec::new(),
            redo: Vec::new(),
            hydrated: false,
            pending_flush: false,
            in_tx: 0,
            loaned: false,
            touched: false,
            dirty: false,
            last_save: now,
            disk: (0, 0),
            opened: now,
            calls: 0,
            dir: dir.clone(),
            path: path.clone(),
        }),
    });
    let mut g = locked();
    // Another thread may have inserted while we were outside the lock: that entry
    // wins, we only refresh it. The guard is dropped before touching `existing`,
    // which locks that entry's own `meta`.
    if let Some(existing) = g.get(&id).cloned() {
        drop(g);
        if let Ok(mut m) = existing.meta.try_write() {
            m.dir = dir;
            m.path = path;
        }
        return id;
    }
    g.insert(id.clone(), sh);
    id
}

/// Forget a project (document + undo stacks). `PROJECT_BUSY` while it is being
/// edited, so a close can never drop a live loan.
pub fn unregister(id: &str) -> R<Project> {
    let sh = shared(id).ok_or_else(|| not_open(id))?;
    if is_busy(id) {
        return Err(busy(id));
    }
    let mut g = locked();
    g.remove(id);
    drop(g);
    // Take the project out of the cell *before* returning: the placeholder is
    // built from the live value, then the guard is released explicitly so nothing
    // of this entry stays borrowed past the call.
    let mut c = match sh.core.try_write() {
        Ok(c) => c,
        Err(_) => return Err(busy(id)),
    };
    let placeholder = empty_project(&c);
    let out = std::mem::replace(&mut *c, placeholder);
    drop(c);
    Ok(out)
}

fn empty_project(p: &Project) -> Project {
    Project {
        dir: p.dir.clone(),
        path: p.path.clone(),
        doc: Document::default(),
    }
}

/// Is this project open in this process?
pub fn is_open(id: &str) -> bool {
    shared(id).is_some()
}

/// `true` while a loan or a transaction holds the document.
pub fn is_busy(id: &str) -> bool {
    match shared(id) {
        Some(sh) => match sh.meta.try_read() {
            Ok(m) => m.loaned || m.in_tx > 0,
            Err(_) => true,
        },
        None => false,
    }
}

/// `true` while a loan is live (used by read-only helpers).
fn loaned_of(id: &str) -> bool {
    match shared(id) {
        Some(sh) => sh.meta.try_read().map(|m| m.loaned).unwrap_or(true),
        None => false,
    }
}

fn set_loan(id: &str, on: bool) {
    if let Some(sh) = shared(id) {
        if let Ok(mut m) = sh.meta.try_write() {
            m.loaned = on;
        }
    }
}

/// Every open project id, sorted.
pub fn open_ids() -> Vec<String> {
    let mut v: Vec<String> = locked().keys().cloned().collect();
    v.sort();
    v
}

/// Cheap per-project summary for `system.health`, `project.list` and `doctor`.
pub fn registry_summary() -> Vec<Value> {
    let shs: Vec<(String, Arc<Shared>)> = {
        let g = locked();
        let mut v: Vec<_> = g.iter().map(|(k, s)| (k.clone(), s.clone())).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    let now = util::now_ms() as u64;
    shs.iter()
        .map(|(id, sh)| {
            // The map guard is long gone: each entry is read under its own two
            // locks, and both guards drop before the `json!` is built.
            let m = match sh.meta.try_read() {
                Ok(m) => m,
                Err(_) => return summary_unavailable(id, now),
            };
            let busy = m.loaned;
            let doc = if busy { None } else { sh.core.try_read().ok().map(|c| c.doc.clone()) };
            let (rev, name, boards, layers) = match &doc {
                Some(d) => (
                    d.revision,
                    d.name.clone(),
                    d.artboards.len(),
                    d.artboards.iter().map(|a| a.layers.len()).sum::<usize>(),
                ),
                None => (0, String::new(), 0, 0),
            };
            let out = json!({
                "p": id, "rev": rev, "name": name, "boards": boards, "layers": layers,
                "assets": doc.as_ref().map(|d| d.assets.len()).unwrap_or(0),
                "undo": m.undo.len(), "redo": m.redo.len(), "dirty": m.dirty || busy,
                "busy": busy, "tx": m.in_tx, "calls": m.calls,
                "openMs": now.saturating_sub(m.opened), "lastSave": m.last_save,
                "dir": m.dir.to_string_lossy().replace('\\', "/"),
            });
            drop(m);
            out
        })
        .collect()
}

/// A project whose meta lock is momentarily held still gets a row, so
/// `project.list` never silently drops an open project.
fn summary_unavailable(id: &str, now: u64) -> Value {
    let dir = dir_of(id).unwrap_or_default();
    json!({
        "p": id, "rev": 0, "name": "", "boards": 0, "layers": 0, "assets": 0,
        "undo": 0, "redo": 0, "dirty": true, "busy": true, "tx": 0, "calls": 0,
        "openMs": now, "lastSave": 0,
        "dir": dir.to_string_lossy().replace('\\', "/"),
    })
}

/// Summary for one project.
pub fn summary(id: &str) -> Option<Value> {
    registry_summary().into_iter().find(|v| v.get("p").and_then(|x| x.as_str()) == Some(id))
}

/// Read-only clone of the live document, for callers outside a loan.
/// `None` when the project is not open or is currently being edited.
pub fn peek(id: &str) -> Option<Document> {
    let sh = shared(id)?;
    if loaned_of(id) {
        return None;
    }
    // Reads go through here, so this is where another door's write becomes visible.
    sync_with_disk(id);
    sh.core.try_read().ok().map(|c| c.doc.clone())
}

/// Directory of an open project.
pub fn dir_of(id: &str) -> Option<PathBuf> {
    let sh = shared(id)?;
    // Read the small value out and release the guard before touching the other
    // lock, instead of matching on a guard that has to outlive the fallback.
    let from_meta = sh
        .meta
        .try_read()
        .ok()
        .and_then(|m| if m.dir.as_os_str().is_empty() { None } else { Some(m.dir.clone()) });
    match from_meta {
        Some(d) => Some(d),
        None => sh.core.try_read().ok().map(|c| c.dir.clone()),
    }
}

/// Absolute `project.json` path of an open project.
pub fn path_of(id: &str) -> Option<PathBuf> {
    let sh = shared(id)?;
    let from_meta = sh
        .meta
        .try_read()
        .ok()
        .and_then(|m| if m.path.as_os_str().is_empty() { None } else { Some(m.path.clone()) });
    match from_meta {
        Some(p) => Some(p),
        None => sh.core.try_read().ok().map(|c| c.path.clone()),
    }
}

fn busy(id: &str) -> Fail {
    Fail::new("PROJECT_BUSY", format!("project \"{id}\" is already being edited"))
        .hint("another caller holds the document; retry, or send the edits as one Cmd::Composite")
        .details(json!({"id": id}))
        .retry()
}

fn not_open(id: &str) -> Fail {
    Fail::new("PROJECT_NOT_FOUND", format!("project \"{id}\" is not open in this process"))
        .hint("open it first (project.open {p:<id>}); every mutating method needs an open project")
        .details(json!({"id": id, "open": open_ids()}))
}

/// Borrow the live project for the duration of `f`, then write it back.
///
/// This is the only supported way to mutate a document from the RPC layer: the
/// desktop window and a remote MCP client both land here, so they share one
/// document. The document is *moved out* of the registry for the call, which is
/// what lets `f` take a plain `&mut Project` (contract shape) without a `RefMut`
/// in every signature; `loaned` blocks any other caller for the same instant.
///
/// A failed `f` still writes the document back (the mutation may be partial in
/// memory), but the dirty flag is not set and no history entry exists, so the next
/// [`crate::history::undo`] or a reload recovers from disk.
pub fn with_project<T>(id: &str, f: impl FnOnce(&mut Project) -> R<T>) -> R<T> {
    let sh = match shared(id) {
        Some(sh) => sh,
        None => return Err(not_open(id)),
    };
    if is_busy(id) {
        return Err(busy(id));
    }
    run_autosave(id);
    sync_with_disk(id);
    let mut core = match sh.core.try_write() {
        Ok(c) => c,
        Err(_) => return Err(busy(id)),
    };
    // The flag goes up *before* the document leaves the cell, so no second caller can
    // ever get the placeholder: `set_loan` only takes `meta` (never the registry), and
    // every `meta` acquisition is a non-blocking `try_*`, so this cannot deadlock.
    set_loan(id, true);
    let placeholder = empty_project(&core);
    let mut p = std::mem::replace(&mut *core, placeholder);
    drop(core); // the guard is not held across `f`: `meta.loaned` is the lock.
    let r = f(&mut p);
    let ok = r.is_ok();
    let mut core = match sh.core.try_write() {
        Ok(c) => c,
        Err(_) => {
            set_loan(id, false);
            return Err(busy(id));
        }
    };
    *core = p;
    drop(core);
    set_loan(id, false);
    // The flag is read and cleared here, and the write lock is released before the
    // flush: `flush_now` opens a loan of its own, which would fail on a held lock.
    let flush = with_meta(id, |m| {
        m.calls += 1;
        if ok {
            m.dirty |= m.touched;
        }
        m.touched = false;
        std::mem::take(&mut m.pending_flush)
    })
    .unwrap_or(false);
    if flush {
        flush_now(id);
    }
    r
}

/// The compact result of a mutation, as every RPC method returns it.
pub fn applied_result(a: &crate::commands::Applied) -> Value {
    json!({
        "ok": true, "rev": a.revision, "changed": a.changed,
        "created": a.created, "warnings": a.warnings,
    })
}

// -------------------------------------------------------- history plumbing

fn with_meta<T>(id: &str, f: impl FnOnce(&mut Meta) -> T) -> Option<T> {
    let sh = shared(id)?;
    let mut guard = sh.meta.try_write().ok()?;
    let m: &mut Meta = &mut guard;
    Some(f(m))
}

fn push_capped(v: &mut Vec<HistoryEntry>, e: HistoryEntry, cap: usize) {
    v.push(e);
    let over = v.len().saturating_sub(cap);
    if over > 0 {
        v.drain(0..over);
    }
}

// ------------------------------------------------------- the persisted trail
//
// Shape of every function here: bind a guard, read/clone what is needed, `drop`,
// *then* do the file I/O — the same discipline `unregister` and `dir_of` follow.
// `RwLock` is not reentrant and every acquisition in this file is a `try_*`, so a
// guard held across a write would not deadlock but *fail*: the trail would silently
// stop being written while the HTTP server queued behind a disk flush.

/// `<project dir>/history.jsonl`, or `None` when this process does not know the
/// project (or its directory) yet. The guard is gone before the path is returned.
fn trail_path(id: &str) -> Option<PathBuf> {
    let dir = dir_of(id)?;
    if dir.as_os_str().is_empty() {
        return None;
    }
    Some(dir.join(HISTORY_FILE))
}

/// Has this project's trail still to be replayed? A read guard, dropped here.
fn needs_hydration(id: &str) -> bool {
    match shared(id) {
        Some(sh) => sh.meta.try_read().map(|m| !m.hydrated).unwrap_or(false),
        None => false,
    }
}

/// One journal line that carries a whole entry (`push`, `put_undo`, `put_redo`):
/// the entry's own fields plus `"k"`, so the file stays readable and
/// `HistoryEntry` ignores the extra key when the line is parsed back.
fn entry_event(kind: &str, e: &HistoryEntry) -> Value {
    let mut m = match serde_json::to_value(e) {
        Ok(v) => v.as_object().cloned().unwrap_or_default(),
        Err(_) => Map::new(),
    };
    m.insert("k".into(), json!(kind));
    Value::Object(m)
}

/// A marker line: the stack moved but no entry changed, so only the kind is needed.
fn marker_event(kind: &str) -> Value {
    json!({"k": kind})
}

/// Append one journal event for `id`. Best effort by design: a trail that cannot be
/// written must not eat the edit it is recording, so the failure goes to the call
/// log instead of being returned to the caller (same rule as a failed autosave).
fn append_trail(id: &str, event: Value) {
    let Some(path) = trail_path(id) else { return };
    let line = match serde_json::to_string(&event) {
        Ok(s) => s,
        Err(_) => return,
    };
    if let Err(e) = append_ring(&path, &line, HISTORY_KEEP, HISTORY_TRIM_AT) {
        log_call(&json!({
            "m": "session.history_trail", "ok": false, "ms": 0, "p": id, "e": e.to_string(),
        }));
    }
}

/// Replay `<project dir>/history.jsonl` into `(undo, redo)`, oldest line first.
///
/// A corrupt file, a truncated last line or a line that is not one of the known
/// kinds is **skipped**, never an error: losing one entry of history is not a
/// reason to fail a call. An unrecognised `"k"` is skipped too, so a future engine
/// can add kinds without breaking this one.
fn read_trail(path: &Path) -> (Vec<HistoryEntry>, Vec<HistoryEntry>) {
    let Ok(bytes) = std::fs::read(path) else { return (Vec::new(), Vec::new()) };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let mut undo: Vec<HistoryEntry> = Vec::new();
    let mut redo: Vec<HistoryEntry> = Vec::new();
    let entry_of = |v: &Value| -> Option<HistoryEntry> {
        serde_json::from_value::<HistoryEntry>(v.clone()).ok()
    };
    let drop_one = |v: &mut Vec<HistoryEntry>, at: u64, label: &str| {
        v.retain(|x| x.at != at || x.label != label);
    };
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(t) else { continue };
        match v.get("k").and_then(|x| x.as_str()).unwrap_or("push") {
            "push" => {
                if let Some(e) = entry_of(&v) {
                    push_capped(&mut undo, e, HISTORY_CAP);
                    // A fresh edit invalidates the redo branch, exactly as in memory.
                    redo.clear();
                }
            }
            "undo" => {
                if let Some(e) = undo.pop() {
                    push_capped(&mut redo, e, HISTORY_CAP);
                }
            }
            "redo" => {
                if let Some(e) = redo.pop() {
                    push_capped(&mut undo, e, HISTORY_CAP);
                }
            }
            "put_undo" => {
                if let Some(e) = entry_of(&v) {
                    drop_one(&mut redo, e.at, &e.label);
                    push_capped(&mut undo, e, HISTORY_CAP);
                }
            }
            "put_redo" => {
                if let Some(e) = entry_of(&v) {
                    drop_one(&mut undo, e.at, &e.label);
                    push_capped(&mut redo, e, HISTORY_CAP);
                }
            }
            _ => {}
        }
    }
    (undo, redo)
}

/// Load a project's persisted trail into the in-memory stacks, at most once per
/// project per process.
///
/// This is the whole reason `history.undo` works from a process that never made the
/// edit: `agentcanvas canvas.transform …` and `agentcanvas history.undo …` are two
/// processes, and without the replay the second one has an empty stack and reports
/// success while changing nothing. Callers are the five history entry points below;
/// the flag means a long-lived desktop process pays for the read once.
pub fn hydrate_history(id: &str) {
    if !needs_hydration(id) {
        return;
    }
    let Some(path) = trail_path(id) else { return };
    // No lock at all while the file is read.
    let (undo, redo) = read_trail(&path);
    with_meta(id, |m| {
        // Another thread may have hydrated while this one was reading: whoever
        // gets the write lock first installs its result, and a missing file still
        // sets the flag so the next call costs one flag check.
        if m.hydrated {
            return;
        }
        m.hydrated = true;
        if m.undo.is_empty() {
            m.undo = undo;
        }
        if m.redo.is_empty() {
            m.redo = redo;
        }
    });
}

/// Record an applied edit: it goes on the undo stack and invalidates the redo
/// stack (you cannot redo into a future you already changed), in memory *and* in
/// `<project dir>/history.jsonl`. Capped at [`HISTORY_CAP`], oldest dropped first.
pub fn push_history(id: &str, e: HistoryEntry) -> R<()> {
    hydrate_history(id);
    let line = entry_event("push", &e);
    with_meta(id, |m| {
        push_capped(&mut m.undo, e, HISTORY_CAP);
        m.redo.clear();
        m.dirty = true;
    })
    .ok_or_else(|| not_open(id))?;
    append_trail(id, line);
    Ok(())
}

fn pop_across(from: &mut Vec<HistoryEntry>, to: &mut Vec<HistoryEntry>, cap: usize) -> Option<HistoryEntry> {
    let e = from.pop()?;
    push_capped(to, e.clone(), cap);
    Some(e)
}

/// Top of the undo stack (already mirrored onto the redo stack, and the move is
/// journaled so the next process sees the same stacks this one left).
pub fn take_undo(id: &str) -> Option<HistoryEntry> {
    hydrate_history(id);
    let sh = shared(id)?;
    let mut guard = sh.meta.try_write().ok()?;
    // One reborrow of the guard, then the two stacks are plain disjoint fields.
    let m: &mut Meta = &mut guard;
    let e = pop_across(&mut m.undo, &mut m.redo, HISTORY_CAP);
    if e.is_some() {
        // The caller is about to apply this entry to the document, and `history.undo`
        // is not one of the methods that saves on its own.
        m.pending_flush = true;
    }
    drop(guard);
    if e.is_some() {
        append_trail(id, marker_event("undo"));
    }
    e
}

/// Top of the redo stack (already mirrored back onto the undo stack).
pub fn take_redo(id: &str) -> Option<HistoryEntry> {
    hydrate_history(id);
    let sh = shared(id)?;
    let mut guard = sh.meta.try_write().ok()?;
    let m: &mut Meta = &mut guard;
    let e = pop_across(&mut m.redo, &mut m.undo, HISTORY_CAP);
    if e.is_some() {
        m.pending_flush = true;
    }
    drop(guard);
    if e.is_some() {
        append_trail(id, marker_event("redo"));
    }
    e
}

/// Put a taken entry back when the apply itself failed, so a broken undo does not
/// silently eat history. The entry is re-pushed on the stack it came from.
pub fn put_back_undo(id: &str, e: HistoryEntry) {
    let line = entry_event("put_undo", &e);
    with_meta(id, |m| {
        m.redo.retain(|x| x.at != e.at || x.label != e.label);
        push_capped(&mut m.undo, e, HISTORY_CAP);
    });
    // Undoing the `undo` marker matters as much as the push: without it a later
    // process would replay an undo that never landed.
    append_trail(id, line);
}

/// Same for the redo stack.
pub fn put_back_redo(id: &str, e: HistoryEntry) {
    let line = entry_event("put_redo", &e);
    with_meta(id, |m| {
        m.undo.retain(|x| x.at != e.at || x.label != e.label);
        push_capped(&mut m.redo, e, HISTORY_CAP);
    });
    append_trail(id, line);
}

/// `(undo, redo)` stacks, oldest first, limited to the newest `n`.
pub fn history_of(id: &str, n: usize) -> (Vec<HistoryEntry>, Vec<HistoryEntry>) {
    hydrate_history(id);
    let sh = match shared(id) {
        Some(s) => s,
        None => return (Vec::new(), Vec::new()),
    };
    let m = match sh.meta.try_read() {
        Ok(m) => m,
        Err(_) => return (Vec::new(), Vec::new()),
    };
    let tail = |v: &Vec<HistoryEntry>| -> Vec<HistoryEntry> {
        if n == 0 || v.len() <= n {
            v.clone()
        } else {
            v[v.len() - n..].to_vec()
        }
    };
    (tail(&m.undo), tail(&m.redo))
}

/// `history.list` shape: newest first, with both commands included.
pub fn history_records(id: &str, n: usize) -> Vec<Value> {
    let (undo, _) = history_of(id, n);
    undo.iter()
        .rev()
        .map(|e| {
            json!({
                "at": e.at, "origin": e.origin, "changed": e.changed, "label": e.label,
                "rev": e.rev, "warnings": e.warnings, "snap": e.snap,
                "undo": serde_json::to_value(&e.undo).unwrap_or(Value::Null),
                "redo": serde_json::to_value(&e.redo).unwrap_or(Value::Null),
            })
        })
        .collect()
}

/// Drop both stacks **in this process**: after a rollback they no longer describe
/// the document. The journal file is deliberately left alone — `project.close`
/// routes through here too, and a trail that vanished on close would defeat the
/// whole point of persisting it (a re-open in any process gets it back).
pub fn clear_history(id: &str) -> usize {
    with_meta(id, |m| {
        let n = m.undo.len() + m.redo.len();
        m.undo.clear();
        m.redo.clear();
        // Already replayed: do not re-read the file the caller just dismissed.
        m.hydrated = true;
        n
    })
    .unwrap_or(0)
}

/// Number of entries available to undo / redo.
pub fn depth(id: &str) -> (usize, usize) {
    hydrate_history(id);
    with_meta(id, |m| (m.undo.len(), m.redo.len())).unwrap_or((0, 0))
}

/// Mark the document as changed. `commands::apply` and the project mutators call
/// this so autosave knows there is something to write.
pub fn mark_changed(id: &str) {
    with_meta(id, |m| {
        m.dirty = true;
        m.touched = true;
    });
}

/// Is there an unsaved change?
pub fn is_dirty(id: &str) -> bool {
    with_meta(id, |m| m.dirty).unwrap_or(false)
}

/// Record a completed save.
pub fn note_saved(id: &str) {
    with_meta(id, |m| {
        m.last_save = util::now_ms() as u64;
        m.dirty = false;
        // Our own write is not "another door moved the file".
        m.disk = disk_stamp(&m.path);
    });
}

fn disk_stamp(path: &Path) -> (u64, u64) {
    match std::fs::metadata(path) {
        Ok(md) => {
            // Nanoseconds: two writes in the same millisecond are routine, and a stamp
            // that cannot tell them apart would keep serving the older document.
            let ns = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            (ns, md.len())
        }
        Err(_) => (0, 0),
    }
}

/// Adopt a write another door made — the CLI, a second window, an HTTP call in another
/// process — as long as this process holds nothing unsaved. Without this, the desktop
/// window keeps serving the document it opened and an agent's edits are invisible until
/// the app is restarted. A file that does not parse back is left alone; the revision
/// check is what reports a real clash.
fn sync_with_disk(id: &str) {
    let Some(sh) = shared(id) else { return };
    let Some((path, meta_dirty, loaned)) = with_meta(id, |m| (m.path.clone(), m.dirty, m.loaned)) else {
        return;
    };
    if loaned {
        return;
    }
    // Two flags say "unsaved work exists": the registry's, and the document's own —
    // the desktop window leaves edits dirty between autosaves, and losing those to a
    // reload would be worse than serving a slightly old document.
    let dirty = meta_dirty || sh.core.try_read().map(|c| c.doc.meta.dirty).unwrap_or(false);
    let now = disk_stamp(&path);
    let known = with_meta(id, |m| m.disk).unwrap_or((0, 0));
    if dirty || now == known {
        if now != known {
            with_meta(id, |m| m.disk = now);
        }
        return;
    }
    if now.0 != 0 {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(doc) = crate::doc::Document::from_json(&text) {
                if let Ok(mut core) = sh.core.try_write() {
                    core.doc = doc;
                }
            }
        }
    }
    with_meta(id, |m| m.disk = now);
}

/// Epoch ms of the last completed save (0 when never).
pub fn last_saved(id: &str) -> u64 {
    with_meta(id, |m| m.last_save).unwrap_or(0)
}

/// Should the autosave hook fire now? False inside a transaction, when clean, and
/// at most once per `interval_secs` (0 disables autosave).
pub fn save_due(id: &str, interval_secs: u64) -> bool {
    if interval_secs == 0 {
        return false;
    }
    with_meta(id, |m| {
        m.in_tx == 0
            && !m.loaned
            && m.dirty
            && util::now_ms() as u64 - m.last_save >= interval_secs * 1000
    })
    .unwrap_or(false)
}

/// Open a transaction: autosave stands down while `in_tx > 0`.
pub fn tx_begin(id: &str) -> R<()> {
    with_meta(id, |m| m.in_tx += 1).ok_or_else(|| not_open(id))
}

/// Close a transaction. Pairs 1:1 with [`tx_begin`].
pub fn tx_end(id: &str) {
    with_meta(id, |m| m.in_tx = m.in_tx.saturating_sub(1));
}

/// Current transaction depth.
pub fn tx_depth(id: &str) -> u64 {
    with_meta(id, |m| m.in_tx as u64).unwrap_or(0)
}

/// Forget every open project (`clear`, and the only sane thing between tests).
pub fn close_all() -> usize {
    let mut g = locked();
    let n = g.len();
    g.clear();
    n
}

/// Read-only view of a project without taking a loan; `None` while it is being
/// edited, so a reader never sees the placeholder document.
pub fn read<T>(id: &str, f: impl FnOnce(&Project) -> T) -> Option<T> {
    let sh = shared(id)?;
    if loaned_of(id) {
        return None;
    }
    let c = sh.core.try_read().ok()?;
    Some(f(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_patch_deletes_with_null() {
        let base = json!({"a": {"b": 1, "c": 2}, "d": 3});
        let out = merge_patch(&base, &json!({"a": {"b": null, "e": 9}}));
        assert_eq!(out, json!({"a": {"c": 2, "e": 9}, "d": 3}));
    }

    #[test]
    fn redact_hides_keys_and_blobs() {
        let v = redact(&json!({"apiKey": "sk-secret", "b64": "A".repeat(200), "id": "n_1"}), 0);
        assert_eq!(v["apiKey"], json!(true));
        assert!(v["b64"].as_str().unwrap_or_default().starts_with("<200B"));
        assert_eq!(v["id"], "n_1");
    }

    #[test]
    fn prefix_check_is_componentwise() {
        let a = Path::new("C:/w/AgentCanvas/projects/p1");
        assert!(!starts_with_posix(a, Path::new("C:/w/AgentCanvas2")));
        assert!(starts_with_posix(a, Path::new("C:/w/AgentCanvas")));
    }

    #[test]
    fn safe_rel_refuses_escape() {
        let r = Path::new("C:/w/AgentCanvas");
        assert!(safe_rel(r, "projects/a").is_ok());
        assert!(safe_rel(r, "../outside").is_err());
    }

    #[test]
    fn append_ring_drops_the_oldest_line_never_the_newest() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("ring.jsonl");
        for i in 0..10 {
            append_ring(&p, &format!("line-{i}"), 4, 6).unwrap();
        }
        let rows: Vec<String> = std::fs::read_to_string(&p).unwrap().lines().map(|l| l.to_string()).collect();
        assert_eq!(rows, vec!["line-6", "line-7", "line-8", "line-9"], "rewriting caps the file, it does not swallow the edit");
    }

    /// The loopback token is the one secret this product stores — so the guarantee has
    /// to be that no *read* path can return it, while the server still can.
    #[test]
    fn the_access_token_is_stored_but_never_returned() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let r = set_config(dir, &json!({"token": "local-loop-key"})).unwrap();
        assert_eq!(r["tokenSet"], json!(true));
        assert!(r.get("token").is_none(), "the read view must not carry the value: {r}");
        assert_eq!(access_token(dir).as_deref(), Some("local-loop-key"), "the server can read it");
        let on_disk = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(on_disk.contains("local-loop-key"), "it really is persisted");

        // A provider key is a different class of secret: never stored at all.
        assert!(set_config(dir, &json!({"provider": {"apiKey": "sk-nope"}})).is_err(), "an external key must not be accepted");

        let cleared = set_config(dir, &json!({"token": null})).unwrap();
        if std::env::var("AGENTCANVAS_TOKEN").is_err() {
            assert_eq!(cleared["tokenSet"], json!(false), "{cleared}");
            assert_eq!(access_token(dir), None);
        }
    }

    /// Two doors, one document: the desktop window and a one-shot CLI share
    /// `project.json`, and whichever door wrote last must be the one the other reads.
    #[test]
    fn a_write_by_another_door_becomes_visible_on_the_next_read() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("proj_sync");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.json");
        let mut doc = crate::doc::Document::default();
        doc.project_id = "proj_sync".into();
        doc.revision = 1;
        doc.name = "first".into();
        std::fs::write(&path, doc.to_json()).unwrap();
        register(crate::project::Project { dir: dir.clone(), path: path.clone(), doc });
        assert_eq!(peek("proj_sync").map(|x| x.revision), Some(1));

        let mut other = crate::doc::Document::default();
        other.project_id = "proj_sync".into();
        other.revision = 7;
        other.name = "written-by-another-door-with-enough-text-to-change-the-size".into();
        std::fs::write(&path, other.to_json()).unwrap();
        assert_eq!(peek("proj_sync").map(|x| x.revision), Some(7), "the read follows the disk");
        assert_eq!(peek("proj_sync").map(|x| x.name).unwrap_or_default(), other.name);

        // Unsaved local work wins: a reload must never eat what the window is holding.
        with_project("proj_sync", |p| {
            p.doc.name = "unsaved-in-this-window".into();
            p.doc.meta.dirty = true;
            Ok(())
        })
        .unwrap();
        let mut third = crate::doc::Document::default();
        third.project_id = "proj_sync".into();
        third.revision = 9;
        third.name = "another-door-again-and-again-with-even-more-text-here".into();
        std::fs::write(&path, third.to_json()).unwrap();
        let held = peek("proj_sync").unwrap();
        assert_eq!(held.name, "unsaved-in-this-window", "dirty local work is not overwritten");
        assert_eq!(held.revision, 7);
        let _ = unregister("proj_sync");
    }
}
