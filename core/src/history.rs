//! Undo / redo / checkpoints / rollback.
//!
//! Two different mechanisms, and it matters which is which:
//!
//! * **Undo/redo stacks** live in `session`'s registry (see [`crate::session::HistoryEntry`]),
//!   capped at 200 entries, and hold *commands* — cheap, and they compose with the
//!   live document. They are journalled to `<project>/history.jsonl`
//!   ([`crate::session::HISTORY_FILE`]) and replayed when a project is opened, so
//!   `history.undo` works in a process that never made the edit — which is what the
//!   one-call-per-process CLI and MCP doors need.
//! * **Checkpoints** are full document snapshots on disk at
//!   `<project>/versions/<snap>.json`, written atomically with
//!   [`crate::util::write`], indexed in `<project>/versions/versions.json`. They are
//!   the crash-safe history and the input to [`compare`].
//!
//! A command's inverse comes from [`crate::commands::Applied::undo`]; the replay
//! command is the original, so a history entry carries both and `redo` never has to
//! re-derive anything.

use crate::commands::{self, ApplyCtx, Applied, Cmd};
use crate::doc::Document;
use crate::error::{Fail, R};
use crate::project::{self, Project};
use crate::session::{self, HistoryEntry};
use crate::util;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Name of the version index inside `<project>/versions/`.
pub const INDEX: &str = "versions.json";

/// Apply a command *and* record it, so `history.undo` can reverse it later.
///
/// This is the mutation entry point every door should use: it is the only place
/// that knows both the command (the redo) and its inverse (the undo).
pub fn record(id: &str, doc: &mut Document, cmd: &Cmd, ctx: &ApplyCtx) -> R<Applied> {
    let a = commands::apply(doc, cmd, ctx)?;
    record_applied(id, cmd, &a, &ctx.origin)?;
    Ok(a)
}

/// Push an already-applied command onto the stack.
pub fn record_applied(id: &str, cmd: &Cmd, a: &Applied, origin: &str) -> R<()> {
    let e = HistoryEntry {
        undo: a.undo.clone(),
        redo: cmd.clone(),
        at: util::now_ms() as u64,
        origin: origin.to_string(),
        changed: a.changed.clone(),
        label: cmd.summary(),
        rev: Some(a.revision),
        warnings: a.warnings.clone(),
        snap: None,
    };
    session::push_history(id, e)
}

// ------------------------------------------------------------------ undo / redo

/// `history.undo` — `{p, n?, rev?}` → `{ok, rev, did, changed, undone, left}`.
///
/// Pops the newest entry, applies its inverse, and pushes the entry onto the redo
/// stack (`session::take_undo` mirrors it for you). The revision is bumped by the
/// inverse command, so undo is itself a visible edit for every other client. If the
/// inverse fails the entry is put back and the error is returned: history is never
/// silently eaten.
pub fn undo(root: &Path, p: &Value) -> R<Value> {
    steps(root, p, true)
}

/// `history.redo` — the mirror image of [`undo`].
pub fn redo(root: &Path, p: &Value) -> R<Value> {
    steps(root, p, false)
}

fn steps(root: &Path, p: &Value, backwards: bool) -> R<Value> {
    let pr = project::ensure_open(root, p)?;
    let id = pr.doc.project_id.clone();
    let want = util::f(p, "n").map(|x| x.max(1.0) as usize).unwrap_or(1);
    let expect = project::rev_of(p);
    let dir = pr.dir.clone();
    session::with_project(&id, |live| {
        // Checked against the live document, not the snapshot `ensure_open` handed
        // out: that is the only revision that is authoritative.
        project::check_rev(&live.doc, expect)?;
        let ctx = ApplyCtx::new(dir.clone(), origin_of(p).as_str());
        let mut entries: Vec<Value> = Vec::new();
        let mut changed: Vec<String> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();
        for _ in 0..want {
            let e = if backwards {
                match session::take_undo(&id) {
                    Some(e) => e,
                    None => break,
                }
            } else {
                match session::take_redo(&id) {
                    Some(e) => e,
                    None => break,
                }
            };
            let cmd = if backwards { e.undo.clone() } else { e.redo.clone() };
            match commands::apply(&mut live.doc, &cmd, &ctx) {
                Ok(a) => {
                    changed.extend(a.changed.iter().cloned());
                    warnings.extend(a.warnings.iter().cloned());
                    entries.push(json!({
                        "label": e.label, "did": e.redo.target(), "rev": a.revision,
                        "origin": e.origin, "at": e.at,
                    }));
                }
                Err(err) => {
                    // Put it back exactly where it came from, then report.
                    if backwards {
                        session::put_back_undo(&id, e);
                    } else {
                        session::put_back_redo(&id, e);
                    }
                    return Err(err);
                }
            }
        }
        if entries.is_empty() {
            let (u, r) = session::depth(&id);
            return Err(Fail::new("INVALID_VALUE", if backwards { "nothing to undo" } else { "nothing to redo" })
                .hint(if backwards {
                    "history.list shows what is left; a rollback or a reload clears the stacks"
                } else {
                    "undo first, or check history.list"
                })
                .details(json!({"p": id, "undo": u, "redo": r})));
        }
        let (u, r) = session::depth(&id);
        Ok(json!({
            "ok": true,
            "rev": live.doc.revision,
            "did": entries[0]["did"].clone(),
            "changed": dedup(changed),
            "steps": entries.len(),
            "entries": entries,
            "left": {"undo": u, "redo": r},
            "warnings": dedup(warnings),
        }))
    })
}

fn origin_of(p: &Value) -> String {
    util::ps(p, "origin").unwrap_or("desktop").to_string()
}

fn dedup(v: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for x in v {
        if !x.is_empty() && !out.contains(&x) {
            out.push(x);
        }
    }
    out
}

/// `history.list {p,n}` — newest first, for one project.
pub fn list(root: &Path, id: &str, n: usize) -> Vec<Value> {
    let _ = root;
    session::history_records(id, n)
}

/// `history(root, n)` — the trail across **every** open project, newest first.
/// (There is no project parameter in the contract signature, so this is global;
/// per-project history is [`list`], which is what `history.list` calls.)
pub fn history(root: &Path, n: usize) -> Vec<Value> {
    let _ = root;
    let mut all: Vec<Value> = Vec::new();
    for id in session::open_ids() {
        for mut v in session::history_records(&id, n) {
            if let Some(o) = v.as_object_mut() {
                o.insert("p".into(), json!(id));
            }
            all.push(v);
        }
    }
    all.sort_by(|a, b| {
        let t = |v: &Value| v.get("at").and_then(|x| x.as_u64()).unwrap_or(0);
        t(b).cmp(&t(a))
    });
    if n > 0 && all.len() > n {
        all.truncate(n);
    }
    all
}

/// `(undo, redo)` depths for one project.
pub fn depth(id: &str) -> (usize, usize) {
    session::depth(id)
}

// ------------------------------------------------------------------ checkpoints

/// `history.checkpoint {p,label}` → `{snap, rev, path, bytes, nodes, boards}`.
///
/// Writes the whole document to `<project>/versions/<snap>.json` and appends a row
/// to the version index, so `compare` and `rollback` can find it later.
pub fn checkpoint(root: &Path, p: &Value) -> R<Value> {
    let pr = project::ensure_open(root, p)?;
    let id = pr.doc.project_id.clone();
    let label = util::ps(p, "label").unwrap_or("checkpoint").to_string();
    let origin = origin_of(p);
    session::with_project(&id, |live| project::checkpoint(&live.doc, &live.dir, &label, &origin))
}

// ------------------------------------------------------------------ rollback

/// `history.rollback {p, snap|rev}` → `{ok, rev, snap, path, sha, undoCleared}`.
///
/// Restores a checkpoint into the *live* document. It writes a checkpoint of the
/// state it is about to destroy first (so a rollback can itself be rolled back), and
/// then clears the undo/redo stacks: those commands describe the document that no
/// longer exists, and pretending otherwise would be a trap. The restored revision is
/// at least `current + 1`, so clients holding an expected revision are forced to
/// re-read rather than silently overwriting a rolled-back document.
pub fn rollback(root: &Path, p: &Value) -> R<Value> {
    let pr = project::ensure_open(root, p)?;
    let id = pr.doc.project_id.clone();
    let dir = pr.dir.clone();
    let snap = match util::ps(p, "snap") {
        Some(s) => s.to_string(),
        None => {
            let want = util::f(p, "rev").map(|x| x as u64).ok_or_else(|| {
                Fail::new("INVALID_SCHEMA", "history.rollback needs snap or rev")
                    .hint("history.rollback {p, snap:snap_x} or {p, rev:12}")
            })?;
            find_snap_by_rev(&dir, want)?
        }
    };
    let path = version_path(&dir, &snap)?;
    let mut restored = read_version(&path)?;
    if restored.project_id.is_empty() {
        restored.project_id = id.clone();
    }
    if restored.project_id != id {
        return Err(Fail::new("INVALID_VALUE", "that snapshot belongs to another project")
            .hint(format!("snapshot project {} vs {}", restored.project_id, id).as_str())
            .details(json!({"snap": snap, "snapProject": restored.project_id, "p": id})));
    }
    let saved_rev = restored.revision;
    session::with_project(&id, |live| {
        let before = live.doc.revision;
        let origin = origin_of(p);
        let safety = project::checkpoint(&live.doc, &live.dir, &format!("pre-rollback of {snap}"), &origin)?;
        restored.meta.origin = Some(origin.clone());
        restored.revision = saved_rev.max(before.saturating_add(1));
        live.doc = restored.clone();
        let cleared = session::clear_history(&id);
        // Persist immediately: a rollback that only lived in RAM would be lost on
        // the next crash, which is the opposite of what a rollback is for.
        let saved = project::save(live, &json!({"origin": origin}))?;
        Ok(json!({
            "ok": true,
            "rev": live.doc.revision,
            "snap": snap,
            "from": {"snap": safety["snap"], "rev": before},
            "path": saved["path"],
            "sha": saved["sha"],
            "undoCleared": cleared,
            "changed": live.doc.nodes.len(),
        }))
    })
}

fn find_snap_by_rev(dir: &Path, rev: u64) -> R<String> {
    let rows = read_index(dir)?;
    for r in rows.iter().rev() {
        if r.get("rev").and_then(|v| v.as_u64()) == Some(rev) {
            if let Some(s) = r.get("snap").and_then(|v| v.as_str()) {
                return Ok(s.to_string());
            }
        }
    }
    Err(Fail::new("NOT_FOUND", format!("no checkpoint at revision {rev}"))
        .hint("history.list / project.snapshot shows the available snapshot ids")
        .details(json!({"rev": rev, "have": rows.iter().filter_map(|r| r.get("rev")).collect::<Vec<_>>()})))
}

// ------------------------------------------------------------------ versions

/// Path of one snapshot file inside a project.
pub fn version_path(dir: &Path, snap: &str) -> R<PathBuf> {
    if snap.trim().is_empty() || snap.contains('/') || snap.contains('\\') || snap.contains("..") {
        return Err(Fail::new("INVALID_VALUE", format!("\"{snap}\" is not a snapshot id"))
            .hint("snapshot ids look like snap_xxxxxxxx; see project.snapshot"));
    }
    let p = dir.join("versions").join(format!("{snap}.json"));
    if !p.exists() {
        return Err(Fail::new("NOT_FOUND", format!("no snapshot \"{snap}\" in {}", dir.display()))
            .hint("project.snapshot p,label writes one; history.list shows the ids")
            .details(json!({"snap": snap, "path": p.to_string_lossy()})));
    }
    Ok(p)
}

/// Read a snapshot file into a document.
pub fn read_version(path: &Path) -> R<Document> {
    let bytes = std::fs::read(path).map_err(|e| Fail::new("IO", format!("{}: {e}", path.display())))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Document::from_json(&text).map_err(|e| {
        Fail::new("INVALID_SCHEMA", format!("{}: {}", util::stem(path), e.msg))
            .hint("the snapshot is damaged; roll back to an earlier one")
    })
}

/// The version index rows, oldest first. A missing index is an empty list (the
/// project simply has no checkpoints yet); a corrupt one is an error, because
/// pretending it is empty would hide the snapshots the user can no longer reach.
pub fn read_index(dir: &Path) -> R<Vec<Value>> {
    let p = dir.join("versions").join(INDEX);
    if !p.exists() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(&p).map_err(|e| Fail::new("IO", format!("{}: {e}", p.display())))?;
    let v: Value = serde_json::from_slice(&bytes)
        .map_err(|e| Fail::new("INVALID_SCHEMA", format!("{}: {e}", p.display())))?;
    Ok(match v {
        Value::Array(a) => a,
        Value::Object(o) => o.get("versions").and_then(|x| x.as_array()).cloned().unwrap_or_default(),
        _ => Vec::new(),
    })
}

fn write_index(dir: &Path, rows: &[Value]) -> R<()> {
    let p = dir.join("versions").join(INDEX);
    util::write(&p, format!("{}\n", serde_json::to_string_pretty(rows).unwrap_or_else(|_| "[]".into())).as_bytes())
}

/// Append a row to the version index (called by `project::checkpoint`).
pub fn index_append(dir: &Path, row: Value) -> R<()> {
    let mut rows = read_index(dir).unwrap_or_default();
    rows.push(row);
    let over = rows.len().saturating_sub(500);
    if over > 0 {
        rows.drain(0..over);
    }
    write_index(dir, &rows)
}

/// All snapshots of a project, newest first.
pub fn versions(dir: &Path) -> R<Vec<Value>> {
    let mut rows = read_index(dir)?;
    rows.reverse();
    Ok(rows)
}

// ------------------------------------------------------------------ compare

/// Locate a snapshot token: a bare `snap_x`, a `proj_x/snap_x` pair, a path to a
/// version file, or `live:<project_id>` for the document currently in RAM.
///
/// `compare(root, a, b)` has no project parameter, so a bare id is resolved by
/// scanning `<root>/projects/*/versions/<id>.json` — snapshot ids are unique, so
/// this is unambiguous in practice.
pub fn locate(root: &Path, token: &str) -> R<(String, Document, Value)> {
    let t = token.trim();
    if t.is_empty() {
        return Err(Fail::new("INVALID_VALUE", "empty snapshot reference"));
    }
    if let Some(rest) = t.strip_prefix("live:") {
        let d = session::peek(rest).ok_or_else(|| {
            Fail::new("PROJECT_NOT_FOUND", format!("project \"{rest}\" is not open, so it has no live document"))
                .hint("project.open first, or compare two saved snapshots")
        })?;
        let rev = d.revision;
        return Ok((format!("live:{rest}"), d, json!({"snap": format!("live:{rest}"), "rev": rev})));
    }
    // `proj/snap`
    if let Some((pid, snap)) = t.split_once('/') {
        let dir = project::dir_for(root, pid)?;
        let path = version_path(&dir, snap)?;
        let d = read_version(&path)?;
        let rev = d.revision;
        return Ok((pid.to_string(), d, json!({"snap": snap, "rev": rev, "file": path.to_string_lossy()})));
    }
    // A direct path to a snapshot file.
    let as_path = Path::new(t);
    if as_path.is_file() {
        let d = read_version(as_path)?;
        let pid = d.project_id.clone();
        let rev = d.revision;
        return Ok((pid, d, json!({"snap": t, "rev": rev, "file": t})));
    }
    // Scan the workspace for `<id>.json`.
    let projects = root.join("projects");
    if projects.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&projects) {
            let mut hits: Vec<PathBuf> = Vec::new();
            for e in entries.flatten() {
                let cand = e.path().join("versions").join(format!("{t}.json"));
                if cand.is_file() {
                    hits.push(cand);
                }
            }
            if hits.len() == 1 {
                let d = read_version(&hits[0])?;
                let pid = d.project_id.clone();
                let rev = d.revision;
                let file = hits[0].to_string_lossy().to_string();
                return Ok((pid, d, json!({"snap": t, "rev": rev, "file": file})));
            }
            if hits.len() > 1 {
                return Err(Fail::new("INVALID_VALUE", format!("\"{t}\" matches {} snapshots", hits.len()))
                    .hint("use <project>/<snapshot> to disambiguate")
                    .details(json!({"hits": hits.iter().map(|h| h.to_string_lossy().replace('\\', "/")).collect::<Vec<_>>()})));
            }
        }
    }
    Err(Fail::new("NOT_FOUND", format!("no snapshot \"{t}\""))
        .hint("project.snapshot p,label writes one; forms accepted: snap_x, proj_x/snap_x, a path, live:proj_x")
        .details(json!({"token": t})))
}

/// `history.compare {p, from, to}` — but the contract signature takes the two
/// tokens directly, so `a`/`b` may be `snap_x`, `proj_x/snap_x`, a file path or
/// `live:proj_x`.
pub fn compare(root: &Path, a: &str, b: &str) -> R<Value> {
    let (pid_a, da, ia) = locate(root, a)?;
    let (pid_b, db, ib) = locate(root, b)?;
    if pid_a != pid_b && !pid_a.is_empty() && !pid_b.is_empty() {
        return Err(Fail::new("INVALID_VALUE", "cannot compare snapshots from two different projects")
            .details(json!({"a": pid_a, "b": pid_b})));
    }
    let diff = structural_diff(&da, &db);
    let mut out = match diff {
        Value::Object(m) => Value::Object(m),
        _ => json!({}),
    };
    if let Some(o) = out.as_object_mut() {
        o.insert("a".into(), ia);
        o.insert("b".into(), ib);
        o.insert("p".into(), json!(if pid_a.is_empty() { pid_b } else { pid_a }));
    }
    Ok(out)
}

/// Structural diff of two documents: added / removed / changed nodes, artboards and
/// assets, with before/after values, restricted to keys that actually differ.
pub fn structural_diff(a: &Document, b: &Document) -> Value {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    for n in &b.nodes {
        match a.node(&n.id) {
            None => added.push(node_brief(n)),
            Some(old) => {
                let keys = diff_value(&node_json(old), &node_json(n));
                if !keys.is_empty() {
                    changed.push(json!({"id": n.id, "keys": keys}));
                }
            }
        }
    }
    for n in &a.nodes {
        if b.node(&n.id).is_none() {
            removed.push(node_brief(n));
        }
    }

    let mut boards = json!({"added": [], "removed": [], "changed": []});
    {
        let mut ba = Vec::new();
        let mut br = Vec::new();
        let mut bc = Vec::new();
        for x in &b.artboards {
            match a.artboard(&x.id) {
                None => ba.push(json!({"id": x.id, "name": x.name, "width": x.width, "height": x.height})),
                Some(old) => {
                    let keys = diff_value(&serde_json::to_value(old).unwrap_or(Value::Null), &serde_json::to_value(x).unwrap_or(Value::Null));
                    if !keys.is_empty() {
                        bc.push(json!({"id": x.id, "keys": keys}));
                    }
                }
            }
        }
        for x in &a.artboards {
            if b.artboard(&x.id).is_none() {
                br.push(json!({"id": x.id, "name": x.name, "width": x.width, "height": x.height}));
            }
        }
        if let Some(o) = boards.as_object_mut() {
            o.insert("added".into(), json!(ba));
            o.insert("removed".into(), json!(br));
            o.insert("changed".into(), json!(bc));
        }
    }

    let mut assets = json!({"added": [], "removed": [], "changed": []});
    {
        let mut aa = Vec::new();
        let mut ar = Vec::new();
        let mut ac = Vec::new();
        for x in &b.assets {
            match a.asset(&x.id).filter(|a| a.id == x.id) {
                None => aa.push(json!({"id": x.id, "name": x.name, "uri": x.uri})),
                Some(old) => {
                    let keys = diff_value(&serde_json::to_value(old).unwrap_or(Value::Null), &serde_json::to_value(x).unwrap_or(Value::Null));
                    if !keys.is_empty() {
                        ac.push(json!({"id": x.id, "keys": keys}));
                    }
                }
            }
        }
        for x in &a.assets {
            if b.assets.iter().all(|y| y.id != x.id) {
                ar.push(json!({"id": x.id, "name": x.name, "uri": x.uri}));
            }
        }
        if let Some(o) = assets.as_object_mut() {
            o.insert("added".into(), json!(aa));
            o.insert("removed".into(), json!(ar));
            o.insert("changed".into(), json!(ac));
        }
    }

    let tokens = diff_value(&serde_json::to_value(&a.tokens).unwrap_or(Value::Null), &serde_json::to_value(&b.tokens).unwrap_or(Value::Null));
    let same = added.is_empty()
        && removed.is_empty()
        && changed.is_empty()
        && empty_arr(&boards, "added")
        && empty_arr(&boards, "removed")
        && empty_arr(&boards, "changed")
        && empty_arr(&assets, "added")
        && empty_arr(&assets, "removed")
        && empty_arr(&assets, "changed")
        && tokens.is_empty();
    let (n_add, n_rem, n_chg) = (added.len(), removed.len(), changed.len());
    json!({
        "same": same,
        "nodes": {"added": added, "removed": removed, "changed": changed},
        "artboards": boards,
        "assets": assets,
        "tokens": tokens,
        "rev": {"a": a.revision, "b": b.revision},
        "counts": {"added": n_add, "removed": n_rem, "changed": n_chg},
    })
}

fn empty_arr(v: &Value, k: &str) -> bool {
    v.get(k).and_then(|x| x.as_array()).map(|a| a.is_empty()).unwrap_or(true)
}

fn node_json(n: &crate::doc::Node) -> Value {
    let mut v = serde_json::to_value(n).unwrap_or(Value::Null);
    // The Rust field is `box_` only because `box` is a keyword; every caller-facing
    // name for it is `box` (`node_brief` below, `canvas.update`'s `box` param, a
    // template's `box.w` path). A diff key must be a key the reader can write back.
    if let Some(o) = v.as_object_mut() {
        if let Some(b) = o.remove("box_") {
            o.insert("box".into(), b);
        }
    }
    v
}

fn node_brief(n: &crate::doc::Node) -> Value {
    json!({
        "id": n.id, "type": n.kind.as_str(), "name": n.name,
        "box": {"x": n.box_.x, "y": n.box_.y, "w": n.box_.w, "h": n.box_.h},
        "board": n.artboard, "parent": n.parent,
    })
}

/// Flat key-path diff: `[{"k":"box.x","before":1,"after":2}]`, only differing keys.
fn diff_value(a: &Value, b: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    walk_diff(a, b, "", &mut out);
    out.sort_by(|x, y| x["k"].as_str().unwrap_or_default().cmp(y["k"].as_str().unwrap_or_default()));
    out
}

/// A value the reader gets from an **absent** key: every field in `doc` is
/// `#[serde(default)]`, so `false` / `""` / `null` / an empty container is not a
/// difference from the key simply not being there. Reported only when the present
/// side carries real information.
fn absence_equivalent(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !*b,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

fn walk_diff(a: &Value, b: &Value, path: &str, out: &mut Vec<Value>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                match (x.get(k), y.get(k)) {
                    (Some(xv), Some(yv)) => walk_diff(xv, yv, &p, out),
                    (Some(xv), None) => {
                        if !absence_equivalent(xv) {
                            out.push(json!({"k": p, "before": xv, "after": null}));
                        }
                    }
                    (None, Some(yv)) => {
                        if !absence_equivalent(yv) {
                            out.push(json!({"k": p, "before": null, "after": yv}));
                        }
                    }
                    (None, None) => {}
                }
            }
        }
        _ => {
            if a != b {
                out.push(json!({"k": if path.is_empty() { "value".to_string() } else { path.to_string() }, "before": a, "after": b}));
            }
        }
    }
}

/// Small helper shared with `project`: build a version-index row.
pub fn row(snap: &str, doc: &Document, label: &str, origin: &str, bytes: u64) -> Value {
    json!({
        "snap": snap, "rev": doc.revision, "at": util::now_ms() as u64, "when": util::stamp(),
        "label": label, "origin": origin, "bytes": bytes,
        "nodes": doc.nodes.len(), "boards": doc.artboards.len(), "assets": doc.assets.len(),
        "file": format!("{snap}.json"),
    })
}

/// Clear the stacks of one project (`project.close` uses it, `rollback` uses it).
pub fn clear(id: &str) -> usize {
    session::clear_history(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_diff_only_reports_differing_keys() {
        let a = json!({"box": {"x": 1.0, "y": 2.0}, "name": "n", "locked": false});
        let b = json!({"box": {"x": 9.0, "y": 2.0}, "name": "n", "visible": true});
        let d = diff_value(&a, &b);
        assert_eq!(d.len(), 2, "{d:?}");
        assert_eq!(d[0]["k"], "box.x");
        assert_eq!(d[0]["before"], 1.0);
        assert_eq!(d[0]["after"], 9.0);
        assert_eq!(d[1]["k"], "visible");
        assert_eq!(d[1]["before"], Value::Null);
    }

    #[test]
    fn structural_diff_spots_a_moved_node() {
        let mut a = Document::default();
        let mut b = a.clone();
        let mut n = crate::doc::Node::new("x", crate::doc::NodeType::Rect, crate::geom::Rect::new(0.0, 0.0, 10.0, 10.0));
        a.nodes.push(n.clone());
        n.box_.x = 5.0;
        b.nodes.push(n);
        let d = structural_diff(&a, &b);
        assert_eq!(d["same"], false);
        assert_eq!(d["nodes"]["changed"][0]["id"], "x");
        assert_eq!(d["nodes"]["changed"][0]["keys"][0]["k"], "box.x");
        let same = structural_diff(&a, &a);
        assert_eq!(same["same"], true);
    }

    #[test]
    fn version_path_rejects_traversal() {
        assert!(version_path(Path::new("."), "../x").is_err());
        assert!(version_path(Path::new("."), "a/b").is_err());
        assert!(version_path(Path::new("."), "").is_err());
    }
}
