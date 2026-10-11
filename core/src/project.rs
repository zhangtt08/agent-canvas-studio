//! Projects on disk, and the bridge between a directory and the live document.
//!
//! A project **is** a directory:
//!
//! ```text
//! <workspace>/projects/<id>/
//!     project.json      the Document, written atomically
//!     assets/           every imported image, deduped by sha256
//!     thumbs/           generated previews, safe to delete
//!     exports/          per-project render output
//!     versions/         checkpoints + versions.json index
//!     preview.png       the last rendered thumbnail
//! ```
//!
//! Nothing is hidden in a database, so a project survives the engine: copy the
//! folder, open it in an editor, diff `project.json` in review.
//!
//! Saving rules, because they are the ones users judge:
//! * `save` never bumps the revision — the revision is **command**-driven, and a
//!   save that changed the number would make an `expectedRevision` check useless.
//! * `save` validates before writing, writes through [`util::write`] (temp file +
//!   rename, so a crash leaves the previous document intact), then **reads the bytes
//!   back** and compares hashes. It cannot report a save that did not hit the disk.
//! * Autosave fires from [`crate::session::with_project`] at most once per
//!   `autosaveSecs`, and never while a transaction is in flight.

use crate::assets;
use crate::commands::{self, ApplyCtx, Applied, Cmd};
use crate::doc::{Artboard, Document, Paint};
use crate::error::{Fail, R};
use crate::geom;
use crate::history;
use crate::render;
use crate::session;
use crate::util;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// A project: where it lives and the document it holds.
///
/// `open`/`ensure_open` hand back a *snapshot* of the registered document; the live
/// one is only reachable through [`session::with_project`], which is why mutating a
/// snapshot without that loan has no effect (and why every RPC mutation goes through
/// the loan).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Project {
    pub dir: PathBuf,
    pub doc: Document,
    pub path: PathBuf,
}

/// Files and sub-directories every project has.
pub const SUBDIRS: &[&str] = &["assets", "thumbs", "exports", "versions"];
/// Name of the document file.
pub const FILE: &str = "project.json";

// ------------------------------------------------------------------ helpers

/// The workspace a project dir belongs to (`<ws>/projects/<id>` → `<ws>`); a project
/// standing on its own is its own root, which keeps writes inside it allowed while
/// anything outside still needs `AGENTCANVAS_OPEN_FS`.
pub fn workspace_of(dir: &Path) -> PathBuf {
    if let Some(parent) = dir.parent() {
        if parent.file_name().and_then(|s| s.to_str()) == Some("projects") {
            if let Some(ws) = parent.parent() {
                return util::clean(ws);
            }
        }
    }
    util::clean(dir)
}

fn check_write(dir: &Path, p: &Path) -> R<()> {
    let ws = workspace_of(dir);
    util::check_writable(p, Some(&ws))
}

/// `<root>/projects/<id>` — the directory a project id maps to.
pub fn dir_for(root: &Path, id: &str) -> R<PathBuf> {
    let id = id.trim().trim_end_matches(".json");
    if id.is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "missing project id"));
    }
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        // A path is allowed (that is how you open a project anywhere), but an id is
        // not: this is what keeps `project.close {p:"../../somewhere"}` harmless.
        let cand = util::clean(Path::new(id));
        if cand.is_absolute() || cand.join(FILE).exists() {
            return Ok(cand);
        }
        return Err(Fail::new("INVALID_VALUE", format!("\"{id}\" is not a project id"))
            .hint("pass a project id like proj_xxxxxxx, or the directory that holds project.json"));
    }
    Ok(util::clean(&root.join("projects").join(id)))
}

/// Find a project directory by id, path, or (as a last resort) by name.
pub fn find_dir(root: &Path, id_or_path: &str) -> R<PathBuf> {
    let raw = id_or_path.trim().trim_end_matches(".json");
    let direct = Path::new(id_or_path);
    if direct.is_dir() && direct.join(FILE).is_file() {
        return Ok(util::clean(direct));
    }
    if direct.is_file() {
        if let Some(p) = direct.parent() {
            if p.join(FILE).is_file() {
                return Ok(util::clean(p));
            }
        }
    }
    let by_id = dir_for(root, raw)?;
    if by_id.join(FILE).is_file() {
        return Ok(by_id);
    }
    // Scan for a matching `project_id` or project name; slow but friendly, and the
    // only way `project.open {p:"Poster"}` can work.
    let projects = root.join("projects");
    if projects.is_dir() {
        let mut by_name: Vec<PathBuf> = Vec::new();
        for e in std::fs::read_dir(&projects).map_err(|e| Fail::new("IO", format!("{}: {e}", projects.display())))? {
            let dir = e?.path();
            let file = dir.join(FILE);
            if !file.is_file() {
                continue;
            }
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let d = Document::from_json(&text).ok();
            if let Some(d) = d {
                if d.project_id == raw {
                    return Ok(util::clean(&dir));
                }
                if d.name.eq_ignore_ascii_case(raw) || util::stem(&dir) == raw {
                    by_name.push(util::clean(&dir));
                }
            }
        }
        if by_name.len() == 1 {
            return Ok(by_name.remove(0));
        }
        if by_name.len() > 1 {
            return Err(Fail::new("INVALID_VALUE", format!("{} projects are named \"{raw}\"", by_name.len()))
                .hint("open one by its id instead")
                .details(json!({"matches": by_name.iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect::<Vec<_>>()})));
        }
    }
    Err(Fail::new("PROJECT_NOT_FOUND", format!("no project \"{raw}\" under {}", util::clean(root).display()))
        .hint("project.create makes one; project.list shows what exists")
        .details(json!({"p": raw})))
}

/// `{"p":...}` / `{"project":...}` / `{"id":...}`, or a bare string.
fn selector(p: &Value) -> Option<String> {
    if let Some(s) = p.as_str() {
        return Some(s.to_string());
    }
    ["p", "project", "projectId", "id", "path"]
        .iter()
        .find_map(|k| p.get(*k).and_then(|v| v.as_str()).map(|s| s.to_string()))
}

/// The expected revision a caller sent, if any (`0` is meaningful, so `Option`).
pub fn rev_of(p: &Value) -> Option<u64> {
    let x = util::f(p, "rev").or_else(|| util::f(p, "expectedRevision"))?;
    if x.is_finite() {
        Some(x.max(0.0).round() as u64)
    } else {
        None
    }
}

/// Optimistic concurrency, the one check every mutating method performs.
///
/// A stale `rev` returns `REVISION_CONFLICT` with the **current** revision in
/// `details`, which is the whole contract: the caller re-reads and retries instead
/// of silently overwriting what the desktop window just changed.
pub fn check_rev(doc: &Document, expect: Option<u64>) -> R<()> {
    match expect {
        None => Ok(()),
        Some(want) if want == doc.revision => Ok(()),
        Some(want) => Err(Fail::new(
            "REVISION_CONFLICT",
            format!("expected revision {want}, document is at {}", doc.revision),
        )
        .hint("re-read with project.get, re-apply your command against the new rev")
        .details(json!({"rev": doc.revision, "expected": want}))
        .retry()),
    }
}

/// [`check_rev`] against a project handle.
pub fn check_rev_p(p: &Project, expect: Option<u64>) -> R<()> {
    check_rev(&p.doc, expect)
}

/// The registry key of a project id or path.
pub fn id_of(root: &Path, id_or_path: &str) -> R<String> {
    if session::is_open(id_or_path) {
        return Ok(id_or_path.to_string());
    }
    let dir = find_dir(root, id_or_path)?;
    Ok(dir.file_name().and_then(|s| s.to_str()).unwrap_or(id_or_path).to_string())
}

// ------------------------------------------------------------------ create

/// Create a real directory and a document with one artboard — or several, when
/// `opts.boards` says so.
///
/// `opts`: `{id, boards:[{w,h,name,preset,id,ratio,safe_margin,role,background}],
/// preset, author, description, background, tags}`. Board ids default to `b1`, `b2`,
/// … so hand-written documents and tests can rely on them.
pub fn create(root: &Path, name: &str, w: f64, h: f64, opts: &Value) -> R<Project> {
    session::set_default_root(root);
    let name = if name.trim().is_empty() { "Untitled" } else { name.trim() };
    let id = match util::ps(opts, "id").filter(|s| !s.is_empty() && !s.contains('/') && !s.contains('\\') && !s.contains("..")) {
        Some(s) => s.to_string(),
        None => util::id("proj"),
    };
    let dir = util::clean(&root.join("projects").join(&id));
    if dir.join(FILE).exists() {
        return Err(Fail::new("PERMISSION_DENIED", format!("project \"{id}\" already exists at {}", dir.display()))
            .hint("pass another name/id, or project.open it")
            .details(json!({"id": id, "dir": dir.to_string_lossy()})));
    }
    check_write(&dir, &dir.join(FILE))?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| Fail::new("IO", format!("{}: {e}", dir.display())))?;
    for s in SUBDIRS {
        std::fs::create_dir_all(dir.join(s)).map_err(|e| Fail::new("IO", format!("{}: {e}", dir.display())))?;
    }

    let mut doc = Document::default();
    doc.schema_version = crate::doc::SCHEMA_VERSION.into();
    doc.project_id = id.clone();
    doc.name = name.to_string();
    doc.revision = 0;
    doc.meta.created_at = Some(util::stamp());
    doc.meta.updated_at = Some(util::stamp());
    doc.meta.author = util::ps(opts, "author").map(|s| s.to_string());
    doc.meta.description = util::ps(opts, "description").map(|s| s.to_string());
    doc.meta.origin = Some(util::ps(opts, "origin").unwrap_or("desktop").to_string());

    let default_bg = util::ps(opts, "background").map(|s| Paint::solid(s));
    validate_bg(&default_bg)?;
    let specs = util::arr(opts, "boards");
    if specs.is_empty() {
        let (bw, bh, _) = preset_or(w, h, util::ps(opts, "preset"))?;
        doc.artboards.push(Artboard {
            id: "b1".into(),
            name: util::ps(opts, "boardName").unwrap_or(name).to_string(),
            width: bw,
            height: bh,
            background: default_bg.clone(),
            visible: true,
            layers: vec![],
            safe_margin: util::f(opts, "safeMargin").or_else(|| util::f(opts, "safe_margin")),
            guides: vec![],
            ratio: util::ps(opts, "ratio").map(|s| s.to_string()),
            role: util::ps(opts, "role").map(|s| s.to_string()),
            template_id: util::ps(opts, "template").map(|s| s.to_string()),
            vars: Map::new(),
        });
    } else {
        for (i, s) in specs.iter().enumerate() {
            let (sw, sh, _) = preset_or(
                util::f(s, "w").unwrap_or(w),
                util::f(s, "h").unwrap_or(h),
                util::ps(s, "preset"),
            )?;
            let idb = util::ps(s, "id").filter(|x| !x.is_empty()).map(|x| x.to_string()).unwrap_or_else(|| format!("b{}", i + 1));
            if doc.artboards.iter().any(|a| a.id == idb) {
                return Err(Fail::new("INVALID_VALUE", format!("duplicate board id \"{idb}\" in opts.boards")));
            }
            let bg = match util::ps(s, "background") {
                Some(x) => Some(Paint::solid(x)),
                None => default_bg.clone(),
            };
            validate_bg(&bg)?;
            doc.artboards.push(Artboard {
                id: idb,
                name: util::ps(s, "name").unwrap_or(&format!("Board {}", i + 1)).to_string(),
                width: sw,
                height: sh,
                background: bg,
                visible: true,
                layers: vec![],
                safe_margin: util::f(s, "safeMargin").or_else(|| util::f(s, "safe_margin")),
                guides: vec![],
                ratio: util::ps(s, "ratio").map(|x| x.to_string()),
                role: util::ps(s, "role").map(|x| x.to_string()),
                template_id: util::ps(s, "template").map(|x| x.to_string()),
                vars: s.get("vars").and_then(|v| v.as_object()).cloned().unwrap_or_default(),
            });
        }
    }
    for b in doc.artboards.iter_mut() {
        if b.ratio.is_none() {
            b.ratio = Some(format!("{}:{}", round_ratio(b.width), round_ratio(b.height)));
        }
    }

    let mut p = Project { dir: dir.clone(), doc, path: dir.join(FILE) };
    // The first write goes through `save`, so a created project is validated,
    // written atomically and read back exactly like every later write. The
    // preview is rendered once, after the version index records the creation.
    save(&mut p, &json!({"pretty": true, "preview": false, "reason": "create"}))?;
    history::index_append(&p.dir, history::row("initial", &p.doc, "created", "engine", 0))?;
    touch_recents(root, &p);
    refresh_preview(&p);
    let out = register(p);
    Ok(out)
}

fn round_ratio(v: f64) -> u64 {
    (v * 100.0).round() as u64 / 100
}

fn preset_or(w: f64, h: f64, preset: Option<&str>) -> R<(f64, f64, &'static str)> {
    if let Some(n) = preset.filter(|s| !s.is_empty()) {
        if let Some(got) = geom::preset(n) {
            return Ok(got);
        }
        if let Some((a, b)) = geom::parse_ratio(n, 1080.0) {
            return Ok((a, b, "ratio"));
        }
        return Err(Fail::new("INVALID_VALUE", format!("unknown preset \"{n}\""))
            .hint("see geom::presets(): poster square story banner xhs ecom-main long a4 wechat card business-card")
            .details(json!({"preset": n})));
    }
    if !(w.is_finite() && h.is_finite()) || w <= 0.0 || h <= 0.0 {
        return Err(Fail::new("INVALID_VALUE", format!("artboard size {w}x{h} is not a positive size"))
            .hint("project.create name,w,h  (or preset:poster)")
            .details(json!({"w": w, "h": h})));
    }
    Ok((w, h, "custom"))
}

fn validate_bg(bg: &Option<Paint>) -> R<()> {
    if let Some(Paint::Solid { color }) = bg {
        if crate::doc::parse_color(color).is_none() {
            return Err(Fail::new("INVALID_VALUE", format!("background \"{color}\" is not a colour"))
                .details(json!({"key": "background", "value": color})));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ open / close

/// Load a project (or hand back the live one if it is already open) and register it.
///
/// Re-opening an open project deliberately does **not** re-read the disk: unsaved
/// edits from the desktop window would vanish. Pass `reload:1` to force a read.
pub fn open(root: &Path, id_or_path: &str) -> R<Project> {
    session::set_default_root(root);
    install_autosave();
    let dir = find_dir(root, id_or_path)?;
    let id_hint = dir.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
    if session::is_open(&id_hint) && !util::ps(root_as_value(), "never").is_some() {
        if let Some(d) = session::peek(&id_hint) {
            return Ok(Project { dir: dir.clone(), doc: d, path: dir.join(FILE) });
        }
    }
    let path = dir.join(FILE);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| Fail::new("IO", format!("{}: {e}", path.display()))
            .hint("the directory exists but project.json is unreadable"))?;
    let mut doc = Document::from_json(&text)?;
    validate_loaded(&mut doc, &dir, id_hint.as_str())?;
    let p = Project { dir: dir.clone(), doc, path };
    touch_recents(root, &p);
    Ok(register(p))
}

/// `open` hands back a snapshot without a project handle to read `reload` from, so
/// this is a no-op placeholder kept for symmetry with the documented params.
fn root_as_value() -> &'static Value {
    static NONE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    NONE.get_or_init(|| json!({}))
}

/// Repair/refuse a document read from disk. Called by `open` and by `save`.
fn validate_loaded(doc: &mut Document, dir: &Path, id_hint: &str) -> R<()> {
    if doc.schema_version.is_empty() {
        doc.schema_version = crate::doc::SCHEMA_VERSION.into();
    }
    if doc.schema_version != crate::doc::SCHEMA_VERSION {
        // Older/newer files are still readable: the fields we do not know are kept.
        tracing_note(format!("schema {} vs engine {}", doc.schema_version, crate::doc::SCHEMA_VERSION));
    }
    if doc.project_id.is_empty() {
        doc.project_id = id_hint.to_string();
    }
    if doc.project_id != id_hint && !id_hint.is_empty() {
        // The directory is the truth: a moved/renamed project keeps working.
        tracing_note(format!("project_id {} moved into {}", doc.project_id, id_hint));
        doc.project_id = id_hint.to_string();
    }
    let mut seen: Vec<&str> = Vec::new();
    for n in &doc.nodes {
        if seen.contains(&n.id.as_str()) {
            return Err(Fail::new("INVALID_SCHEMA", format!("duplicate node id \"{}\"", n.id))
                .hint("fix the file by hand, or rollback to a checkpoint")
                .details(json!({"id": n.id, "file": dir.join(FILE).to_string_lossy()})));
        }
        seen.push(n.id.as_str());
    }
    // Dangling references are a documented, inspectable state (`inspect` has an
    // `unlinked` code), so they are noted, not fatal.
    for b in &doc.artboards {
        for l in &b.layers {
            if doc.node(l).is_none() {
                tracing_note(format!("artboard {} lists unknown layer {}", b.id, l));
            }
        }
    }
    for n in &doc.nodes {
        if let Some(p) = &n.parent {
            if doc.node(p).is_none() {
                tracing_note(format!("node {} has unknown parent {}", n.id, p));
            }
        }
        if let Some(a) = &n.artboard {
            if doc.artboard(a).is_none() {
                tracing_note(format!("node {} is on missing artboard {}", n.id, a));
            }
        }
    }
    Ok(())
}

fn tracing_note(msg: String) {
    session::log_call(&json!({"m": "project.note", "ok": true, "ms": 0, "e": msg}));
}

/// Unregister a project. Saves first if it is dirty, so closing never loses work;
/// the error from that save is returned rather than swallowed.
pub fn close(root: &Path, id: &str) -> R<()> {
    let _ = root;
    let id = id.trim().trim_end_matches(".json");
    if !session::is_open(id) {
        // Not open: make sure a stray directory path cannot "close" nothing loudly.
        let _ = find_dir(root, id);
        return Ok(());
    }
    if session::is_dirty(id) {
        session::with_project(id, |p| save(p, &json!({"reason": "close"})))?;
    }
    history::clear(id);
    session::unregister(id)?;
    Ok(())
}

// ------------------------------------------------------------------ save

/// Re-validate, write atomically, verify, refresh `preview.png`.
///
/// The revision is **not** bumped here: it counts edits, not saves, so an
/// `expectedRevision` check stays meaningful across a save.
/// `opts`: `{rev: expected, pretty: bool, preview: bool, origin, label}`.
/// Returns `{path, rev, sha, bytes, preview}`.
pub fn save(p: &mut Project, opts: &Value) -> R<Value> {
    check_rev(&p.doc, rev_of(opts))?;
    let bytes = validate_and_serialize(p, util::opt_b(opts, "pretty", true))?;
    check_write(&p.dir, &p.path)?;
    util::write(&p.path, &bytes).map_err(|e| Fail::new("IO", format!("{}: {e}", p.path.display())))?;

    // Prove it landed, before anything claims success.
    let back = std::fs::read(&p.path).map_err(|e| Fail::new("IO", format!("verify {}: {e}", p.path.display())))?;
    let want = util::sha256_bytes(&bytes);
    let got = util::sha256_bytes(&back);
    if want != got {
        return Err(Fail::new("IO", "project.json did not round-trip after write")
            .hint("the disk changed under us; re-check permissions and save again")
            .details(json!({"path": p.path.to_string_lossy(), "want": want, "got": got})));
    }
    let mut out = json!({
        "ok": true, "path": session::show(&p.path), "abs": p.path.to_string_lossy().replace('\\', "/"),
        "rev": p.doc.revision, "sha": got, "bytes": bytes.len(), "saved": true,
    });
    if util::opt_b(opts, "preview", true) {
        if let Some(prev) = refresh_preview(p) {
            out["preview"] = prev;
        }
    }
    let id = p.doc.project_id.clone();
    session::note_saved(&id);
    let _ = id;
    out["updated"] = json!(p.doc.meta.updated_at.clone().unwrap_or_default());
    Ok(out)
}

/// Stamp the metadata and produce the exact bytes that will be written.
fn validate_and_serialize(p: &mut Project, pretty: bool) -> R<Vec<u8>> {
    // Validate by round-tripping: if it does not parse back, it is not a document.
    let probe = p.doc.to_json();
    let parsed = Document::from_json(&probe)?;
    let mut doc = parsed;
    // `id_hint` is read out before `doc` is handed over mutably.
    let pid = doc.project_id.clone();
    validate_loaded(&mut doc, &p.dir, &pid)?;
    if !commands::bad_colors(&doc).is_empty() {
        let bad = commands::bad_colors(&doc);
        return Err(Fail::new("INVALID_VALUE", format!("{} colour value(s) do not parse", bad.len()))
            .hint("inspect.validate lists them; project.rollback returns to a clean checkpoint")
            .details(json!({"first": bad[0], "count": bad.len()})));
    }
    doc.meta.updated_at = Some(util::stamp());
    doc.meta.dirty = false;
    p.doc = doc;
    let text = if pretty { p.doc.to_pretty() } else { p.doc.to_json() };
    Ok(text.into_bytes())
}

/// `save_if_stale`, driven by the registry key alone: called by the autosave hook
/// from inside `session::with_project`, so it must not ask for the root.
pub fn autosave_hook(id: &str) -> R<()> {
    save_if_stale(id).map(|_| ())
}

/// Install the autosave hook (idempotent).
pub fn install_autosave() {
    session::set_autosave(autosave_hook);
}

/// Autosave: write only when the document is dirty, the interval has elapsed, and no
/// transaction is in flight. Returns `true` when a file was written.
///
/// A failed autosave is reported (and recorded in the call log) instead of being
/// swallowed: silently losing work would be the worse failure.
pub fn save_if_stale(id: &str) -> R<bool> {
    let secs = session::autosave_secs(&session::root());
    if secs == 0 || !session::save_due(id, secs) {
        return Ok(false);
    }
    let mut wrote = false;
    session::with_project(id, |p| {
        save(p, &json!({"reason": "autosave"}))?;
        wrote = true;
        Ok(())
    })?;
    let _ = id;
    Ok(wrote)
}

/// Register the project and install the autosave hook.
pub fn register(p: Project) -> Project {
    install_autosave();
    let id = session::key_of(&p);
    let dir = p.dir.clone();
    let path = p.path.clone();
    let doc = p.doc.clone();
    session::register(p);
    Project { dir, path, doc }
}

// ------------------------------------------------------------------ info

/// `{p, rev, name, boards, assets, nodes, dir, bytes}` — the RPC summary shape.
pub fn describe(p: &Project) -> Value {
    json!({
        "p": p.doc.project_id,
        "rev": p.doc.revision,
        "name": p.doc.name,
        "schema": p.doc.schema_version,
        "boards": p.doc.artboards.iter().map(|b| json!({
            "b": b.id, "name": b.name, "w": b.width, "h": b.height, "layers": b.layers.len(),
            "visible": b.visible, "background": b.background,
        })).collect::<Vec<_>>(),
        "assets": p.doc.assets.len(),
        "nodes": p.doc.nodes.len(),
        "tokens": {
            "colors": p.doc.tokens.colors.len(), "fonts": p.doc.tokens.fonts.len(),
            "spacing": p.doc.tokens.spacing.len(), "radii": p.doc.tokens.radii.len(),
        },
        "dir": session::show(&p.dir),
        "abs": p.dir.to_string_lossy().replace('\\', "/"),
        "path": session::show(&p.path),
        "updated": p.doc.meta.updated_at.clone().unwrap_or_default(),
        "origin": p.doc.meta.origin.clone().unwrap_or_default(),
        "dirty": session::is_dirty(&p.doc.project_id),
        "undo": history::depth(&p.doc.project_id).0,
        "redo": history::depth(&p.doc.project_id).1,
    })
}

/// Open projects first, then every project directory under `<root>/projects`.
pub fn list(root: &Path) -> R<Vec<Value>> {
    let projects = root.join("projects");
    let mut out: Vec<Value> = Vec::new();
    if !projects.is_dir() {
        return Ok(out);
    }
    let rd = std::fs::read_dir(&projects).map_err(|e| Fail::new("IO", format!("{}: {e}", projects.display())))?;
    for e in rd.flatten() {
        let dir = e.path();
        let file = dir.join(FILE);
        if !file.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let doc = match Document::from_json(&text) {
            Ok(d) => d,
            Err(err) => {
                out.push(json!({
                    "p": util::stem(&dir), "dir": session::show(&dir), "e": err.msg, "code": err.code, "broken": true,
                }));
                continue;
            }
        };
        let md = std::fs::metadata(&file).ok();
        out.push(json!({
            "p": doc.project_id, "name": doc.name, "rev": doc.revision,
            "boards": doc.artboards.len(), "layers": doc.nodes.len(), "assets": doc.assets.len(),
            "updated": doc.meta.updated_at.clone().unwrap_or_default(),
            "bytes": md.map(|m| m.len()).unwrap_or(0),
            "dir": session::show(&dir), "abs": dir.to_string_lossy().replace('\\', "/"),
            "open": session::is_open(&doc.project_id),
        }));
    }
    out.sort_by(|a, b| b["updated"].as_str().unwrap_or("").cmp(a["updated"].as_str().unwrap_or("")));
    Ok(out)
}

/// `<workspace>/recents.json`, newest first, capped at 20. Entries whose directory
/// vanished are dropped on read rather than reported as projects.
pub fn recents(root: &Path) -> R<Vec<Value>> {
    let path = root.join("recents.json");
    let v: Value = match std::fs::read(&path) {
        Ok(b) => serde_json::from_slice(&b).unwrap_or(json!([])),
        Err(_) => json!([]),
    };
    let rows = v.as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for r in rows {
        let dir = r.get("abs").and_then(|x| x.as_str()).map(PathBuf::from);
        let keep = match &dir {
            Some(d) => d.join(FILE).is_file(),
            None => false,
        };
        if keep {
            out.push(r);
        }
    }
    Ok(out)
}

/// Push a project to the front of the recents list.
pub fn touch_recents(root: &Path, p: &Project) {
    let path = root.join("recents.json");
    let mut rows: Vec<Value> = recents(root).unwrap_or_default();
    let id = p.doc.project_id.clone();
    rows.retain(|r| r.get("p").and_then(|v| v.as_str()) != Some(id.as_str()));
    rows.insert(0, json!({
        "p": id, "name": p.doc.name, "rev": p.doc.revision,
        "dir": session::show(&p.dir), "abs": p.dir.to_string_lossy().replace('\\', "/"),
        "at": util::stamp(), "when": util::now_ms(),
    }));
    rows.truncate(20);
    if util::check_writable(&path, Some(root)).is_ok() {
        let body = format!("{}\n", serde_json::to_string_pretty(&Value::Array(rows)).unwrap_or_else(|_| "[]".into()));
        let _ = util::write(&path, body.as_bytes());
    }
}

// ------------------------------------------------------------------ commands

/// Apply one command to the open project, record it for undo, and answer in the
/// compact RPC shape. This is the one call the desktop, HTTP and MCP all use.
///
/// `p` = `{p:<project>, rev?:<expected>, cmd:{t:...}, force?:bool, origin?:str,
/// save?:bool}`.
pub fn command(root: &Path, p: &Value) -> R<Value> {
    let cmd_v = p.get("cmd").or_else(|| p.get("command")).ok_or_else(|| {
        Fail::new("INVALID_SCHEMA", "needs cmd:{t:...}").hint("canvas.add / canvas.batch build these for you")
    })?;
    let cmd = commands::parse(cmd_v)?;
    let pr = ensure_open(root, p)?;
    let id = pr.doc.project_id.clone();
    let dir = pr.dir.clone();
    let expect = rev_of(p);
    let force = util::opt_b(p, "force", false);
    let origin = util::ps(p, "origin").unwrap_or("desktop").to_string();
    let autosave = util::ps(p, "save").map(|s| s.to_string());
    session::with_project(&id, |live| {
        check_rev(&live.doc, expect)?;
        let ctx = ApplyCtx { dir: dir.clone(), force, origin: origin.clone() };
        let a = history::record(&id, &mut live.doc, &cmd, &ctx)?;
        let mut out = session::applied_result(&a);
        if util::opt_b(p, "save", false) {
            out["saved"] = save(live, &json!({"origin": origin.clone()}))?;
        }
        if let Some(when) = autosave {
            out["autosaveHint"] = json!(when);
        }
        Ok(out)
    })
}

/// Apply a batch as one transaction (`canvas.batch`). One revision bump, all or
/// nothing.
pub fn batch(root: &Path, p: &Value) -> R<Value> {
    let rows = util::arr(p, "cmds");
    if rows.is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "canvas.batch needs cmds:[{t:...}]"));
    }
    let mut cmds = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        cmds.push(commands::parse(r).map_err(|e| e.details(json!({"index": i})))?);
    }
    let pr = ensure_open(root, p)?;
    let id = pr.doc.project_id.clone();
    let dir = pr.dir.clone();
    let expect = rev_of(p);
    let ctx = ApplyCtx {
        dir,
        force: util::opt_b(p, "force", false),
        origin: util::ps(p, "origin").unwrap_or("desktop").to_string(),
    };
    session::with_project(&id, |live| {
        check_rev(&live.doc, expect)?;
        let a = history::record(&id, &mut live.doc, &Cmd::Composite { cmds }, &ctx)?;
        Ok(session::applied_result(&a))
    })
}

// ------------------------------------------------------------------ snapshots

/// `project.snapshot {p,label}` — a full checkpoint of the live document.
pub fn checkpoint(doc: &Document, dir: &Path, label: &str, origin: &str) -> R<Value> {
    let snap = util::id("snap");
    let vdir = dir.join("versions");
    std::fs::create_dir_all(&vdir).map_err(|e| Fail::new("IO", format!("{}: {e}", vdir.display())))?;
    let path = vdir.join(format!("{snap}.json"));
    check_write(dir, &path)?;
    let body = doc.to_pretty();
    util::write(&path, body.as_bytes()).map_err(|e| Fail::new("IO", format!("{}: {e}", path.display())))?;
    let back = std::fs::read(&path).map_err(|e| Fail::new("IO", format!("{}: {e}", path.display())))?;
    if util::sha256_bytes(&back) != util::sha256_bytes(body.as_bytes()) {
        return Err(Fail::new("IO", "snapshot did not round-trip"));
    }
    let bytes = back.len() as u64;
    let row = history::row(&snap, doc, if label.is_empty() { "checkpoint" } else { label }, if origin.is_empty() { "engine" } else { origin }, bytes);
    history::index_append(dir, row)?;
    Ok(json!({
        "ok": true, "snap": snap, "rev": doc.revision, "label": label,
        "path": session::show(&path), "abs": path.to_string_lossy().replace('\\', "/"),
        "bytes": bytes, "sha": util::sha256_bytes(&back), "nodes": doc.nodes.len(), "boards": doc.artboards.len(),
    }))
}

/// `project.snapshot p,label` on the live project.
pub fn snapshot_p(root: &Path, p: &Value) -> R<Value> {
    let pr = ensure_open(root, p)?;
    let id = pr.doc.project_id.clone();
    let label = util::ps(p, "label").unwrap_or("snapshot").to_string();
    let origin = util::ps(p, "origin").unwrap_or("engine").to_string();
    session::with_project(&id, |live| checkpoint(&live.doc, &live.dir, &label, &origin))
}

/// Contract shape: a bare snapshot id for a project handle.
pub fn snapshot(p: &Project) -> R<String> {
    checkpoint(&p.doc, &p.dir, "snapshot", "engine").map(|v| v["snap"].as_str().unwrap_or_default().to_string())
}

/// The checkpoints of one project, newest first.
pub fn versions(root: &Path, p: &Value) -> R<Vec<Value>> {
    let pr = ensure_open(root, p)?;
    history::versions(&pr.dir)
}

/// Load a checkpoint into a project handle (does not touch the registry).
pub fn load_snapshot(root: &Path, id: &str, snap: &str) -> R<Document> {
    let dir = dir_for(root, id)?;
    let path = history::version_path(&dir, snap)?;
    history::read_version(&path)
}

/// `project.restore {p,snap}` → a live project holding the restored document.
///
/// The pre-restore state is checkpointed first so this is itself reversible, and the
/// restore is written to disk immediately.
pub fn restore(root: &Path, id: &str, snap: &str) -> R<Project> {
    install_autosave();
    let dir = find_dir(root, id)?;
    let pid = dir.file_name().and_then(|s| s.to_str()).unwrap_or(id).to_string();
    let path = history::version_path(&dir, snap)?;
    let mut restored = history::read_version(&path)?;
    if session::is_open(&pid) {
        let mut out: Option<Project> = None;
        session::with_project(&pid, |live| {
            checkpoint(&live.doc, &live.dir, &format!("pre-restore of {snap}"), "engine")?;
            let before = live.doc.revision;
            restored.project_id = pid.clone();
            restored.revision = restored.revision.max(before + 1);
            live.dir = dir.clone();
            live.path = dir.join(FILE);
            live.doc = restored.clone();
            let saved = save(live, &json!({"reason": "restore"}))?;
            let _ = saved;
            history::clear(&pid);
            out = Some(live.clone());
            Ok(())
        })?;
        // The fallback only runs when the loan reported nothing: rebuild the handle
        // from the parts that are still owned here, deriving `path` before `dir` is
        // moved into the struct.
        Ok(out.unwrap_or_else(|| {
            let project_path = dir.join(FILE);
            Project { dir, path: project_path, doc: restored }
        }))
    } else {
        restored.project_id = pid.clone();
        let mut p = Project { dir: dir.clone(), doc: restored, path: dir.join(FILE) };
        save(&mut p, &json!({"reason": "restore"}))?;
        Ok(register(p))
    }
}

// ------------------------------------------------------------------ bundles

/// Copy the whole project directory to `<out>/<name>.acanvas/` — a plain folder, no
/// zip dependency — and return the manifest it wrote.
pub fn export_bundle(p: &Project, out: &Path) -> R<Value> {
    let name = bundle_name(&p.doc.name);
    let target = if out.extension().and_then(|s| s.to_str()) == Some("acanvas") {
        util::clean(out)
    } else {
        util::clean(&out.join(format!("{name}.acanvas")))
    };
    check_write(&p.dir, &target)?;
    if target.join(FILE).is_file() {
        let existing = std::fs::read_to_string(target.join(FILE)).unwrap_or_default();
        let d = Document::from_json(&existing);
        if let Ok(d) = d {
            if !d.project_id.is_empty() && d.project_id != p.doc.project_id {
                return Err(Fail::new("PERMISSION_DENIED", format!("{} holds project \"{}\"", target.display(), d.project_id))
                    .hint("choose another out path or name; a bundle is never merged into another project")
                    .details(json!({"target": target.to_string_lossy(), "existing": d.project_id})));
            }
        }
    }
    std::fs::create_dir_all(&target).map_err(|e| Fail::new("IO", format!("{}: {e}", target.display())))?;
    let mut files: Vec<Value> = Vec::new();
    let mut bytes_total = 0u64;
    // The live document goes out in its saved form, not whatever is in RAM only.
    let mut fresh = p.clone();
    fresh.doc.meta.updated_at = Some(util::stamp());
    util::check_writable(&target.join(FILE), Some(&workspace_of(&p.dir)))?;
    util::write(&target.join(FILE), fresh.doc.to_pretty().as_bytes())?;
    let manifest = copy_tree(&p.dir, &target, &mut files, true)?;
    let _ = manifest;
    for f in &files {
        bytes_total += f["bytes"].as_u64().unwrap_or(0);
    }
    let entry = json!({"path": FILE, "bytes": fresh.doc.to_pretty().len(), "sha": util::sha256_bytes(fresh.doc.to_pretty().as_bytes())});
    if !files.iter().any(|f| f["path"] == entry["path"]) {
        files.insert(0, entry);
    }
    let bundle = json!({
        "kind": "agentcanvas-bundle", "bundleVersion": 1,
        "id": p.doc.project_id, "name": p.doc.name, "rev": p.doc.revision,
        "created": util::stamp(), "engine": crate::version(),
        "files": files.iter().map(|f| f["path"].clone()).collect::<Vec<_>>(),
    });
    util::write(&target.join("bundle.json"), format!("{}\n", serde_json::to_string_pretty(&bundle).unwrap_or_default()).as_bytes())?;
    Ok(json!({
        "ok": true, "path": session::show(&target), "abs": target.to_string_lossy().replace('\\', "/"),
        "bytes": bytes_total, "files": files, "count": files.len(), "id": p.doc.project_id, "name": p.doc.name,
    }))
}

fn bundle_name(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
            out.push(c);
        } else if c == '/' || c == '\\' {
            // A path separator becomes the word separator, so a namespaced name
            // stays readable and no separator survives in the folder name.
            out.push('-');
        } else {
            out.push('_');
        }
    }
    let out = out.trim().replace(' ', "-");
    if out.is_empty() {
        "project".to_string()
    } else if out.chars().count() > 48 {
        out.chars().take(48).collect()
    } else {
        out
    }
}

/// Recursively copy `src` into `dst`. `skip_existing_dir` avoids copying a bundle
/// into itself. Returns the number of files touched.
fn copy_tree(src: &Path, dst: &Path, files: &mut Vec<Value>, top: bool) -> R<usize> {
    let mut n = 0;
    let rd = std::fs::read_dir(src).map_err(|e| Fail::new("IO", format!("{}: {e}", src.display())))?;
    for e in rd {
        let e = e.map_err(|e| Fail::new("IO", format!("{}: {e}", src.display())))?;
        let sp = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.ends_with(".tmp") || name == ".DS_Store" {
            continue;
        }
        let dp = dst.join(&name);
        if top && util::clean(&dp) == util::clean(src) {
            continue;
        }
        check_write(src, &dp)?;
        if sp.is_dir() {
            std::fs::create_dir_all(&dp).map_err(|er| Fail::new("IO", format!("{}: {er}", dp.display())))?;
            n += copy_tree(&sp, &dp, files, false)?;
        } else if sp.is_file() {
            let bytes = std::fs::read(&sp).map_err(|er| Fail::new("IO", format!("{}: {er}", sp.display())))?;
            util::write(&dp, &bytes)?;
            let rel = dp.strip_prefix(dst).unwrap_or(&dp).to_string_lossy().replace('\\', "/");
            files.push(json!({"path": rel, "bytes": bytes.len(), "sha": util::sha256_bytes(&bytes)}));
            n += 1;
        }
    }
    Ok(n)
}

/// Copy a bundle (or a plain project directory) into the workspace under a fresh id,
/// rewriting `project_id`, asset paths and thumbnails.
pub fn import_bundle(root: &Path, src: &Path) -> R<Project> {
    install_autosave();
    session::set_default_root(root);
    let src = util::clean(src);
    if !src.is_dir() {
        return Err(Fail::new("NOT_FOUND", format!("{} is not a directory", src.display()))
            .hint("a bundle is a plain <name>.acanvas folder; unpack it first"));
    }
    if !src.join(FILE).is_file() {
        return Err(Fail::new("INVALID_SCHEMA", format!("{} has no {FILE}", src.display()))
            .hint("not an AgentCanvas bundle"));
    }
    let new_id = util::id("proj");
    let dir = util::clean(&root.join("projects").join(&new_id));
    check_write(&dir, &dir.join(FILE))?;
    std::fs::create_dir_all(&dir).map_err(|e| Fail::new("IO", format!("{}: {e}", dir.display())))?;
    for s in SUBDIRS {
        std::fs::create_dir_all(dir.join(s)).map_err(|e| Fail::new("IO", format!("{}: {e}", dir.join(s).display())))?;
    }
    let mut files = Vec::new();
    copy_tree(&src, &dir, &mut files, true)?;

    let path = dir.join(FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| Fail::new("IO", format!("{}: {e}", path.display())))?;
    let mut doc = Document::from_json(&text)?;
    let old_id = doc.project_id.clone();
    doc.project_id = new_id.clone();
    doc.meta.updated_at = Some(util::stamp());
    // Asset paths: an absolute or escaping uri cannot travel, so it is re-pointed at
    // the bundle's own `assets/` directory by file name.
    let mut warnings = Vec::new();
    for a in doc.assets.iter_mut() {
        let raw = a.uri.replace('\\', "/");
        let leaf = raw.rsplit('/').next().unwrap_or("").to_string();
        let escaped = raw.contains("..") || Path::new(&raw).is_absolute();
        if escaped || !Path::new(&raw).is_relative() {
            let cand = dir.join("assets").join(&leaf);
            if cand.is_file() {
                warnings.push(format!("asset {} re-pointed to assets/{leaf}", a.id));
                a.uri = format!("assets/{leaf}");
            } else {
                warnings.push(format!("asset {} uri \"{raw}\" was absolute and assets/{leaf} is missing", a.id));
            }
        }
        if let Some(t) = a.thumb.clone() {
            let leaf = t.replace('\\', "/").rsplit('/').next().unwrap_or("").to_string();
            a.thumb = Some(format!("thumbs/{leaf}"));
        }
    }
    validate_loaded(&mut doc, &dir, new_id.as_str())?;
    let mut p = Project { dir: dir.clone(), doc, path: path.clone() };
    util::write(&path, p.doc.to_pretty().as_bytes())?;
    let _ = old_id;
    p.doc.meta.origin = Some("import".into());
    let out = register(p);
    touch_recents(root, &out);
    for w in warnings {
        tracing_note(w);
    }
    Ok(out)
}

/// Duplicate an open project under a new id, files and all.
pub fn clone_project(root: &Path, id: &str, name: Option<&str>) -> R<Project> {
    let src = find_dir(root, id)?;
    let mut p = open(root, src.to_str().unwrap_or(""))?;
    if let Some(d) = session::peek(&p.doc.project_id) {
        p.doc = d;
    }
    let out_dir = util::clean(&root.join("projects"));
    let bundle = export_bundle(&p, &out_dir)?;
    let _ = bundle;
    let dir = util::outp(&format!("projects/{}", util::id("proj")), Some(root));
    std::fs::create_dir_all(&dir).map_err(|e| Fail::new("IO", format!("{}: {e}", dir.display())))?;
    let mut files = Vec::new();
    copy_tree(&src, &dir, &mut files, true)?;
    let path = dir.join(FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| Fail::new("IO", format!("{}: {e}", path.display())))?;
    let mut doc = Document::from_json(&text)?;
    doc.project_id = util::stem(&dir);
    doc.name = name.filter(|s| !s.trim().is_empty()).map(|s| s.to_string()).unwrap_or_else(|| format!("{} copy", p.doc.name));
    doc.revision = 0;
    doc.meta.created_at = Some(util::stamp());
    let new_pid = doc.project_id.clone();
    validate_loaded(&mut doc, &dir, &new_pid)?;
    let mut np = Project { dir, doc, path };
    util::write(&np.path, np.doc.to_pretty().as_bytes())?;
    let out = register(np.clone());
    touch_recents(root, &out);
    Ok(out)
}

// ------------------------------------------------------------------ rendering glue

/// Rewrite `preview.png` from the single paint path. Returns `None` when the
/// renderer is unavailable, having logged why — a preview is never a reason to fail
/// a save that already hit the disk.
pub fn refresh_preview(p: &Project) -> Option<Value> {
    let o = render::RenderOpts {
        scale: 0.25,
        format: render::Format::Png,
        quality: 0,
        background: None,
        width: Some(480),
        height: None,
    };
    // The project directory is the asset base: without it the card preview paints the
    // layout over an empty photo, because nothing can resolve `assets/<hash>.png`.
    let (bytes, art) = render::to_bytes_dir(&p.doc, &o, &[], Some(&p.dir)).ok()?;
    let path = p.dir.join("preview.png");
    if check_write(&p.dir, &path).is_err() {
        return None;
    }
    util::write(&path, &bytes).ok()?;
    Some(json!({
        "path": session::show(&path), "abs": path.to_string_lossy().replace('\\', "/"),
        "bytes": art.bytes, "w": art.width, "h": art.height, "sha": art.sha256,
    }))
}

/// Assets the document points at that are not on disk (see `asset.missing`).
pub fn missing(root: &Path, p: &Value) -> R<Vec<Value>> {
    let pr = ensure_open(root, p)?;
    Ok(assets::missing(&pr.doc, &pr.dir).unwrap_or_default())
}

/// One asset record plus where it is used.
pub fn asset_info(root: &Path, id: &str, asset_id: &str) -> R<Value> {
    let pr = open(root, id)?;
    assets::inspect(&pr.doc, &pr.dir, asset_id)
}

// ------------------------------------------------------------------ ensure_open

/// Resolve `{p}` / `{project}` / `{id}` — or a bare string — into an open project,
/// opening it from disk when needed, and check `rev` when the caller sent one.
///
/// Every mutating RPC method starts here, which is what guarantees the desktop and
/// a remote agent are looking at the same document.
pub fn ensure_open(root: &Path, p: &Value) -> R<Project> {
    install_autosave();
    let sel = selector(p).unwrap_or_default();
    if sel.trim().is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "this method needs p:<project id or path>")
            .hint("project.create returns {p}; project.list shows the ids")
            .details(if p.is_object() { json!({"keys": p.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()}) } else { json!(p) }));
    }
    // Already open? Use the live document, do not re-read the disk.
    let key = sel.replace('\\', "/").rsplit('/').next().unwrap_or("").trim_end_matches(".json").to_string();
    if key.is_empty() || !session::is_open(&key) {
        let dir = find_dir(root, &sel)?;
        let key2 = dir.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
        if !session::is_open(&key2) {
            let pr = open(root, &sel)?;
            check_rev(&pr.doc, rev_of(p))?;
            return Ok(pr);
        }
    }
    let d = session::peek(&key).ok_or_else(|| {
        Fail::new("PROJECT_BUSY", format!("project \"{key}\" is being edited right now"))
            .hint("retry the call, or read once more after the other caller finishes")
            .details(json!({"p": key}))
            .retry()
    })?;
    let pr = Project { dir: session::dir_of(&key).unwrap_or_default(), doc: d, path: session::path_of(&key).unwrap_or_else(|| PathBuf::from(FILE)) };
    check_rev(&pr.doc, rev_of(p))?;
    Ok(pr)
}

// ------------------------------------------------------------------ apply helper

/// The `commands::apply` wrapper RPC methods use when they already hold a project.
pub fn apply_to(p: &mut Project, cmd: &Cmd, ctx: &ApplyCtx) -> R<Applied> {
    let id = p.doc.project_id.clone();
    let a = commands::apply(&mut p.doc, cmd, ctx)?;
    if !id.is_empty() {
        let _ = history::record_applied(&id, cmd, &a, &ctx.origin);
    }
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_name_is_a_safe_folder() {
        assert_eq!(bundle_name("Spring / Sale 2026!"), "Spring---Sale-2026_");
        assert_eq!(bundle_name("   "), "project");
        assert_eq!(bundle_name("海报 3:4"), "海报-3_4");
    }

    #[test]
    fn workspace_of_walks_up_from_projects() {
        let d = Path::new("C:/w/AgentCanvas/projects/proj_abc");
        assert_eq!(workspace_of(d), PathBuf::from("C:/w/AgentCanvas"));
        let lone = Path::new("D:/elsewhere/thing");
        assert_eq!(workspace_of(lone), PathBuf::from("D:/elsewhere/thing"));
    }

    #[test]
    fn rev_of_reads_both_spellings_and_zero() {
        assert_eq!(rev_of(&json!({"rev": 0})), Some(0));
        assert_eq!(rev_of(&json!({"expectedRevision": 7})), Some(7));
        assert_eq!(rev_of(&json!({"rev": "9"})), Some(9));
        assert_eq!(rev_of(&json!({})), None);
    }

    #[test]
    fn check_rev_reports_the_current_number() {
        let mut d = Document::default();
        d.revision = 12;
        let e = check_rev(&d, Some(7)).unwrap_err();
        assert_eq!(e.code, "REVISION_CONFLICT");
        assert_eq!(e.details.unwrap()["rev"], 12);
        assert!(check_rev(&d, None).is_ok());
        assert!(check_rev(&d, Some(12)).is_ok());
    }

    #[test]
    fn selector_accepts_the_documented_aliases() {
        assert_eq!(selector(&json!({"p": "a"})).unwrap(), "a");
        assert_eq!(selector(&json!({"project": "b"})).unwrap(), "b");
        assert_eq!(selector(&json!("c")).unwrap(), "c");
        assert_eq!(selector(&json!({"x": 1})), None);
    }
}
