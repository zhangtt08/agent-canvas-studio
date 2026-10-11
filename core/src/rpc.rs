//! The single tool surface. CLI, HTTP, MCP and the desktop webview all arrive
//! here, so a human click and an agent call are the same code path with the same
//! history entry.
//!
//! Response shape rules (agents pay per byte):
//! - success is compact JSON with short keys, no indentation unless `pretty:1`
//! - failure is `{e, code, hint?, details?, retryable?}` and never a stack trace
//! - mutating calls echo `{ok, rev, changed}` — `rev` is what the caller passed
//!   back as `rev` next time, `changed` lists touched node ids

use crate::doc::Document;
use crate::error::{Fail, R};
use crate::{assets, commands, geom, history, imageops, inspect, jobs, layout, project, render, session, svg, templates, text, util};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Ctx {
    pub root: Option<PathBuf>,
    pub origin: String,
}

impl Default for Ctx {
    fn default() -> Self {
        Self { root: None, origin: "cli".into() }
    }
}

pub fn root_of(ctx: &Ctx) -> PathBuf {
    util::root(ctx.root.as_deref())
}

/// Top level: times the call, writes the audit line, applies `fields` trimming.
pub fn call(m: &str, p: &Value, ctx: &Ctx) -> R<Value> {
    let t0 = util::now_ms();
    util::set_ambient_root(ctx.root.as_deref());
    let r = dispatch(m, p, ctx);
    let entry = json!({
        "t": util::stamp(),
        "ms": util::now_ms() - t0,
        "m": m,
        "src": ctx.origin,
        "p": brief_params(p),
        "ok": r.is_ok(),
        "e": r.as_ref().err().map(|e| e.code).unwrap_or(""),
    });
    session::log_call(&entry);
    let mut v = r?;
    // One funnel for every door (CLI, MCP, HTTP), so no reply pays tokens for `.0`.
    util::whole_numbers(&mut v);
    Ok(match fields_of(p) {
        Some(f) => pick(v, &f),
        None if util::opt_b(p, "pretty", false) => pretty(v),
        None => v,
    })
}

fn pretty(v: Value) -> Value {
    // `serde_json::to_string_pretty` is not a Value transform; the CLI prints the
    // indented form itself when `pretty:1`, so the value stays untouched here.
    v
}

fn brief_params(p: &Value) -> Value {
    let mut o = serde_json::Map::new();
    for k in ["p", "b", "id", "path", "in", "out", "name", "op", "t", "f"] {
        if let Some(v) = p.get(k) {
            o.insert(k.into(), v.clone());
        }
    }
    if o.is_empty() {
        o.insert("n_keys".into(), json!(p.as_object().map(|m| m.len()).unwrap_or(0)));
    }
    Value::Object(o)
}

/// One solver conflict on the wire: which node, which intent, what it pointed at,
/// and why that is a problem. `with` only appears when a target was named.
fn conflict_value(c: &layout::Conflict) -> Value {
    let mut o = json!({"n": c.node, "i": c.intent, "why": c.why});
    if let Some(w) = &c.with {
        o["with"] = json!(w);
    }
    o
}

/// `fields` is a comma string over JSON and a list from the CLI (`fields=rev,name`
/// is the ergonomic form, and the CLI's comma rule turns it into an array).
fn fields_of(p: &Value) -> Option<String> {
    if let Some(s) = util::ps(p, "fields") {
        return Some(s.to_string());
    }
    p.get("fields")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(","))
}

/// A list parameter that also accepts one bare value: `paths=a.png` is a list of one.
/// `util::arr` only sees arrays, so the CLI's single-path form used to arrive empty.
fn list_of(p: &Value, k: &str) -> Vec<Value> {
    let a = util::arr(p, k);
    if !a.is_empty() {
        return a;
    }
    p.get(k).filter(|v| v.is_string()).map(|v| vec![v.clone()]).unwrap_or_default()
}

/// `project`/`history` read the door out of the params (they have no `Ctx`), and both
/// fall back to `desktop`. Stamp it so a CLI undo is not recorded as a window edit.
fn with_origin(p: &Value, ctx: &Ctx) -> Value {
    let mut v = p.clone();
    if let Some(o) = v.as_object_mut() {
        o.insert("origin".into(), json!(ctx.origin.as_str()));
    }
    v
}

fn pick(v: Value, fields: &str) -> Value {
    let want: Vec<&str> = fields.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    match v {
        Value::Object(m) => Value::Object(
            want.iter().filter_map(|k| m.get(*k).map(|val| ((*k).to_string(), val.clone()))).collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(|x| pick(x, fields)).collect()),
        other => other,
    }
}

// ------------------------------------------------------------------ project glue
//
// ADAPT: these three helpers are the only place that knows how an open project is
// reached. Everything below is transport-agnostic.

fn pid(p: &Value) -> String {
    util::ps(p, "p")
        .or_else(|| util::ps(p, "project"))
        .or_else(|| util::ps(p, "projectId"))
        .unwrap_or("")
        .to_string()
}

fn ensure(p: &Value, ctx: &Ctx) -> R<String> {
    let dir = root_of(ctx);
    let id = pid(p);
    if id.is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "missing \"p\" (project id)")
            .hint("project.create returns {p}; pass it as \"p\" on every later call"));
    }
    let pr = project::ensure_open(&dir, p)?;
    check_rev(&pr.doc, p)?;
    Ok(pr.doc.project_id.clone())
}

fn check_rev(doc: &Document, p: &Value) -> R<()> {
    if let Some(want) = util::f(p, "rev") {
        if want as u64 != doc.revision {
            return Err(Fail::new("REVISION_CONFLICT", format!("expected rev {want}, now {}", doc.revision))
                .details(json!({"rev": doc.revision}))
                .retry());
        }
    }
    Ok(())
}

/// Read-modify-write one document under the registry lock and **persist it before
/// replying**. A CLI or MCP call is its own process, so a mutation that only lives
/// in memory is a mutation that never happened.
///
/// `commands::apply` owns the revision bump for command-driven paths; callers that
/// mutate the document directly bump it themselves inside the closure.
fn edit<T: serde::Serialize>(p: &Value, ctx: &Ctx, f: impl FnOnce(&mut Document, &Path) -> R<T>) -> R<Value> {
    let want = pid(p);
    if want.is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "missing \"p\" (project id)"));
    }
    let mut out: Value = json!({});
    session::with_project(&want, |pr| {
        check_rev(&pr.doc, p)?;
        let v = f(&mut pr.doc, &pr.dir)?;
        pr.doc.meta.dirty = true;
        pr.doc.meta.origin = Some(ctx.origin.clone());
        pr.doc.meta.updated_at = Some(util::stamp());
        if util::opt_b(p, "save", true) {
            let _ = project::save(pr, &json!({"preview": util::opt_b(p, "preview", false), "reason": ctx.origin.as_str()}));
        }
        out = json!({"ok": true, "rev": pr.doc.revision, "r": v});
        Ok(())
    })?;
    Ok(out)
}

/// Apply one command through `commands::apply`, mapped to the wire shape.
fn run_cmd(p: &Value, ctx: &Ctx, cmd: commands::Cmd) -> R<Value> {
    let force = util::opt_b(p, "force", false);
    let solve = util::opt_b(p, "solve", true);
    let mut out: Value = json!({});
    let _ = edit(p, ctx, |doc, dir| {
        let ctx2 = commands::ApplyCtx { dir: dir.to_path_buf(), force, origin: ctx.origin.clone() };
        let a = commands::apply(doc, &cmd, &ctx2)?;
        // One command, one history entry — for a drag from the window exactly as
        // for a `canvas.update` from an agent, otherwise undo diverges by door.
        let _ = history::record_applied(&pid(p), &cmd, &a, &ctx.origin);
        let mut warnings = a.warnings.clone();
        if solve {
            let board = board_of_cmd(&cmd);
            if let Ok(rep) = layout::solve(doc, board.as_deref()) {
                warnings.extend(rep.conflicts.iter().map(|c| format!("{}: {}", c.node, c.why)));
            }
        }
        out = json!({"ok":true,"rev":doc.revision,"changed":a.changed,"created":a.created,"warnings":warnings});
        Ok(a)
    })?;
    Ok(out)
}

fn board_of_cmd(cmd: &commands::Cmd) -> Option<String> {
    let v = serde_json::to_value(cmd).ok()?;
    v.get("b").or_else(|| v.get("board")).and_then(|x| x.as_str()).map(|s| s.to_string())
}

fn get_doc(p: &Value, ctx: &Ctx) -> R<Document> {
    let dir = root_of(ctx);
    let id = pid(p);
    if id.is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "missing \"p\""));
    }
    // Reads open the project from disk when this process has not got it yet, so a
    // one-shot CLI call does not have to begin with `project.open`.
    let pr = project::ensure_open(&dir, p)?;
    Ok(pr.doc)
}

fn board_id(doc: &Document, p: &Value) -> R<String> {
    if let Some(b) = util::ps(p, "b").or_else(|| util::ps(p, "board")).or_else(|| util::ps(p, "artboardId")) {
        if doc.artboard(b).is_some() {
            return Ok(b.to_string());
        }
        if let Some(ab) = doc.artboards.iter().find(|a| a.name == b) {
            return Ok(ab.id.clone());
        }
        return Err(Fail::new("ARTBOARD_NOT_FOUND", format!("no artboard \"{b}\""))
            .details(json!({"boards": doc.artboards.iter().map(|a| json!({"b":a.id,"name":a.name})).collect::<Vec<_>>()})));
    }
    doc.first_artboard().map(|a| a.id.clone()).ok_or_else(|| {
        Fail::new("ARTBOARD_NOT_FOUND", "project has no artboard").hint("artboard.create {p,w,h} first")
    })
}

fn node_id(doc: &Document, p: &Value) -> R<String> {
    let raw = util::ps(p, "id").or_else(|| util::ps(p, "nodeId")).unwrap_or("");
    if raw.is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "missing \"id\""));
    }
    if doc.node(raw).is_some() {
        return Ok(raw.to_string());
    }
    if let Some(n) = doc.nodes.iter().find(|n| n.name == raw) {
        return Ok(n.id.clone());
    }
    Err(Fail::new("NODE_NOT_FOUND", format!("no node \"{raw}\""))
        .hint("inspect.document lists ids, or canvas.add returns the new id"))
}

// ------------------------------------------------------------------ dispatch

pub fn dispatch(m: &str, p: &Value, ctx: &Ctx) -> R<Value> {
    let dir = root_of(ctx);
    match m {
        // ------------------------------------------------------ system
        "system.caps" | "caps" => caps(),
        "system.health" | "health" => Ok(json!({
            "ok": true, "ver": crate::version(), "schema": crate::doc::SCHEMA_VERSION,
            "port": crate::http_port(), "jobs": jobs::snapshot().len(),
            "pid": std::process::id(),
            "fonts": text::Registry::shared().families().len(),
            // The resolved absolute root: this is the string an agent puts into an MCP
            // config, and `short()` would answer "" for the root it is relative to.
            "root": dir.to_string_lossy().replace('\\', "/"),
        })),
        "system.version" | "version" => Ok(json!({"ver":crate::version(),"schema":crate::doc::SCHEMA_VERSION,"build":build_tag()})),
        "system.doctor" | "doctor" => doctor(&dir),
        "system.tools" | "tools" => {
            let all = mcp_tools();
            // The tool list is the first thing an agent reads, and the JSON Schemas are
            // most of its bulk. `full=1` is the expensive option, not the default.
            if util::opt_b(p, "full", false) {
                Ok(all)
            } else {
                let short = all
                    .as_array()
                    .map(|v| {
                        v.iter()
                            .map(|t| json!({"n": t["name"], "m": t["method"], "d": t["description"]}))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(Value::Array(short))
            }
        }
        "system.schema" | "schema" => Ok(json!({
            "doc": serde_json::to_value(Document::default()).unwrap_or(Value::Null),
            "codes": crate::error::CODES, "cmds": cmd_shapes(), "intents": intent_keys(),
        })),
        "system.config" | "config" => match p.get("set") {
            Some(patch) if patch.is_object() => session::set_config(&dir, patch),
            _ => Ok(session::config(&dir)),
        },
        // Does the endpoint in `provider.baseUrl` actually answer? An optional extra
        // must be checkable without a model call, and `PROVIDER_NOT_CONFIGURED` has to
        // stay a fact rather than a guess.
        "system.provider.probe" | "provider.probe" => {
            let mut cfg = session::config(&dir);
            if let Some(u) = util::ps(p, "url") {
                if let Some(o) = cfg.get_mut("provider").and_then(|v| v.as_object_mut()) {
                    o.insert("baseUrl".into(), json!(u));
                }
            }
            let ms = util::f(p, "timeout").unwrap_or(2500.0).clamp(100.0, 20_000.0) as u64;
            crate::provider::probe(&cfg, ms)
        }
        "system.demo" | "demo" => demo(p, ctx),
        "log" => Ok(Value::Array(session::read_log(util::f(p, "n").unwrap_or(25.0) as usize))),
        "clear" => session::clear_log(&dir).map(|n| json!({"ok":true,"cleared":n})),
        "jobs" => Ok(Value::Array(jobs::snapshot())),
        "cancel" => Ok(json!({"ok": jobs::cancel(util::req_s(p, "id").unwrap_or("")) })),
        "help" => Ok(json!({"use": cheatsheet()})),
        "methods" => Ok(Value::Array(methods().iter().map(|(n, d)| json!({"m":n,"d":d})).collect())),

        // ------------------------------------------------------ batch
        "batch" => {
            let jobs = util::arr(p, "jobs").into_iter().chain(util::arr(p, "ops")).collect::<Vec<_>>();
            let stop_on_err = util::opt_b(p, "stop_on_err", false);
            let mut out = Vec::with_capacity(jobs.len());
            for (i, j) in jobs.iter().enumerate() {
                let mm = util::ps(j, "m").or_else(|| util::ps(j, "op")).unwrap_or("");
                let pp = j.get("p").cloned().unwrap_or_else(|| {
                    let mut o = j.as_object().cloned().unwrap_or_default();
                    o.remove("m");
                    o.remove("op");
                    Value::Object(o)
                });
                let mut entry = match call(mm, &pp, ctx) {
                    Ok(v) => {
                        let mut o = v.as_object().cloned().unwrap_or_else(|| {
                            let mut x = serde_json::Map::new();
                            x.insert("r".into(), v.clone());
                            x
                        });
                        o.insert("i".into(), json!(i));
                        o.insert("ok".into(), json!(true));
                        Value::Object(o)
                    }
                    Err(e) => json!({"i":i,"ok":false,"e":e.msg,"code":e.code,"hint":e.hint}),
                };
                let failed = entry.get("ok").and_then(|x| x.as_bool()) == Some(false);
                if failed && stop_on_err {
                    out.push(entry);
                    break;
                }
                out.append(&mut vec![entry]);
            }
            Ok(json!({"ok": out.iter().all(|x| x.get("ok").and_then(|v| v.as_bool()) != Some(false)), "r": out}))
        }

        // ------------------------------------------------------ project
        "project.create" => {
            let name = util::ps(p, "name").unwrap_or("Untitled").to_string();
            let (w, h) = size_of(p)?.unwrap_or((1080.0, 1440.0));
            // Stamp the door that made the project; a document from MCP must not
            // claim it was drawn in the desktop window.
            let mut opts = p.clone();
            if opts.get("origin").is_none() {
                if let Some(o) = opts.as_object_mut() {
                    o.insert("origin".into(), json!(ctx.origin.as_str()));
                }
            }
            let pr = project::create(&dir, &name, w, h, &opts)?;
            let out = json!({"p":pr.doc.project_id,"rev":pr.doc.revision,"dir":util::short(&pr.dir,None),
                             "boards":pr.doc.artboards.iter().map(|a| json!({"b":a.id,"name":a.name,"w":a.width,"h":a.height})).collect::<Vec<_>>()});
            let _ = pr;
            Ok(out)
        }
        "project.open" => {
            let pr = project::open(&dir, util::req_s(p, "p").unwrap_or(""))?;
            Ok(summary(&pr.doc))
        }
        "project.get" => {
            let doc = get_doc(p, ctx)?;
            let depth = util::f(p, "depth").unwrap_or(1.0) as u8;
            if depth == 0 {
                return Ok(summary(&doc));
            }
            let mut v = serde_json::to_value(&doc)?;
            // One spelling on the wire. `depth:0` answers `rev`/`p`, and a caller that
            // reads the whole document must not need a second vocabulary (`revision`,
            // `project_id`) — those long names stay valid when reading project.json.
            if let Some(o) = v.as_object_mut() {
                if let Some(r) = o.remove("revision") {
                    o.insert("rev".into(), r);
                }
                if let Some(i) = o.remove("project_id") {
                    o.insert("p".into(), i);
                }
            }
            Ok(v)
        }
        "project.list" | "project.recent" => {
            if m.ends_with("recent") {
                Ok(Value::Array(project::recents(&dir)?))
            } else {
                Ok(Value::Array(project::list(&dir)?))
            }
        }
        "project.save" => {
            let id = ensure(p, ctx)?;
            session::with_project(&id, |pr| project::save(pr, p))
        }
        "project.close" => {
            let id = pid(p);
            project::close(&dir, &id)?;
            Ok(json!({"ok":true}))
        }
        "project.clone" => {
            let id = ensure(p, ctx)?;
            project::clone_project(&dir, &id, util::ps(p, "name")).map(|pr| summary(&pr.doc))
        }
        "project.snapshot" => project::snapshot_p(&dir, p),
        "project.restore" => project::restore(&dir, &pid(p), util::req_s(p, "snap").unwrap_or("")).map(|pr| summary(&pr.doc)),
        "project.export_bundle" => {
            let id = ensure(p, ctx)?;
            let out = util::outp(util::req_s(p, "o")?, Some(&dir));
            let pr = project::open(&dir, &id)?;
            project::export_bundle(&pr, &out)
        }
        "project.import_bundle" => {
            let src = util::inp(util::req_s(p, "path")?, Some(&dir))?;
            project::import_bundle(&dir, &src).map(|pr| summary(&pr.doc))
        }
        "project.autosave" => Ok(json!({"ok": project::save_if_stale(&pid(p))? })),

        // ------------------------------------------------------ artboard
        "artboard.create" => {
            ensure(p, ctx)?;
            let (w, h) = size_of(p)?.unwrap_or((1080.0, 1440.0));
            run_cmd(p, ctx, commands::Cmd::BoardAdd { board: new_board(p, w, h)? })
        }
        "artboard.list" => {
            let doc = get_doc(p, ctx)?;
            Ok(Value::Array(doc.artboards.iter().map(|a| json!({"b":a.id,"name":a.name,"w":a.width,"h":a.height,"n":a.layers.len()})).collect()))
        }
        "artboard.resize" => {
            let doc = get_doc(p, ctx)?;
            let b = board_id(&doc, p)?;
            let (w, h) = size_of(p)?.ok_or_else(|| Fail::new("INVALID_SCHEMA", "need w,h or preset or ratio"))?;
            ensure(p, ctx)?;
            run_cmd(p, ctx, commands::Cmd::Reflow { board: b, w, h, mode: util::ps(p, "mode").unwrap_or("reflow").to_string() })
        }
        "artboard.duplicate" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let b = board_id(&doc, p)?;
            duplicate_boards(p, ctx, &b)
        }
        "artboard.delete" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let b = board_id(&doc, p)?;
            let ab = doc.artboard(&b).cloned().ok_or_else(|| Fail::new("ARTBOARD_NOT_FOUND", b.clone()))?;
            run_cmd(p, ctx, commands::Cmd::BoardDel { id: b, board: ab })
        }
        "artboard.set" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let b = board_id(&doc, p)?;
            run_cmd(p, ctx, commands::Cmd::BoardSet { id: b, patch: p.get("s").cloned().unwrap_or_else(|| json!({})) })
        }

        // ------------------------------------------------------ assets
        "asset.import" => {
            let id = ensure(p, ctx)?;
            let mut res: Vec<Value> = vec![];
            session::with_project(&id, |pr| {
                let list: Vec<String> = list_of(p, "paths")
                    .into_iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .chain(util::ps(p, "path").map(|s| vec![s.to_string()]).unwrap_or_default())
                    .collect();
                if list.is_empty() {
                    return Err(Fail::new("INVALID_SCHEMA", "need path or paths[]"));
                }
                for s in list {
                    let src = util::inp(&s, Some(&dir))?;
                    let imp = assets::import(&mut pr.doc, &pr.dir, &src, p)?;
                    res.push(serde_json::to_value(&imp)?);
                }
                pr.doc.revision += 1;
                let _ = project::save(pr, &json!({"preview": false, "reason": ctx.origin.as_str()}));
                Ok(())
            })?;
            Ok(json!({"ok":true,"rev": get_doc(p, ctx)?.revision, "assets":res}))
        }
        "asset.list" => {
            let doc = get_doc(p, ctx)?;
            let q = util::ps(p, "q").unwrap_or("");
            Ok(Value::Array(
                doc.assets
                    .iter()
                    .filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q.to_lowercase()) || a.id.contains(q))
                    .map(|a| json!({"id":a.id,"name":a.name,"w":a.width,"h":a.height,"kind":a.kind,"bytes":a.bytes}))
                    .collect(),
            ))
        }
        "asset.inspect" => {
            let doc = get_doc(p, ctx)?;
            let id = util::req_s(p, "id").unwrap_or("").to_string();
            let pr = project::ensure_open(&dir, &json!({"p":pid(p)}))?;
            assets::inspect(&doc, &pr.dir, &id)
        }
        "asset.replace" => {
            let id = ensure(p, ctx)?;
            let src = util::inp(util::req_s(p, "path")?, Some(&dir))?;
            let aid = util::req_s(p, "id")?.to_string();
            let mut out = Value::Null;
            session::with_project(&id, |pr| {
                out = serde_json::to_value(assets::replace(&mut pr.doc, &pr.dir, &aid, &src)?)?;
                pr.doc.revision += 1;
                let _ = project::save(pr, &json!({"preview": false, "reason": ctx.origin.as_str()}));
                Ok(())
            })?;
            Ok(out)
        }
        "asset.remove" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let aid = util::req_s(p, "id")?.to_string();
            let asset = doc.asset(&aid).cloned().ok_or_else(|| Fail::new("ASSET_NOT_FOUND", aid.clone()))?;
            run_cmd(p, ctx, commands::Cmd::AssetDel { id: aid, asset })
        }
        "asset.thumb" => {
            let doc = get_doc(p, ctx)?;
            let pr = project::ensure_open(&dir, &json!({"p":pid(p)}))?;
            let path = assets::thumb(&doc, &pr.dir, util::req_s(p, "id")?, util::f(p, "w").unwrap_or(256.0) as u32)?;
            Ok(json!({"path":util::short(&path, Some(&dir)),"url":format!("/asset?path={}", util::short(&path, Some(&dir)))}))
        }
        "asset.missing" => {
            let doc = get_doc(p, ctx)?;
            let pr = project::ensure_open(&dir, &json!({"p":pid(p)}))?;
            Ok(Value::Array(assets::missing(&doc, &pr.dir)?))
        }

        // ------------------------------------------------------ canvas
        "canvas.add" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let b = board_id(&doc, p)?;
            let node = build_node(p, &doc, &b)?;
            run_cmd(p, ctx, commands::Cmd::NodeAdd { node, board: b, at: util::f(p, "at").map(|x| x as usize) })
        }
        "canvas.add_many" | "poster.nodes" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let b = board_id(&doc, p)?;
            let mut cmds = vec![];
            for spec in util::arr(p, "nodes") {
                let merged = merge_val(&spec, &json!({"b":b}));
                cmds.push(commands::Cmd::NodeAdd { node: build_node(&merged, &doc, &b)?, board: b.clone(), at: util::f(&spec, "at").map(|x| x as usize) });
            }
            run_cmd(p, ctx, commands::Cmd::Composite { cmds })
        }
        "canvas.update" | "canvas.set" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let patch = p.get("s").or_else(|| p.get("patch")).or_else(|| p.get("style")).cloned().unwrap_or_else(|| {
                let mut o = p.as_object().cloned().unwrap_or_default();
                for k in ["p", "id", "ids", "b", "rev", "force", "solve", "fields", "pretty"] {
                    o.remove(k);
                }
                Value::Object(o)
            });
            if let Some(ids) = p.get("ids") {
                let v: Vec<String> = ids.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                return run_cmd(p, ctx, commands::Cmd::NodesSet { ids: v, patch });
            }
            let id = node_id(&doc, p)?;
            run_cmd(p, ctx, commands::Cmd::NodeSet { id, patch })
        }
        "canvas.text" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let id = node_id(&doc, p)?;
            run_cmd(p, ctx, commands::Cmd::Text { id, content: util::req_s(p, "content").or_else(|_| util::req_s(p, "v"))?.to_string() })
        }
        "canvas.image" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let id = node_id(&doc, p)?;
            run_cmd(p, ctx, commands::Cmd::NodeSet { id, patch: json!({"image":{"asset": util::req_s(p,"asset")?}}) })
        }
        "canvas.transform" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let id = node_id(&doc, p)?;
            let old = doc.node(&id).map(|n| n.box_).unwrap_or_default();
            let x = util::f(p, "x").unwrap_or(old.x);
            let y = util::f(p, "y").unwrap_or(old.y);
            let w = util::f(p, "w").unwrap_or(old.w);
            let h = util::f(p, "h").unwrap_or(old.h);
            let mut r = run_cmd(p, ctx, commands::Cmd::Transform { id: id.clone(), x, y, w, h })?;
            if let (Some(rot), Some(o)) = (util::f(p, "rot"), r.as_object_mut()) {
                o.insert("rot".into(), json!(rot));
                let _ = run_cmd(p, ctx, commands::Cmd::NodeSet { id, patch: json!({"rotation":rot}) });
            }
            Ok(r)
        }
        "canvas.style" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let id = node_id(&doc, p)?;
            let mut patch = serde_json::Map::new();
            for k in ["fill", "stroke", "radius", "color", "fx"] {
                if let Some(v) = p.get(k) {
                    let key = if k == "fx" { "fx" } else { "style" };
                    if key == "style" {
                        let mut st = patch.get("style").and_then(|x| x.as_object()).cloned().unwrap_or_default();
                        st.insert(k.into(), v.clone());
                        patch.insert("style".into(), Value::Object(st));
                    } else {
                        patch.insert(key.into(), v.clone());
                    }
                }
            }
            for k in ["opacity", "blend", "visible", "locked", "rotation", "name", "z_role"] {
                if let Some(v) = p.get(k) {
                    patch.insert(k.into(), v.clone());
                }
            }
            run_cmd(p, ctx, commands::Cmd::NodeSet { id, patch: Value::Object(patch) })
        }
        "canvas.reorder" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let id = node_id(&doc, p)?;
            let b = board_id(&doc, p)?;
            run_cmd(p, ctx, commands::Cmd::NodeMove { id, board: b, to: util::f(p, "to").unwrap_or(0.0) as usize, parent: util::ps(p, "parent").map(String::from) })
        }
        "canvas.group" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let b = board_id(&doc, p)?;
            let ids: Vec<String> = util::arr(p, "ids").into_iter().filter_map(|v| v.as_str().map(String::from)).collect();
            run_cmd(p, ctx, commands::Cmd::Group { ids, board: b })
        }
        "canvas.ungroup" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let id = node_id(&doc, p)?;
            run_cmd(p, ctx, commands::Cmd::Ungroup { id })
        }
        "canvas.duplicate" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let ids: Vec<String> = util::arr(p, "ids")
                .into_iter()
                .filter_map(|v| v.as_str().map(String::from))
                .chain(util::ps(p, "id").map(String::from))
                .collect();
            run_cmd(p, ctx, commands::Cmd::Duplicate { ids })
        }
        "canvas.delete" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let ids: Vec<String> = util::arr(p, "ids")
                .into_iter()
                .filter_map(|v| v.as_str().map(String::from))
                .chain(util::ps(p, "id").map(String::from))
                .collect();
            let mut out = json!([]);
            for id in ids {
                let id = node_id(&doc, &json!({"id":id}))?;
                out = run_cmd(p, ctx, commands::Cmd::NodeDel { id })?;
            }
            Ok(out)
        }
        "canvas.align" => {
            ensure(p, ctx)?;
            let mut doc = get_doc(p, ctx)?;
            let ids: Vec<String> = util::arr(p, "ids").into_iter().filter_map(|v| v.as_str().map(String::from)).collect();
            let changed = layout::align(&mut doc, &ids, util::ps(p, "axis").unwrap_or("x"), util::ps(p, "how").unwrap_or("center"))?;
            commit_patch(p, ctx, &doc, &changed)
        }
        "canvas.distribute" => {
            ensure(p, ctx)?;
            let mut doc = get_doc(p, ctx)?;
            let ids: Vec<String> = util::arr(p, "ids").into_iter().filter_map(|v| v.as_str().map(String::from)).collect();
            let changed = layout::distribute(&mut doc, &ids, util::ps(p, "axis").unwrap_or("x"))?;
            commit_patch(p, ctx, &doc, &changed)
        }
        "canvas.batch" => {
            ensure(p, ctx)?;
            let raw = util::arr(p, "cmds");
            let mut cmds = Vec::with_capacity(raw.len());
            for c in raw {
                cmds.push(serde_json::from_value::<commands::Cmd>(c)?);
            }
            run_cmd(p, ctx, commands::Cmd::Composite { cmds })
        }

        // ------------------------------------------------------ layout
        "layout.apply" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let id = node_id(&doc, p)?;
            run_cmd(p, ctx, commands::Cmd::LayoutIntent {
                id,
                intent: p.get("intent").cloned().unwrap_or_else(|| json!({})),
                mode: util::ps(p, "mode").unwrap_or("constraint").to_string(),
            })
        }
        "layout.set_mode" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let id = node_id(&doc, p)?;
            run_cmd(p, ctx, commands::Cmd::NodeSet { id, patch: json!({"layout":{"mode": util::req_s(p,"mode")?}}) })
        }
        "layout.resolve" | "layout.auto" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let b = match util::ps(p, "b") {
                Some(_) => Some(board_id(&doc, p)?),
                None => None,
            };
            let mut rep: Value = json!({});
            session::with_project(&pid(p), |pr| {
                let r = layout::solve(&mut pr.doc, b.as_deref())?;
                // A solve that moved nothing must not burn a revision: an
                // `expectedRevision` client would see a conflict for a no-op.
                if !r.changed.is_empty() {
                    pr.doc.revision += 1;
                    let _ = project::save(pr, &json!({"preview": false, "reason": ctx.origin.as_str()}));
                }
                rep = json!({"ok":true,"rev":pr.doc.revision,"changed":r.changed,"conflicts":r.conflicts.iter().map(conflict_value).collect::<Vec<_>>(),"passes":r.passes});
                Ok(())
            })?;
            Ok(rep)
        }
        "layout.explain" => {
            let doc = get_doc(p, ctx)?;
            let id = node_id(&doc, p)?;
            layout::explain(&doc, &id)
        }
        "layout.reflow" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let b = board_id(&doc, p)?;
            let (w, h) = size_of(p)?.ok_or_else(|| Fail::new("INVALID_SCHEMA", "need w,h or preset or ratio"))?;
            run_cmd(p, ctx, commands::Cmd::Reflow { board: b, w, h, mode: util::ps(p, "mode").unwrap_or("reflow").to_string() })
        }
        "layout.lock" => {
            ensure(p, ctx)?;
            let doc = get_doc(p, ctx)?;
            let id = node_id(&doc, p)?;
            let pin = util::opt_b(p, "pinned", true);
            run_cmd(p, ctx, commands::Cmd::NodeSet { id, patch: json!({"layout":{"pinned":pin}}) })
        }

        // ------------------------------------------------------ inspect
        "inspect.validate" | "validate" => {
            let doc = get_doc(p, ctx)?;
            let pr = project::ensure_open(&dir, &json!({"p":pid(p)}))?;
            inspect::validate(&doc, &pr.dir, p)
        }
        "inspect.document" => {
            let doc = get_doc(p, ctx)?;
            Ok(inspect::diff_docs(&Document::default(), &doc))
        }
        "inspect.node" => {
            let doc = get_doc(p, ctx)?;
            let id = node_id(&doc, p)?;
            Ok(inspect::geometry(&doc, &[id]).into_iter().next().unwrap_or(Value::Null))
        }
        "inspect.geometry" => {
            let doc = get_doc(p, ctx)?;
            let ids: Vec<String> = util::arr(p, "ids").into_iter().filter_map(|v| v.as_str().map(String::from)).collect();
            Ok(Value::Array(inspect::geometry(&doc, &ids)))
        }
        "inspect.text" => {
            let doc = get_doc(p, ctx)?;
            let id = node_id(&doc, p)?;
            inspect::text_metrics(&doc, &id)
        }
        "inspect.conflicts" => {
            let doc = get_doc(p, ctx)?;
            let b = board_id(&doc, p)?;
            Ok(Value::Array(inspect::conflicts(&doc, &b)))
        }
        "inspect.assets" => {
            let doc = get_doc(p, ctx)?;
            Ok(Value::Array(inspect::asset_usage(&doc)))
        }
        "inspect.diff" => {
            let doc = get_doc(p, ctx)?;
            match (util::ps(p, "a"), util::ps(p, "b2")) {
                (Some(x), Some(y)) => {
                    let pr = project::ensure_open(&dir, &json!({"p": pid(p)}))?;
                    let a = project::load_snapshot(&dir, &pr.doc.project_id, x)?;
                    let b = project::load_snapshot(&dir, &pr.doc.project_id, y)?;
                    Ok(inspect::diff_docs(&a, &b))
                }
                _ => match util::ps(p, "with") {
                    Some(other) => {
                        let od = project::open(&dir, other)?;
                        Ok(inspect::diff_docs(&od.doc, &doc))
                    }
                    None => Ok(json!({"ok":true,"rev":doc.revision,"note":"pass with=<project> or a=<snap> b2=<snap>"})),
                },
            }
        }
        "inspect.fix" => {
            let doc = get_doc(p, ctx)?;
            let pr = project::ensure_open(&dir, &json!({"p":pid(p)}))?;
            let rep = inspect::validate(&doc, &pr.dir, p)?;
            // `validate` already embeds each suggested fix as a serialised `Cmd`,
            // so the repair path round-trips through the same command format an
            // agent would send by hand.
            let cmds: Vec<commands::Cmd> = rep
                .get("issues")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|i| i.get("f").cloned())
                .filter_map(|c| serde_json::from_value::<commands::Cmd>(c).ok())
                .collect();
            if util::opt_b(p, "dry", false) {
                return Ok(json!({"cmds":cmds,"n":cmds.len()}));
            }
            run_cmd(p, ctx, commands::Cmd::Composite { cmds })
        }

        // ------------------------------------------------------ render
        "render.svg" => {
            let doc = get_doc(p, ctx)?;
            let o = svg_opts(&doc, p, ctx)?;
            let pr = project::ensure_open(&dir, &json!({"p": pid(p)})).ok();
            let d = pr.as_ref().map(|x| x.dir.clone());
            let (mut s, warnings) = match util::ps(p, "b") {
                Some(_) => {
                    let one = board_id(&doc, p)?;
                    svg::compile_w(&doc, &o, d.as_deref(), Some(&one))?
                }
                None => svg::compile_w(&doc, &o, d.as_deref(), None)?,
            };
            // A render that quietly dropped a layer is the worst possible outcome
            // for an agent, so the degradations travel with the picture.
            if !warnings.is_empty() && !s.contains("agentcanvas:warn") {
                s.push_str(&format!("\n<!-- agentcanvas:warn {} -->\n", warnings.join(" | ")));
            }
            match util::ps(p, "o") {
                Some(name) => {
                    let path = util::outp(name, Some(&dir));
                    util::check_writable(&path, Some(&dir))?;
                    util::write(&path, s.as_bytes())?;
                    Ok(json!({"path":util::short(&path,Some(&dir)),"bytes":s.len(),"sha":util::sha256_bytes(s.as_bytes()),"warnings":warnings}))
                }
                None => Ok(Value::String(s)),
            }
        }
        "render.preview" => {
            let doc = get_doc(p, ctx)?;
            let pr = project::ensure_open(&dir, &json!({"p":pid(p)}))?;
            let mut o = svg_opts(&doc, p, ctx)?;
            // The rasteriser runs in this process, so it can only paint what it can
            // read: an `asset?id=` reference resolves for the webview and for nobody
            // else, and the preview PNG used to ship without a single photo in it.
            o.inline_assets = true;
            let svg_s = svg::compile_at(&doc, &o, Some(&pr.dir))?;
            let scale = util::f(p, "sc").unwrap_or(0.5);
            let (w, h, rgba) = render::raster(&svg_s, scale)?;
            let bytes = render::encode_png(w, h, &rgba)?;
            // Two names on purpose: `preview.png` is what the window shows, and the
            // revision-stamped copy is what `image.compare` diffs two states with.
            // Overwriting a single file would make "did my edit do anything?" unanswerable.
            let rev = doc.revision;
            let stamped = pr.dir.join(format!("preview-r{rev}.png"));
            util::write(&stamped, &bytes)?;
            let latest = pr.dir.join("preview.png");
            util::write(&latest, &bytes)?;
            Ok(json!({"path":util::short(&stamped,Some(&dir)),"latest":util::short(&latest,Some(&dir)),
                      "url":format!("/file?path={}", util::short(&latest,Some(&dir))),
                      "w":w,"h":h,"bytes":bytes.len(),"rev":rev,"sha":util::short_hash(&util::sha256_bytes(&bytes))}))
        }
        "render.export" | "render.export_batch" => {
            let doc = get_doc(p, ctx)?;
            let pr = project::ensure_open(&dir, &json!({"p":pid(p)}))?;
            let boards: Vec<String> = if m.ends_with("batch") {
                util::arr(p, "boards").into_iter().filter_map(|v| v.as_str().map(String::from)).collect()
            } else {
                match util::ps(p, "b") {
                    Some(_) => vec![board_id(&doc, p)?],
                    None => doc.artboards.iter().filter(|a| a.visible).map(|a| a.id.clone()).collect(),
                }
            };
            let fmt = util::ps(p, "f").unwrap_or("png").to_string();
            let o = render::RenderOpts {
                scale: util::f(p, "sc").unwrap_or(1.0),
                format: fmt_of(&fmt)?,
                quality: util::f(p, "q").unwrap_or(92.0) as u8,
                background: util::ps(p, "bg").map(String::from).or_else(|| if util::opt_b(p, "transparent", false) { Some("none".into()) } else { None }),
                width: util::f(p, "w").map(|x| x as u32),
                height: util::f(p, "h").map(|x| x as u32),
            };
            let dir_out = match util::ps(p, "o") {
                Some(s) => util::outp(s, Some(&dir)),
                None => pr.dir.join("exports"),
            };
            util::check_writable(&dir_out, Some(&dir))?;
            let spec = p.clone();
            let mut files: Vec<Value> = vec![];
            if m.ends_with("batch") {
                for s in util::arr(p, "specs") {
                    let mut oo = o.clone();
                    if let Some(f) = util::ps(&s, "f") {
                        oo.format = fmt_of(f)?;
                    }
                    oo.scale = util::f(&s, "sc").unwrap_or(oo.scale);
                    oo.width = util::f(&s, "w").map(|x| x as u32).or(oo.width);
                    oo.height = util::f(&s, "h").map(|x| x as u32).or(oo.height);
                    let r = render::export_with(&doc, &dir_out, util::ps(&s, "suffix").unwrap_or("out"), &oo, &boards, Some(&pr.dir))?;
                    files.extend(r.iter().map(|x| serde_json::to_value(x)).collect::<Result<Vec<_>, _>>()?);
                }
            } else {
                let _ = spec;
                let r = render::export_with(&doc, &dir_out, util::ps(p, "name").unwrap_or(&doc.name), &o, &boards, Some(&pr.dir))?;
                files = r.iter().map(|x| serde_json::to_value(x)).collect::<Result<Vec<_>, _>>()?;
            }
            Ok(json!({"ok":true,"files":files,"n":files.len()}))
        }
        "render.diff" => {
            let a = util::inp(util::req_s(p, "a")?, Some(&dir))?;
            let b = util::inp(util::req_s(p, "b2").or_else(|_| util::req_s(p, "b"))?, Some(&dir))?;
            render::diff(&a, &b)
        }

        // ------------------------------------------------------ history
        "history.undo" => history::undo(&dir, &with_origin(p, ctx)),
        "history.redo" => history::redo(&dir, &with_origin(p, ctx)),
        "history.list" => {
            // Without this the answer depends on which door asked: a fresh process
            // would report an empty trail for a project that has plenty of history
            // journaled on disk. `ensure_open` hydrates it.
            project::ensure_open(&dir, &json!({"p": pid(p)}))?;
            Ok(Value::Array(history::list(&dir, &pid(p), util::f(p, "n").unwrap_or(30.0) as usize)))
        }
        "history.checkpoint" => history::checkpoint(&dir, &with_origin(p, ctx)),
        "history.compare" => {
            let pr = project::ensure_open(&dir, &json!({"p": pid(p)}))?;
            let a = project::load_snapshot(&dir, &pr.doc.project_id, util::req_s(p, "from")?)?;
            let b = project::load_snapshot(&dir, &pr.doc.project_id, util::req_s(p, "to")?)?;
            Ok(inspect::diff_docs(&a, &b))
        }
        "history.rollback" => history::rollback(&dir, &with_origin(p, ctx)),

        // ------------------------------------------------------ templates
        "template.save" => {
            let doc = get_doc(p, ctx)?;
            let b = board_id(&doc, p)?;
            // Templates are shared across projects, so they live in the workspace,
            // not inside the project they came from — otherwise `template.list`
            // could never find them again.
            templates::save_from_board(&doc, &dir, &b, util::ps(p, "name").unwrap_or("tpl"))
        }
        "template.list" => {
            let mut v = Value::Array(templates::list(&dir)?);
            if let Value::Array(a) = &mut v {
                a.push(json!({"builtin": templates::builtin()}));
            }
            Ok(v)
        }
        "template.use" => {
            ensure(p, ctx)?;
            let mut out = Value::Null;
            let tpl = util::req_s(p, "tpl")?.to_string();
            let pidv = pid(p);
            session::with_project(&pidv, |pr| {
                out = serde_json::to_value(templates::instantiate(&dir, &tpl, &mut pr.doc, p)?)?;
                pr.doc.revision += 1;
                let _ = project::save(pr, &json!({"preview": false, "reason": ctx.origin.as_str()}));
                Ok(())
            })?;
            Ok(json!({"ok":true,"ids":out}))
        }
        "template.vars" | "template.apply_variables" => {
            ensure(p, ctx)?;
            let vars = p.get("vars").cloned().unwrap_or_else(|| json!({}));
            let mut out = Value::Null;
            let pidv = pid(p);
            session::with_project(&pidv, |pr| {
                out = serde_json::to_value(templates::apply_variables(&mut pr.doc, &vars)?)?;
                let _ = layout::solve(&mut pr.doc, None);
                pr.doc.revision += 1;
                let _ = project::save(pr, &json!({"preview": false, "reason": ctx.origin.as_str()}));
                Ok(())
            })?;
            Ok(json!({"ok":true,"changed":out}))
        }
        "template.variants" => {
            ensure(p, ctx)?;
            let mut out = Value::Null;
            let pidv = pid(p);
            let pp = p.clone();
            session::with_project(&pidv, |pr| {
                out = templates::create_variants(&mut pr.doc, &pp)?;
                let _ = layout::solve(&mut pr.doc, None);
                pr.doc.revision += 1;
                let _ = project::save(pr, &json!({"preview": false, "reason": ctx.origin.as_str()}));
                Ok(())
            })?;
            Ok(out)
        }
        "template.builtin" => Ok(Value::Array(templates::builtin().iter().map(|(n, d)| json!({"tpl":n,"d":d})).collect())),

        // ------------------------------------------------------ poster (one call)
        "poster" => {
            let mut spec = p.get("spec").cloned().unwrap_or_else(|| p.clone());
            if let Some(o) = spec.as_object_mut() {
                o.entry("origin".to_string()).or_insert_with(|| json!(ctx.origin.as_str()));
            }
            templates::poster(&dir, &spec)
        }

        // ------------------------------------------------------ image ops
        "image.probe" => {
            let f = util::inp(util::req_s(p, "path").or_else(|_| util::req_s(p, "in"))?, Some(&dir))?;
            imageops::probe(&f)
        }
        "image.op" => image_op(p, ctx, false),
        "image.chain" => image_op(p, ctx, true),
        "image.batch" => {
            let list: Vec<String> = list_of(p, "paths").into_iter().filter_map(|v| v.as_str().map(String::from)).collect();
            if list.is_empty() {
                return Err(Fail::new("INVALID_SCHEMA", "paths[] is empty"));
            }
            let steps: Vec<Value> = match p.get("chain") {
                Some(Value::String(s)) => imageops::parse_chain(s)?,
                Some(v) => v.as_array().cloned().unwrap_or_default(),
                None => vec![],
            };
            let preset = util::ps(p, "preset").map(String::from);
            let out_dir = util::outp(util::ps(p, "out").unwrap_or("out"), Some(&dir));
            util::check_writable(&out_dir, Some(&dir))?;
            let fmt = util::ps(p, "fmt").map(String::from);
            let q = util::f(p, "q").unwrap_or(92.0) as u8;
            let root = dir.clone();
            let total = list.len();
            jobs::run::<Value>(&format!("image.batch {total}"), move |h| {
                let mut res: Vec<Value> = Vec::with_capacity(total);
                for (i, s) in list.iter().enumerate() {
                    if h.cancelled() {
                        res.push(json!({"i": i, "ok": false, "code": "CANCELLED", "e": "cancelled by user"}));
                        break;
                    }
                    let r = (|| -> R<Value> {
                        let src = util::inp(s, Some(&root))?;
                        let dst = out_dir.join(format!("{}.{}", util::stem(&src), fmt.clone().unwrap_or_else(|| util::ext(&src)).as_str()));
                        let img = imageops::load(&src)?;
                        let img = match &preset {
                            Some(_) => img,
                            None => imageops::run_chain(img, &steps)?,
                        };
                        imageops::save(&img, &dst, q)?;
                        let bytes = std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
                        Ok(json!({"i":i,"ok":true,"out":util::short(&dst,Some(&root)),"w":img.w,"h":img.h,"bytes":bytes}))
                    })();
                    let _ = preset;
                    res.push(r.unwrap_or_else(|e| json!({"i":i,"ok":false,"e":e.msg,"code":e.code,"hint":e.hint})));
                    h.progress((i + 1) as f64 / total.max(1) as f64, "file");
                }
                Ok(json!({"ok": res.iter().all(|x| x.get("ok") != Some(&json!(false))), "r": res}))
            })
        }
        "image.preset" => {
            let src = util::inp(util::req_s(p, "in")?, Some(&dir))?;
            let dst = util::outp(util::req_s(p, "out")?, Some(&dir));
            util::check_writable(&dst, Some(&dir))?;
            imageops::apply_preset(util::req_s(p, "name")?, &src, &dst)
        }
        "image.presets" => Ok(Value::Array(imageops::presets().iter().map(|(n, d)| json!({"name":n,"d":d})).collect())),
        "image.ops" => Ok(Value::Array(imageops::ops().iter().map(|(n, d)| json!({"op":n,"d":d})).collect())),
        "image.palette" => {
            let f = util::inp(util::req_s(p, "path").or_else(|_| util::req_s(p, "in"))?, Some(&dir))?;
            let img = imageops::load(&f)?;
            let n = util::f(p, "n").unwrap_or(6.0) as usize;
            Ok(json!({"colors": imageops::palette_of(&img, n).iter().map(|(hex, cov)| json!({"hex":hex,"pct":util::round(*cov*100.0,1)})).collect::<Vec<_>>()}))
        }
        "image.hash" => {
            let mut o = serde_json::Map::new();
            for s in util::arr(p, "paths") {
                let Some(st) = s.as_str() else { continue };
                let f = util::inp(st, Some(&dir))?;
                let img = imageops::load(&f)?;
                o.insert(util::short(&f, Some(&dir)), json!({"dhash": imageops::dhash(&img), "ahash": imageops::ahash(&img)}));
            }
            Ok(Value::Object(o))
        }
        "image.compare" => {
            let a = util::inp(util::req_s(p, "a")?, Some(&dir))?;
            let b = util::inp(util::req_s(p, "b2").or_else(|_| util::req_s(p, "b"))?, Some(&dir))?;
            render::diff(&a, &b)
        }

        // ------------------------------------------------------ text
        "text.measure" => {
            let s = util::req_s(p, "text")?.to_string();
            let family = text::Registry::shared().resolve(util::ps(p, "family").unwrap_or(""));
            let m = text::MeasureOpts {
                family: family.clone(),
                weight: util::f(p, "weight").unwrap_or(400.0) as u16,
                italic: util::opt_b(p, "italic", false),
                size: util::f(p, "size").unwrap_or(16.0),
                ls: util::f(p, "ls").unwrap_or(0.0),
                line_h: util::f(p, "lh").unwrap_or(0.0),
                max_w: util::f(p, "max_w").unwrap_or(f64::INFINITY),
                align: util::ps(p, "align").unwrap_or("left").to_string(),
                valign: util::ps(p, "valign").unwrap_or("top").to_string(),
            };
            let r = text::layout_text(&s, &m, util::ps(p, "mode").unwrap_or("fixed"));
            Ok(serde_json::to_value(&r)?)
        }
        "text.fonts" | "fonts" => {
            let q = util::ps(p, "q").unwrap_or("");
            let fams = text::Registry::shared().families();
            Ok(json!({"n":fams.len(),"families":fams.iter().filter(|f| q.is_empty() || f.to_lowercase().contains(&q.to_lowercase())).cloned().collect::<Vec<_>>()}))
        }
        "text.register" => {
            let f = util::inp(util::req_s(p, "path")?, Some(&dir))?;
            let fam = text::Registry::shared().register_file(&f)?;
            Ok(json!({"ok":true,"family":fam}))
        }

        other => Err(Fail::new("INVALID_SCHEMA", format!("unknown method \"{other}\""))
            .hint("run caps, or help for the 10-line cheat sheet")
            .details(json!({"near": nearest(other)}))),
    }
}

/// align/distribute computed boxes in a scratch copy; push them back as one
/// transaction so undo stays a single step.
fn commit_patch(p: &Value, ctx: &Ctx, doc: &Document, changed: &[String]) -> R<Value> {
    let mut cmds = vec![];
    for id in changed {
        if let Some(n) = doc.node(id) {
            cmds.push(commands::Cmd::Transform { id: id.clone(), x: n.box_.x, y: n.box_.y, w: n.box_.w, h: n.box_.h });
        }
    }
    run_cmd(p, ctx, commands::Cmd::Composite { cmds })
}

fn image_op(p: &Value, ctx: &Ctx, chained: bool) -> R<Value> {
    let dir = root_of(ctx);
    let src = util::inp(util::req_s(p, "in").or_else(|_| util::req_s(p, "path"))?, Some(&dir))?;
    let img = imageops::load(&src)?;
    let img = if chained {
        let steps: Vec<Value> = match p.get("chain") {
            Some(Value::String(s)) => imageops::parse_chain(s)?,
            Some(v) => v.as_array().cloned().unwrap_or_default(),
            None => vec![],
        };
        imageops::run_chain(img, &steps)?
    } else {
        let args = p.get("args").cloned().unwrap_or_else(|| p.clone());
        imageops::op(util::req_s(p, "op")?, img, &args)?
    };
    match util::ps(p, "out") {
        Some(name) => {
            let dst = util::outp(name, Some(&dir));
            util::check_writable(&dst, Some(&dir))?;
            imageops::save(&img, &dst, util::f(p, "q").unwrap_or(92.0) as u8)?;
            out_summary(&dst)
        }
        None => {
            let bg = util::ps(p, "bg").and_then(crate::doc::parse_color).map(|(r, g, b, _)| [r, g, b]).unwrap_or([255, 255, 255]);
            let (b, meta) = imageops::encode(&img, util::ps(p, "fmt").unwrap_or("png"), util::f(p, "q").unwrap_or(92.0) as u8, bg)?;
            Ok(json!({"w":img.w,"h":img.h,"bytes":b.len(),"b64": base64_encode(&b),"meta":meta}))
        }
    }
}

/// `artboard.duplicate {to:["1:1","4:5"]}` — copy a board and every node on it
/// into fresh ratios. Ids are rewritten inside parent/children and inside layout
/// intents; miss that and the copy solves against the original board's nodes.
fn duplicate_boards(p: &Value, ctx: &Ctx, src: &str) -> R<Value> {
    let ratios: Vec<String> = util::arr(p, "to")
        .into_iter()
        .filter_map(|v| v.as_str().map(String::from))
        .chain(util::ps(p, "ratio").map(String::from))
        .collect();
    if ratios.is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "to[] is empty").hint("artboard.duplicate {p,b,to:[\"1:1\",\"4:5\",\"9:16\"]}"));
    }
    let doc = get_doc(p, ctx)?;
    let ab = doc.artboard(src).cloned().ok_or_else(|| Fail::new("ARTBOARD_NOT_FOUND", src.to_string()))?;
    let nodes: Vec<crate::doc::Node> = doc.paint_order(src).iter().filter_map(|i| doc.node(i)).cloned().collect();
    let mut cmds: Vec<commands::Cmd> = vec![];
    let mut made: Vec<Value> = vec![];
    for r in &ratios {
        let (w, h) = geom::parse_ratio(r, ab.width)
            .ok_or_else(|| Fail::new("INVALID_SCHEMA", format!("bad ratio \"{r}\"")).hint("use 1:1, 16:9, 4:5 or 1080x1440"))?;
        let mut nb = ab.clone();
        nb.id = util::id("b");
        nb.name = format!("{} {r}", ab.name);
        nb.width = util::round(w, 1);
        nb.height = util::round(h, 1);
        nb.layers = vec![];
        nb.ratio = Some(r.clone());
        let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for n in &nodes {
            map.insert(n.id.clone(), util::id(n.kind.as_str()));
        }
        cmds.push(commands::Cmd::BoardAdd { board: nb.clone() });
        for (i, n) in nodes.iter().enumerate() {
            let mut c = n.clone();
            c.id = map.get(&n.id).cloned().unwrap_or_else(|| util::id(c.kind.as_str()));
            c.artboard = Some(nb.id.clone());
            c.parent = n.parent.as_ref().and_then(|x| map.get(x)).cloned();
            c.children = n.children.iter().filter_map(|x| map.get(x)).cloned().collect();
            c.layout.provenance = vec![];
            if let Some(intent) = c.layout.intent.take() {
                let mut iv = serde_json::to_value(&intent)?;
                remap_ids(&mut iv, &map);
                c.layout.intent = Some(serde_json::from_value(iv)?);
            }
            cmds.push(commands::Cmd::NodeAdd { node: c, board: nb.id.clone(), at: Some(i) });
        }
        made.push(json!({"b": nb.id, "name": nb.name, "w": nb.width, "h": nb.height, "ratio": r}));
    }
    run_cmd(p, ctx, commands::Cmd::Composite { cmds })?;
    for m in made.iter_mut() {
        if let Some(b) = m.get("b").and_then(|x| x.as_str()).map(String::from) {
            let key = b.clone();
            let _ = edit(p, ctx, |doc, _| {
                let r = layout::solve(doc, Some(&key))?;
                if let Some(o) = m.as_object_mut() {
                    o.insert("conflicts".into(), json!(r.conflicts.len()));
                }
                Ok(())
            });
        }
    }
    Ok(json!({"ok": true, "boards": made, "from": src}))
}

/// Rewrite node ids that appear as relation targets anywhere inside an intent.
fn remap_ids(v: &mut Value, map: &std::collections::HashMap<String, String>) {
    match v {
        Value::Object(o) => {
            for (k, val) in o.iter_mut() {
                if matches!(k.as_str(), "of" | "center_in" | "by") {
                    if let Some(s) = val.as_str() {
                        if let Some(n) = map.get(s) {
                            *val = Value::String(n.clone());
                        }
                    }
                } else {
                    remap_ids(val, map);
                }
            }
        }
        Value::Array(a) => {
            for (i, val) in a.iter_mut().enumerate() {
                if let Some(s) = val.as_str() {
                    if let Some(n) = map.get(s) {
                        *val = Value::String(n.clone());
                    }
                } else {
                    remap_ids(val, map);
                }
                let _ = i;
            }
        }
        _ => {}
    }
}

fn base64_encode(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn out_summary(p: &Path) -> R<Value> {
    let meta = std::fs::metadata(p).map_err(|_| Fail::new("IO", format!("write failed: {}", p.display())))?;
    let sha = util::sha256_file(p)?;
    Ok(json!({"out":util::short(p, None),"bytes":meta.len(),"sha":sha,"w":if let Ok(i)=imageops::load(p) {json!([i.w,i.h])} else {Value::Null}}))
}

// ------------------------------------------------------------------ builders

fn size_of(p: &Value) -> R<Option<(f64, f64)>> {
    if let Some(name) = util::ps(p, "preset") {
        return match geom::preset(name) {
            Some((w, h, _)) => Ok(Some((w, h))),
            None => Err(Fail::new("INVALID_SCHEMA", format!("unknown preset \"{name}\""))
                .details(json!({"presets": geom::presets().iter().map(|x| x.0).collect::<Vec<_>>()}))),
        };
    }
    if let Some(r) = util::ps(p, "ratio") {
        let base = util::f(p, "base").unwrap_or(1080.0);
        return match geom::parse_ratio(r, base) {
            Some((w, h)) => Ok(Some((util::round(w, 1), util::round(h, 1)))),
            None => Err(Fail::new("INVALID_SCHEMA", format!("bad ratio \"{r}\"")).hint("use 1:1, 16:9, 4:5 or 1080x1440")),
        };
    }
    match (util::f(p, "w"), util::f(p, "h")) {
        (Some(w), Some(h)) => Ok(Some((w, h))),
        _ => Ok(None),
    }
}

fn new_board(p: &Value, w: f64, h: f64) -> R<crate::doc::Artboard> {
    Ok(crate::doc::Artboard {
        id: util::id("b"),
        name: util::ps(p, "board_name").unwrap_or("画板").to_string(),
        width: w,
        height: h,
        background: Some(match util::ps(p, "background") {
            Some(c) => crate::doc::Paint::solid(&expand_color(&crate::doc::Tokens::default(), c)),
            None => crate::doc::Paint::None,
        }),
        visible: true,
        layers: vec![],
        safe_margin: util::f(p, "safe"),
        guides: vec![],
        ratio: Some(format!("{}:{}", util::round(w / gcd(w, h), 2), util::round(h / gcd(w, h), 2))),
        role: util::ps(p, "role").map(String::from),
        template_id: None,
        vars: Default::default(),
    })
}

fn gcd(a: f64, b: f64) -> f64 {
    let (mut x, mut y) = (a.round().abs() as i64, b.round().abs() as i64);
    while y != 0 {
        let t = y;
        y = x % y;
        x = t;
    }
    (if x == 0 { 1 } else { x }) as f64
}

/// `$token` colours expand against the document palette before anything reads them.
fn expand_color(tokens: &crate::doc::Tokens, s: &str) -> String {
    tokens.expand(s)
}

fn build_node(p: &Value, doc: &Document, board: &str) -> R<crate::doc::Node> {
    let kind = crate::doc::NodeType::parse(util::ps(p, "t").or_else(|| util::ps(p, "type")).unwrap_or("rect"))
        .ok_or_else(|| Fail::new("INVALID_SCHEMA", format!("unknown node type \"{}\"", util::ps(p, "t").unwrap_or(""))))?;
    let ab = doc.artboard(board).map(|a| (a.width, a.height)).unwrap_or((1080.0, 1440.0));
    let mut r = crate::geom::Rect::new(
        util::f(p, "x").unwrap_or(0.0),
        util::f(p, "y").unwrap_or(0.0),
        util::f(p, "w").unwrap_or(ab.0 / 2.0),
        util::f(p, "h").unwrap_or(ab.1 / 4.0),
    );
    let id = util::ps(p, "id").map(String::from).unwrap_or_else(|| util::id(kind.as_str().get(..2).unwrap_or("n")));
    let mut n = crate::doc::Node::new(&id, kind, r);
    // `canvas.add t=image asset=a w=700` means "700 wide, correct height", not
    // "700 wide and whatever default height the box had".
    if kind == crate::doc::NodeType::Image {
        if let Some(asset) = util::ps(p, "asset").and_then(|a| doc.asset(a)) {
            if asset.width > 0.0 && asset.height > 0.0 {
                let asp = asset.width / asset.height;
                match (util::f(p, "w"), util::f(p, "h")) {
                    (Some(_), None) => r.h = r.w / asp,
                    (None, Some(_)) => r.w = r.h * asp,
                    _ => {}
                }
                n.box_ = r;
            }
        }
    }
    n.name = util::ps(p, "name").map(String::from).unwrap_or_else(|| kind.as_str().to_string());
    n.artboard = Some(board.to_string());
    n.rotation = util::f(p, "rot").unwrap_or(0.0);
    n.opacity = util::f(p, "opacity").unwrap_or(1.0);
    n.visible = util::opt_b(p, "visible", true);
    n.locked = util::opt_b(p, "locked", false);
    n.blend = util::ps(p, "blend").map(String::from);
    n.z_role = util::ps(p, "z_role").map(String::from);
    if let Some(o) = p.get("style") {
        n.style = Some(serde_json::from_value(o.clone())?);
    }
    if let Some(o) = p.get("fx") {
        n.fx = Some(serde_json::from_value(o.clone())?);
    }
    if let Some(o) = p.get("l").or_else(|| p.get("layout")) {
        n.layout = serde_json::from_value(o.clone())?;
    }
    if let Some(intent) = p.get("intent") {
        n.layout.mode = crate::doc::LayoutMode::Constraint;
        n.layout.intent = Some(serde_json::from_value(intent.clone())?);
    }
    match kind {
        crate::doc::NodeType::Text => {
            let content = util::ps(p, "text").or_else(|| util::ps(p, "content")).unwrap_or("文字").to_string();
            let mut t = crate::doc::TextSpec {
                content,
                family: util::ps(p, "family").map(String::from),
                size: Some(util::f(p, "size").unwrap_or(48.0)),
                weight: Some(util::f(p, "weight").unwrap_or(400.0) as u16),
                color: Some(util::ps(p, "color").unwrap_or("#1C1C1C").to_string()),
                align: Some(util::ps(p, "align").unwrap_or("left").to_string()),
                valign: Some(util::ps(p, "valign").unwrap_or("top").to_string()),
                line_ratio: Some(util::f(p, "lh").unwrap_or(1.35)),
                letter_spacing: Some(util::f(p, "ls").unwrap_or(0.0)),
                box_mode: Some(util::ps(p, "box_mode").unwrap_or("auto-height").to_string()),
                overflow: Some(util::ps(p, "overflow").unwrap_or("shrink").to_string()),
                ..Default::default()
            };
            if let Some(fr) = util::ps(p, "font_ref") {
                t.font_ref = Some(fr.to_string());
            }
            n.text = Some(t);
        }
        crate::doc::NodeType::Image => {
            let a = util::ps(p, "asset").unwrap_or("");
            let found = doc.asset(a).map(|x| x.id.clone()).ok_or_else(|| {
                Fail::new("ASSET_NOT_FOUND", format!("no asset \"{a}\"")).hint("asset.import {path} first, then pass the returned id")
            })?;
            n.image = Some(crate::doc::ImageSpec {
                asset: found,
                fit: geom::fit_of(util::ps(p, "fit").unwrap_or("cover")).unwrap_or_default(),
                crop: p.get("crop").and_then(|v| serde_json::from_value(v.clone()).ok()),
                brightness: util::f(p, "brightness"),
                contrast: util::f(p, "contrast"),
                saturate: util::f(p, "saturate"),
                hue: util::f(p, "hue"),
                blur: util::f(p, "blur"),
                grayscale: None,
                tint: util::ps(p, "tint").map(String::from),
                position: util::ps(p, "pos").map(String::from),
                opacity: util::f(p, "img_opacity"),
                tile: util::f(p, "tile"),
            });
        }
        crate::doc::NodeType::Path | crate::doc::NodeType::Polygon | crate::doc::NodeType::Svg | crate::doc::NodeType::Line => {
            let v = p.get("vector").cloned().unwrap_or_else(|| {
                let mut o = serde_json::Map::new();
                if let Some(d) = util::ps(p, "d") {
                    o.insert("d".into(), json!(d));
                }
                if let Some(s) = util::f(p, "sides") {
                    o.insert("sides".into(), json!(s as u32));
                }
                if let Some(pts) = p.get("points") {
                    o.insert("points".into(), pts.clone());
                }
                if let Some(a) = util::ps(p, "asset") {
                    o.insert("asset".into(), json!(a));
                }
                if let Some(i) = util::ps(p, "inline") {
                    o.insert("inline".into(), json!(i));
                }
                Value::Object(o)
            });
            n.vector = Some(serde_json::from_value(v)?);
        }
        _ => {}
    }
    if let Some(c) = p.get("clip") {
        n.clip = Some(serde_json::from_value(c.clone())?);
    }
    Ok(n)
}

fn svg_opts(doc: &Document, p: &Value, ctx: &Ctx) -> R<svg::SvgOpts> {
    let boards: Vec<String> = match util::ps(p, "b") {
        Some(_) => vec![board_id(doc, p)?],
        None => match p.get("boards") {
            Some(v) => v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
            None => doc.artboards.iter().filter(|a| a.visible).map(|a| a.id.clone()).collect(),
        },
    };
    let preview = util::opt_b(p, "preview", false);
    let _ = ctx;
    Ok(svg::SvgOpts {
        boards,
        inline_assets: !preview && util::opt_b(p, "embed", true),
        asset_url: if preview { Some("/asset?id".into()) } else { None },
        font_url: if preview { Some("/font".into()) } else { None },
        embed_fonts: util::opt_b(p, "embed_fonts", !preview),
        show_guides: util::opt_b(p, "guides", false),
        omit_ids: util::arr(p, "omit").into_iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        scale: util::f(p, "scale").unwrap_or(1.0),
    })
}

fn fmt_of(s: &str) -> R<render::Format> {
    Ok(match s.to_lowercase().as_str() {
        "png" => render::Format::Png,
        "jpg" | "jpeg" => render::Format::Jpeg,
        "webp" => render::Format::Webp,
        "svg" => render::Format::Svg,
        "pdf" => render::Format::Pdf,
        other => return Err(Fail::new("UNSUPPORTED", format!("format \"{other}\"")).hint("png | jpg | webp | svg | pdf")),
    })
}

fn merge_val(a: &Value, b: &Value) -> Value {
    let mut o = a.as_object().cloned().unwrap_or_default();
    if let Some(bm) = b.as_object() {
        for (k, v) in bm {
            o.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    Value::Object(o)
}

fn summary(doc: &Document) -> Value {
    json!({
        "p": doc.project_id, "rev": doc.revision, "name": doc.name,
        "boards": doc.artboards.iter().map(|a| json!({"b":a.id,"name":a.name,"w":a.width,"h":a.height,"n":a.layers.len()})).collect::<Vec<_>>(),
        "assets": doc.assets.len(), "nodes": doc.nodes.len(),
    })
}

fn build_tag() -> String {
    format!("{} {}", crate::version(), std::env::consts::OS)
}

// ------------------------------------------------------------------ self-description

/// The one call an agent should make first. Kept under ~5 KB on purpose.
pub fn caps() -> R<Value> {
    Ok(json!({
        "name": "AgentCanvas",
        "ver": crate::version(),
        "schema": crate::doc::SCHEMA_VERSION,
        "doc": "one document, three doors: desktop UI, HTTP, MCP all call the same methods",
        "call": {"http":"POST /rpc {\"m\":..,\"p\":..}","cli":"agentcanvas <m> k=v | agentcanvas rpc '{...}'","mcp":"agentcanvas mcp"},
        "read_this_first": "caps is the whole surface; system.tools gives JSON schemas, help gives 10 lines",
        "methods": methods().iter().map(|(n, d)| format!("{n}  {d}")).collect::<Vec<_>>(),
        "image_ops": imageops::ops().iter().map(|(n,_)| *n).collect::<Vec<_>>(),
        "image_presets": imageops::presets().iter().map(|(n,_)| *n).collect::<Vec<_>>(),
        "templates": templates::builtin().iter().map(|(n,_)| *n).collect::<Vec<_>>(),
        "presets": geom::presets().iter().map(|(k, w, h, d)| json!([k, w, h, d])).collect::<Vec<_>>(),
        "keys": {"p":"project","b":"artboard","id":"node","rev":"expectedRevision","t":"node type","s":"style","l":"layout","o":"out path","sc":"scale","q":"quality","f":"format","in":"input path","in[]":"list","chain":"image op list","intent":"semantic layout intent","fields":"trim result","pretty":"indent"},
        "intents": intent_keys(),
        "node_types": ["text","image","rect","ellipse","line","polygon","path","group","frame","svg"],
        "limits": {"max_px": 60_000_000, "max_in_bytes": 200_000_000, "job_timeout_s": 600},
        "codes": crate::error::CODES.iter().map(|(c,d)| format!("{c}: {d}")).collect::<Vec<_>>(),
        "paths": {"root": util::short(&util::root(None), None), "projects": "projects/<id>/", "exports": "projects/<id>/exports/"},
        "port": crate::http_port(),
        "fonts": text::Registry::shared().families().len(),
        "loop": "inspect.validate -> canvas.batch -> render.preview -> image.compare -> render.export",
    }))
}

pub fn intent_keys() -> Vec<&'static str> {
    vec!["anchor","margin","margin_top","margin_right","margin_bottom","margin_left","above","below","left_of","right_of","center_in","align_x","align_y","align_to","size","fit","safe_margin","distribute","equal_gap","keep_aspect_ratio","flex","grid","priority"]
}

pub fn cmd_shapes() -> Vec<Value> {
    [
        "NodeAdd", "NodeDel", "NodeSet", "NodesSet", "NodeMove", "BoardAdd", "BoardDel", "BoardSet",
        "AssetAdd", "AssetDel", "TokenSet", "Text", "Transform", "LayoutIntent", "Style", "Group",
        "Ungroup", "Duplicate", "Reflow", "Solve", "Composite",
    ]
    .iter()
    .map(|c| json!({"t": c}))
    .collect()
}

pub fn methods() -> Vec<(&'static str, &'static str)> {
    vec![
        ("caps", "everything an agent needs in one call"),
        ("help", "10-line cheat sheet"),
        ("doctor", "environment + limits check"),
        ("tools", "MCP tool list with schemas"),
        ("schema", "document skeleton + error codes + cmd shapes"),
        ("config", "read settings, or set:{...} to write"),
        ("demo", "build a real poster end to end"),
        ("batch", "jobs:[{m,p}] in one round trip"),
        ("log", "recent call log"),
        ("system.provider.probe", "[url timeout] -> is the provider reachable, its model ids"),
        ("jobs", "async job table"),
        ("cancel", "stop a job by id"),
        ("project.create", "name w h [preset] [boards]"),
        ("project.open", "p"),
        ("project.get", "p [depth:0]"),
        ("project.list", ""),
        ("project.save", "p"),
        ("project.close", "p"),
        ("project.clone", "p name"),
        ("project.snapshot", "p [label]"),
        ("project.restore", "p snap"),
        ("project.export_bundle", "p o"),
        ("project.import_bundle", "path"),
        ("artboard.create", "p w h [preset] [background]"),
        ("artboard.list", "p"),
        ("artboard.resize", "p b w h"),
        ("artboard.duplicate", "p b to:[ratios]"),
        ("artboard.delete", "p b"),
        ("artboard.set", "p b s:{}"),
        ("asset.import", "p path|paths[]"),
        ("asset.list", "p [q]"),
        ("asset.inspect", "p id"),
        ("asset.replace", "p id path"),
        ("asset.remove", "p id"),
        ("asset.thumb", "p id [w]"),
        ("asset.missing", "p"),
        ("canvas.add", "p b t x y w h [text|asset|style|intent]"),
        ("canvas.add_many", "p b nodes:[]"),
        ("canvas.update", "p id s:{}|l:{}|box:{}"),
        ("canvas.set", "p ids[] patch:{}"),
        ("canvas.text", "p id content"),
        ("canvas.image", "p id asset [fit]"),
        ("canvas.transform", "p id x y w h [rot]"),
        ("canvas.style", "p id fill|stroke|radius|opacity|fx"),
        ("canvas.reorder", "p id to [parent]"),
        ("canvas.group", "p ids[]"),
        ("canvas.ungroup", "p id"),
        ("canvas.duplicate", "p ids[]"),
        ("canvas.delete", "p ids[]|id"),
        ("canvas.align", "p ids[] axis how"),
        ("canvas.distribute", "p ids[] axis"),
        ("canvas.batch", "p cmds:[{t,...}] one transaction"),
        ("layout.apply", "p id intent:{} [mode]"),
        ("layout.set_mode", "p id mode"),
        ("layout.resolve", "p [b]"),
        ("layout.explain", "p id why is it there"),
        ("layout.reflow", "p b w h mode"),
        ("layout.lock", "p id pinned"),
        ("inspect.validate", "p [b] -> issues with evidence + fix"),
        ("inspect.document", "p tree summary"),
        ("inspect.node", "p id"),
        ("inspect.geometry", "p ids[]"),
        ("inspect.text", "p id metrics"),
        ("inspect.conflicts", "p b"),
        ("inspect.assets", "p usage"),
        ("inspect.diff", "p with:<project>|a snap b2 snap"),
        ("inspect.fix", "p [dry] apply suggested cmds"),
        ("render.preview", "p [b] -> real png + url"),
        ("render.svg", "p [b] [preview:1] [o]"),
        ("render.export", "p [b] f sc q o [transparent]"),
        ("render.export_batch", "p boards[] specs:[{f,sc,w,h,suffix}]"),
        ("render.diff", "a b -> similarity + changed box"),
        ("history.undo", "p [n]"),
        ("history.redo", "p [n]"),
        ("history.list", "p [n]"),
        ("history.checkpoint", "p [label]"),
        ("history.compare", "p from to"),
        ("history.rollback", "p rev|snap"),
        ("template.save", "p b name"),
        ("template.list", ""),
        ("template.use", "p tpl [vars]"),
        ("template.vars", "p vars:{title:..,product:..}"),
        ("template.variants", "p from colors[] copy:{} -> {boards,ids}"),
        ("template.builtin", "list built-in layouts"),
        ("poster", "one call: assets+copy+size -> project+preview+export"),
        ("image.probe", "path"),
        ("image.op", "in out op args:{}"),
        ("image.chain", "in out chain:[{op,..}] or \"crop 400x400 > scale .5\""),
        ("image.batch", "paths[] chain|preset out [fmt q]"),
        ("image.preset", "name in out"),
        ("image.presets", ""),
        ("image.ops", "list every op"),
        ("image.palette", "path [n]"),
        ("image.hash", "paths[] dhash+ahash"),
        ("image.compare", "a b"),
        ("text.measure", "text family size -> w h lines"),
        ("text.fonts", "[q]"),
        ("text.register", "path"),
    ]
}

pub fn cheatsheet() -> Vec<String> {
    vec![
        "agentcanvas caps                       # the whole surface, one call",
        "agentcanvas poster title=新品上市 image=a.png preset=poster f=png  # end to end",
        "agentcanvas project.create name=KV w=1080 h=1440",
        "agentcanvas asset.import p=proj_x path=C:\\pics\\a.png",
        "agentcanvas canvas.add p=proj_x b=b1 t=text text=标题 size=88 intent={\"anchor\":\"top\",\"margin\":110}",
        "agentcanvas canvas.add p=proj_x b=b1 t=image asset=as1 intent={\"below\":\"n1\",\"align_x\":\"center\",\"w\":710}",
        "agentcanvas inspect.validate p=proj_x   # issues with numbers + fix cmds",
        "agentcanvas render.export p=proj_x f=png sc=2   # files with sha256",
        "agentcanvas image.chain in=a.jpg out=b.webp chain=\"crop 1:1 > scale .5 > sharpen 1.2\" q=80",
        "agentcanvas batch '[{\"m\":\"caps\"},{\"m\":\"project.list\"}]'",
        "agentcanvas mcp                          # stdio MCP server for agents",
        "agentcanvas http --port 8899             # REST + /asset + /font",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn nearest(want: &str) -> Vec<&'static str> {
    // Compare on the part after the dot too: an agent that writes `canvas.addText`
    // is close to `canvas.add`, and a raw edit distance over the whole dotted name
    // says "nothing similar exists", which wastes the retry.
    let tail = |s: &str| s.rsplit('.').next().unwrap_or(s).to_lowercase();
    let base = want.to_lowercase();
    let bt = tail(want);
    let mut scored: Vec<(usize, &'static str)> = methods()
        .iter()
        .map(|(n, _)| {
            let full = lev(&base, n);
            let by_tail = lev(&bt, &tail(n));
            (full.min(by_tail + 1), *n)
        })
        .filter(|(d, _)| *d <= 6)
        .collect();
    scored.sort();
    scored.into_iter().take(5).map(|(_, n)| n).collect()
}

fn lev(a: &str, b: &str) -> usize {
    let (x, y): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut dp: Vec<Vec<usize>> = vec![vec![0; y.len() + 1]; x.len() + 1];
    for i in 0..=x.len() {
        dp[i][0] = i;
    }
    for j in 0..=y.len() {
        dp[0][j] = j;
    }
    for i in 1..=x.len() {
        for j in 1..=y.len() {
            let c = if x[i - 1] == y[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1).min(dp[i][j - 1] + 1).min(dp[i - 1][j - 1] + c);
        }
    }
    dp[x.len()][y.len()]
}

pub fn doctor(root: &Path) -> R<Value> {
    let dir = util::root(Some(root));
    let writable = {
        let t = dir.join(".write-test");
        let ok = std::fs::write(&t, b"x").is_ok();
        let _ = std::fs::remove_file(&t);
        ok
    };
    let fonts = text::Registry::shared().families();
    let cjk = fonts.iter().any(|f| f.to_lowercase().contains("cjk") || f.contains("雅黑") || f.contains("宋") || f.to_lowercase().contains("hei") || f.contains("苹方") || f.contains("思源"));
    // Presence only: an optional provider must be checkable without a network round trip.
    let cfg = session::config(&dir);
    let prov = cfg.get("provider").cloned().unwrap_or(json!({}));
    let provider = json!({
        "name": prov.get("name").cloned().unwrap_or(json!("")),
        "model": prov.get("model").cloned().unwrap_or(json!("")),
        "base_url_set": prov.get("baseUrlSet").cloned().unwrap_or(json!(false)),
        "api_key_set": prov.get("apiKeySet").cloned().unwrap_or(json!(false)),
    });
    Ok(json!({
        "ok": writable && !fonts.is_empty(),
        "root": util::short(&dir, None), "writable": writable,
        "fonts": fonts.len(), "cjk_font": cjk,
        "port": crate::http_port(), "http": crate::http_port() != 0,
        "provider": provider, "token_set": cfg.get("tokenSet").cloned().unwrap_or(json!(false)),
        "version": crate::version(),
        "limits": {"max_px": 60_000_000, "job_timeout_s": 600},
        "warnings": if !cjk { vec!["no CJK font found: Chinese text will fall back"] } else { vec![] },
    }))
}

pub fn demo(p: &Value, ctx: &Ctx) -> R<Value> {
    let spec = json!({
        "name": util::ps(p, "name").unwrap_or("demo-poster"),
        "preset": util::ps(p, "preset").unwrap_or("poster"),
        "title": util::ps(p, "title").unwrap_or("少即是多"),
        "subtitle": util::ps(p, "subtitle").unwrap_or("LESS BUT BETTER"),
        "body": util::ps(p, "body").unwrap_or("为 AI 代理准备的画布与排版引擎"),
        "accent": util::ps(p, "accent").unwrap_or("#D95748"),
        "tpl": util::ps(p, "tpl").unwrap_or("poster_kv"),
        "f": util::ps(p, "f").unwrap_or("png"),
        "out": util::ps(p, "out").unwrap_or("demo"),
    });
    let spec = match util::ps(p, "image") {
        Some(i) => merge_val(&spec, &json!({"image": i})),
        None => spec,
    };
    let _ = ctx;
    templates::poster(&root_of(ctx), &spec)
}

/// MCP view of the surface: aggregated tools, not 100 tiny ones.
pub fn mcp_tools() -> Value {
    let t = |name: &str, desc: &str, method: &str, props: Value, req: &[&str]| json!({
        "name": name,
        "description": desc,
        "inputSchema": {
            "type": "object",
            "properties": props,
            "required": req,
            "additionalProperties": true
        },
        "method": method
    });
    let s = |d: &str| json!({"type":"string","description":d});
    let n = |d: &str| json!({"type":"number","description":d});
    let a = |d: &str| json!({"type":"array","description":d,"items":{"type":["string","object","number"]}});
    let o = |d: &str| json!({"type":"object","description":d});
    Value::Array(vec![
        t("agentcanvas_caps", "One call that describes the whole product: methods, ops, intents, limits, error codes. Call this before anything else.", "system.caps", json!({}), &[]),
        t("project_create", "Create a design project. Returns {p} which every other call needs as \"p\".", "project.create", json!({"name":s("project name"),"w":n("width px"),"h":n("height px"),"preset":s("poster|square|story|banner|xhs|ecom-main|long|a4"),"boards":a("extra boards [{w,h,name}]")}), &["name"]),
        t("project_open", "Open an existing project by id or path.", "project.open", json!({"p":s("project id or path")}), &["p"]),
        t("project_get", "Read the whole document (scene graph) or a summary with depth:0.", "project.get", json!({"p":s("project"),"depth":n("0 summary, 1 full")}), &["p"]),
        t("project_save", "Save to disk atomically. Returns path, rev and sha256.", "project.save", json!({"p":s("project")}), &["p"]),
        t("project_list", "All projects, newest first.", "project.list", json!({}), &[]),
        t("artboard_create", "Add a board. w/h or preset or ratio (1:1, 16:9).", "artboard.create", json!({"p":s("project"),"w":n("px"),"h":n("px"),"preset":s("size preset"),"background":s("#hex or none"),"safe":n("safe margin px")}), &["p"]),
        t("artboard_duplicate", "Duplicate a board into other ratios, reflowing layout intents.", "artboard.duplicate", json!({"p":s("project"),"b":s("board"),"to":a("ratios like [\"1:1\",\"9:16\"]")}), &["p","b"]),
        t("asset_import", "Import image files into the project (deduped by sha256). Returns ids.", "asset.import", json!({"p":s("project"),"path":s("file"),"paths":a("files")}), &["p"]),
        t("canvas_add", "Add a layer: text|image|rect|ellipse|line|polygon|path|svg|group|frame. Give it semantic layout via intent.", "canvas.add", json!({"p":s("project"),"b":s("board"),"t":s("node type"),"x":n("px"),"y":n("px"),"w":n("px"),"h":n("px"),"text":s("content for text nodes"),"asset":s("asset id for image nodes"),"size":n("font size"),"color":s("#hex"),"family":s("font family"),"style":o("fill/stroke/radius"),"intent":o("semantic layout intent"),"name":s("layer name")}), &["p","t"]),
        t("canvas_update", "Deep-merge a patch into a node: style, layout, box, text. null removes a key.", "canvas.update", json!({"p":s("project"),"id":s("node"),"s":o("style patch"),"l":o("layout patch"),"box":o("{x,y,w,h}"),"text":s("new content")}), &["p","id"]),
        t("canvas_batch", "Many commands in one transaction: all succeed or none do. One revision bump.", "canvas.batch", json!({"p":s("project"),"cmds":a("[{t:\"NodeAdd\",...},{t:\"Transform\",id,x,y,w,h}]"),"rev":n("expectedRevision")}), &["p","cmds"]),
        t("canvas_delete", "Delete layers (locked ones need force).", "canvas.delete", json!({"p":s("project"),"ids":a("node ids"),"force":json!({"type":"boolean"})}), &["p"]),
        t("canvas_align", "Align or distribute layers: axis x|y, how left|center|right|top|middle|bottom.", "canvas.align", json!({"p":s("project"),"ids":a("node ids"),"axis":s("x|y"),"how":s("left|center|right|top|middle|bottom")}), &["p","ids"]),
        t("layout_apply", "Attach semantic constraints (below/above/center_in/anchor/margin/size/flex/grid) and solve.", "layout.apply", json!({"p":s("project"),"id":s("node"),"intent":o("constraint object"),"mode":s("constraint|manual|flex|grid")}), &["p","id","intent"]),
        t("layout_resolve", "Solve the whole board or one artboard; returns changed ids and conflicts.", "layout.resolve", json!({"p":s("project"),"b":s("optional board")}), &["p"]),
        t("layout_explain", "Why is this node where it is? Ordered intent provenance, plus conflicts.", "layout.explain", json!({"p":s("project"),"id":s("node")}), &["p","id"]),
        t("inspect_validate", "Machine-readable quality report: offboard, text-overflow, contrast, low-res, missing asset/font, conflicts. Every issue carries numeric evidence and a fix command.", "inspect.validate", json!({"p":s("project"),"b":s("optional board"),"contrast":json!({"type":"boolean"}),"sev_min":s("error|warn|info")}), &["p"]),
        t("inspect_fix", "Apply the suggested fixes (or dry:1 to see them).", "inspect.fix", json!({"p":s("project"),"dry":json!({"type":"boolean"})}), &["p"]),
        t("inspect_diff", "Structural diff between two snapshots or two projects.", "inspect.diff", json!({"p":s("project"),"a":s("snapshot"),"b2":s("snapshot"),"with":s("other project")}), &["p"]),
        t("render_preview", "Rasterise the current document to a real PNG and return its path + url. Cheap, call it after edits.", "render.preview", json!({"p":s("project"),"b":s("optional board"),"sc":n("scale 0..4")}), &["p"]),
        t("render_export", "Export PNG / JPG / WebP / SVG / PDF. Returns files with size, dims and sha256.", "render.export", json!({"p":s("project"),"b":s("board or omit for all"),"f":s("png|jpg|webp|svg|pdf"),"sc":n("scale"),"q":n("quality 1-100"),"o":s("output dir or file"),"transparent":json!({"type":"boolean"})}), &["p"]),
        t("render_svg", "The compiled SVG — the exact string the rasteriser consumes.", "render.svg", json!({"p":s("project"),"b":s("optional board"),"preview":json!({"type":"boolean"})}), &["p"]),
        t("history_undo", "Undo (n steps). Human and agent edits share one history.", "history.undo", json!({"p":s("project"),"n":n("steps")}), &["p"]),
        t("history_redo", "Redo (n steps).", "history.redo", json!({"p":s("project"),"n":n("steps")}), &["p"]),
        t("history_list", "The command trail with origin (desktop|mcp|http|cli).", "history.list", json!({"p":s("project"),"n":n("entries")}), &["p"]),
        t("template_use", "Instantiate a built-in or saved template into a project.", "template.use", json!({"p":s("project"),"tpl":s("template name"),"vars":o("title/subtitle/asset/accent")}), &["p","tpl"]),
        t("template_variants", "Build a series: N boards sharing one layout with different colours/copy/assets.", "template.variants", json!({"p":s("project"),"from":n("source board index"),"colors":a("accent hexes"),"copy":o("{title:[...]}"),"assets":a("asset ids"),"count":n("n")}), &["p"]),
        t("poster", "One call, finished poster: creates the project, imports images, lays out, solves, validates, auto-fixes, previews and exports. Best first choice for a whole deliverable.", "poster", json!({"title":s("main headline"),"subtitle":s("line 2"),"body":s("paragraph"),"image":s("product photo path"),"images":a("several photos"),"accent":s("#hex"),"preset":s("size preset"),"w":n("px"),"h":n("px"),"tpl":s("built-in layout name"),"f":s("export format"),"out":s("output dir"),"id":s("fixed project id, keeps paths stable across calls")}), &["title"]),
        t("image_op", "Single raster operation on a file: crop, resize, scale, pad, rotate, flip, round, border, adjust, blur, sharpen, noise, gray, threshold, tint, overlay, watermark, background, remove_bg, trim, pixelate, vignette, optimize.", "image.op", json!({"in":s("input path"),"out":s("output path"),"op":s("operation"),"args":o("op arguments"),"q":n("quality")}), &["in","op"]),
        t("image_chain", "Run a chain of raster ops. chain accepts the compact string form \"crop 1:1 > scale 0.5 > sharpen 1.2 > watermark 品牌\".", "image.chain", json!({"in":s("input"),"out":s("output"),"chain":a("steps or one string"),"q":n("quality"),"fmt":s("png|jpg|webp")}), &["in","chain"]),
        t("image_batch", "Same chain over many files. Returns per-file ok/out/e without aborting the batch.", "image.batch", json!({"paths":a("input files"),"chain":a("steps or string"),"preset":s("named preset"),"out":s("output dir"),"fmt":s("format"),"q":n("quality")}), &["paths"]),
        t("image_probe", "Real dimensions, format, alpha, bytes, sha256, palette of a file.", "image.probe", json!({"path":s("file")}), &["path"]),
        t("image_compare", "Similarity between two images: score 0-100, per-channel error, changed-pixel count, bounding box of the difference.", "image.compare", json!({"a":s("image"),"b2":s("image")}), &["a","b2"]),
        t("text_measure", "Measure text with the same font engine the renderer uses: width, height, wrapped lines, fallback warnings.", "text.measure", json!({"text":s("string"),"family":s("font family"),"size":n("px"),"max_w":n("wrap width"),"weight":n("100-900")}), &["text"]),
        t("system_doctor", "Environment check: writable root, font count, CJK availability, port.", "system.doctor", json!({}), &[]),
        t("system_tools", "This list, plus the full method table.", "system.tools", json!({"full":json!({"type":"boolean"})}), &[]),
        t("batch", "Several method calls in one round trip. jobs:[{m,p},...]", "batch", json!({"jobs":a("[{m,p}]")}), &["jobs"]),
    ])
}
