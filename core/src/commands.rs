//! Every mutation in the product is one of these commands — from the desktop UI,
//! from HTTP, from MCP or from the CLI. That is what makes the three doors behave
//! identically, and what makes every edit reversible: each command knows how to
//! describe its own inverse.
//!
//! Shape of the wire format (`serde` tag `"t"`):
//! `{"t":"NodeAdd","board":"b1","node":{"id":"logo","type":"image","box":{...}}}`
//!
//! ```text
//! command            undo produced
//! -------            -----------------------------------------------------------
//! NodeAdd     ->     NodeDel{id}
//! NodeDel     ->     NodeAdd (plus one per detached descendant, as a Composite)
//! NodeSet     ->     NodeSet with a *minimal* patch of only the previous values
//! NodesSet    ->     one NodeSet per node that actually changed (Composite)
//! NodeMove    ->     NodeMove back to the previous container and index
//! BoardAdd    ->     BoardDel carrying the artboard that was removed
//! BoardDel    ->     Composite: BoardAdd, then the cascaded nodes re-added
//! BoardSet    ->     BoardSet, minimal previous-value patch
//! AssetAdd    ->     AssetDel carrying the previous record
//! AssetDel    ->     AssetAdd
//! TokenSet    ->     TokenSet with the previous value (`null` when it was absent)
//! Text        ->     Text with the previous content
//! Transform   ->     Transform with the previous box, then a NodeSet restoring the
//!                    previous `layout.mode`/`pinned` (a transform means "a human put
//!                    this here", so it marks the node `manual` + `pinned`; undo
//!                    hands it back to the solver)
//! LayoutIntent->     LayoutIntent with the previous intent and mode
//! Style       ->     Style with the previous style object (`null` when absent)
//! Group       ->     Ungroup{the new group id}
//! Ungroup     ->     Group{the children, in order} — a fresh group id
//! Duplicate   ->     NodeDel of each created root (subtrees go with them)
//! Reflow      ->     Composite of per-node Transform of the pre-reflow boxes
//! Solve       ->     Composite of per-node Transform of the pre-solve boxes
//! Composite   ->     Composite of the member undos, in reverse order
//! ```
//!
//! Two rules hold everywhere:
//! * `apply` bumps `doc.revision` **exactly once** per top-level command; nested
//!   commands inside a `Composite` do not bump it again.
//! * Unknown ids are refused with `NODE_NOT_FOUND` / `ARTBOARD_NOT_FOUND` /
//!   `ASSET_NOT_FOUND`, always with the id in `details`, and nothing is mutated.

use crate::doc::{self, Artboard, Asset, ColorToken, Document, FontToken, Node, NodeType, NumToken};
use crate::error::{Fail, R};
use crate::geom::Rect;
use crate::util;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;

/// One reversible edit. The variant names and fields are the wire format.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "t")]
pub enum Cmd {
    NodeAdd { node: Node, board: String, at: Option<usize> },
    NodeDel { id: String },
    /// Deep merge onto the node; `null` deletes a key.
    NodeSet { id: String, patch: Value },
    NodesSet { ids: Vec<String>, patch: Value },
    NodeMove { id: String, board: String, to: usize, parent: Option<String> },
    BoardAdd { board: Artboard },
    /// Carries the removed artboard so the undo is lossless (`board` is required
    /// by serde; see the deviations note in the module docs).
    BoardDel { id: String, board: Artboard },
    BoardSet { id: String, patch: Value },
    AssetAdd { asset: Asset },
    AssetDel { id: String, asset: Asset },
    TokenSet { group: String, name: String, value: Value },
    Text { id: String, content: String },
    Transform { id: String, x: f64, y: f64, w: f64, h: f64 },
    LayoutIntent { id: String, intent: Value, mode: String },
    Style { id: String, style: Value },
    Group { ids: Vec<String>, board: String },
    Ungroup { id: String },
    Duplicate { ids: Vec<String> },
    Reflow { board: String, w: f64, h: f64, mode: String },
    Solve { board: Option<String> },
    /// One transaction: all members apply, or none of them does.
    Composite { cmds: Vec<Cmd> },
}

/// What a command did, and how to take it back.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Applied {
    /// Node / artboard / asset ids touched, in the order they were touched.
    pub changed: Vec<String>,
    /// The exact inverse. Apply it through [`apply`] to undo.
    pub undo: Cmd,
    /// Non-fatal notes (layout conflicts, tokens that did not resolve).
    pub warnings: Vec<String>,
    /// Ids created by the command (new nodes, new group, duplicated roots).
    pub created: Vec<String>,
    /// `doc.revision` after the command.
    pub revision: u64,
}

/// Who is asking, and how strict to be.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ApplyCtx {
    /// Project directory: where assets and exports live for this command.
    pub dir: PathBuf,
    /// Bypass the `locked` guard (`asset.remove force`, `canvas.delete force`).
    pub force: bool,
    /// `desktop` | `mcp` | `http` | `cli`; recorded into `doc.meta.origin`.
    pub origin: String,
}

impl ApplyCtx {
    pub fn new(dir: impl Into<PathBuf>, origin: &str) -> Self {
        Self { dir: dir.into(), force: false, origin: origin.to_string() }
    }
    pub fn force(mut self, f: bool) -> Self {
        self.force = f;
        self
    }
}

impl Default for ApplyCtx {
    fn default() -> Self {
        Self { dir: PathBuf::new(), force: false, origin: "desktop".into() }
    }
}

// ------------------------------------------------------------------- entry point

/// Apply one command and return its inverse.
///
/// Bumps `doc.revision` exactly once, records `ctx.origin` into `doc.meta.origin`
/// and flags the open project (if any) as needing a save. On error the document is
/// unchanged, including for a `Composite`, which is applied to a clone and only
/// swapped in when every member succeeded.
pub fn apply(doc: &mut Document, cmd: &Cmd, ctx: &ApplyCtx) -> R<Applied> {
    let mut a = inner(doc, cmd, ctx)?;
    doc.revision = doc.revision.saturating_add(1);
    doc.meta.origin = Some(if ctx.origin.is_empty() { "desktop".into() } else { ctx.origin.clone() });
    a.revision = doc.revision;
    if !doc.project_id.is_empty() {
        crate::session::mark_changed(&doc.project_id);
    }
    Ok(a)
}

/// Apply several commands as one transaction with one revision bump.
pub fn apply_many(doc: &mut Document, cmds: &[Cmd], ctx: &ApplyCtx) -> R<Applied> {
    apply(doc, &Cmd::Composite { cmds: cmds.to_vec() }, ctx)
}

/// Parse a command from RPC JSON, turning a serde failure into a readable
/// `INVALID_SCHEMA` that names the tag it could not match.
pub fn parse(v: &Value) -> R<Cmd> {
    serde_json::from_value::<Cmd>(v.clone()).map_err(|e| {
        Fail::new("INVALID_SCHEMA", format!("cmd: {e}"))
            .hint("expect {\"t\":\"<Variant>\",...fields}; see canvas.batch for the shapes")
            .details(json!({
                "t": v.get("t"),
                "variants": ["NodeAdd","NodeDel","NodeSet","NodesSet","NodeMove","BoardAdd","BoardDel",
                    "BoardSet","AssetAdd","AssetDel","TokenSet","Text","Transform","LayoutIntent","Style",
                    "Group","Ungroup","Duplicate","Reflow","Solve","Composite"],
            }))
    })
}

/// Apply each command in order; a failure leaves `doc` untouched.
fn inner(doc: &mut Document, cmd: &Cmd, ctx: &ApplyCtx) -> R<Applied> {
    match cmd {
        Cmd::Composite { cmds } => composite(doc, cmds, ctx),
        other => single(doc, other, ctx),
    }
}

fn composite(doc: &mut Document, cmds: &[Cmd], ctx: &ApplyCtx) -> R<Applied> {
    if cmds.is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "Composite needs at least one command"));
    }
    // Transaction: work on a clone, publish only when every member succeeded.
    let mut work = doc.clone();
    let mut changed = Vec::new();
    let mut created = Vec::new();
    let mut warnings = Vec::new();
    let mut undos = Vec::new();
    for (i, m) in cmds.iter().enumerate() {
        let a = inner(&mut work, m, ctx).map_err(|e| e.details(json!({"index": i, "cmd": m.name()})))?;
        changed.extend(a.changed);
        created.extend(a.created);
        warnings.extend(a.warnings);
        undos.push(a.undo);
    }
    undos.reverse();
    *doc = work;
    Ok(Applied {
        changed: dedup(changed),
        created: dedup(created),
        warnings: dedup(warnings),
        undo: Cmd::Composite { cmds: undos },
        revision: doc.revision,
    })
}

fn single(doc: &mut Document, cmd: &Cmd, ctx: &ApplyCtx) -> R<Applied> {
    match cmd {
        Cmd::NodeAdd { node, board, at } => node_add(doc, node, board, *at, ctx),
        Cmd::NodeDel { id } => node_del(doc, id, ctx),
        Cmd::NodeSet { id, patch } => node_set(doc, id, patch, ctx),
        Cmd::NodesSet { ids, patch } => nodes_set(doc, ids, patch, ctx),
        Cmd::NodeMove { id, board, to, parent } => node_move(doc, id, board, *to, parent.as_deref(), ctx),
        Cmd::BoardAdd { board } => board_add(doc, board),
        Cmd::BoardDel { id, board } => board_del(doc, id, board, ctx),
        Cmd::BoardSet { id, patch } => board_set(doc, id, patch, ctx),
        Cmd::AssetAdd { asset } => asset_add(doc, asset),
        Cmd::AssetDel { id, asset } => asset_del(doc, id, asset, ctx),
        Cmd::TokenSet { group, name, value } => token_set(doc, group, name, value),
        Cmd::Text { id, content } => text_set(doc, id, content, ctx),
        Cmd::Transform { id, x, y, w, h } => transform(doc, id, *x, *y, *w, *h, ctx),
        Cmd::LayoutIntent { id, intent, mode } => layout_intent(doc, id, intent, mode, ctx),
        Cmd::Style { id, style } => style_set(doc, id, style, ctx),
        Cmd::Group { ids, board } => group(doc, ids, board, ctx),
        Cmd::Ungroup { id } => ungroup(doc, id, ctx),
        Cmd::Duplicate { ids } => duplicate(doc, ids, ctx),
        Cmd::Reflow { board, w, h, mode } => reflow(doc, board, *w, *h, mode, ctx),
        Cmd::Solve { board } => solve_cmd(doc, board.as_deref(), ctx),
        Cmd::Composite { cmds } => composite(doc, cmds, ctx),
    }
}

// ------------------------------------------------------------------- lookups

fn node_idx(doc: &Document, id: &str) -> R<usize> {
    doc.nodes.iter().position(|n| n.id == id).ok_or_else(|| {
        Fail::new("NODE_NOT_FOUND", format!("no node \"{id}\""))
            .hint("canvas.add to create it, or check the id against project.get")
            .details(json!({"id": id}))
    })
}

fn board_idx(doc: &Document, id: &str) -> R<usize> {
    doc.artboards.iter().position(|a| a.id == id).ok_or_else(|| {
        Fail::new("ARTBOARD_NOT_FOUND", format!("no artboard \"{id}\""))
            .hint("artboard.list shows the ids in this project")
            .details(json!({"id": id}))
    })
}

fn asset_idx(doc: &Document, id: &str) -> R<usize> {
    doc.assets.iter().position(|a| a.id == id).ok_or_else(|| {
        Fail::new("ASSET_NOT_FOUND", format!("no asset \"{id}\""))
            .hint("asset.import first, then use the returned id")
            .details(json!({"id": id}))
    })
}

/// Refuse to touch a locked node unless the caller forced it.
fn guard(doc: &Document, id: &str, ctx: &ApplyCtx) -> R<()> {
    if ctx.force {
        return Ok(());
    }
    let i = node_idx(doc, id)?;
    if doc.nodes[i].locked {
        return Err(locked_err(id));
    }
    Ok(())
}

fn locked_err(id: &str) -> Fail {
    Fail::new("LOCKED", format!("node \"{id}\" is locked"))
        .hint("layout.lock {id, pinned:0} first, or pass force:true")
        .details(json!({"id": id}))
}

/// Lock guard over a whole subtree (delete, duplicate, group).
fn guard_tree(doc: &Document, ids: &[String], ctx: &ApplyCtx) -> R<()> {
    if ctx.force {
        return Ok(());
    }
    for id in ids {
        node_idx(doc, id)?;
        for d in doc.subtree(id) {
            if doc.node(&d).map(|n| n.locked).unwrap_or(false) {
                return Err(locked_err(&d));
            }
        }
    }
    Ok(())
}

fn node_value(n: &Node) -> R<Value> {
    serde_json::to_value(n).map_err(|e| Fail::new("INTERNAL", format!("node to json: {e}")))
}

fn board_value(a: &Artboard) -> R<Value> {
    serde_json::to_value(a).map_err(|e| Fail::new("INTERNAL", format!("artboard to json: {e}")))
}

fn from_value<T: serde::de::DeserializeOwned>(v: Value, what: &str) -> R<T> {
    serde_json::from_value(v).map_err(|e| {
        Fail::new("INVALID_SCHEMA", format!("{what}: {e}")).hint("check the field names against doc.rs")
    })
}

// ------------------------------------------------------------------- patches

/// Deep merge where `null` deletes a key (same rule as `session::merge_patch`, kept
/// here so commands have no layer-1 dependency).
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

/// A patch that writes nothing at all: `{}` or `[]`.
fn is_empty_patch(v: &Value) -> bool {
    match v {
        Value::Object(o) => o.is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

/// The smallest patch that turns `after` back into `before`.
///
/// Only keys that actually differ appear, and nested objects stay nested so the
/// result can be fed straight into [`merge_patch`]. A key `after` gained is `null`
/// (delete it); a key `after` lost carries its old value.
pub fn undo_patch(before: &Value, after: &Value) -> Value {
    if before == after {
        return json!({});
    }
    match (before, after) {
        (Value::Object(b), Value::Object(a)) => {
            let mut m = Map::new();
            for (k, av) in a {
                match b.get(k) {
                    None => {
                        m.insert(k.clone(), Value::Null);
                    }
                    Some(bv) => {
                        let sub = undo_patch(bv, av);
                        // An empty sub-patch means the whole subtree already matches,
                        // so writing it would record a change that is not one.
                        if !sub.is_null() && !is_empty_patch(&sub) {
                            m.insert(k.clone(), sub);
                        }
                    }
                }
            }
            for (k, bv) in b {
                if !a.contains_key(k) {
                    m.insert(k.clone(), bv.clone());
                }
            }
            Value::Object(m)
        }
        _ => before.clone(),
    }
}

/// Keys that carry a colour, in any nesting.
fn color_keys(k: &str) -> bool {
    matches!(k, "color" | "tint" | "inner_glow" | "innerGlow" | "background" | "fill" | "stroke")
}

/// Walk a merged value and return the first colour string that does not parse.
fn bad_color(tokens: &doc::Tokens, v: &Value, path: &str, hits: &mut Vec<(String, String)>) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                match x {
                    Value::String(s) => {
                        let looks = color_keys(k) || s.starts_with('#') || s.starts_with("rgb");
                        if looks {
                            let resolved = if s.trim_start().starts_with('$') {
                                match tokens.color(s.trim()) {
                                    Some(v) => v,
                                    // An unresolvable token is inspect's problem, not
                                    // a colour syntax error.
                                    None => {
                                        hits.push((p, s.clone()));
                                        continue;
                                    }
                                }
                            } else {
                                tokens.expand(s)
                            };
                            if doc::parse_color(&resolved).is_none() {
                                hits.push((p, s.clone()));
                            }
                        }
                    }
                    other => bad_color(tokens, other, &p, hits),
                }
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                bad_color(tokens, x, &format!("{path}[{i}]"), hits);
            }
        }
        _ => {}
    }
}

fn check_colors(tokens: &doc::Tokens, v: &Value, id: &str) -> R<()> {
    let mut hits = Vec::new();
    bad_color(tokens, v, "", &mut hits);
    match hits.into_iter().next() {
        Some((key, value)) => Err(Fail::new("INVALID_VALUE", format!("\"{value}\" is not a colour"))
            .hint("use #rgb, #rrggbb, #rrggbbaa, rgb()/rgba(), white|black|red|transparent, or $token")
            .details(json!({"id": id, "key": key, "value": value}))),
        None => Ok(()),
    }
}

/// Numeric sanity: no NaN, no infinite, no negative sizes. `INVALID_VALUE` names the
/// key and carries the offending value, so the caller can see what it sent.
fn check_num(v: &Value, id: &str, key: &str) -> R<()> {
    if let Some(x) = v.as_f64() {
        if !x.is_finite() {
            return Err(Fail::new("INVALID_VALUE", format!("{key} must be finite")).details(json!({"id": id, "key": key, "value": x})));
        }
    }
    Ok(())
}

fn validate_node(n: &Node, tokens: &doc::Tokens) -> R<()> {
    let id = n.id.clone();
    if !n.box_.is_finite() {
        return Err(Fail::new("INVALID_VALUE", "box must be finite").details(json!({"id": id, "box": n.box_})));
    }
    if n.box_.w < 0.0 || n.box_.h < 0.0 {
        return Err(Fail::new("INVALID_VALUE", "box w/h cannot be negative")
            .hint("use a size of 0 or more; negative w/h is not a flip")
            .details(json!({"id": id, "w": n.box_.w, "h": n.box_.h})));
    }
    if !(0.0..=1.0).contains(&n.opacity) {
        return Err(Fail::new("INVALID_VALUE", format!("opacity={} is outside 0..=1", n.opacity))
            .details(json!({"id": id, "key": "opacity", "value": n.opacity})));
    }
    if !n.rotation.is_finite() {
        return Err(Fail::new("INVALID_VALUE", "rotation must be finite").details(json!({"id": id, "key": "rotation"})));
    }
    let v = node_value(n)?;
    check_colors(tokens, &v, &id)?;
    if let Some(t) = n.text.as_ref() {
        let tv = serde_json::to_value(t).unwrap_or(Value::Null);
        for k in ["size", "line_height", "letter_spacing", "max_font_size", "min_font_size", "padding"] {
            if let Some(x) = tv.get(k) {
                check_num(x, &id, &format!("text.{k}"))?;
            }
        }
    }
    Ok(())
}

/// Keys a patch may not write: identity and tree shape belong to other commands.
const FORBIDDEN_NODE_KEYS: &[&str] = &["id", "parent", "children"];

fn patch_reject_forbidden(patch: &Value, forbidden: &[&str], id: &str, use_instead: &str) -> R<()> {
    fn walk(v: &Value, forbidden: &[&str], out: &mut Vec<String>) {
        if let Some(o) = v.as_object() {
            for (k, x) in o {
                if forbidden.iter().any(|f| f == k) {
                    out.push(k.clone());
                }
                walk(x, forbidden, out);
            }
        } else if let Some(a) = v.as_array() {
            for x in a {
                walk(x, forbidden, out);
            }
        }
    }
    let mut hits = Vec::new();
    walk(patch, forbidden, &mut hits);
    // `hits` is also reported in `details`, so the first offender is cloned out
    // rather than moved out of the list.
    match hits.first().cloned() {
        Some(k) => Err(Fail::new("INVALID_VALUE", format!("a patch cannot write \"{k}\""))
            .hint(use_instead)
            .details(json!({"id": id, "key": k, "forbidden": hits}))),
        None => Ok(()),
    }
}

// ------------------------------------------------------------------- node ops

fn position_of(doc: &Document, n: &Node) -> (Option<String>, Option<String>, usize) {
    let board = n.artboard.clone();
    if let Some(p) = n.parent.as_ref() {
        let idx = doc
            .node(p)
            .and_then(|pp| pp.children.iter().position(|c| c == &n.id))
            .unwrap_or(0);
        return (board, Some(p.clone()), idx);
    }
    let idx = board
        .as_deref()
        .and_then(|b| doc.artboard(b))
        .and_then(|ab| ab.layers.iter().position(|x| x == &n.id))
        .unwrap_or(0);
    (board, None, idx)
}

fn detach(doc: &mut Document, id: &str) {
    let (parent, board) = match doc.node(id) {
        Some(n) => (n.parent.clone(), n.artboard.clone()),
        None => return,
    };
    match parent {
        Some(p) => {
            if let Some(pp) = doc.node_mut(&p) {
                pp.children.retain(|c| c != id);
            }
        }
        None => {
            if let Some(b) = board {
                if let Some(ab) = doc.artboard_mut(&b) {
                    ab.layers.retain(|x| x != id);
                }
            }
        }
    }
}

/// Insert `id` into `parent`'s children, or into the artboard's layers.
///
/// When the container already lists `id` the slot is kept untouched: that is what
/// makes a subtree restore (the undo of `NodeDel`, whose node clones still carry
/// their `children` / the artboard still carries its `layers`) land in the exact
/// original positions instead of duplicating entries.
fn attach(doc: &mut Document, board: &str, parent: Option<&str>, id: &str, at: Option<usize>) {
    match parent {
        Some(p) => {
            if let Some(pp) = doc.node_mut(p) {
                if pp.children.iter().any(|c| c == id) {
                    return;
                }
                let i = at.map(|a| a.min(pp.children.len())).unwrap_or(pp.children.len());
                pp.children.insert(i, id.to_string());
            }
        }
        None => {
            if let Some(ab) = doc.artboard_mut(board) {
                if ab.layers.iter().any(|l| l == id) {
                    return;
                }
                let i = at.map(|a| a.min(ab.layers.len())).unwrap_or(ab.layers.len());
                ab.layers.insert(i, id.to_string());
            }
        }
    }
}

fn container_of(doc: &Document, board: &str, parent: Option<&str>) -> R<Vec<String>> {
    match parent {
        Some(p) => {
            let i = node_idx(doc, p)?;
            let n = &doc.nodes[i];
            if !matches!(n.kind, NodeType::Group | NodeType::Frame) {
                return Err(Fail::new("INVALID_VALUE", format!("node \"{p}\" is a {} and cannot hold children", n.kind.as_str()))
                    .hint("only group and frame nodes contain children")
                    .details(json!({"id": p, "type": n.kind.as_str()})));
            }
            Ok(n.children.clone())
        }
        None => {
            let i = board_idx(doc, board)?;
            Ok(doc.artboards[i].layers.clone())
        }
    }
}

fn artboard_of_node(doc: &Document, n: &Node) -> Option<String> {
    doc.artboard_of(n).map(|a| a.id.clone())
}

fn board_has_constraint(doc: &Document, board: Option<&str>) -> bool {
    doc.nodes.iter().any(|n| {
        let here = match board {
            None => true,
            Some(b) => n.artboard.as_deref() == Some(b) || doc.artboard_of(n).map(|a| a.id == b).unwrap_or(false),
        };
        here && n.layout.mode == crate::doc::LayoutMode::Constraint
    })
}

/// The board a command affects, for the solve pass.
fn affected_board(doc: &Document, ids: &[String]) -> Option<String> {
    for id in ids {
        if let Some(n) = doc.node(id) {
            if let Some(b) = artboard_of_node(doc, n) {
                return Some(b);
            }
        }
    }
    None
}

fn boxes(doc: &Document, board: Option<&str>) -> HashMap<String, Rect> {
    let ids: Vec<String> = match board {
        Some(b) => doc.paint_order(b),
        None => doc.nodes.iter().map(|n| n.id.clone()).collect(),
    };
    ids.iter()
        .filter_map(|id| doc.node(id).map(|n| (id.clone(), n.box_)))
        .collect()
}

/// Ask the solver to settle constraint-mode nodes, and fold what it reports into
/// `warnings`. A solver error is *demoted to a warning*: a constraint the author
/// cannot express must not throw away the edit they actually made.
fn post_solve(doc: &mut Document, board: Option<&str>, pre: &HashMap<String, Rect>, a: &mut Applied) {
    let Some(b) = board else { return };
    let must = {
        let touched_constraint = a
            .changed
            .iter()
            .chain(a.created.iter())
            .filter_map(|id| doc.node(id))
            .any(|n| n.layout.mode == crate::doc::LayoutMode::Constraint);
        touched_constraint || board_has_constraint(doc, Some(&b))
    };
    if !must {
        return;
    }
    if let Err(e) = crate::layout::solve(doc, Some(&b)) {
        a.warnings.push(format!("solve skipped: {}", e));
        return;
    }
    // The solver may have moved siblings the command's own undo does not cover.
    let mut extra: Vec<Cmd> = Vec::new();
    for id in doc.paint_order(&b) {
        if a.changed.contains(&id) || a.created.contains(&id) {
            continue;
        }
        if let (Some(old), Some(n)) = (pre.get(&id), doc.node(&id)) {
            if *old != n.box_ {
                extra.push(Cmd::Transform { id: id.clone(), x: old.x, y: old.y, w: old.w, h: old.h });
            }
        }
    }
    augment_undo(a, extra);
}

/// Wrap the command's own inverse plus solver repairs into one transaction.
fn augment_undo(a: &mut Applied, extra: Vec<Cmd>) {
    if extra.is_empty() {
        return;
    }
    let own = std::mem::replace(&mut a.undo, Cmd::Solve { board: None });
    let mut cmds = extra;
    match own {
        Cmd::Composite { cmds: inner } => cmds.extend(inner),
        other => cmds.push(other),
    }
    a.undo = Cmd::Composite { cmds };
}

fn undo_extra(a: &mut Applied, pre: &HashMap<String, Rect>, doc: &Document) {
    let mut extra = Vec::new();
    for (id, old) in pre {
        if a.changed.contains(id) || a.created.contains(id) {
            continue;
        }
        if let Some(n) = doc.node(id) {
            if *old != n.box_ {
                extra.push(Cmd::Transform { id: id.clone(), x: old.x, y: old.y, w: old.w, h: old.h });
            }
        }
    }
    augment_undo(a, extra);
}

fn node_add(doc: &mut Document, node: &Node, board: &str, at: Option<usize>, _ctx: &ApplyCtx) -> R<Applied> {
    let _ = board_idx(doc, board)?;
    if node.id.trim().is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "NodeAdd needs node.id"));
    }
    if doc.node(&node.id).is_some() {
        return Err(Fail::new("INVALID_VALUE", format!("node id \"{}\" already exists", node.id))
            .hint("ids must be unique in a document; use Duplicate to copy, or pick another id")
            .details(json!({"id": node.id})));
    }
    if !node.rotation.is_finite() || !node.box_.is_finite() {
        return Err(Fail::new("INVALID_VALUE", "new node has a non-finite box or rotation")
            .details(json!({"id": node.id, "box": node.box_, "rotation": node.rotation})));
    }
    let parent = node.parent.clone();
    if let Some(p) = parent.as_deref() {
        let _ = container_of(doc, board, Some(p))?;
        if p == node.id {
            return Err(Fail::new("INVALID_VALUE", "a node cannot be its own parent"));
        }
    } else {
        let _ = container_of(doc, board, None)?;
    }
    validate_node(node, &doc.tokens)?;
    // The children of a fresh node must not already belong to someone else.
    for c in &node.children {
        if c == &node.id {
            return Err(Fail::new("INVALID_VALUE", format!("node \"{}\" lists itself as a child", node.id)));
        }
        if let Some(cn) = doc.node(c) {
            if cn.parent.is_some() && cn.parent.as_deref() != Some(node.id.as_str()) {
                return Err(Fail::new("INVALID_VALUE", format!("child node \"{c}\" already has a parent"))
                    .details(json!({"id": c, "parent": cn.parent})));
            }
        }
    }

    let mut fresh = node.clone();
    fresh.artboard = match parent.as_deref().and_then(|p| doc.node(p)) {
        Some(pp) => pp.artboard.clone().or_else(|| Some(board.to_string())),
        None => Some(board.to_string()),
    };
    let board_id = fresh.artboard.clone().unwrap_or_else(|| board.to_string());
    let mut created = vec![fresh.id.clone()];
    created.extend(node.children.iter().cloned());

    doc.nodes.push(fresh.clone());
    for c in &node.children {
        if let Some(cn) = doc.node_mut(c) {
            cn.parent = Some(fresh.id.clone());
            cn.artboard = Some(board_id.clone());
        }
    }
    attach(doc, &board_id, fresh.parent.as_deref(), &fresh.id, at);

    let mut a = Applied {
        changed: vec![fresh.id.clone()],
        created: dedup(created),
        warnings: Vec::new(),
        undo: Cmd::NodeDel { id: fresh.id.clone() },
        revision: doc.revision,
    };
    let pre = boxes(doc, Some(&board_id));
    post_solve(doc, Some(&board_id), &pre, &mut a);
    Ok(a)
}

fn node_del(doc: &mut Document, id: &str, ctx: &ApplyCtx) -> R<Applied> {
    let i = node_idx(doc, id)?;
    guard_tree(doc, &[id.to_string()], ctx)?;
    let root = doc.nodes[i].clone();
    let order = doc.subtree(id);
    let captured: Vec<Node> = order.iter().filter_map(|d| doc.node(d).cloned()).collect();
    let (_, _, idx) = position_of(doc, &root);
    let mut warnings = Vec::new();
    let board = match root.artboard.clone() {
        Some(b) => b,
        None => match doc.artboards.len() {
            1 => {
                warnings.push(format!(
                    "node \"{id}\" had no artboard; its undo is filed under the only board {}",
                    doc.artboards[0].id
                ));
                doc.artboards[0].id.clone()
            }
            _ => String::new(),
        },
    };
    if board.is_empty() {
        warnings.push(format!(
            "node \"{id}\" belongs to no artboard, so undo cannot re-attach it; NodeAdd needs a board id"
        ));
    }

    for d in &order {
        detach(doc, d);
    }
    doc.nodes.retain(|n| !order.contains(&n.id));

    // One NodeAdd when nothing else went with it; a Composite when the subtree did.
    let undo = if captured.len() == 1 {
        Cmd::NodeAdd { node: captured[0].clone(), board: board.clone(), at: Some(idx) }
    } else {
        Cmd::Composite {
            cmds: captured
                .iter()
                .map(|n| Cmd::NodeAdd {
                    node: n.clone(),
                    board: n.artboard.clone().unwrap_or_else(|| board.clone()),
                    at: None,
                })
                .collect(),
        }
    };
    let mut a = Applied {
        changed: order.clone(),
        created: Vec::new(),
        warnings,
        undo,
        revision: doc.revision,
    };
    let pre: HashMap<String, Rect> = captured.iter().map(|n| (n.id.clone(), n.box_)).collect();
    if !board.is_empty() {
        post_solve(doc, Some(&board), &pre, &mut a);
    }
    Ok(a)
}

fn node_set(doc: &mut Document, id: &str, patch: &Value, ctx: &ApplyCtx) -> R<Applied> {
    patch_reject_forbidden(
        patch,
        FORBIDDEN_NODE_KEYS,
        id,
        "write the tree with NodeMove / canvas.group instead of NodeSet",
    )?;
    let i = node_idx(doc, id)?;
    guard(doc, id, ctx)?;
    let before = doc.nodes[i].clone();
    let mut v = node_value(&before)?;
    let merged = merge_patch(&v, patch);
    let after: Node = from_value(merged.clone(), "node patch")?;
    validate_node(&after, &doc.tokens)?;
    let minimal = undo_patch(&v, &merged);
    let changed_here = minimal.as_object().map(|m| !m.is_empty()).unwrap_or(false);

    let board = after.artboard.clone().or_else(|| before.artboard.clone());
    doc.nodes[i] = after;
    let mut a = Applied {
        changed: if changed_here { vec![id.to_string()] } else { Vec::new() },
        created: Vec::new(),
        warnings: Vec::new(),
        undo: Cmd::NodeSet { id: id.to_string(), patch: minimal },
        revision: doc.revision,
    };
    if !changed_here {
        return Ok(a);
    }
    let pre = boxes(doc, board.as_deref());
    post_solve(doc, board.as_deref(), &pre, &mut a);
    Ok(a)
}

fn nodes_set(doc: &mut Document, ids: &[String], patch: &Value, ctx: &ApplyCtx) -> R<Applied> {
    if ids.is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "NodesSet needs ids"));
    }
    // All-or-nothing: resolve and validate every id before touching anything.
    for id in ids {
        node_idx(doc, id)?;
        guard(doc, id, ctx)?;
    }
    let mut work = doc.clone();
    let mut changed = Vec::new();
    let mut undos = Vec::new();
    let mut board = None;
    for id in ids {
        let i = node_idx(&work, id)?;
        let before = node_value(&work.nodes[i])?;
        let merged = merge_patch(&before, patch);
        let after: Node = from_value(merged.clone(), "node patch")?;
        validate_node(&after, &work.tokens)?;
        let minimal = undo_patch(&before, &merged);
        if minimal.as_object().map(|m| !m.is_empty()).unwrap_or(false) {
            undos.push(Cmd::NodeSet { id: id.clone(), patch: minimal });
            changed.push(id.clone());
            if board.is_none() {
                board = after.artboard.clone();
            }
        }
        work.nodes[i] = after;
    }
    let pre = boxes(&work, board.as_deref());
    let undo = match undos.len() {
        0 => Cmd::NodesSet { ids: ids.to_vec(), patch: json!({}) },
        1 => undos.pop().unwrap_or(Cmd::NodesSet { ids: ids.to_vec(), patch: json!({}) }),
        _ => Cmd::Composite { cmds: undos },
    };
    let mut a = Applied {
        changed,
        created: Vec::new(),
        warnings: Vec::new(),
        undo,
        revision: doc.revision,
    };
    *doc = work;
    if !a.changed.is_empty() {
        post_solve(doc, board.as_deref(), &pre, &mut a);
    }
    Ok(a)
}

fn node_move(doc: &mut Document, id: &str, board: &str, to: usize, parent: Option<&str>, ctx: &ApplyCtx) -> R<Applied> {
    let i = node_idx(doc, id)?;
    guard(doc, id, ctx)?;
    let _ = board_idx(doc, board)?;
    // Validate the destination container (a group/frame, or the artboard itself).
    let _ = container_of(doc, board, parent)?;
    // Re-ordering inside the same container is exactly what `canvas.reorder` does,
    // so that is allowed; only a node inside itself is refused.
    if let Some(p) = parent {
        if doc.subtree(id).contains(&p.to_string()) {
            return Err(Fail::new("INVALID_VALUE", format!("cannot move \"{id}\" inside its own child \"{p}\""))
                .hint("a group cannot live inside something it contains")
                .details(json!({"id": id, "parent": p})));
        }
    }
    let (old_board, old_parent, old_idx) = position_of(doc, &doc.nodes[i]);
    detach(doc, id);
    if let Some(n) = doc.node_mut(id) {
        n.parent = parent.map(|s| s.to_string());
        n.artboard = Some(board.to_string());
    }
    for d in doc.subtree(id) {
        if d != id {
            if let Some(n) = doc.node_mut(&d) {
                n.artboard = Some(board.to_string());
            }
        }
    }
    attach(doc, board, parent, id, Some(to));
    let mut a = Applied {
        changed: vec![id.to_string()],
        created: Vec::new(),
        warnings: Vec::new(),
        undo: Cmd::NodeMove {
            id: id.to_string(),
            board: old_board.unwrap_or_else(|| board.to_string()),
            to: old_idx,
            parent: old_parent,
        },
        revision: doc.revision,
    };
    let pre = boxes(doc, Some(board));
    post_solve(doc, Some(board), &pre, &mut a);
    Ok(a)
}

// ------------------------------------------------------------------- artboards

fn board_add(doc: &mut Document, board: &Artboard) -> R<Applied> {
    if board.id.trim().is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "BoardAdd needs board.id"));
    }
    if doc.artboard(&board.id).is_some() {
        return Err(Fail::new("INVALID_VALUE", format!("artboard id \"{}\" already exists", board.id))
            .details(json!({"id": board.id})));
    }
    if !board.width.is_finite() || !board.height.is_finite() || board.width <= 0.0 || board.height <= 0.0 {
        return Err(Fail::new("INVALID_VALUE", "artboard width/height must be positive and finite")
            .details(json!({"id": board.id, "width": board.width, "height": board.height})));
    }
    check_colors(&doc.tokens, &board_value(board)?, &board.id)?;
    let mut b = board.clone();
    for l in &b.layers.clone() {
        if doc.node(l).is_none() {
            return Err(Fail::new("NODE_NOT_FOUND", format!("artboard lists unknown layer \"{l}\""))
                .details(json!({"id": l, "board": b.id})));
        }
    }
    doc.artboards.push(b.clone());
    Ok(Applied {
        changed: vec![b.id.clone()],
        created: vec![b.id.clone()],
        warnings: Vec::new(),
        undo: Cmd::BoardDel { id: b.id.clone(), board: b },
        revision: doc.revision,
    })
}

fn board_del(doc: &mut Document, id: &str, prev: &Artboard, ctx: &ApplyCtx) -> R<Applied> {
    let i = board_idx(doc, id)?;
    if prev.id != id {
        return Err(Fail::new("INVALID_VALUE", "BoardDel.board.id must equal the id being removed")
            .hint("BoardDel carries the artboard so its undo can restore it")
            .details(json!({"id": id, "boardId": prev.id})));
    }
    // Cascade: every top layer and its subtree goes with the artboard.
    let layers = doc.artboards[i].layers.clone();
    let mut doomed: Vec<String> = Vec::new();
    for l in &layers {
        for d in doc.subtree(l) {
            if !doomed.contains(&d) {
                doomed.push(d);
            }
        }
    }
    if !ctx.force {
        for d in &doomed {
            if doc.node(d).map(|n| n.locked).unwrap_or(false) {
                return Err(locked_err(d));
            }
        }
    }
    let captured: Vec<Node> = doomed.iter().filter_map(|d| doc.node(d)).cloned().collect();
    let stored = doc.artboards[i].clone();
    let mut undos: Vec<Cmd> = Vec::new();
    for n in &captured {
        undos.push(Cmd::NodeAdd { node: n.clone(), board: id.to_string(), at: None });
    }
    // BoardAdd goes *after* the node adds in the list so that reversing puts it
    // first: a Composite undo applies its members in order, and the nodes need
    // their artboard to exist before they can be re-attached to it. Their captured
    // clones still carry `children`, and the restored artboard still carries
    // `layers`, so `attach` keeps those slots instead of duplicating them.
    undos.push(Cmd::BoardAdd { board: stored.clone() });
    undos.reverse();
    doc.artboards.remove(i);
    doc.nodes.retain(|n| !doomed.contains(&n.id));
    let a = Applied {
        changed: dedup(vec![id.to_string()].into_iter().chain(doomed.iter().cloned()).collect()),
        created: Vec::new(),
        warnings: if doomed.is_empty() {
            Vec::new()
        } else {
            vec![format!("removed {} node(s) with the artboard", doomed.len())]
        },
        undo: Cmd::Composite { cmds: undos },
        revision: doc.revision,
    };
    Ok(a)
}

fn board_set(doc: &mut Document, id: &str, patch: &Value, ctx: &ApplyCtx) -> R<Applied> {
    patch_reject_forbidden(patch, &["id", "layers"], id, "use artboard.resize / canvas.reorder for those")?;
    let i = board_idx(doc, id)?;
    if !ctx.force && doc.artboards[i].layers.iter().filter_map(|l| doc.node(l)).any(|n| n.locked) {
        return Err(Fail::new("LOCKED", format!("artboard \"{id}\" holds locked nodes"))
            .hint("unlock them, layout.lock, or pass force:true")
            .details(json!({"id": id})));
    }
    let before = board_value(&doc.artboards[i])?;
    let merged = merge_patch(&before, patch);
    let after: Artboard = from_value(merged.clone(), "artboard patch")?;
    if !after.width.is_finite() || !after.height.is_finite() || after.width <= 0.0 || after.height <= 0.0 {
        return Err(Fail::new("INVALID_VALUE", "width/height must be positive and finite")
            .hint("artboard.resize p,b,w,h")
            .details(json!({"id": id, "width": after.width, "height": after.height})));
    }
    check_colors(&doc.tokens, &merged, id)?;
    let minimal = undo_patch(&before, &merged);
    let differs = minimal.as_object().map(|m| !m.is_empty()).unwrap_or(false);
    doc.artboards[i] = after;
    let mut a = Applied {
        changed: if differs { vec![id.to_string()] } else { Vec::new() },
        created: Vec::new(),
        warnings: Vec::new(),
        undo: Cmd::BoardSet { id: id.to_string(), patch: minimal },
        revision: doc.revision,
    };
    if !differs {
        return Ok(a);
    }
    // A resized artboard changes what every constraint in it resolves to.
    let pre = boxes(doc, Some(id));
    post_solve(doc, Some(id), &pre, &mut a);
    Ok(a)
}

// ------------------------------------------------------------------- assets

fn asset_add(doc: &mut Document, asset: &Asset) -> R<Applied> {
    if asset.id.trim().is_empty() || asset.uri.trim().is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "AssetAdd needs asset.id and asset.uri"));
    }
    if doc.assets.iter().any(|a| a.id == asset.id) {
        return Err(Fail::new("INVALID_VALUE", format!("asset id \"{}\" already exists", asset.id))
            .hint("assets are keyed by id; use AssetDel first or assets.replace")
            .details(json!({"id": asset.id})));
    }
    if !asset.width.is_finite() || !asset.height.is_finite() || asset.width < 0.0 || asset.height < 0.0 {
        return Err(Fail::new("INVALID_VALUE", "asset width/height must be >= 0 and finite")
            .details(json!({"id": asset.id, "width": asset.width, "height": asset.height})));
    }
    doc.assets.push(asset.clone());
    Ok(Applied {
        changed: vec![asset.id.clone()],
        created: vec![asset.id.clone()],
        warnings: Vec::new(),
        undo: Cmd::AssetDel { id: asset.id.clone(), asset: asset.clone() },
        revision: doc.revision,
    })
}

fn asset_del(doc: &mut Document, id: &str, prev: &Asset, ctx: &ApplyCtx) -> R<Applied> {
    let i = asset_idx(doc, id)?;
    if prev.id != id {
        return Err(Fail::new("INVALID_VALUE", "AssetDel.asset.id must equal the id being removed")
            .hint("AssetDel carries the previous record so undo can restore it")
            .details(json!({"id": id, "assetId": prev.id})));
    }
    let used_by: Vec<String> = doc
        .nodes
        .iter()
        .filter(|n| {
            n.image.as_ref().map(|i| i.asset == id).unwrap_or(false)
                || n.vector.as_ref().and_then(|v| v.asset.clone()).map(|a| a == id).unwrap_or(false)
        })
        .map(|n| n.id.clone())
        .collect();
    if !used_by.is_empty() && !ctx.force {
        return Err(Fail::new("PERMISSION_DENIED", format!("asset \"{id}\" is used by {} node(s)", used_by.len()))
            .hint("pass force:true to delete it anyway, or re-point the nodes first")
            .details(json!({"id": id, "usedBy": used_by})));
    }
    let stored = doc.assets[i].clone();
    doc.assets.remove(i);
    Ok(Applied {
        changed: vec![id.to_string()],
        created: Vec::new(),
        warnings: if used_by.is_empty() { Vec::new() } else { vec![format!("removed asset used by {}", used_by.join(", "))] },
        undo: Cmd::AssetAdd { asset: stored },
        revision: doc.revision,
    })
}

// ------------------------------------------------------------------- tokens

fn token_set(doc: &mut Document, group: &str, name: &str, value: &Value) -> R<Applied> {
    if name.trim().is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "TokenSet needs a name"));
    }
    let old: Option<Value>;
    let undo_value: Value;
    match group {
        "colors" => {
            let t = &doc.tokens;
            old = t.colors.iter().find(|c| c.name == name).map(|c| json!({"value": c.value, "note": c.note}));
            if value.is_null() {
                doc.tokens.colors.retain(|c| c.name != name);
            } else {
                let (v, note) = read_color_token(value)?;
                check_colors(&doc.tokens, &json!({"color": v}), name)?;
                let list = &mut doc.tokens.colors;
                match list.iter_mut().find(|c| c.name == name) {
                    Some(c) => {
                        c.value = v;
                        c.note = note;
                    }
                    None => list.push(ColorToken { name: name.to_string(), value: v, note }),
                }
            }
            undo_value = old.clone().unwrap_or(Value::Null);
        }
        "spacing" | "radii" => {
            let get = |d: &Document| -> Option<f64> {
                let list: &[NumToken] = if group == "spacing" { &d.tokens.spacing } else { &d.tokens.radii };
                list.iter().find(|n| n.name == name).map(|n| n.value)
            };
            old = get(doc).map(|v| json!(v));
            if value.is_null() {
                if group == "spacing" {
                    doc.tokens.spacing.retain(|n| n.name != name);
                } else {
                    doc.tokens.radii.retain(|n| n.name != name);
                }
            } else {
                let x = value.as_f64().ok_or_else(|| {
                    Fail::new("INVALID_VALUE", format!("{group} token \"{name}\" needs a number"))
                        .details(json!({"group": group, "name": name, "value": value}))
                })?;
                if !x.is_finite() || x < 0.0 {
                    return Err(Fail::new("INVALID_VALUE", format!("{group} token must be a finite number >= 0"))
                        .details(json!({"group": group, "name": name, "value": x})));
                }
                let list = if group == "spacing" { &mut doc.tokens.spacing } else { &mut doc.tokens.radii };
                match list.iter_mut().find(|n| n.name == name) {
                    Some(n) => n.value = x,
                    None => list.push(NumToken { name: name.to_string(), value: x }),
                }
            }
            undo_value = old.clone().unwrap_or(Value::Null);
        }
        "fonts" => {
            old = doc
                .tokens
                .fonts
                .iter()
                .find(|f| f.name == name)
                .and_then(|f| serde_json::to_value(f).ok());
            if value.is_null() {
                doc.tokens.fonts.retain(|f| f.name != name);
            } else {
                let mut v = value.clone();
                if let Some(o) = v.as_object_mut() {
                    o.entry("name".to_string()).or_insert_with(|| json!(name));
                }
                let t: FontToken = from_value(v, "font token")?;
                let list = &mut doc.tokens.fonts;
                match list.iter_mut().find(|f| f.name == name) {
                    Some(f) => *f = t,
                    None => list.push(FontToken { name: name.to_string(), ..t }),
                }
            }
            undo_value = old.clone().unwrap_or(Value::Null);
        }
        other => {
            return Err(Fail::new("INVALID_VALUE", format!("unknown token group \"{other}\""))
                .hint("groups are colors | fonts | spacing | radii")
                .details(json!({"group": other, "allowed": ["colors", "fonts", "spacing", "radii"]})))
        }
    }
    // A token that was just rewritten may change what colours resolve to.
    let mut warnings = Vec::new();
    if group == "colors" && !value.is_null() {
        if let Some(v) = doc.tokens.color(&format!("${name}")) {
            if doc::parse_color(&v).is_none() {
                warnings.push(format!("token ${name} resolves to a colour that does not parse: {v}"));
            }
        }
    }
    Ok(Applied {
        changed: vec![format!("{group}.{name}")],
        created: if old.is_none() && !value.is_null() { vec![format!("{group}.{name}")] } else { Vec::new() },
        warnings,
        undo: Cmd::TokenSet { group: group.to_string(), name: name.to_string(), value: undo_value },
        revision: doc.revision,
    })
}

fn read_color_token(value: &Value) -> R<(String, Option<String>)> {
    match value {
        Value::String(s) => Ok((s.clone(), None)),
        Value::Object(o) => {
            let s = o
                .get("value")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Fail::new("INVALID_SCHEMA", "a color token needs a string \"value\""))?
                .to_string();
            let note = o.get("note").and_then(|v| v.as_str()).map(|s| s.to_string());
            Ok((s, note))
        }
        other => Err(Fail::new("INVALID_VALUE", "a color token must be a string or {value,note}")
            .details(json!({"got": other}))),
    }
}

// ------------------------------------------------------------------- paint / text

fn style_set(doc: &mut Document, id: &str, style: &Value, ctx: &ApplyCtx) -> R<Applied> {
    let i = node_idx(doc, id)?;
    guard(doc, id, ctx)?;
    let before = doc.nodes[i].style.clone();
    let old_v = match &before {
        Some(s) => serde_json::to_value(s).map_err(|e| Fail::new("INTERNAL", format!("style: {e}")))?,
        None => Value::Null,
    };
    if style.is_null() {
        doc.nodes[i].style = None;
        return Ok(Applied {
            changed: vec![id.to_string()],
            created: Vec::new(),
            warnings: Vec::new(),
            undo: Cmd::Style { id: id.to_string(), style: old_v },
            revision: doc.revision,
        });
    }
    if let Some(o) = style.as_object() {
        const ALLOWED: &[&str] = &["fill", "stroke", "radius", "color"];
        if let Some(k) = o.keys().find(|k| !ALLOWED.contains(&k.as_str())) {
            return Err(Fail::new("INVALID_VALUE", format!("style has no field \"{k}\""))
                .hint("Style carries exactly fill | stroke | radius | color; node fields belong to NodeSet")
                .details(json!({"id": id, "key": k, "allowed": ALLOWED})));
        }
    }
    let base = old_v.clone();
    let merged = if base.is_null() { style.clone() } else { merge_patch(&base, style) };
    let after: doc::Style = from_value(merged.clone(), "style")?;
    check_colors(&doc.tokens, &merged, id)?;
    if let Some(c) = &after.color {
        if !c.starts_with('$') && doc::parse_color(c).is_none() {
            return Err(Fail::new("INVALID_VALUE", format!("style.color \"{}\" is not a colour", c))
                .details(json!({"id": id, "key": "style.color", "value": c})));
        }
    }
    doc.nodes[i].style = Some(after);
    // `null` when the node had no style at all (undo then removes it again),
    // otherwise the minimal previous-value patch — an empty object when nothing
    // actually changed, which replays as a no-op.
    let undo_style = if old_v.is_null() { Value::Null } else { undo_patch(&old_v, &merged) };
    let changed = old_v != merged;
    Ok(Applied {
        changed: if changed { vec![id.to_string()] } else { Vec::new() },
        created: Vec::new(),
        warnings: Vec::new(),
        undo: Cmd::Style { id: id.to_string(), style: undo_style },
        revision: doc.revision,
    })
}

fn text_set(doc: &mut Document, id: &str, content: &str, ctx: &ApplyCtx) -> R<Applied> {
    let i = node_idx(doc, id)?;
    guard(doc, id, ctx)?;
    if doc.nodes[i].kind != NodeType::Text && doc.nodes[i].text.is_none() {
        return Err(Fail::new("INVALID_VALUE", format!("node \"{id}\" is a {} and has no text", doc.nodes[i].kind.as_str()))
            .hint("canvas.add a text node first, or NodeSet the type")
            .details(json!({"id": id, "type": doc.nodes[i].kind.as_str()})));
    }
    let old = doc
        .nodes[i]
        .text
        .as_ref()
        .map(|t| t.content.clone())
        .unwrap_or_default();
    if doc.nodes[i].text.is_none() {
        // All TextSpec fields but `content` are optional, so this builds a valid spec.
        let t: doc::TextSpec = from_value(json!({"content": content}), "text spec")?;
        doc.nodes[i].text = Some(t);
    } else if let Some(t) = doc.nodes[i].text.as_mut() {
        t.content = content.to_string();
    }
    let board = doc.nodes[i].artboard.clone();
    let mut a = Applied {
        changed: if old == content { Vec::new() } else { vec![id.to_string()] },
        created: Vec::new(),
        warnings: Vec::new(),
        undo: Cmd::Text { id: id.to_string(), content: old },
        revision: doc.revision,
    };
    let pre = boxes(doc, board.as_deref());
    post_solve(doc, board.as_deref(), &pre, &mut a);
    Ok(a)
}

fn transform(doc: &mut Document, id: &str, x: f64, y: f64, w: f64, h: f64, ctx: &ApplyCtx) -> R<Applied> {
    let i = node_idx(doc, id)?;
    guard(doc, id, ctx)?;
    for (k, v) in [("x", x), ("y", y), ("w", w), ("h", h)] {
        if !v.is_finite() {
            return Err(Fail::new("INVALID_VALUE", format!("{k} must be a finite number, got {v}"))
                .hint("coordinates are CSS pixels; NaN and infinity are rejected")
                .details(json!({"id": id, "key": k, "value": v})));
        }
    }
    if w < 0.0 || h < 0.0 {
        return Err(Fail::new("INVALID_VALUE", "w and h cannot be negative")
            .hint("flip with NodeSet {flipX:true}; a box size is never negative")
            .details(json!({"id": id, "w": w, "h": h})));
    }
    let old = doc.nodes[i].box_;
    // A hand-placed box is a human decision, so it takes the node out of the
    // solver's hands: `layout::skip` only protects `mode == Manual` or
    // `pinned == Some(true)`, and without both flags the very next `layout::solve`
    // (which `rpc::run_cmd` runs after every command) would drop the node straight
    // back on its intent. Captured *before* the mutation so the undo can hand the
    // node back to constraint control instead of leaving it pinned forever.
    let old_mode = doc.nodes[i].layout.mode;
    let old_pinned = doc.nodes[i].layout.pinned;
    let board = doc.nodes[i].artboard.clone();
    doc.nodes[i].box_ = Rect::new(x, y, w, h);
    doc.nodes[i].layout.mode = crate::doc::LayoutMode::Manual;
    doc.nodes[i].layout.pinned = Some(true);
    let box_undo = Cmd::Transform { id: id.to_string(), x: old.x, y: old.y, w: old.w, h: old.h };
    // `undo` = put the box back, *then* restore what the solver used to own. The
    // order matters: the box `Transform` re-pins, and `pinned: null` deletes the key
    // rather than writing a `false` the document never had.
    let undo = if old_mode == crate::doc::LayoutMode::Manual && old_pinned == Some(true) {
        box_undo
    } else {
        let pinned = match old_pinned {
            Some(b) => json!(b),
            None => Value::Null,
        };
        Cmd::Composite {
            cmds: vec![
                box_undo,
                Cmd::NodeSet {
                    id: id.to_string(),
                    patch: json!({"layout": {"mode": old_mode.as_str(), "pinned": pinned}}),
                },
            ],
        }
    };
    let mut a = Applied {
        changed: vec![id.to_string()],
        created: Vec::new(),
        warnings: Vec::new(),
        undo,
        revision: doc.revision,
    };
    let pre = boxes(doc, board.as_deref());
    post_solve(doc, board.as_deref(), &pre, &mut a);
    Ok(a)
}

fn layout_intent(doc: &mut Document, id: &str, intent: &Value, mode: &str, ctx: &ApplyCtx) -> R<Applied> {
    let i = node_idx(doc, id)?;
    guard(doc, id, ctx)?;
    let m = crate::doc::LayoutMode::parse(mode).ok_or_else(|| {
        Fail::new("INVALID_VALUE", format!("unknown layout mode \"{mode}\""))
            .hint("mode is manual | constraint | flex | grid")
            .details(json!({"id": id, "mode": mode}))
    })?;
    let old_intent = doc
        .nodes[i]
        .layout
        .intent
        .as_ref()
        .and_then(|v| serde_json::to_value(v).ok())
        .unwrap_or(Value::Null);
    let old_mode = json!(doc.nodes[i].layout.mode.as_str());
    if !intent.is_null() {
        let probe = merge_patch(&old_intent, intent);
        let parsed: crate::doc::Intent = from_value(probe, "layout intent")?;
        doc.nodes[i].layout.intent = Some(parsed);
    } else {
        doc.nodes[i].layout.intent = None;
    }
    doc.nodes[i].layout.mode = m;
    if m == crate::doc::LayoutMode::Manual {
        // A human took over: the solver must not move it again.
        doc.nodes[i].layout.pinned = Some(true);
    }
    let board = doc.nodes[i].artboard.clone();
    let mut a = Applied {
        changed: vec![id.to_string()],
        created: Vec::new(),
        warnings: Vec::new(),
        undo: Cmd::LayoutIntent {
            id: id.to_string(),
            intent: old_intent,
            mode: old_mode.as_str().unwrap_or("manual").to_string(),
        },
        revision: doc.revision,
    };
    let pre = boxes(doc, board.as_deref());
    post_solve(doc, board.as_deref(), &pre, &mut a);
    Ok(a)
}

// ------------------------------------------------------------------- tree ops

fn group(doc: &mut Document, ids: &[String], board: &str, ctx: &ApplyCtx) -> R<Applied> {
    if ids.is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "Group needs at least one node id"));
    }
    let _ = board_idx(doc, board)?;
    for id in ids {
        let _ = node_idx(doc, id)?;
        guard(doc, id, ctx)?;
        let n = &doc.nodes[node_idx(doc, id)?];
        let belongs = artboard_of_node(doc, n).as_deref() == Some(board);
        if !belongs {
            return Err(Fail::new("INVALID_VALUE", format!("node \"{id}\" is not in artboard \"{board}\""))
                .details(json!({"id": id, "board": board, "actual": artboard_of_node(doc, n)})));
        }
    }
    let work_ids: Vec<String> = dedup(ids.to_vec());
    let gid = unique_id(doc, "g");
    // Where the group lands: the lowest slot any member occupied.
    let layers = doc.artboard(board).map(|a| a.layers.clone()).unwrap_or_default();
    let slot = work_ids
        .iter()
        .filter_map(|id| layers.iter().position(|l| l == id))
        .min()
        .unwrap_or(layers.len());
    let box_union = doc.bounds_of(&work_ids).unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0));
    let first_parent = work_ids
        .first()
        .and_then(|id| doc.node(id))
        .and_then(|n| n.parent.clone());

    let mut g = Node::new(&gid, NodeType::Group, box_union);
    g.name = "group".into();
    g.artboard = Some(board.to_string());
    g.parent = first_parent.clone();
    g.children = work_ids.clone();
    for id in &work_ids {
        detach(doc, id);
        if let Some(n) = doc.node_mut(id) {
            n.parent = Some(gid.clone());
            n.artboard = Some(board.to_string());
        }
    }
    doc.nodes.push(g.clone());
    attach(doc, board, first_parent.as_deref(), &gid, Some(slot));
    // Children keep their relative order: re-attach in the requested sequence.
    if let Some(n) = doc.node_mut(&gid) {
        n.children = work_ids.clone();
    }

    Ok(Applied {
        changed: work_ids.clone(),
        created: vec![gid.clone()],
        warnings: Vec::new(),
        undo: Cmd::Ungroup { id: gid },
        revision: doc.revision,
    })
}

fn ungroup(doc: &mut Document, id: &str, ctx: &ApplyCtx) -> R<Applied> {
    let i = node_idx(doc, id)?;
    guard(doc, id, ctx)?;
    guard_tree(doc, &[id.to_string()], ctx)?;
    if doc.nodes[i].kind != NodeType::Group {
        return Err(Fail::new("INVALID_VALUE", format!("node \"{id}\" is a {}, not a group", doc.nodes[i].kind.as_str()))
            .details(json!({"id": id, "type": doc.nodes[i].kind.as_str()})));
    }
    let children = doc.nodes[i].children.clone();
    let board = doc.nodes[i].artboard.clone().unwrap_or_default();
    let parent = doc.nodes[i].parent.clone();
    let idx = match &parent {
        Some(p) => doc.node(p).map(|pp| pp.children.iter().position(|c| c == id).unwrap_or(0)).unwrap_or(0),
        None => doc
            .artboard(&board)
            .map(|ab| ab.layers.iter().position(|l| l == id).unwrap_or(0))
            .unwrap_or(0),
    };
    detach(doc, id);
    doc.nodes.retain(|n| n.id != id);
    for (k, c) in children.iter().enumerate() {
        if let Some(n) = doc.node_mut(c) {
            n.parent = parent.clone();
            n.artboard = if parent.is_some() { n.artboard.clone() } else { Some(board.clone()) };
        }
        attach(doc, &board, parent.as_deref(), c, Some(idx + k));
    }
    Ok(Applied {
        changed: children.clone(),
        created: Vec::new(),
        warnings: vec![format!("undoing this Ungroup creates a new group id (Group generates one)")],
        undo: Cmd::Group { ids: children, board },
        revision: doc.revision,
    })
}

fn unique_id(doc: &Document, prefix: &str) -> String {
    let mut used: Vec<&String> = doc.nodes.iter().map(|n| &n.id).collect();
    used.extend(doc.artboards.iter().map(|a| &a.id));
    for _ in 0..64 {
        let id = util::id(prefix);
        if !used.contains(&&id) {
            return id;
        }
    }
    // Practically unreachable; still never returns a colliding id silently.
    format!("{prefix}_{}", util::now_ms())
}

fn duplicate(doc: &mut Document, ids: &[String], ctx: &ApplyCtx) -> R<Applied> {
    if ids.is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "Duplicate needs at least one node id"));
    }
    for id in ids {
        node_idx(doc, id)?;
    }
    guard_tree(doc, ids, ctx)?;
    // A selected node inside another selected node's subtree is already copied.
    let mut roots: Vec<String> = Vec::new();
    for id in ids {
        let covered = ids.iter().any(|p| p != id && doc.subtree(p).contains(id));
        if !covered && !roots.contains(id) {
            roots.push(id.clone());
        }
    }
    // Work on a clone: a failed id mid-way must not leave half a copy behind.
    let mut work = doc.clone();
    let mut created: Vec<String> = Vec::new();
    let mut new_roots: Vec<String> = Vec::new();
    for r in &roots {
        let src_root = match work.node(r) {
            Some(n) => n.clone(),
            None => continue,
        };
        let subs = work.subtree(r);
        let mut map: HashMap<String, String> = HashMap::new();
        for s in &subs {
            map.insert(s.clone(), unique_id(&work, "n"));
        }
        for s in &subs {
            let src = match work.node(s) {
                Some(n) => n.clone(),
                None => continue,
            };
            let mut copy = src.clone();
            copy.id = map[s].clone();
            // Inside the subtree ids are remapped; the root keeps its real parent.
            copy.parent = src.parent.as_ref().map(|p| map.get(p).cloned().unwrap_or_else(|| p.clone()));
            copy.children = src.children.iter().map(|c| map.get(c).cloned().unwrap_or_else(|| c.clone())).collect();
            if s == r {
                copy.name = format!("{} copy", src.name);
            }
            created.push(copy.id.clone());
            work.nodes.push(copy);
        }
        // Place the copy directly above the original in the same container.
        let new_id = map[r].clone();
        match position_of(&work, &src_root) {
            (_, Some(p), idx) => {
                if let Some(pp) = work.node_mut(&p) {
                    let at = (idx + 1).min(pp.children.len());
                    pp.children.insert(at, new_id.clone());
                }
            }
            (Some(b), None, idx) => {
                if let Some(ab) = work.artboard_mut(&b) {
                    let at = (idx + 1).min(ab.layers.len());
                    ab.layers.insert(at, new_id.clone());
                }
            }
            (None, None, _) => {
                return Err(Fail::new("INVALID_VALUE", format!("node \"{r}\" is not in any artboard, so it cannot be duplicated"))
                    .details(json!({"id": r})));
            }
        }
        new_roots.push(new_id);
    }
    *doc = work;
    let undo = match new_roots.len() {
        1 => Cmd::NodeDel { id: new_roots[0].clone() },
        _ => Cmd::Composite { cmds: new_roots.iter().map(|id| Cmd::NodeDel { id: id.clone() }).collect() },
    };
    let board = affected_board(doc, &created);
    let mut a = Applied {
        changed: new_roots.clone(),
        created: dedup(created),
        warnings: Vec::new(),
        undo,
        revision: doc.revision,
    };
    if let Some(b) = board {
        let pre = boxes(doc, Some(&b));
        post_solve(doc, Some(&b), &pre, &mut a);
    }
    Ok(a)
}

// ------------------------------------------------------------------- solver

fn conflict_notes(rep: &crate::layout::SolveReport) -> Vec<String> {
    rep.conflicts
        .iter()
        .map(|c| {
            format!(
                "LAYOUT_CONFLICT {}: {}{}",
                c.node,
                c.intent,
                c.with.clone().map(|w| format!(" with {w}")).unwrap_or_default()
            )
        })
        .collect()
}

/// Boxes the solver moved, as the Transform commands that put them back.
fn box_undo(pre: &HashMap<String, Rect>, doc: &Document) -> Vec<Cmd> {
    let mut out = Vec::new();
    let mut ids: Vec<&String> = pre.keys().collect();
    ids.sort();
    for id in ids {
        if let Some(n) = doc.node(id) {
            let old = pre[id];
            if old != n.box_ {
                out.push(Cmd::Transform { id: id.clone(), x: old.x, y: old.y, w: old.w, h: old.h });
            }
        }
    }
    out
}

fn solve_cmd(doc: &mut Document, board: Option<&str>, _ctx: &ApplyCtx) -> R<Applied> {
    if let Some(b) = board {
        let _ = board_idx(doc, b)?;
    }
    let pre = boxes(doc, board);
    let rep = crate::layout::solve(doc, board)?;
    let cmds = box_undo(&pre, doc);
    let mut changed: Vec<String> = cmds.iter().map(|c| c.target().unwrap_or_default().to_string()).collect();
    // Collected first: the filter reads `changed` while the extend writes it.
    let extra: Vec<String> = rep.changed.iter().filter(|c| !changed.contains(c)).cloned().collect();
    changed.extend(extra);
    Ok(Applied {
        changed: dedup(changed),
        created: Vec::new(),
        warnings: conflict_notes(&rep),
        undo: wrap_undo(cmds),
        revision: doc.revision,
    })
}

fn reflow(doc: &mut Document, board: &str, w: f64, h: f64, mode: &str, _ctx: &ApplyCtx) -> R<Applied> {
    let _ = board_idx(doc, board)?;
    for (k, v) in [("w", w), ("h", h)] {
        if !v.is_finite() || v <= 0.0 {
            return Err(Fail::new("INVALID_VALUE", format!("reflow {k} must be a positive finite number"))
                .details(json!({"board": board, "key": k, "value": v})));
        }
    }
    let pre = boxes(doc, Some(board));
    let rep = crate::layout::reflow(doc, board, w, h, mode)?;
    let mut cmds = box_undo(&pre, doc);
    // The artboard size itself was the requested change, so it is not undone; only
    // node boxes are, exactly as documented.
    if cmds.is_empty() {
        cmds.push(Cmd::Solve { board: Some(board.to_string()) });
    }
    let changed: Vec<String> = cmds.iter().map(|c| c.target().unwrap_or_default().to_string()).filter(|s| !s.is_empty()).collect();
    Ok(Applied {
        changed: dedup(changed),
        created: Vec::new(),
        warnings: conflict_notes(&rep),
        undo: wrap_undo(cmds),
        revision: doc.revision,
    })
}

fn wrap_undo(cmds: Vec<Cmd>) -> Cmd {
    match cmds.len() {
        0 => Cmd::Solve { board: None },
        1 => cmds.into_iter().next().unwrap(),
        _ => Cmd::Composite { cmds },
    }
}

// ------------------------------------------------------------------- misc

fn dedup(mut v: Vec<String>) -> Vec<String> {
    let mut seen = Vec::new();
    v.retain(|x| {
        if seen.contains(x) || x.is_empty() {
            false
        } else {
            seen.push(x.clone());
            true
        }
    });
    v
}

impl Cmd {
    /// Variant name, used for history labels and the call log.
    pub fn name(&self) -> &'static str {
        match self {
            Cmd::NodeAdd { .. } => "NodeAdd",
            Cmd::NodeDel { .. } => "NodeDel",
            Cmd::NodeSet { .. } => "NodeSet",
            Cmd::NodesSet { .. } => "NodesSet",
            Cmd::NodeMove { .. } => "NodeMove",
            Cmd::BoardAdd { .. } => "BoardAdd",
            Cmd::BoardDel { .. } => "BoardDel",
            Cmd::BoardSet { .. } => "BoardSet",
            Cmd::AssetAdd { .. } => "AssetAdd",
            Cmd::AssetDel { .. } => "AssetDel",
            Cmd::TokenSet { .. } => "TokenSet",
            Cmd::Text { .. } => "Text",
            Cmd::Transform { .. } => "Transform",
            Cmd::LayoutIntent { .. } => "LayoutIntent",
            Cmd::Style { .. } => "Style",
            Cmd::Group { .. } => "Group",
            Cmd::Ungroup { .. } => "Ungroup",
            Cmd::Duplicate { .. } => "Duplicate",
            Cmd::Reflow { .. } => "Reflow",
            Cmd::Solve { .. } => "Solve",
            Cmd::Composite { .. } => "Composite",
        }
    }

    /// The primary id this command acts on. `TokenSet` has none (a token name is
    /// not an id), and `Composite` reports its first member's.
    pub fn target(&self) -> Option<&str> {
        match self {
            Cmd::NodeAdd { node, .. } => Some(&node.id),
            Cmd::NodeDel { id }
            | Cmd::NodeSet { id, .. }
            | Cmd::NodeMove { id, .. }
            | Cmd::Text { id, .. }
            | Cmd::Transform { id, .. }
            | Cmd::LayoutIntent { id, .. }
            | Cmd::Style { id, .. }
            | Cmd::Ungroup { id }
            | Cmd::BoardSet { id, .. }
            | Cmd::BoardDel { id, .. }
            | Cmd::AssetDel { id, .. } => Some(id),
            Cmd::BoardAdd { board } => Some(&board.id),
            Cmd::AssetAdd { asset } => Some(&asset.id),
            Cmd::TokenSet { .. } => None,
            Cmd::Reflow { board, .. } => Some(board),
            Cmd::Solve { board } => board.as_deref(),
            Cmd::NodesSet { ids, .. } | Cmd::Group { ids, .. } | Cmd::Duplicate { ids, .. } => ids.first().map(|s| s.as_str()),
            Cmd::Composite { cmds } => cmds.first().and_then(|c| c.target()),
        }
    }

    /// Every id the command acts on.
    pub fn targets(&self) -> Vec<String> {
        match self {
            Cmd::NodesSet { ids, .. } | Cmd::Group { ids, .. } | Cmd::Duplicate { ids, .. } => ids.clone(),
            Cmd::Composite { cmds } => cmds.iter().flat_map(|c| c.targets()).collect(),
            other => other.target().map(|s| vec![s.to_string()]).unwrap_or_default(),
        }
    }

    /// Does this command change geometry, so the solver should get a look-in?
    pub fn is_geometry(&self) -> bool {
        matches!(
            self,
            Cmd::NodeAdd { .. }
                | Cmd::NodeDel { .. }
                | Cmd::NodeSet { .. }
                | Cmd::NodesSet { .. }
                | Cmd::NodeMove { .. }
                | Cmd::BoardSet { .. }
                | Cmd::Text { .. }
                | Cmd::Transform { .. }
                | Cmd::LayoutIntent { .. }
                | Cmd::Group { .. }
                | Cmd::Ungroup { .. }
                | Cmd::Duplicate { .. }
                | Cmd::Reflow { .. }
                | Cmd::Solve { .. }
        )
    }

    /// Short description for `history.list`.
    pub fn summary(&self) -> String {
        match self {
            Cmd::Composite { cmds } => format!("Composite({})", cmds.len()),
            other => match other.target() {
                Some(t) => format!("{} {t}", other.name()),
                None => other.name().to_string(),
            },
        }
    }
}

/// Every colour string in the document that does **not** parse, as
/// `(where it lives, the bad value)`. `inspect.validate` uses this for its
/// `bad-color` issue; the engine refuses to write such a value in the first place.
pub fn bad_colors(doc: &Document) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for n in &doc.nodes {
        if let Ok(v) = node_value(n) {
            let mut hits = Vec::new();
            bad_color(&doc.tokens, &v, &n.id, &mut hits);
            out.extend(hits);
        }
    }
    for a in &doc.artboards {
        if let Ok(v) = board_value(a) {
            let mut hits = Vec::new();
            bad_color(&doc.tokens, &v, &a.id, &mut hits);
            out.extend(hits);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Artboard;

    fn doc() -> Document {
        let mut d = Document::default();
        d.project_id = "p_test".into();
        d.artboards.push(Artboard {
            id: "b1".into(),
            name: "Board".into(),
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
            vars: Map::new(),
        });
        d
    }

    fn node(id: &str) -> Node {
        Node::new(id, NodeType::Rect, Rect::new(1.0, 2.0, 3.0, 4.0))
    }

    #[test]
    fn wire_tag_is_t() {
        let cmd = Cmd::NodeAdd { node: node("n1"), board: "b1".into(), at: None };
        let v = serde_json::to_value(&cmd).unwrap();
        assert_eq!(v["t"], "NodeAdd");
        assert_eq!(v["board"], "b1");
        let back: Cmd = serde_json::from_value(v).unwrap();
        assert_eq!(back.name(), "NodeAdd");
    }

    #[test]
    fn undo_patch_is_minimal() {
        let before = json!({"opacity": 1.0, "style": {"fill": {"t": "solid", "color": "#fff"}, "radius": {"all": 2.0}}});
        let after = json!({"opacity": 0.5, "style": {"fill": {"t": "solid", "color": "#fff"}, "radius": {"all": 2.0}}});
        let p = undo_patch(&before, &after);
        assert_eq!(p, json!({"opacity": 1.0}));
        assert_eq!(merge_patch(&after, &p), before);
    }

    #[test]
    fn unknown_ids_carry_the_id() {
        let mut d = doc();
        let e = apply(&mut d, &Cmd::Transform { id: "nope".into(), x: 0.0, y: 0.0, w: 1.0, h: 1.0 }, &ApplyCtx::default())
            .unwrap_err();
        assert_eq!(e.code, "NODE_NOT_FOUND");
        assert_eq!(e.details.unwrap()["id"], "nope");
        let e = apply(&mut d, &Cmd::BoardSet { id: "ghost".into(), patch: json!({"name": "x"}) }, &ApplyCtx::default())
            .unwrap_err();
        assert_eq!(e.code, "ARTBOARD_NOT_FOUND");
    }

    #[test]
    fn locked_and_forced() {
        let mut d = doc();
        let mut n = node("lock1");
        n.locked = true;
        d.nodes.push(n);
        d.artboards[0].layers.push("lock1".into());
        let e = apply(&mut d, &Cmd::Transform { id: "lock1".into(), x: 9.0, y: 9.0, w: 9.0, h: 9.0 }, &ApplyCtx::default())
            .unwrap_err();
        assert_eq!(e.code, "LOCKED");
        let ctx = ApplyCtx::default().force(true);
        apply(&mut d, &Cmd::Transform { id: "lock1".into(), x: 9.0, y: 9.0, w: 9.0, h: 9.0 }, &ctx).unwrap();
        assert_eq!(d.node("lock1").unwrap().box_.x, 9.0);
    }

    #[test]
    fn revision_bumps_once_per_top_level_command() {
        let mut d = doc();
        let ctx = ApplyCtx::default();
        let a = apply(&mut d, &Cmd::NodeAdd { node: node("a"), board: "b1".into(), at: None }, &ctx).unwrap();
        assert_eq!(a.revision, 1);
        let batch = apply(
            &mut d,
            &Cmd::Composite {
                cmds: vec![
                    Cmd::NodeAdd { node: node("b"), board: "b1".into(), at: None },
                    Cmd::NodeAdd { node: node("c"), board: "b1".into(), at: None },
                ],
            },
            &ctx,
        )
        .unwrap();
        assert_eq!(batch.revision, 2, "a Composite is one transaction");
        assert_eq!(d.node("b").unwrap().opacity, 1.0);
    }

    #[test]
    fn bad_color_is_reported_with_the_value() {
        let mut d = doc();
        d.nodes.push(node("c1"));
        let e = apply(
            &mut d,
            &Cmd::NodeSet { id: "c1".into(), patch: json!({"style": {"color": "not-a-color"}}) },
            &ApplyCtx::default(),
        )
        .unwrap_err();
        assert_eq!(e.code, "INVALID_VALUE");
        assert_eq!(e.details.unwrap()["value"], "not-a-color");
        assert!(d.node("c1").unwrap().style.is_none(), "failed patch must not persist");
    }
}
