//! Tokens, templates, variants and the one-call `poster` product path.
//!
//! A *template* is a real JSON file at `<dir>/templates/<name>.json`:
//! `{name, note, tokens, boards:[{w,h,ratio,background,safe_margin}], nodes:[...],
//!   vars:[{key,node,keypath,kind}]}`. `vars` declares which node fields are
//! editable per instance (a title's text, a product image, an accent colour, a
//! price's box width…).
//!
//! Instantiation copies a template's nodes into a document with **fresh ids**,
//! rewriting `parent`, `children`, the board's `layers`, and every layout intent
//! that points at another node (`below.of`, `align_to.of`, `center_in`, `clip.by`).
//! Miss one of those references and the solved geometry silently collapses, so the
//! reference rewrite is unit-tested.
//!
//! Ids are shaped `{stamp}-{slot}`: `stamp` is a per-instantiation unique token
//! (guaranteeing disjoint ids across two instantiations in one document) and `slot`
//! is the node's position in the template (so every variant of a series keeps the
//! same id *suffix* and the same intent structure, and only the declared var
//! fields differ).
//!
//! Built-in layouts ship as *data* — a JSON node/intent/var spec per layout run
//! through one shared builder — never as a code branch per layout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::assets;
use crate::commands::{self, ApplyCtx, Cmd};
use crate::doc::{self, Artboard, Document, FontToken, Node, Paint, Radius, Style};
use crate::error::{Fail, R};
use crate::geom::{self, Fit};
use crate::inspect;
use crate::layout;
use crate::project;
use crate::render::{self, Format, RenderOpts};
use crate::util;

// ------------------------------------------------------------------ data types

/// An editable field declared by a template.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VarDef {
    pub key: String,
    /// Node id: the *template-local* id in a file/builtin, resolved to a concrete
    /// id when stored on an instantiated board.
    pub node: String,
    pub keypath: String,
    #[serde(default)]
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BoardMeta {
    w: f64,
    h: f64,
    #[serde(default)]
    ratio: Option<String>,
    #[serde(default)]
    background: Option<Paint>,
    #[serde(default)]
    safe_margin: Option<f64>,
    #[serde(default)]
    name: Option<String>,
}

/// On-disk template shape.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct TplFile {
    name: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    tokens: doc::Tokens,
    boards: Vec<BoardMeta>,
    nodes: Vec<Node>,
    #[serde(default)]
    vars: Vec<VarDef>,
}

/// The unified "something we can instantiate" — either a file template or a
/// built-in spec — normalised to Node-shaped JSON + var defs.
struct Spec {
    name: String,
    note: Option<String>,
    board: BoardMeta,
    tokens: doc::Tokens,
    objs: Vec<Value>,
    vars: Vec<VarDef>,
}

// ------------------------------------------------------------------ public API

/// List every available template: built-ins first, then user files.
pub fn list(dir: &Path) -> R<Vec<Value>> {
    let mut out: Vec<Value> = builtin()
        .into_iter()
        .map(|(name, desc)| json!({ "name": name, "desc": desc, "builtin": true }))
        .collect();
    let tdir = templates_dir(dir);
    if let Ok(rd) = std::fs::read_dir(&tdir) {
        let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        files.sort();
        for p in files {
            if util::ext(&p) != "json" {
                continue;
            }
            let s = load_spec(dir, &util::stem(&p));
            let (note, vars) = match s {
                Ok(sp) => (sp.note, sp.vars.len()),
                Err(_) => (None, 0),
            };
            out.push(json!({
                "name": util::stem(&p),
                "desc": note.unwrap_or_default(),
                "builtin": false,
                "path": p.to_string_lossy().replace('\\', "/"),
                "vars": vars,
            }));
        }
    }
    Ok(out)
}

/// Extract a board's node set into a reusable template file, auto-detecting
/// candidate variables (text content, image refs, colours that appear in a token).
pub fn save_from_board(doc: &Document, dir: &Path, board: &str, name: &str) -> R<Value> {
    let ab = doc
        .artboard(board)
        .ok_or_else(|| Fail::new("ARTBOARD_NOT_FOUND", format!("no board \"{board}\"")))?;
    let order = doc.paint_order(board);
    let mut vars: Vec<VarDef> = Vec::new();
    let token_colors: Vec<String> = doc.tokens.colors.iter().map(|c| c.value.clone()).collect();
    let mut used_keys: HashMap<String, u32> = HashMap::new();
    for id in &order {
        let n = match doc.node(id) {
            Some(n) => n,
            None => continue,
        };
        // Candidate variable: text content.
        if let Some(t) = &n.text {
            if !t.content.is_empty() {
                let key = candidate_key(&n.name, "text", id, &mut used_keys);
                vars.push(VarDef { key, node: id.clone(), keypath: "text.content".into(), kind: "text".into() });
            }
            if let Some(c) = &t.color {
                if token_colors.iter().any(|tc| tc == c) {
                    let key = candidate_key(&n.name, "color", id, &mut used_keys);
                    vars.push(VarDef { key, node: id.clone(), keypath: "text.color".into(), kind: "color".into() });
                }
            }
        }
        // Candidate variable: image asset reference.
        if let Some(i) = &n.image {
            if !i.asset.is_empty() {
                let key = candidate_key(&n.name, "image", id, &mut used_keys);
                vars.push(VarDef { key, node: id.clone(), keypath: "image.asset".into(), kind: "image".into() });
            }
        }
        // Candidate variable: a solid fill whose colour is a known token value.
        if let Some(st) = &n.style {
            if let Some(Paint::Solid { color }) = &st.fill {
                if token_colors.iter().any(|tc| tc == color) {
                    let key = candidate_key(&n.name, "accent", id, &mut used_keys);
                    vars.push(VarDef { key, node: id.clone(), keypath: "style.fill.color".into(), kind: "color".into() });
                }
            }
        }
    }

    let meta = BoardMeta {
        w: ab.width,
        h: ab.height,
        ratio: ab.ratio.clone(),
        background: ab.background.clone(),
        safe_margin: ab.safe_margin,
        name: Some(ab.name.clone()),
    };
    let file = TplFile {
        name: name.to_string(),
        note: Some(format!("extracted from board {board}")),
        tokens: doc.tokens.clone(),
        boards: vec![meta],
        // Serialise fresh nodes (parent/children/box all carried through).
        nodes: order.iter().filter_map(|id| doc.node(id)).cloned().collect(),
        vars: vars.clone(),
    };
    let path = template_path(dir, name);
    let text = serde_json::to_string_pretty(&file).map_err(|e| Fail::new("INVALID_SCHEMA", e.to_string()))?;
    util::write(&path, text.as_bytes())?;
    Ok(json!({
        "tpl": name,
        "path": path.to_string_lossy().replace('\\', "/"),
        "nodes": order.len(),
        "vars": file.vars.iter().map(|v| json!({ "key": v.key, "node": v.node, "keypath": v.keypath, "kind": v.kind })).collect::<Vec<_>>(),
    }))
}

/// Copy a template's nodes into `into` with fresh ids. `opts` accepts
/// `{board:"<existing id>", at:n, name, vars:{key:value}}`; without `board` a new
/// artboard sized from the template is created. Returns the new node ids.
pub fn instantiate(dir: &Path, tpl: &str, into: &mut Document, opts: &Value) -> R<Vec<String>> {
    let spec = load_spec(dir, tpl)?;
    Ok(instantiate_spec(into, spec, opts)?)
}

/// Set node fields by `keypath`. Accepted shapes:
/// - `{"<nodeId>": {"<keypath>": value, ...}}`
/// - `{"<varKey>": value}` when the value was declared by an instantiated
///   template stored on a board (resolved through its var defs)
/// - `[{"node":id,"keypath":kp,"value":v}, ...]`
///
/// Type mismatches (a number in a colour field, etc.) return `INVALID_VALUE`.
pub fn apply_variables(doc: &mut Document, vars: &Value) -> R<Vec<String>> {
    let mut changed = Vec::new();
    let defs = collect_defs(doc);
    match vars {
        Value::Object(map) => {
            for (k, v) in map {
                if doc.node(k).is_some() && v.is_object() {
                    for (kp, val) in v.as_object().unwrap() {
                        let (id, _) = set_node_value(doc, k, kp, val, &kind_for_keypath(kp))?;
                        push_once(&mut changed, id);
                    }
                } else if defs.iter().any(|d| &d.key == k) {
                    // by var key: may target several nodes across boards
                    let targets: Vec<VarDef> = defs.iter().filter(|d| &d.key == k && doc.node(&d.node).is_some()).cloned().collect();
                    if targets.is_empty() {
                        return Err(Fail::new("INVALID_SCHEMA", format!("var key \"{k}\" has no live node")));
                    }
                    for d in targets {
                        let (id, _) = set_node_value(doc, &d.node, &d.keypath, v, &d.kind)?;
                        push_once(&mut changed, id);
                    }
                } else {
                    return Err(Fail::new("INVALID_SCHEMA", format!("unknown node or var key \"{k}\""))
                        .hint("pass a real node id -> {keypath: value}, or a var key declared by an instantiated template"));
                }
            }
        }
        Value::Array(items) => {
            for it in items {
                let node = util::ps(it, "node").ok_or_else(|| Fail::new("INVALID_SCHEMA", "vars entry needs \"node\""))?;
                let kp = util::ps(it, "keypath").ok_or_else(|| Fail::new("INVALID_SCHEMA", "vars entry needs \"keypath\""))?;
                let kind = util::ps(it, "kind").map(|s| s.to_string()).unwrap_or_else(|| kind_for_keypath(kp));
                let val = it.get("value").cloned().unwrap_or(Value::Null);
                let (id, _) = set_node_value(doc, node, kp, &val, &kind)?;
                push_once(&mut changed, id);
            }
        }
        _ => return Err(Fail::new("INVALID_SCHEMA", "vars must be an object or array")),
    }
    Ok(changed)
}

/// Produce N sibling boards from one template board by crossing colour/copy/asset
/// lists. Every variant keeps the identical relative layout and id suffixes; only
/// the declared var fields differ. `opts` = `{from:"<board>", colors:[], copy:{k:[]}
/// | [{k:v}], assets:[], count:n}`. Returns `{boards:[new board ids], ids:[new node ids]}`.
pub fn create_variants(doc: &mut Document, opts: &Value) -> R<Value> {
    let src_id = util::ps(opts, "from")
        .or_else(|| util::ps(opts, "board"))
        .or_else(|| util::ps(opts, "b"))
        .map(|s| s.to_string())
        .or_else(|| {
            opts.get("from").and_then(|v| v.as_f64()).and_then(|n| {
                if n < 0.0 { return None; }
                doc.artboards.get(n as usize).map(|a| a.id.clone())
            })
        })
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Fail::new("INVALID_SCHEMA", "create_variants needs \"from\": a source board id"))?;
    let src = doc
        .artboard(&src_id)
        .ok_or_else(|| Fail::new("ARTBOARD_NOT_FOUND", format!("no board \"{src_id}\"")))?
        .clone();
    let order = doc.paint_order(&src_id);
    let defs = src
        .vars
        .get("template")
        .and_then(|t| t.get("vars"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    let var_defs: Vec<VarDef> = serde_json::from_value(defs).unwrap_or_default();

    let colors = util::arr(opts, "colors");
    let assets_l = util::arr(opts, "assets");
    let copy_map = read_copy(opts.get("copy"));

    let mut n = util::f(opts, "count").unwrap_or(0.0) as usize;
    if n == 0 {
        let copy_max = copy_map.values().map(|v| v.len()).max().unwrap_or(0);
        n = *[colors.len(), assets_l.len(), copy_max, 1].iter().max().unwrap();
    }
    let n = n.max(1);

    let source_objs: Vec<Value> = order.iter().filter_map(|id| doc.node(id)).map(|n| serde_json::to_value(n).unwrap_or(Value::Null)).collect();
    let mut all_ids = Vec::new();
    let mut boards = Vec::new();
    let suffix = util::ps(opts, "suffix").unwrap_or("v");

    for v in 0..n {
        let new_board_id = util::id("b");
        let mut board = src.clone();
        board.id = new_board_id.clone();
        board.name = format!("{} {suffix}{v}", src.name);
        board.layers = vec![];
        board.vars = serde_json::Map::new();
        doc.artboards.push(board);
        boards.push(new_board_id.clone());

        let (ids, map, layers) = clone_objs_into(doc, &new_board_id, &source_objs);
        // Rewrite intent references already handled in clone; now set the board's
        // top-level layer list.
        if let Some(ab) = doc.artboard_mut(&new_board_id) {
            ab.layers = layers;
        }

        // Remap the stored template record's node ids onto the fresh ids.
        let new_defs: Vec<VarDef> = var_defs
            .iter()
            .map(|d| VarDef {
                key: d.key.clone(),
                node: map.get(&d.node).cloned().unwrap_or_else(|| d.node.clone()),
                keypath: d.keypath.clone(),
                kind: d.kind.clone(),
            })
            .collect();
        if let Some(ab) = doc.artboard_mut(&new_board_id) {
            ab.vars.insert(
                "template".into(),
                json!({ "name": src.vars.get("template").and_then(|t| t.get("name")).cloned().unwrap_or(json!("variants")), "vars": new_defs }),
            );
        }

        for d in &new_defs {
            if let Some(val) = variant_value(v, d, n, &colors, &copy_map, &assets_l) {
                if doc.node(&d.node).is_some() {
                    let _ = set_node_value(doc, &d.node, &d.keypath, &val, &d.kind)?;
                }
            }
        }
        all_ids.extend(ids);
    }
    Ok(json!({ "boards": boards, "ids": all_ids }))
}

/// Names + descriptions of the built-in layouts.
pub fn builtin() -> Vec<(&'static str, &'static str)> {
    BUILTINS.iter().map(|b| (b.name, b.desc)).collect()
}

/// Instantiate a built-in layout into `doc`. Returns the created node ids.
pub fn apply_builtin(doc: &mut Document, _dir: &Path, name: &str) -> R<Vec<String>> {
    let spec = builtin_spec(name)?;
    Ok(instantiate_spec(doc, spec, &json!({}))?)
}

// ------------------------------------------------------------------ poster

/// The one-call product path. `spec` = `{name, preset|w,h, layout, title,
/// subtitle, body, image|images:[], accent, logo, format, sc, q, out, safe}`.
///
/// Creates a project, imports the images, lays them out with a built-in, injects
/// the copy/colour/asset variables, solves, validates (auto-fixing `offboard` and
/// `text-overflow` once), previews and exports — one round trip.
pub fn poster(doc_dir: &Path, spec: &Value) -> R<Value> {
    let name = util::ps(spec, "name").map(|s| s.to_string()).unwrap_or_else(|| util::id("p"));
    let (w, h, preset_name) = resolve_size(spec)?;
    let layout_name = util::ps(spec, "layout")
        .or_else(|| util::ps(spec, "tpl"))
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_layout(preset_name.as_deref(), w, h));
    if !BUILTINS.iter().any(|b| b.name == layout_name) {
        return Err(Fail::new("INVALID_VALUE", format!("unknown built-in layout \"{layout_name}\""))
            .hint("see template.list for built-in names"));
    }

    // 1. project. An `id` in the spec makes the whole call repeatable: the project
    //    directory and the `preview.png` inside it then keep a stable path instead
    //    of landing under a fresh random id every run.
    let mut prj_opts = json!({"origin": util::ps(spec, "origin").unwrap_or("library")});
    if let Some(id) = util::ps(spec, "id") {
        if let Some(o) = prj_opts.as_object_mut() {
            o.insert("id".into(), json!(id));
        }
    }
    let mut prj = project::create(doc_dir, &name, w, h, &prj_opts)?;
    let board0 = prj
        .doc
        .first_artboard()
        .map(|a| a.id.clone())
        .unwrap_or_default();

    // 2. import images
    let mut imgs: Vec<String> = Vec::new();
    for src in image_sources(spec) {
        let p = Path::new(&src);
        match assets::import(&mut prj.doc, &prj.dir, p, &json!({})) {
            Ok(im) => imgs.push(im.id),
            Err(e) => return Err(e),
        }
    }

    // 3. lay out from the built-in spec, replacing the auto-created board.
    let spec_obj = builtin_spec(&layout_name)?;
    let mut opts = json!({ "w": w, "h": h, "name": name });
    if let Some(m) = util::f(spec, "safe") {
        opts["safe"] = json!(m);
    }
    let ids = instantiate_spec(&mut prj.doc, spec_obj, &opts)?;
    // Remove the empty default board so the poster is exactly one artboard.
    if !board0.is_empty() {
        let has_nodes = prj
            .doc
            .nodes
            .iter()
            .any(|n| n.artboard.as_deref() == Some(board0.as_str()));
        if !has_nodes {
            prj.doc.artboards.retain(|a| a.id != board0);
        }
    }
    let board_ids: Vec<String> = prj.doc.artboards.iter().map(|a| a.id.clone()).collect();
    let main_board = board_ids.first().cloned().unwrap_or_default();

    // 4. inject variables. Only fields the layout actually declared are touched:
    //    colour vars read `accent`, image vars take the imported assets in order,
    //    and every text var is filled from a same-named spec field. `vars` in the
    //    spec is a pass-through for any remaining declared key or node id.
    let defs = collect_defs(&prj.doc);
    let def_keys: HashMap<&str, &VarDef> = defs.iter().map(|d| (d.key.as_str(), d)).collect();
    let mut vars = serde_json::Map::new();
    let img_defs: Vec<&VarDef> = defs.iter().filter(|d| d.kind == "image" || d.keypath == "image.asset").collect();
    for (i, d) in img_defs.iter().enumerate() {
        if let Some(a) = imgs.get(i) {
            vars.insert(d.key.clone(), json!(a));
        }
    }
    for d in &defs {
        if d.kind == "image" || d.keypath == "image.asset" {
            continue;
        }
        if d.kind == "color" {
            if let Some(a) = util::ps(spec, "accent") {
                vars.insert(d.key.clone(), json!(a));
                continue;
            }
        }
        if let Some(v) = util::ps(spec, &d.key) {
            vars.insert(d.key.clone(), json!(v));
        }
    }
    if let Some(extra) = spec.get("vars").and_then(|v| v.as_object()) {
        for (k, val) in extra {
            if def_keys.contains_key(k.as_str()) || prj.doc.node(k).is_some() {
                vars.insert(k.clone(), val.clone());
            }
        }
    }
    if !vars.is_empty() {
        apply_variables(&mut prj.doc, &Value::Object(vars))?;
    }

    // 5. solve every board.
    let _ = layout::solve(&mut prj.doc, None)?;

    // 6. validate + auto-fix offboard/text-overflow once.
    let opts_v = json!({ "contrast": util::opt_b(spec, "contrast", true), "lowres": util::opt_b(spec, "lowres", true) });
    let first = inspect::validate(&prj.doc, &prj.dir, &opts_v)?;
    let cmds = fixes_for(&first, &["offboard", "text-overflow"]);
    let report = if cmds.is_empty() {
        first
    } else {
        let ctx = ApplyCtx { dir: prj.dir.clone(), force: true, origin: "poster".into() };
        let _ = commands::apply(&mut prj.doc, &Cmd::Composite { cmds }, &ctx)?;
        let _ = layout::solve(&mut prj.doc, Some(&main_board))?;
        inspect::validate(&prj.doc, &prj.dir, &opts_v)?
    };

    // 7. preview + export.
    let fmt_str = util::ps(spec, "format").unwrap_or("png").to_string();
    let scale = util::f(spec, "sc").unwrap_or(1.0);
    let q = util::f(spec, "q").unwrap_or(92.0) as u8;
    let out_dir = match util::ps(spec, "out") {
        Some(o) => util::outp(o, Some(doc_dir)),
        None => prj.dir.join("exports"),
    };
    let _ = std::fs::create_dir_all(&out_dir);

    let mut files: Vec<Value> = Vec::new();
    let ro = RenderOpts { scale, format: parse_format(&fmt_str), quality: q, background: None, width: None, height: None };
    // `proj` is the asset base: the imported files live in `<project>/assets/`, and
    // passing `out_dir` for both jobs makes every raster silently paint nothing.
    let exported = render::export_with(&prj.doc, &out_dir, &name, &ro, &[], Some(&prj.dir))?;
    for r in &exported {
        files.push(rendered_json(r));
    }
    // A small preview so an agent can eyeball the result cheaply.
    if !matches!(parse_format(&fmt_str), Format::Svg | Format::Pdf) {
        let po = RenderOpts { scale: (scale * 0.3).max(0.1), format: Format::Png, quality: q, background: None, width: None, height: None };
        if let Ok(pv) = render::export_with(&prj.doc, &out_dir, &format!("{name}-preview"), &po, &[], Some(&prj.dir)) {
            for r in &pv {
                files.push(rendered_json(r));
            }
        }
    }

    // 8. persist the project document.
    let _ = project::save(&mut prj, &json!({}));

    let issues = report.get("issues").cloned().unwrap_or_else(|| json!([]));
    Ok(json!({
        "p": if prj.doc.project_id.is_empty() { prj.doc.name.clone() } else { prj.doc.project_id.clone() },
        "dir": util::short(&prj.dir, None),
        "boards": board_ids,
        "ids": ids,
        "files": files,
        "issues": issues,
        "report": { "ok": report.get("ok").cloned().unwrap_or(json!(false)), "errors": report.get("errors").cloned().unwrap_or(json!(0)), "warns": report.get("warns").cloned().unwrap_or(json!(0)) },
    }))
}

// ------------------------------------------------------------------ built-in data

struct Builtin {
    name: &'static str,
    desc: &'static str,
    spec: fn() -> Value,
}

fn board_meta(w: f64, h: f64, ratio: &str, bg: &str, safe: f64) -> Value {
    json!({ "w": w, "h": h, "ratio": ratio, "background": { "t": "solid", "color": bg }, "safe_margin": safe })
}

// The palette is chosen so every built-in's text clears 4.5:1 against its
// effective background (dark ink board + light type, light paper + dark ink,
// and dark ink on the coral accent pill = ~6:1). Colours are written in `rgb()`
// functional form: it round-trips identically through `doc::parse_color` and
// keeps the inspector's contrast maths exact.
const INK: &str = "rgb(16,19,24)";
const PAPER: &str = "rgb(247,245,241)";
const WHITE: &str = "rgb(255,255,255)";
const ACCENT: &str = "rgb(255,90,95)";
const MUTE_ON_DARK: &str = "rgb(199,205,219)";
const MUTE_ON_LIGHT: &str = "rgb(58,66,80)";

/// Text node that is the visual headline of a layout (z_role content).
fn headline(id: &str, content: &str, size: f64, color: &str, intent: Value, bx: [f64; 4]) -> Value {
    json!({
        "id": id, "t": "text", "name": id, "z_role": "content",
        "text": { "content": content, "size": size, "color": color, "line_ratio": 1.2, "overflow": "shrink", "align": "left" },
        "layout": { "mode": "constraint", "intent": intent },
        "box": [bx[0], bx[1], bx[2], bx[3]],
    })
}

/// Smaller supporting text (z_role label).
fn body_text(id: &str, content: &str, size: f64, color: &str, intent: Value, bx: [f64; 4]) -> Value {
    json!({
        "id": id, "t": "text", "name": id, "z_role": "label",
        "text": { "content": content, "size": size, "color": color, "line_ratio": 1.4, "overflow": "shrink", "align": "left" },
        "layout": { "mode": "constraint", "intent": intent },
        "box": [bx[0], bx[1], bx[2], bx[3]],
    })
}

fn shape(id: &str, kind: &str, fill: &str, radius: f64, intent: Value, bx: [f64; 4], zrole: &str) -> Value {
    json!({
        "id": id, "t": kind, "name": id, "z_role": zrole,
        "s": { "fill": { "t": "solid", "color": fill }, "radius": { "all": radius } },
        "layout": { "mode": "constraint", "intent": intent },
        "box": [bx[0], bx[1], bx[2], bx[3]],
    })
}

fn image_node(id: &str, asset: &str, fit: &str, intent: Value, bx: [f64; 4], zrole: &str) -> Value {
    json!({
        "id": id, "t": "image", "name": id, "z_role": zrole,
        "image": { "asset": asset, "fit": fit },
        "layout": { "mode": "constraint", "intent": intent },
        "box": [bx[0], bx[1], bx[2], bx[3]],
    })
}

fn poster_kv() -> Value {
    json!({
        "board": board_meta(1080.0, 1440.0, "3:4", INK, 72.0),
        "tokens": { "colors": [ {"name":"accent","value":ACCENT}, {"name":"ink","value":INK}, {"name":"paper","value":WHITE} ] },
        "nodes": [
            image_node("hero", "", "contain", json!({ "anchor": "center", "size": { "w_ref": 0.62, "h_ref": 0.40 }, "below": { "of": "headline", "gap": 24, "cross": "center" } }), [240.0, 380.0, 660.0, 540.0], "content"),
            headline("headline", "BOLD HEADLINE", 96.0, WHITE, json!({ "anchor": "top", "margin_top": 120.0, "size": { "w_ref": 0.86, "min_h": 120.0 } }), [86.0, 120.0, 907.0, 120.0]),
            body_text("subtitle", "a supporting line of copy", 40.0, MUTE_ON_DARK, json!({ "below": { "of": "headline", "gap": 16 }, "align_to": { "of": "headline", "edge": "left" }, "size": { "w_ref": 0.8 } }), [86.0, 250.0, 864.0, 56.0]),
            shape("brandbar", "rect", ACCENT, 6.0, json!({ "left_of": { "of": "brand", "gap": 16, "cross": "middle" }, "size": { "w": 72.0, "h": 10.0 } }), [86.0, 1300.0, 72.0, 10.0], "decor"),
            body_text("brand", "yourbrand", 34.0, WHITE, json!({ "anchor": "bottom-left", "margin_bottom": 96.0, "margin_left": 86.0, "size": { "w": 420.0, "min_h": 44.0 } }), [174.0, 1300.0, 420.0, 44.0]),
        ],
        "vars": [
            { "key": "title", "node": "headline", "keypath": "text.content", "kind": "text" },
            { "key": "subtitle", "node": "subtitle", "keypath": "text.content", "kind": "text" },
            { "key": "image", "node": "hero", "keypath": "image.asset", "kind": "image" },
            { "key": "accent", "node": "brandbar", "keypath": "style.fill.color", "kind": "color" },
            { "key": "logo", "node": "brand", "keypath": "text.content", "kind": "text" },
        ],
    })
}

fn ecom_main() -> Value {
    json!({
        "board": board_meta(800.0, 800.0, "1:1", PAPER, 40.0),
        "tokens": { "colors": [ {"name":"accent","value":ACCENT} ] },
        "nodes": [
            image_node("product", "", "contain", json!({ "anchor": "center", "size": { "w_ref": 0.82, "h_ref": 0.72 } }), [72.0, 40.0, 656.0, 560.0], "content"),
            shape("pricepill", "rect", ACCENT, 999.0, json!({ "anchor": "bottom", "margin_bottom": 120.0, "align_x": "center", "size": { "w_ref": 0.34, "min_h": 72.0 } }), [264.0, 660.0, 272.0, 72.0], "label"),
            headline("price", "¥199", 48.0, INK, json!({ "center_in": "pricepill" }), [316.0, 682.0, 168.0, 44.0]),
            body_text("name", "Product name here", 30.0, MUTE_ON_LIGHT, json!({ "anchor": "top", "margin_top": 40.0, "align_x": "center", "size": { "w_ref": 0.8, "min_h": 40.0 } }), [80.0, 40.0, 640.0, 40.0]),
        ],
        "vars": [
            { "key": "title", "node": "name", "keypath": "text.content", "kind": "text" },
            { "key": "price", "node": "price", "keypath": "text.content", "kind": "text" },
            { "key": "image", "node": "product", "keypath": "image.asset", "kind": "image" },
            { "key": "accent", "node": "pricepill", "keypath": "style.fill.color", "kind": "color" },
        ],
    })
}

fn story_ad() -> Value {
    json!({
        "board": board_meta(1080.0, 1920.0, "9:16", INK, 96.0),
        "tokens": { "colors": [ {"name":"accent","value":ACCENT} ] },
        "nodes": [
            image_node("hero", "", "cover", json!({ "anchor": "center", "size": { "w_ref": 1.0, "h_ref": 0.5 }, "margin_top": 120.0 }), [0.0, 240.0, 1080.0, 900.0], "content"),
            headline("headline", "BIG\nTYPE", 180.0, WHITE, json!({ "anchor": "top", "margin_top": 220.0, "align_x": "center", "size": { "w_ref": 0.8, "min_h": 200.0 } }), [108.0, 220.0, 864.0, 200.0]),
            body_text("subtitle", "why you should care, right now", 44.0, MUTE_ON_DARK, json!({ "below": { "of": "headline", "gap": 24, "cross": "center" }, "align_x": "center", "size": { "w_ref": 0.72 } }), [151.0, 720.0, 778.0, 120.0]),
            shape("cta", "rect", ACCENT, 999.0, json!({ "anchor": "bottom", "margin_bottom": 200.0, "align_x": "center", "size": { "w": 520.0, "h": 120.0 } }), [280.0, 1600.0, 520.0, 120.0], "label"),
            headline("ctatext", "SHOP NOW", 52.0, INK, json!({ "center_in": "cta" }), [384.0, 1640.0, 312.0, 48.0]),
        ],
        "vars": [
            { "key": "title", "node": "headline", "keypath": "text.content", "kind": "text" },
            { "key": "subtitle", "node": "subtitle", "keypath": "text.content", "kind": "text" },
            { "key": "cta", "node": "ctatext", "keypath": "text.content", "kind": "text" },
            { "key": "image", "node": "hero", "keypath": "image.asset", "kind": "image" },
            { "key": "accent", "node": "cta", "keypath": "style.fill.color", "kind": "color" },
        ],
    })
}

fn card_quote() -> Value {
    json!({
        "board": board_meta(1600.0, 500.0, "16:5", PAPER, 48.0),
        "tokens": { "colors": [ {"name":"accent","value":ACCENT} ] },
        "nodes": [
            shape("rule", "rect", ACCENT, 3.0, json!({ "anchor": "left", "margin_left": 80.0, "align_y": "middle", "size": { "w": 10.0, "h_ref": 0.6 } }), [80.0, 100.0, 10.0, 300.0], "decor"),
            headline("quote", "\"The one line that makes the whole piece.\"", 62.0, INK, json!({ "anchor": "center", "margin_left": 140.0, "size": { "w_ref": 0.72, "min_h": 200.0 } }), [150.0, 120.0, 1152.0, 200.0]),
            body_text("author", "— Name, Role", 30.0, MUTE_ON_LIGHT, json!({ "below": { "of": "quote", "gap": 24 }, "align_to": { "of": "quote", "edge": "left" }, "size": { "w_ref": 0.4 } }), [150.0, 340.0, 640.0, 40.0]),
        ],
        "vars": [
            { "key": "title", "node": "quote", "keypath": "text.content", "kind": "text" },
            { "key": "subtitle", "node": "author", "keypath": "text.content", "kind": "text" },
            { "key": "accent", "node": "rule", "keypath": "style.fill.color", "kind": "color" },
        ],
    })
}

fn banner_promo() -> Value {
    json!({
        "board": board_meta(1920.0, 1080.0, "16:9", INK, 72.0),
        "tokens": { "colors": [ {"name":"accent","value":ACCENT} ] },
        "nodes": [
            shape("panel", "rect", "rgb(27,33,48)", 24.0, json!({ "anchor": "left", "size": { "w_ref": 0.5, "h_ref": 1.0 } }), [0.0, 0.0, 960.0, 1080.0], "background"),
            headline("headline", "MEGA SALE", 128.0, WHITE, json!({ "anchor": "center", "margin_right": 1000.0, "size": { "w_ref": 0.42, "min_h": 150.0 } }), [120.0, 300.0, 800.0, 150.0]),
            body_text("subtitle", "up to 50% off this weekend only", 44.0, MUTE_ON_DARK, json!({ "below": { "of": "headline", "gap": 24 }, "align_to": { "of": "headline", "edge": "left" }, "size": { "w_ref": 0.4 } }), [120.0, 474.0, 768.0, 120.0]),
            shape("cta", "rect", ACCENT, 999.0, json!({ "below": { "of": "subtitle", "gap": 48 }, "align_to": { "of": "headline", "edge": "left" }, "size": { "w": 420.0, "h": 110.0 } }), [120.0, 642.0, 420.0, 110.0], "label"),
            headline("ctatext", "GET THE DEAL", 44.0, INK, json!({ "center_in": "cta" }), [190.0, 674.0, 280.0, 44.0]),
            image_node("product", "", "contain", json!({ "anchor": "right", "size": { "w_ref": 0.44, "h_ref": 0.8 } }), [1056.0, 108.0, 845.0, 864.0], "content"),
        ],
        "vars": [
            { "key": "title", "node": "headline", "keypath": "text.content", "kind": "text" },
            { "key": "subtitle", "node": "subtitle", "keypath": "text.content", "kind": "text" },
            { "key": "cta", "node": "ctatext", "keypath": "text.content", "kind": "text" },
            { "key": "image", "node": "product", "keypath": "image.asset", "kind": "image" },
            { "key": "accent", "node": "cta", "keypath": "style.fill.color", "kind": "color" },
        ],
    })
}

fn cover_editorial() -> Value {
    // "title behind subject": the title sits *earlier* in paint order than the
    // product, but the product is sized to leave the headline peeking above it so
    // the inspector never reports the title as fully covered.
    json!({
        "board": board_meta(1080.0, 1440.0, "3:4", PAPER, 72.0),
        "tokens": { "colors": [ {"name":"accent","value":ACCENT} ] },
        "nodes": [
            headline("masthead", "ISSUE ONE", 34.0, MUTE_ON_LIGHT, json!({ "anchor": "top", "align_x": "center", "margin_top": 96.0, "size": { "w_ref": 0.6 } }), [216.0, 96.0, 648.0, 44.0]),
            headline("title", "THE SUBJECT", 200.0, INK, json!({ "anchor": "top", "margin_top": 160.0, "align_x": "center", "size": { "w_ref": 0.92, "min_h": 220.0 } }), [43.0, 160.0, 994.0, 220.0]),
            image_node("subject", "", "contain", json!({ "below": { "of": "title", "gap": -40, "cross": "center" }, "size": { "w_ref": 0.66, "h_ref": 0.5 } }), [184.0, 340.0, 713.0, 720.0], "content"),
            shape("accentline", "rect", ACCENT, 4.0, json!({ "anchor": "bottom", "margin_bottom": 200.0, "align_x": "center", "size": { "w_ref": 0.3, "h": 8.0 } }), [378.0, 1232.0, 324.0, 8.0], "decor"),
            body_text("coverline", "and a coverline to tease the story inside", 34.0, MUTE_ON_LIGHT, json!({ "anchor": "bottom", "margin_bottom": 120.0, "align_x": "center", "size": { "w_ref": 0.7 } }), [162.0, 1272.0, 756.0, 96.0]),
        ],
        "vars": [
            { "key": "title", "node": "title", "keypath": "text.content", "kind": "text" },
            { "key": "subtitle", "node": "coverline", "keypath": "text.content", "kind": "text" },
            { "key": "image", "node": "subject", "keypath": "image.asset", "kind": "image" },
            { "key": "accent", "node": "accentline", "keypath": "style.fill.color", "kind": "color" },
        ],
    })
}

fn grid_collage() -> Value {
    let gap = 16.0;
    let cell = 524.0;
    let mut cells = Vec::new();
    for i in 0..4 {
        let col = i % 2;
        let row = i / 2;
        let x = gap + col as f64 * (cell + gap);
        let y = gap + row as f64 * (cell + gap);
        let id = format!("photo{}", i + 1);
        cells.push(image_node(
            &id,
            "",
            "cover",
            json!({ "anchor": "top-left", "margin_left": x, "margin_top": y, "size": { "w": cell, "h": cell } }),
            [x, y, cell, cell],
            "content",
        ));
    }
    let vars: Vec<Value> = (1..=4)
        .map(|i| json!({ "key": format!("photo{i}"), "node": format!("photo{i}"), "keypath": "image.asset", "kind": "image" }))
        .collect();
    json!({
        "board": board_meta(1080.0, 1080.0, "1:1", INK, 8.0),
        "tokens": { "colors": [ {"name":"accent","value":ACCENT} ] },
        "nodes": cells,
        "vars": vars,
    })
}

fn long_image() -> Value {
    json!({
        "board": board_meta(750.0, 3000.0, "1:4", PAPER, 40.0),
        "tokens": { "colors": [ {"name":"accent","value":ACCENT} ] },
        "nodes": [
            headline("title", "A LONG READ", 72.0, INK, json!({ "anchor": "top", "margin_top": 96.0, "align_x": "center", "size": { "w_ref": 0.84 } }), [60.0, 96.0, 630.0, 90.0]),
            image_node("hero", "", "cover", json!({ "below": { "of": "title", "gap": 32 }, "align_x": "center", "size": { "w_ref": 0.9, "h": 420.0 } }), [37.0, 218.0, 675.0, 420.0], "content"),
            body_text("body1", "First section of the story copy. It wraps across the column and shrinks to fit its box so nothing spills.", 34.0, MUTE_ON_LIGHT, json!({ "below": { "of": "hero", "gap": 40 }, "align_x": "center", "size": { "w_ref": 0.84, "min_h": 320.0 } }), [56.0, 678.0, 637.0, 320.0]),
            shape("divider", "rect", ACCENT, 3.0, json!({ "below": { "of": "body1", "gap": 48 }, "align_x": "center", "size": { "w_ref": 0.2, "h": 8.0 } }), [300.0, 1046.0, 150.0, 8.0], "decor"),
            body_text("body2", "Second section continues the narrative down the long canvas, one column, comfortable measure.", 34.0, MUTE_ON_LIGHT, json!({ "below": { "of": "divider", "gap": 40 }, "align_x": "center", "size": { "w_ref": 0.84, "min_h": 320.0 } }), [56.0, 1094.0, 637.0, 320.0]),
            image_node("footer", "", "contain", json!({ "below": { "of": "body2", "gap": 40 }, "align_x": "center", "size": { "w_ref": 0.7, "h": 320.0 } }), [112.0, 1454.0, 525.0, 320.0], "content"),
        ],
        "vars": [
            { "key": "title", "node": "title", "keypath": "text.content", "kind": "text" },
            { "key": "body", "node": "body1", "keypath": "text.content", "kind": "text" },
            { "key": "image", "node": "hero", "keypath": "image.asset", "kind": "image" },
            { "key": "logo", "node": "footer", "keypath": "image.asset", "kind": "image" },
            { "key": "accent", "node": "divider", "keypath": "style.fill.color", "kind": "color" },
        ],
    })
}

static BUILTINS: [Builtin; 8] = [
    Builtin { name: "poster_kv", desc: "Key-visual poster: headline top, product centre, brand bottom-left, 72px safe margin.", spec: poster_kv },
    Builtin { name: "ecom_main", desc: "Square e-commerce main image: product fills the frame, price badge below.", spec: ecom_main },
    Builtin { name: "story_ad", desc: "9:16 vertical story ad: big type, hero image, CTA pill.", spec: story_ad },
    Builtin { name: "card_quote", desc: "Wide quote card: accent rule, pull-quote, attribution.", spec: card_quote },
    Builtin { name: "banner_promo", desc: "16:9 promo banner: copy column + CTA on the left, product on the right.", spec: banner_promo },
    Builtin { name: "cover_editorial", desc: "Magazine cover: masthead, big title behind the subject, coverline.", spec: cover_editorial },
    Builtin { name: "grid_collage", desc: "2x2 photo grid collage with even gutters.", spec: grid_collage },
    Builtin { name: "long_image", desc: "Stacked-section vertical long image (图文长图).", spec: long_image },
];

// ------------------------------------------------------------------ spec loading

fn builtin_spec(name: &str) -> R<Spec> {
    let b = BUILTINS
        .iter()
        .find(|b| b.name == name)
        .ok_or_else(|| Fail::new("NOT_FOUND", format!("no built-in template \"{name}\"")).hint("see template.list"))?;
    Ok(spec_from_json(&(b.spec)(), b.name))
}

fn spec_from_json(v: &Value, name: &str) -> Spec {
    let board: BoardMeta = serde_json::from_value(v.get("board").cloned().unwrap_or(Value::Null))
        .unwrap_or(BoardMeta { w: 1080.0, h: 1440.0, ratio: None, background: None, safe_margin: None, name: None });
    let objs: Vec<Value> = v
        .get("nodes")
        .and_then(|a| a.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|s| spec_node_to_json(&s))
        .collect();
    let vars: Vec<VarDef> = v
        .get("vars")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| serde_json::from_value(x.clone()).ok()).collect())
        .unwrap_or_default();
    let tokens = v
        .get("tokens")
        .cloned()
        .and_then(|t| serde_json::from_value::<doc::Tokens>(t).ok())
        .unwrap_or_default();
    Spec { name: name.to_string(), note: Some(name.to_string()), board, tokens, objs, vars }
}

fn load_spec(dir: &Path, tpl: &str) -> R<Spec> {
    let p = template_path(dir, tpl);
    if p.is_file() {
        let bytes = util::read(&p)?;
        let file: TplFile = serde_json::from_slice(&bytes)
            .map_err(|e| Fail::new("INVALID_SCHEMA", format!("template {}: {e}", p.display())))?;
        let board = file.boards.first().cloned().unwrap_or(BoardMeta { w: 1080.0, h: 1440.0, ratio: None, background: None, safe_margin: None, name: None });
        let objs = file.nodes.iter().map(|n| serde_json::to_value(n).unwrap_or(Value::Null)).collect();
        return Ok(Spec { name: file.name, note: file.note, board, tokens: file.tokens, objs, vars: file.vars });
    }
    builtin_spec(tpl)
}

/// Map the compact authoring keys (`t`/`box`/`s`/`l`) onto the `Node` serde shape.
fn spec_node_to_json(spec: &Value) -> Value {
    let mut o = serde_json::Map::new();
    o.insert("id".into(), spec.get("id").cloned().unwrap_or_else(|| json!(util::id("n"))));
    o.insert("type".into(), spec.get("t").cloned().unwrap_or_else(|| json!("rect")));
    if let Some(b) = spec.get("box") {
        o.insert("box_".into(), box_arr_to_obj(b));
    } else {
        o.insert("box_".into(), json!({ "x": 0.0, "y": 0.0, "w": 100.0, "h": 100.0 }));
    }
    for (from, to) in [("name", "name"), ("z_role", "z_role"), ("role", "role"), ("opacity", "opacity"), ("rotation", "rotation"), ("origin", "origin"), ("text", "text"), ("image", "image"), ("vector", "vector"), ("clip", "clip"), ("fx", "fx"), ("data", "data")] {
        if let Some(v) = spec.get(from) {
            o.insert(to.into(), v.clone());
        }
    }
    if let Some(st) = spec.get("s") {
        o.insert("style".into(), st.clone());
    } else if let Some(st) = spec.get("style") {
        o.insert("style".into(), st.clone());
    }
    // Layout may be given as the alias `l` or the full `layout` key.
    if let Some(l) = spec.get("l").or_else(|| spec.get("layout")) {
        o.insert("layout".into(), normalize_layout(l));
    } else {
        o.insert("layout".into(), json!({ "mode": "manual" }));
    }
    Value::Object(o)
}

fn normalize_layout(l: &Value) -> Value {
    let mut o = serde_json::Map::new();
    o.insert("mode".into(), l.get("mode").cloned().unwrap_or_else(|| json!("manual")));
    if let Some(intent) = l.get("intent") {
        o.insert("intent".into(), intent.clone());
    }
    Value::Object(o)
}

fn box_arr_to_obj(b: &Value) -> Value {
    if let Some(a) = b.as_array() {
        let g = |i: usize| a.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0);
        json!({ "x": g(0), "y": g(1), "w": g(2), "h": g(3) })
    } else {
        b.clone()
    }
}

// ------------------------------------------------------------------ instantiation

fn instantiate_spec(into: &mut Document, spec: Spec, opts: &Value) -> R<Vec<String>> {
    let board_id = match util::ps(opts, "board").filter(|s| !s.is_empty()) {
        Some(existing) => {
            if into.artboard(existing).is_none() {
                return Err(Fail::new("ARTBOARD_NOT_FOUND", format!("no board \"{existing}\"")));
            }
            existing.to_string()
        }
        None => {
            let mut meta = spec.board.clone();
            if let Some(m) = util::f(opts, "safe") {
                meta.safe_margin = Some(m);
            }
            let ab = make_board(&meta, opts);
            let id = ab.id.clone();
            into.artboards.push(ab);
            id
        }
    };

    merge_tokens(into, &spec.tokens);

    let (ids, map, layers) = clone_objs_into(into, &board_id, &spec.objs);
    if let Some(ab) = into.artboard_mut(&board_id) {
        ab.layers = layers;
    }

    // Store the template record so create_variants / apply_variables can resolve
    // var keys to concrete (new) node ids + keypaths.
    let new_defs: Vec<VarDef> = spec
        .vars
        .iter()
        .map(|d| VarDef { key: d.key.clone(), node: map.get(&d.node).cloned().unwrap_or_else(|| d.node.clone()), keypath: d.keypath.clone(), kind: d.kind.clone() })
        .collect();
    if let Some(ab) = into.artboard_mut(&board_id) {
        ab.vars.insert(
            "template".into(),
            json!({ "name": spec.name, "vars": new_defs }),
        );
    }

    // Optional variables passed inline.
    if let Some(vars) = opts.get("vars") {
        if !vars.is_null() {
            apply_variables(into, vars)?;
        }
    }
    Ok(ids)
}

fn make_board(meta: &BoardMeta, opts: &Value) -> Artboard {
    let w = util::f(opts, "w").unwrap_or(meta.w);
    let h = util::f(opts, "h").unwrap_or(meta.h);
    Artboard {
        id: util::id("b"),
        name: util::ps(opts, "name").map(|s| s.to_string()).or_else(|| meta.name.clone()).unwrap_or_else(|| "board".into()),
        width: w,
        height: h,
        background: meta.background.clone(),
        visible: true,
        layers: vec![],
        safe_margin: meta.safe_margin,
        guides: vec![],
        ratio: meta.ratio.clone(),
        role: None,
        template_id: None,
        vars: serde_json::Map::new(),
    }
}

/// Clone a set of Node-shaped JSON objects into `doc` under a fresh id stamp,
/// rewriting every internal reference. Returns the new ids, the old->new map and
/// the top-level layer order.
fn clone_objs_into(doc: &mut Document, board_id: &str, objs: &[Value]) -> (Vec<String>, HashMap<String, String>, Vec<String>) {
    let stamp = util::id("t");
    let mut map: HashMap<String, String> = HashMap::new();
    let mut ids = Vec::new();
    let mut working: Vec<Value> = Vec::with_capacity(objs.len());
    for (i, obj) in objs.iter().enumerate() {
        let old = obj.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let nid = format!("{stamp}-{i}");
        if !old.is_empty() {
            map.insert(old, nid.clone());
        }
        ids.push(nid.clone());
        let mut o = obj.clone();
        o["id"] = json!(nid);
        o["artboard"] = json!(board_id);
        working.push(o);
    }
    // Second pass: rewrite references now that the full map exists.
    let mut layers = Vec::new();
    let mut nodes = Vec::new();
    for mut o in working {
        rewrite_refs(&mut o, &map);
        let is_top = o.get("parent").and_then(|p| p.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
        if let Some(n) = parse_node(o) {
            if !is_top {
                layers.push(n.id.clone());
            }
            nodes.push(n);
        }
    }
    for n in nodes {
        doc.nodes.push(n);
    }
    (ids, map, layers)
}

/// Rewrite `parent`, `children`, and every layout-intent node reference from
/// template-local ids to their fresh ids.
fn rewrite_refs(o: &mut Value, map: &HashMap<String, String>) {
    if let Some(p) = o.get("parent").and_then(|v| v.as_str()).map(|s| s.to_string()) {
        if let Some(np) = map.get(&p) {
            o["parent"] = json!(np);
        }
    }
    if let Some(ch) = o.get_mut("children").and_then(|v| v.as_array_mut()) {
        for c in ch.iter_mut() {
            if let Some(s) = c.as_str().map(|x| x.to_string()) {
                if let Some(ns) = map.get(&s) {
                    *c = json!(ns);
                }
            }
        }
    }
    if let Some(by) = o.get("clip").and_then(|c| c.get("by")).and_then(|v| v.as_str()).map(|s| s.to_string()) {
        if let Some(nb) = map.get(&by) {
            if let Some(cl) = o.get_mut("clip") {
                cl["by"] = json!(nb);
            }
        }
    }
    let intent = match o.get_mut("layout").and_then(|l| l.get_mut("intent")).and_then(|i| i.as_object_mut()) {
        Some(i) => i,
        None => return,
    };
    for key in ["above", "below", "left_of", "right_of"] {
        if let Some(rel) = intent.get_mut(key).and_then(|r| r.as_object_mut()) {
            if let Some(of) = rel.get("of").and_then(|v| v.as_str()).map(|s| s.to_string()) {
                if let Some(nof) = map.get(&of) {
                    rel["of"] = json!(nof);
                }
            }
        }
    }
    if let Some(at) = intent.get_mut("align_to").and_then(|r| r.as_object_mut()) {
        if let Some(of) = at.get("of").and_then(|v| v.as_str()).map(|s| s.to_string()) {
            if let Some(nof) = map.get(&of) {
                at["of"] = json!(nof);
            }
        }
    }
    if let Some(ci) = intent.get("center_in").and_then(|v| v.as_str()).map(|s| s.to_string()) {
        if let Some(nci) = map.get(&ci) {
            intent["center_in"] = json!(nci);
        }
    }
}

fn parse_node(o: Value) -> Option<Node> {
    serde_json::from_value::<Node>(o).ok()
}

// ------------------------------------------------------------------ keypath setter

/// Set one field of one node by keypath with type checking.
fn set_node_value(doc: &mut Document, node: &str, keypath: &str, val: &Value, kind: &str) -> R<(String, ())> {
    // Snapshot the tokens first: `$token` expansion needs them while we hold a
    // mutable borrow of the node.
    let tokens = doc.tokens.clone();
    let n = doc
        .node_mut(node)
        .ok_or_else(|| Fail::new("NODE_NOT_FOUND", format!("no node \"{node}\"")))?;
    set_field(n, keypath, val, kind, &tokens)?;
    Ok((node.to_string(), ()))
}

/// Apply a field on an already-borrowed node (used by variants where we re-look-up
/// the node through the document).
fn set_field(n: &mut Node, keypath: &str, val: &Value, kind: &str, tokens: &doc::Tokens) -> R<()> {
    // `kind` is a hint from the template var declaration; the keypath itself
    // carries the authoritative type check in each arm below.
    let _ = kind;
    match keypath {
        "box.x" => n.box_.x = as_num(val, keypath)?,
        "box.y" => n.box_.y = as_num(val, keypath)?,
        "box.w" => n.box_.w = as_num(val, keypath)?,
        "box.h" => n.box_.h = as_num(val, keypath)?,
        "opacity" => n.opacity = as_num(val, keypath)?.clamp(0.0, 1.0),
        "rotation" => n.rotation = as_num(val, keypath)?,
        "name" => n.name = as_str(val, keypath)?.to_string(),
        "text.content" => ensure_text(n).content = as_str(val, keypath)?.to_string(),
        "text.size" => ensure_text(n).size = Some(as_num(val, keypath)?),
        "text.line_height" => ensure_text(n).line_height = Some(as_num(val, keypath)?),
        "text.line_ratio" => ensure_text(n).line_ratio = Some(as_num(val, keypath)?),
        "text.letter_spacing" => ensure_text(n).letter_spacing = Some(as_num(val, keypath)?),
        "text.weight" => ensure_text(n).weight = Some(as_num(val, keypath)? as u16),
        "text.color" => ensure_text(n).color = Some(as_color(val, keypath, tokens)?),
        "text.family" => ensure_text(n).family = Some(as_str(val, keypath)?.to_string()),
        "text.align" => ensure_text(n).align = Some(as_str(val, keypath)?.to_string()),
        "text.valign" => ensure_text(n).valign = Some(as_str(val, keypath)?.to_string()),
        "text.overflow" => ensure_text(n).overflow = Some(as_str(val, keypath)?.to_string()),
        "text.box_mode" => ensure_text(n).box_mode = Some(as_str(val, keypath)?.to_string()),
        "image.asset" => ensure_image(n).asset = as_str(val, keypath)?.to_string(),
        "image.fit" => {
            let f = as_str(val, keypath)?;
            let fit = crate::geom::fit_of(f).ok_or_else(|| Fail::new("INVALID_VALUE", format!("\"{f}\" is not a fit mode")))?;
            ensure_image(n).fit = fit;
        }
        "style.fill.color" => {
            let c = as_color(val, keypath, tokens)?;
            ensure_style(n).fill = Some(Paint::Solid { color: c });
        }
        "style.color" => {
            let c = as_color(val, keypath, tokens)?;
            ensure_style(n).color = Some(c);
        }
        "style.stroke.color" => {
            let c = as_color(val, keypath, tokens)?;
            let st = ensure_style(n);
            let stroke = st.stroke.get_or_insert_with(|| doc::Stroke { color: c.clone(), width: 1.0, align: None, dash: vec![], cap: None });
            stroke.color = c;
        }
        "style.radius.all" => {
            let r = as_num(val, keypath)?;
            let st = ensure_style(n);
            let rad = st.radius.get_or_insert(Radius::default());
            rad.all = r;
        }
        _ => {
            return Err(Fail::new("INVALID_SCHEMA", format!("unsupported var keypath \"{keypath}\""))
                .hint("use text.content / image.asset / style.fill.color / box.w / box.h / opacity / ..."));
        }
    }
    Ok(())
}

/// Infer the expected value category from a keypath (used when a raw keypath is
/// supplied without an explicit kind).
fn kind_for_keypath(keypath: &str) -> String {
    if keypath.contains("color") || keypath.ends_with(".fill") {
        "color".to_string()
    } else if keypath.starts_with("box.")
        || matches!(keypath, "opacity" | "rotation" | "text.size" | "text.line_height" | "text.line_ratio" | "text.letter_spacing" | "text.weight" | "style.radius.all")
    {
        "number".to_string()
    } else {
        "string".to_string()
    }
}

fn as_num(val: &Value, keypath: &str) -> R<f64> {
    match val {
        Value::Number(_) => Ok(val.as_f64().unwrap_or(0.0)),
        Value::String(s) => s.trim().parse::<f64>().map_err(|_| Fail::new("INVALID_VALUE", format!("value for {keypath} is not a number: \"{s}\""))),
        other => Err(Fail::new("INVALID_VALUE", format!("value for {keypath} must be a number, got {}", type_name(other)))),
    }
}

fn as_str<'v>(val: &'v Value, keypath: &str) -> R<&'v str> {
    match val {
        Value::String(s) => Ok(s),
        other => Err(Fail::new("INVALID_VALUE", format!("value for {keypath} must be a string, got {}", type_name(other)))),
    }
}

fn as_color(val: &Value, keypath: &str, tokens: &doc::Tokens) -> R<String> {
    let s = as_str(val, keypath)?;
    let expanded = tokens.expand(s);
    if doc::parse_color(&expanded).is_none() {
        return Err(Fail::new("INVALID_VALUE", format!("value for {keypath} is not a colour: \"{s}\"")));
    }
    Ok(s.to_string())
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn ensure_text(n: &mut Node) -> &mut doc::TextSpec {
    if n.text.is_none() {
        n.text = Some(doc::TextSpec {
            content: String::new(),
            family: None,
            fallback: None,
            size: Some(28.0),
            weight: None,
            style_italic: None,
            color: None,
            line_height: None,
            line_ratio: Some(1.3),
            letter_spacing: None,
            align: Some("left".into()),
            valign: Some("top".into()),
            box_mode: None,
            overflow: None,
            max_font_size: None,
            min_font_size: None,
            uppercase: None,
            rotate_each: None,
            lines: vec![],
            font_ref: None,
            paint: None,
            stroke: None,
            bg: None,
            padding: None,
        });
    }
    n.text.as_mut().unwrap()
}

fn ensure_style(n: &mut Node) -> &mut Style {
    if n.style.is_none() {
        n.style = Some(Style::default());
    }
    n.style.as_mut().unwrap()
}

fn ensure_image(n: &mut Node) -> &mut doc::ImageSpec {
    if n.image.is_none() {
        n.image = Some(doc::ImageSpec {
            asset: String::new(),
            fit: Fit::Cover,
            crop: None,
            brightness: None,
            contrast: None,
            saturate: None,
            hue: None,
            blur: None,
            grayscale: None,
            tint: None,
            position: None,
            opacity: None,
            tile: None,
        });
    }
    n.image.as_mut().unwrap()
}

// ------------------------------------------------------------------ helpers

fn collect_defs(doc: &Document) -> Vec<VarDef> {
    let mut out = Vec::new();
    for ab in &doc.artboards {
        if let Some(t) = ab.vars.get("template") {
            if let Some(vars) = t.get("vars").and_then(|v| v.as_array()) {
                for d in vars {
                    if let Ok(def) = serde_json::from_value::<VarDef>(d.clone()) {
                        out.push(def);
                    }
                }
            }
        }
    }
    out
}

fn read_copy(v: Option<&Value>) -> HashMap<String, Vec<Value>> {
    let mut m: HashMap<String, Vec<Value>> = HashMap::new();
    match v {
        Some(Value::Object(o)) => {
            for (k, val) in o {
                if let Some(arr) = val.as_array() {
                    m.insert(k.clone(), arr.clone());
                } else {
                    m.insert(k.clone(), vec![val.clone()]);
                }
            }
        }
        Some(Value::Array(items)) => {
            // array of per-variant objects -> pivot into key columns
            let mut keys: Vec<String> = Vec::new();
            for it in items {
                if let Some(o) = it.as_object() {
                    for k in o.keys() {
                        if !keys.contains(k) {
                            keys.push(k.clone());
                        }
                    }
                }
            }
            for k in &keys {
                m.insert(k.clone(), items.iter().map(|it| it.get(k).cloned().unwrap_or(Value::Null)).collect());
            }
            // array of scalars -> treat as a single "title" column
            if keys.is_empty() {
                m.insert("title".into(), items.clone());
            }
        }
        _ => {}
    }
    m
}

fn variant_value(v: usize, d: &VarDef, n: usize, colors: &[Value], copy: &HashMap<String, Vec<Value>>, assets: &[Value]) -> Option<Value> {
    if let Some(col) = copy.get(&d.key) {
        if !col.is_empty() {
            return col.get(v % col.len()).cloned();
        }
    }
    // Copy arrays keyed by common role names fall through to matching vars.
    if d.kind == "color" && !colors.is_empty() {
        return colors.get(v % colors.len()).cloned();
    }
    if (d.kind == "image" || d.keypath == "image.asset") && !assets.is_empty() {
        return assets.get(v % assets.len()).cloned();
    }
    if matches!(d.key.as_str(), "accent" | "color" | "colour") && !colors.is_empty() {
        return colors.get(v % colors.len()).cloned();
    }
    if matches!(d.key.as_str(), "image" | "asset") && !assets.is_empty() {
        return assets.get(v % assets.len()).cloned();
    }
    let _ = n;
    None
}

fn image_sources(spec: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(a) = spec.get("images").and_then(|v| v.as_array()) {
        for s in a {
            if let Some(p) = s.as_str() {
                out.push(p.to_string());
            }
        }
    }
    if let Some(s) = util::ps(spec, "image") {
        if !out.iter().any(|x| x == s) {
            out.push(s.to_string());
        }
    }
    if let Some(a) = spec.get("image").and_then(|v| v.as_array()) {
        for s in a {
            if let Some(p) = s.as_str() {
                if !out.iter().any(|x| x == p) {
                    out.push(p.to_string());
                }
            }
        }
    }
    out
}

fn resolve_size(spec: &Value) -> R<(f64, f64, Option<String>)> {
    if let Some(p) = util::ps(spec, "preset") {
        if let Some((w, h, _)) = geom::preset(p) {
            return Ok((w, h, Some(p.to_string())));
        }
        if let Some((w, h)) = geom::parse_ratio(p, 1080.0) {
            return Ok((w, h, Some(p.to_string())));
        }
        return Err(Fail::new("INVALID_VALUE", format!("unknown preset \"{p}\"")).hint("e.g. poster|square|story|banner|xhs|ecom-main|long|wechat|card|a4"));
    }
    if let Some(r) = util::ps(spec, "ratio") {
        let base = util::f(spec, "w").unwrap_or(1080.0);
        let (w, h) = geom::parse_ratio(r, base).ok_or_else(|| Fail::new("INVALID_VALUE", format!("bad ratio \"{r}\"")))?;
        return Ok((w, h, None));
    }
    let w = util::f(spec, "w").unwrap_or(1080.0);
    let h = util::f(spec, "h").unwrap_or(1440.0);
    Ok((w, h, None))
}

fn default_layout(preset: Option<&str>, w: f64, h: f64) -> String {
    if let Some(p) = preset {
        let n = p.to_lowercase();
        if n.contains("story") || n.contains("9:16") || n.contains("reel") {
            return "story_ad".into();
        }
        if n.contains("ecom") || n.contains("square") {
            return "ecom_main".into();
        }
        if n.contains("banner") || n.contains("16:9") {
            return "banner_promo".into();
        }
        if n.contains("long") {
            return "long_image".into();
        }
        if n.contains("card") {
            return "card_quote".into();
        }
    }
    if (w - h).abs() < 1.0 {
        return "ecom_main".into();
    }
    if w > h {
        return "banner_promo".into();
    }
    "poster_kv".into()
}

fn parse_format(s: &str) -> Format {
    match s.to_lowercase().as_str() {
        "jpg" | "jpeg" => Format::Jpeg,
        "webp" => Format::Webp,
        "svg" => Format::Svg,
        "pdf" => Format::Pdf,
        _ => Format::Png,
    }
}

fn fixes_for(report: &Value, codes: &[&str]) -> Vec<Cmd> {
    let mut out = Vec::new();
    if let Some(issues) = report.get("issues").and_then(|v| v.as_array()) {
        for i in issues {
            let code = i.get("c").and_then(|v| v.as_str()).unwrap_or("");
            if codes.contains(&code) {
                if let Some(f) = i.get("f") {
                    if let Ok(c) = serde_json::from_value::<Cmd>(f.clone()) {
                        out.push(c);
                    }
                }
            }
        }
    }
    out
}

fn rendered_json(r: &render::Rendered) -> Value {
    let mut v = json!({
        // `path` is `Option<String>` in render.rs but the contract documented
        // `Option<PathBuf>`; `format!` through Display works for both.
        "path": r.path.as_ref().map(|p| format!("{p}").replace('\\', "/")).unwrap_or_default(),
        "f": r.format,
        "w": r.width,
        "h": r.height,
        "bytes": r.bytes,
        "sha": r.sha256,
        "mime": r.mime,
    });
    // Only when the renderer had to compromise: a poster that quietly lost its photo
    // is worse than one that says so, and an empty list is not worth its tokens.
    if !r.warns.is_empty() {
        v["warns"] = json!(r.warns);
    }
    v
}

fn merge_tokens(doc: &mut Document, add: &doc::Tokens) {
    for c in &add.colors {
        if !doc.tokens.colors.iter().any(|x| x.name == c.name) {
            doc.tokens.colors.push(c.clone());
        }
    }
    for f in &add.fonts {
        if !doc.tokens.fonts.iter().any(|x| x.name == f.name) {
            doc.tokens.fonts.push(FontToken { name: f.name.clone(), family: f.family.clone(), weight: f.weight, size: f.size, line_height: f.line_height, letter_spacing: f.letter_spacing });
        }
    }
}

fn candidate_key(name: &str, fallback: &str, id: &str, used: &mut HashMap<String, u32>) -> String {
    let base = name.trim().to_lowercase().replace(' ', "_");
    let base = if base.is_empty() { format!("{fallback}_{}", util::short_hash(id)) } else { base };
    let c = used.entry(base.clone()).or_insert(0);
    *c += 1;
    if *c > 1 {
        format!("{base}{c}")
    } else {
        base
    }
}

fn push_once(v: &mut Vec<String>, id: String) {
    if !v.contains(&id) {
        v.push(id);
    }
}

fn templates_dir(dir: &Path) -> PathBuf {
    dir.join("templates")
}

fn template_path(dir: &Path, name: &str) -> PathBuf {
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    templates_dir(dir).join(format!("{safe}.json"))
}

// ------------------------------------------------------------------ unit tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_refs_maps_sibling_ids() {
        let mut map = HashMap::new();
        map.insert("a".to_string(), "X-0".to_string());
        map.insert("b".to_string(), "X-1".to_string());
        let mut o = json!({
            "id": "X-1",
            "type": "text",
            "box_": { "x": 0.0, "y": 0.0, "w": 10.0, "h": 10.0 },
            "layout": { "mode": "constraint", "intent": { "below": { "of": "a" }, "center_in": "a" } },
        });
        rewrite_refs(&mut o, &map);
        assert_eq!(o["layout"]["intent"]["below"]["of"], json!("X-0"));
        assert_eq!(o["layout"]["intent"]["center_in"], json!("X-0"));
    }

    #[test]
    fn clone_objs_disjoint_and_layers() {
        let mut doc = Document::default();
        doc.artboards.push(Artboard {
            id: "b1".into(),
            name: "b1".into(),
            width: 100.0,
            height: 100.0,
            background: None,
            visible: true,
            layers: vec![],
            safe_margin: None,
            guides: vec![],
            ratio: None,
            role: None,
            template_id: None,
            vars: serde_json::Map::new(),
        });
        let objs = vec![
            json!({ "id": "top", "type": "text", "box_": { "x": 0.0, "y": 0.0, "w": 50.0, "h": 20.0 }, "text": { "content": "hi", "lines": [] } }),
            json!({ "id": "kid", "type": "rect", "box_": { "x": 0.0, "y": 0.0, "w": 10.0, "h": 10.0 }, "parent": "top" }),
        ];
        let (ids, _map, layers) = clone_objs_into(&mut doc, "b1", &objs);
        assert_eq!(ids.len(), 2);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0], ids[0]);
        // The kid's parent now points at the new top id, not "top".
        let kid = doc.node(&ids[1]).unwrap();
        assert_eq!(kid.parent.as_deref().unwrap(), ids[0].as_str());
    }

    #[test]
    fn keypath_type_checks() {
        let mut doc = Document::default();
        doc.nodes.push({
            let mut n = Node::new("n1", doc::NodeType::Rect, geom::Rect::new(0.0, 0.0, 10.0, 10.0));
            n.artboard = None;
            n
        });
        // number landed in a colour field
        let err = set_node_value(&mut doc, "n1", "style.fill.color", &json!(42), "color");
        assert!(err.is_err());
        assert_eq!(err.err().unwrap().code, "INVALID_VALUE");
        // valid colour sets
        set_node_value(&mut doc, "n1", "style.fill.color", &json!("#ff5a5f"), "color").unwrap();
        assert!(matches!(doc.node("n1").unwrap().style.as_ref().unwrap().fill, Some(Paint::Solid { .. })));
    }

    #[test]
    fn builtins_are_eight() {
        let names: Vec<&str> = builtin().iter().map(|b| b.0).collect();
        for want in ["poster_kv", "ecom_main", "story_ad", "card_quote", "banner_promo", "cover_editorial", "grid_collage", "long_image"] {
            assert!(names.contains(&want), "missing builtin {want}");
        }
    }
}
