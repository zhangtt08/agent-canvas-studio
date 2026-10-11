//! Machine-readable quality inspection.
//!
//! This module is how an agent *sees* its own work. The report is
//! **evidence-bearing**: every problem is paired with the numbers that prove it
//! (pixel overflows, contrast ratios, font sizes, line counts) and, where a fix
//! is mechanical, a real [`Cmd`] the caller can apply without asking a human.
//!
//! ## How overlaps are judged (the non-obvious rule)
//!
//! Overlap is **not** an error by default — designers stack backgrounds and
//! decorations on purpose. What the inspector cares about is *z-roles*:
//!
//! - `background` / `decor` may be covered freely; they are scenery.
//! - `content` / `label` must remain legible.
//!
//! We therefore only emit an `invisible` ("covered") issue when a node that is
//! later in the artboard's paint order is *opaque* (opacity >= 0.99, no blend,
//! and either a fully solid fill or a non-transparent image) **and** its rotated
//! box completely swallows a `content`/`label` node's rotated box. Anything less
//! (partial overlap, a translucent card, a photo the text sits beside) is fine
//! and stays silent. A node with `opacity == 0` is invisible for a different,
//! unambiguous reason and is reported at `info` severity.
//!
//! Everything else — off-board, unsolved text, dead asset references, poor
//! contrast — is judged with the numeric evidence described per code below.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::commands::Cmd;
use crate::doc::{self, Artboard, Document, Node, NodeType, Paint};
use crate::error::{Fail, R};
use crate::geom::Rect;
use crate::layout;
use crate::text::{self, MeasureOpts};
use crate::util;

/// A single finding. `sev` is one of `error|warn|info`; `code` is stable and
/// agents switch on it. `evidence` carries the numbers, `fix` (when present) is a
/// serialized [`Cmd`] the caller can apply verbatim.
#[derive(Clone, Debug)]
pub struct Issue {
    pub sev: &'static str,
    pub code: &'static str,
    pub node: Option<String>,
    pub board: Option<String>,
    pub msg: String,
    pub evidence: Value,
    pub fix: Option<Value>,
}

impl Issue {
    fn rank(sev: &str) -> u8 {
        match sev {
            "error" => 2,
            "warn" => 1,
            _ => 0,
        }
    }
}

// ------------------------------------------------------------- public surface

/// Walk every board (or just `opts.board`) and produce the compact, agent-facing
/// report `{ok, errors, warns, infos, issues:[{s,c,n,b,m,e,f?}], boards:[{b,nodes,ink}]}`.
///
/// `opts` honours `contrast:0/1` (default 1), `lowres:0/1` (default 1),
/// `sev_min:"info"|"warn"|"error"` (default "info") and `board:"<id>"`.
pub fn validate(doc: &Document, dir: &Path, opts: &Value) -> R<Value> {
    let want_contrast = util::opt_b(opts, "contrast", true);
    let want_lowres = util::opt_b(opts, "lowres", true);
    let sev_min = Issue::rank(util::ps(opts, "sev_min").unwrap_or("info"));
    let only_board = util::ps(opts, "board").filter(|s| !s.is_empty());

    let mut insp = Inspector {
        doc,
        dir,
        want_contrast,
        want_lowres,
        sev_min,
        reg: text::Registry::shared(),
        issues: Vec::new(),
    };

    // Document-wide integrity checks (skipped when scoped to one board so the
    // other boards' nodes are not mislabelled as unlinked).
    if only_board.is_none() {
        insp.dup_ids();
    }

    // Per-board inspection in document order, nodes in paint order.
    for ab in &doc.artboards {
        if !ab.visible {
            continue;
        }
        if let Some(b) = only_board {
            if ab.id != b {
                continue;
            }
        }
        insp.board(ab);
    }

    if only_board.is_none() {
        insp.unlinked();
    }

    // Constraint conflicts: solve a throwaway copy and surface the solver's
    // conflict list without touching the caller's document.
    insp.conflicts(only_board);

    let issues = insp.issues;
    Ok(summarize(doc, &issues, only_board))
}

/// Boxes + resolved roles for the requested ids (or all nodes when `ids` empty).
pub fn geometry(doc: &Document, ids: &[String]) -> Vec<Value> {
    let mut out = Vec::new();
    for n in &doc.nodes {
        if !ids.is_empty() && !ids.iter().any(|i| i == &n.id) {
            continue;
        }
        let rb = n.rotated_box();
        let mut o = json!({
            "id": n.id,
            "t": n.kind.as_str(),
            "x": round(n.box_.x), "y": round(n.box_.y),
            "w": round(n.box_.w), "h": round(n.box_.h),
            "rot": round(n.rotation),
            "rb": [round(rb.x), round(rb.y), round(rb.w), round(rb.h)],
        });
        if let Some(a) = n.artboard.as_deref() {
            o["b"] = json!(a);
        }
        if let Some(p) = n.parent.as_deref() {
            o["parent"] = json!(p);
        }
        if let Some(z) = n.z_role.as_deref() {
            o["z_role"] = json!(z);
        }
        out.push(o);
    }
    out
}

/// Solve the node's text with the shared font registry and report the metrics the
/// compiler will actually use.
pub fn text_metrics(doc: &Document, id: &str) -> R<Value> {
    let n = doc.node(id).ok_or_else(|| Fail::new("NODE_NOT_FOUND", format!("no node \"{id}\"")))?;
    let spec = n
        .text
        .as_ref()
        .ok_or_else(|| Fail::new("INVALID_SCHEMA", format!("node {id} is not a text node")))?;
    let reg = text::Registry::shared();
    let m = measure(doc, spec, n.box_, reg);
    let mode = spec.overflow.clone().unwrap_or_else(|| "clip".into());
    let mm = text::layout_text(&spec.content, &m, &mode);
    Ok(json!({
        "id": id,
        "lines": mm.lines,
        "width": round(mm.width),
        "height": round(mm.height),
        "line_height": round(mm.line_height),
        "ascender": round(mm.ascender),
        "descender": round(mm.descender),
        "overflow": mm.overflow,
        "truncated": mm.truncated,
        "font": mm.font,
        "fell_back": mm.fell_back,
        "need": round(mm.height),
        "have": round(n.box_.h),
        "box": [round(n.box_.x), round(n.box_.y), round(n.box_.w), round(n.box_.h)],
        "font_size": round(m.size),
    }))
}

/// Re-solve a copy of the document and surface the layout conflicts without
/// mutating the caller's document.
pub fn conflicts(doc: &Document, board: &str) -> Vec<Value> {
    let mut clone = doc.clone();
    let report = match layout::solve(&mut clone, Some(board)) {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    report
        .conflicts
        .into_iter()
        .map(|c| {
            let mut o = json!({ "node": c.node, "intent": c.intent, "why": c.why });
            if let Some(w) = c.with {
                o["with"] = json!(w);
            }
            o
        })
        .collect()
}

/// One row per asset: `{id, name, uses, nodes:[...]}` — the usage map for
/// `inspect.assets`.
pub fn asset_usage(doc: &Document) -> Vec<Value> {
    let mut refs: HashMap<String, Vec<String>> = HashMap::new();
    for n in &doc.nodes {
        if let Some(img) = &n.image {
            if !img.asset.is_empty() {
                refs.entry(img.asset.clone()).or_default().push(n.id.clone());
            }
        }
        if let Some(st) = &n.style {
            if let Some(p) = &st.fill {
                if let Paint::Image { asset, .. } = p {
                    if !asset.is_empty() {
                        refs.entry(asset.clone()).or_default().push(n.id.clone());
                    }
                }
            }
        }
        if let Some(v) = &n.vector {
            if let Some(a) = &v.asset {
                if !a.is_empty() {
                    refs.entry(a.clone()).or_default().push(n.id.clone());
                }
            }
        }
    }
    let mut out = Vec::new();
    for a in &doc.assets {
        let nodes = refs.remove(&a.id).unwrap_or_default();
        let mut o = json!({
            "id": a.id,
            "name": a.name,
            "uses": nodes.len(),
            "kind": a.kind.as_deref().unwrap_or("image"),
        });
        if !nodes.is_empty() {
            o["nodes"] = json!(nodes);
        }
        out.push(o);
    }
    // Assets referenced by nodes but missing from the registry (dead links).
    for (id, nodes) in refs {
        out.push(json!({
            "id": id,
            "name": "",
            "uses": nodes.len(),
            "missing": true,
            "nodes": nodes,
        }));
    }
    out
}

/// Node-by-node structural diff of two documents — the payload `history.compare`
/// and the "what did the agent just do" panel consume. Every change carries a
/// dotted key path and before/after values.
pub fn diff_docs(a: &Document, b: &Document) -> Value {
    let am = node_map(a);
    let bm = node_map(b);
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();

    for id in bm.keys() {
        if !am.contains_key(id) {
            added.push(id.clone());
        }
    }
    for id in am.keys() {
        if !bm.contains_key(id) {
            removed.push(id.clone());
        }
    }
    // Deterministic ordering over shared ids.
    let mut shared: Vec<&String> = am.keys().filter(|k| bm.contains_key(*k)).collect();
    shared.sort();
    for id in shared {
        let diffs = flatten_diff(&am[id], &bm[id]);
        if !diffs.is_empty() {
            changed.push(json!({ "id": id, "keys": diffs }));
        }
    }

    let board_diff = board_diff(a, b);
    json!({
        "added": added,
        "removed": removed,
        "changed": changed,
        "boards": board_diff,
        "summary": format!(
            "+{} -{} ~{} boards+{}",
            added.len(),
            removed.len(),
            changed.len(),
            board_diff.get("added").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0)
        ),
    })
}

/// Collect the serialized `Cmd`s attached to issues, in order, for `inspect.fix`.
pub fn suggest_fixes(issues: &[Issue]) -> Vec<Cmd> {
    let mut out = Vec::new();
    for i in issues {
        if let Some(f) = &i.fix {
            if let Ok(c) = serde_json::from_value::<Cmd>(f.clone()) {
                out.push(c);
            }
        }
    }
    out
}

// -------------------------------------------------------------- internals

const SAFE_TOL: f64 = 0.5;

fn round(v: f64) -> f64 {
    util::round(v, 2)
}

fn rect_val(r: Rect) -> Value {
    json!({ "x": round(r.x), "y": round(r.y), "w": round(r.w), "h": round(r.h) })
}

fn box_arr(r: Rect) -> Value {
    json!([round(r.x), round(r.y), round(r.w), round(r.h)])
}

/// Build measurement options from a text spec, substituting `$tokens` and
/// resolving the font family against the shared registry.
pub(crate) fn measure(doc: &Document, spec: &doc::TextSpec, bx: Rect, reg: &text::Registry) -> MeasureOpts {
    let family_raw = spec
        .family
        .clone()
        .or_else(|| spec.font_ref.clone().and_then(|r| doc.tokens.font(&r).map(|f| f.family.clone())))
        .unwrap_or_default();
    let family = reg.resolve(&doc.tokens.expand(&family_raw));
    let weight = spec.weight.unwrap_or(400);
    let italic = spec.style_italic.unwrap_or(false);
    let size = spec.size.unwrap_or(16.0);
    let ls = spec.letter_spacing.unwrap_or(0.0);
    let line_h = spec.line_h();
    let max_w = (bx.w - spec.padding.unwrap_or(0.0) * 2.0).max(1.0);
    MeasureOpts {
        family,
        weight,
        italic,
        size,
        ls,
        line_h,
        max_w,
        align: spec.align.clone().unwrap_or_else(|| "left".into()),
        valign: spec.valign.clone().unwrap_or_else(|| "top".into()),
    }
}

/// `true` when a node counts as *ink* (content that should be visible). Scenery
/// (`background`/`decor`) is excluded; anything else (content, label, unmarked)
/// contributes to the board's ink coverage.
fn is_ink(n: &Node) -> bool {
    match n.z_role.as_deref() {
        Some("background") | Some("decor") => false,
        _ => true,
    }
}

/// A node that can be a target of the "covered" invisibility check.
fn is_coverable(n: &Node) -> bool {
    matches!(n.z_role.as_deref(), Some("content") | Some("label"))
}

/// An *opaque, area-covering* node: a solid fully-opaque fill, or a photo/image
/// without reported alpha, painted at (near) full opacity with no blend mode.
fn is_opaque(doc: &Document, n: &Node) -> bool {
    if !n.visible || n.opacity < 0.99 || n.blend.is_some() {
        return false;
    }
    match n.kind {
        NodeType::Rect
        | NodeType::Ellipse
        | NodeType::Frame
        | NodeType::Polygon
        | NodeType::Path
        | NodeType::Svg => match n.style.as_ref().and_then(|s| s.fill.as_ref()) {
            Some(Paint::Solid { color }) => {
                let e = doc.tokens.expand(color);
                doc::parse_color(&e).map(|(_, _, _, a)| a >= 0.99).unwrap_or(false)
            }
            // Gradients are treated as opaque here; images as fills if referenced.
            Some(Paint::Linear { .. }) | Some(Paint::Radial { .. }) => true,
            Some(Paint::Image { asset, .. }) => !asset.is_empty(),
            _ => false,
        },
        NodeType::Image => n
            .image
            .as_ref()
            .and_then(|i| doc.asset(&i.asset).and_then(|a| a.alpha))
            .map(|alpha| !alpha)
            .unwrap_or(true),
        _ => false,
    }
}

/// Nearest opaque *solid* colour behind `idx` in paint order that covers the node
/// centre, else the artboard background colour. Returns `None` when no reliable
/// colour is known (e.g. a photo behind the text) so contrast is skipped rather
/// than guessed.
fn effective_bg(doc: &Document, ab: &Artboard, order: &[String], idx: usize, center: (f64, f64)) -> Option<String> {
    for j in (0..idx).rev() {
        let n = doc.node(&order[j])?;
        if !is_opaque(doc, n) {
            continue;
        }
        if !n.rotated_box().contains(&Rect::new(center.0 - 0.5, center.1 - 0.5, 1.0, 1.0), 1.0) {
            continue;
        }
        if let Some(p) = n.style.as_ref().and_then(|s| s.fill.as_ref()) {
            if let Paint::Solid { color } = p {
                let e = doc.tokens.expand(color);
                if doc::parse_color(&e).is_some() {
                    return Some(e);
                }
            }
        }
    }
    if let Some(Paint::Solid { color }) = ab.background.as_ref() {
        let e = doc.tokens.expand(color);
        if doc::parse_color(&e).is_some() {
            return Some(e);
        }
    }
    None
}

/// Union area of a set of rectangles via coordinate compression — cheap and exact,
/// used for the `ink` layout signal.
fn union_area(rects: &[Rect]) -> f64 {
    let mut xs: Vec<f64> = vec![];
    let mut ys: Vec<f64> = vec![];
    for r in rects {
        xs.push(r.x);
        xs.push(r.x2());
        ys.push(r.y);
        ys.push(r.y2());
    }
    if xs.is_empty() {
        return 0.0;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    ys.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let mut area = 0.0;
    for cx in xs.windows(2) {
        let (lx, rx) = (cx[0], cx[1]);
        if rx - lx <= 1e-9 {
            continue;
        }
        let mx = (lx + rx) / 2.0;
        for cy in ys.windows(2) {
            let (ty, by) = (cy[0], cy[1]);
            if by - ty <= 1e-9 {
                continue;
            }
            let my = (ty + by) / 2.0;
            let p = Rect::new(mx, my, 1.0, 1.0);
            if rects.iter().any(|r| r.contains(&p, 1e-6)) {
                area += (rx - lx) * (by - ty);
            }
        }
    }
    area
}

/// Which semantic kinds are "clickable / brand" targets that must stay tappable.
fn is_target(n: &Node) -> bool {
    if let Some(r) = n.role.as_deref() {
        if matches!(r, "button" | "cta" | "link" | "badge" | "logo" | "brand" | "interactive" | "target") {
            return true;
        }
    }
    let hay = n.name.to_lowercase();
    ["btn", "button", "cta", "logo", "brand", "badge", "target", "price", "buy"]
        .iter()
        .any(|k| hay.contains(k))
}

struct Inspector<'a> {
    doc: &'a Document,
    dir: &'a Path,
    want_contrast: bool,
    want_lowres: bool,
    sev_min: u8,
    reg: &'a text::Registry,
    issues: Vec<Issue>,
}

impl<'a> Inspector<'a> {
    fn push(&mut self, i: Issue) {
        if Issue::rank(i.sev) >= self.sev_min {
            self.issues.push(i);
        }
    }

    fn dup_ids(&mut self) {
        let mut seen: HashMap<String, u32> = HashMap::new();
        for n in &self.doc.nodes {
            *seen.entry(n.id.clone()).or_default() += 1;
        }
        let mut dupes: Vec<(String, u32)> = seen
            .into_iter()
            .filter(|(_, c)| *c > 1)
            .map(|(k, c)| (k, c))
            .collect();
        dupes.sort();
        for (id, c) in dupes {
            self.push(Issue {
                sev: "error",
                code: "dup-id",
                node: Some(id.clone()),
                board: None,
                msg: format!("duplicate node id \"{id}\" appears {c} times"),
                evidence: json!({ "count": c }),
                fix: None,
            });
        }
    }

    fn unlinked(&mut self) {
        let mut reach: HashMap<String, ()> = HashMap::new();
        for ab in &self.doc.artboards {
            for id in self.doc.paint_order(&ab.id) {
                reach.insert(id, ());
            }
        }
        for n in &self.doc.nodes {
            if reach.contains_key(n.id.as_str()) {
                continue;
            }
            let mut e = json!({});
            if let Some(b) = n.artboard.as_deref() {
                e["artboard"] = json!(b);
            }
            if let Some(p) = n.parent.as_deref() {
                e["parent"] = json!(p);
            }
            self.push(Issue {
                sev: "info",
                code: "unlinked",
                node: Some(n.id.clone()),
                board: n.artboard.clone(),
                msg: format!("node \"{}\" is not reachable from any board", n.id),
                evidence: e,
                fix: Some(serde_json::to_value(Cmd::NodeDel { id: n.id.clone() }).unwrap_or(Value::Null)),
            });
        }
    }

    fn conflicts(&mut self, only_board: Option<&str>) {
        let mut clone = self.doc.clone();
        let report = match layout::solve(&mut clone, None) {
            Ok(r) => r,
            Err(_) => return,
        };
        for c in report.conflicts {
            let board = self
                .doc
                .node(&c.node)
                .and_then(|n| self.doc.artboard_of(n))
                .map(|a| a.id.clone())
                .unwrap_or_default();
            if let Some(b) = only_board {
                if board != b {
                    continue;
                }
            }
            let mut e = json!({ "intent": c.intent, "why": c.why });
            if let Some(w) = c.with {
                e["with"] = json!(w);
            }
            self.push(Issue {
                sev: "warn",
                code: "conflict",
                node: Some(c.node),
                board: if board.is_empty() { None } else { Some(board) },
                msg: format!("layout conflict: {}", c.why),
                evidence: e,
                fix: None,
            });
        }
    }

    fn board(&mut self, ab: &Artboard) {
        let order = self.doc.paint_order(&ab.id);
        let nodes: Vec<&Node> = order.iter().filter_map(|id| self.doc.node(id)).collect();

        // bad-color on the board background itself.
        if let Some(Paint::Solid { color }) = ab.background.as_ref() {
            self.bad_color(color, None, &ab.id, "background");
        }

        let board_rect = Rect::new(0.0, 0.0, ab.width, ab.height);

        for (idx, n) in nodes.iter().enumerate() {
            self.bad_color_node(n);
            self.offboard(n, &ab.id, board_rect);
            self.zero_size(n, &ab.id);
            self.empty_text(n, &ab.id);
            self.text_overflow(n, &ab.id);
            self.font_issues(n, &ab.id);
            self.assets(n, &ab.id);
            if self.want_lowres {
                self.low_res(n, &ab.id);
            }
            if self.want_contrast {
                self.contrast(n, ab, &order, idx);
            }
            self.safe_area(n, ab, board_rect);
            self.tiny_target(n, &ab.id);
            self.invisible_covered(n, ab, &order, idx, board_rect);
        }
    }

    // --- colour --------------------------------------------------------------

    fn bad_color(&mut self, raw: &str, node: Option<String>, board: &str, key: &str) {
        let expanded = self.doc.tokens.expand(raw);
        if doc::parse_color(&expanded).is_some() {
            return;
        }
        self.push(Issue {
            sev: "error",
            code: "bad-color",
            node: node.clone(),
            board: Some(board.to_string()),
            msg: format!("unparseable colour \"{raw}\" at {key}"),
            evidence: json!({ "value": raw, "node": node.unwrap_or_else(|| board.to_string()), "key": key }),
            fix: None,
        });
    }

    fn bad_color_node(&mut self, n: &Node) {
        let board = n.artboard.clone().unwrap_or_default();
        if let Some(st) = &n.style {
            if let Some(p) = &st.fill {
                self.paint_colors(p, &n.id, &board);
            }
            if let Some(sc) = &st.color {
                self.bad_color(sc, Some(n.id.clone()), &board, "style.color");
            }
            if let Some(so) = &st.stroke {
                self.bad_color(&so.color, Some(n.id.clone()), &board, "style.stroke.color");
            }
        }
        if let Some(t) = &n.text {
            if let Some(c) = &t.color {
                self.bad_color(c, Some(n.id.clone()), &board, "text.color");
            }
            if let Some(p) = &t.paint {
                self.paint_colors(p, &n.id, &board);
            }
        }
        if let Some(i) = &n.image {
            if let Some(t) = &i.tint {
                self.bad_color(t, Some(n.id.clone()), &board, "image.tint");
            }
        }
        if let Some(fx) = &n.fx {
            for sh in &fx.shadows {
                self.bad_color(&sh.color, Some(n.id.clone()), &board, "fx.shadow.color");
            }
            if let Some(g) = &fx.inner_glow {
                self.bad_color(g, Some(n.id.clone()), &board, "fx.inner_glow");
            }
        }
    }

    fn paint_colors(&mut self, p: &Paint, node: &str, board: &str) {
        match p {
            Paint::Solid { color } => self.bad_color(color, Some(node.to_string()), board, "style.fill.color"),
            Paint::Linear { stops, .. } | Paint::Radial { stops, .. } => {
                for s in stops {
                    self.bad_color(&s.color, Some(node.to_string()), board, "style.fill.stop");
                }
            }
            _ => {}
        }
    }

    // --- geometry ------------------------------------------------------------

    fn offboard(&mut self, n: &Node, board_id: &str, board_rect: Rect) {
        let rb = n.rotated_box();
        if !rb.is_finite() {
            return;
        }
        let t = (0.0 - rb.y).max(0.0);
        let l = (0.0 - rb.x).max(0.0);
        let r = (rb.x2() - board_rect.w).max(0.0);
        let b = (rb.y2() - board_rect.h).max(0.0);
        if t <= SAFE_TOL && r <= SAFE_TOL && b <= SAFE_TOL && l <= SAFE_TOL {
            return;
        }
        let fully_out = rb.intersection(&board_rect).is_none();
        let fix = if fully_out {
            None
        } else {
            // Translate the pre-rotation box by the net overflow so the rotated
            // box lands fully inside; rigid translation keeps the AABB delta equal.
            let nx = n.box_.x + (l - r);
            let ny = n.box_.y + (t - b);
            Some(Cmd::Transform { id: n.id.clone(), x: nx, y: ny, w: n.box_.w, h: n.box_.h })
        };
        self.push(Issue {
            sev: if fully_out { "error" } else { "warn" },
            code: "offboard",
            node: Some(n.id.clone()),
            board: Some(board_id.to_string()),
            msg: format!(
                "\"{}\" extends past the board {}",
                n.id,
                if fully_out { "entirely" } else { "slightly" }
            ),
            evidence: json!({
                "box": rect_val(rb),
                "board": { "w": round(board_rect.w), "h": round(board_rect.h) },
                "overflow": { "t": round(t), "r": round(r), "b": round(b), "l": round(l) },
                "fully_out": fully_out,
            }),
            fix: fix.and_then(|c| serde_json::to_value(c).ok()),
        });
    }

    fn zero_size(&mut self, n: &Node, board_id: &str) {
        if n.box_.w >= 1.0 && n.box_.h >= 1.0 {
            return;
        }
        self.push(Issue {
            sev: "warn",
            code: "zero-size",
            node: Some(n.id.clone()),
            board: Some(board_id.to_string()),
            msg: format!("\"{}\" has a degenerate size", n.id),
            evidence: json!({ "w": round(n.box_.w), "h": round(n.box_.h) }),
            fix: None,
        });
    }

    fn safe_area(&mut self, n: &Node, ab: &Artboard, board_rect: Rect) {
        let m = match ab.safe_margin {
            Some(m) if m > 0.0 => m,
            _ => return,
        };
        if !is_ink(n) {
            return;
        }
        let rb = n.rotated_box();
        let inner = board_rect.inset(m, m, m, m);
        if inner.contains(&rb, SAFE_TOL) {
            return;
        }
        let t = (inner.y - rb.y).max(0.0);
        let l = (inner.x - rb.x).max(0.0);
        let r = (rb.x2() - inner.x2()).max(0.0);
        let b = (rb.y2() - inner.y2()).max(0.0);
        self.push(Issue {
            sev: "warn",
            code: "safe-area",
            node: Some(n.id.clone()),
            board: Some(ab.id.clone()),
            msg: format!("\"{}\" crosses the {m}px safe margin", n.id),
            evidence: json!({
                "margin": round(m),
                "box": rect_val(rb),
                "overflow": { "t": round(t), "l": round(l), "r": round(r), "b": round(b) },
            }),
            // Translate the box back inside the safe area.
            fix: Some(
                serde_json::to_value(Cmd::Transform {
                    id: n.id.clone(),
                    x: n.box_.x + (l - r),
                    y: n.box_.y + (t - b),
                    w: n.box_.w,
                    h: n.box_.h,
                })
                .unwrap_or(Value::Null),
            ),
        });
    }

    fn tiny_target(&mut self, n: &Node, board_id: &str) {
        if !is_target(n) {
            return;
        }
        let rb = n.rotated_box();
        let min = rb.w.min(rb.h);
        if min >= 48.0 {
            return;
        }
        self.push(Issue {
            sev: "warn",
            code: "tiny-target",
            node: Some(n.id.clone()),
            board: Some(board_id.to_string()),
            msg: format!("\"{}\" is a tap/brand target under 48px", n.id),
            evidence: json!({ "w": round(rb.w), "h": round(rb.h), "min": round(min), "need": 48 }),
            fix: None,
        });
    }

    // --- text ----------------------------------------------------------------

    fn empty_text(&mut self, n: &Node, board_id: &str) {
        if let Some(t) = &n.text {
            if t.content.trim().is_empty() {
                self.push(Issue {
                    sev: "info",
                    code: "empty-text",
                    node: Some(n.id.clone()),
                    board: Some(board_id.to_string()),
                    msg: format!("\"{}\" has no text content", n.id),
                    evidence: json!({ "chars": t.content.chars().count() }),
                    fix: None,
                });
            }
        }
    }

    fn text_overflow(&mut self, n: &Node, board_id: &str) {
        let spec = match &n.text {
            Some(s) if !s.content.trim().is_empty() => s,
            _ => return,
        };
        if matches!(spec.box_mode.as_deref(), Some("auto-height") | Some("auto")) {
            return; // height is derived; the box grows to fit.
        }
        let m = measure(self.doc, spec, n.box_, self.reg);
        let mode = spec.overflow.clone().unwrap_or_else(|| "clip".into());
        let mm = text::layout_text(&spec.content, &m, &mode);
        let need = mm.height;
        let have = n.box_.h;
        let bad = mm.overflow || mm.truncated || need > have + 1.0;
        if !bad {
            return;
        }
        let size = m.size;
        // Shrink-to-fit: scale the font by the available/needed ratio.
        let mut ns = size * (have / need.max(1.0));
        if let Some(min) = spec.min_font_size {
            ns = ns.max(min);
        }
        ns = ns.max(4.0);
        let patch = json!({ "text": { "size": round(ns), "overflow": "shrink" } });
        self.push(Issue {
            sev: "warn",
            code: "text-overflow",
            node: Some(n.id.clone()),
            board: Some(board_id.to_string()),
            msg: format!(
                "\"{}\" needs {}px but has {}px ({} lines)",
                n.id,
                round(need),
                round(have),
                mm.lines.len()
            ),
            evidence: json!({
                "lines": mm.lines.len(),
                "need": round(need),
                "have": round(have),
                "font_size": round(size),
                "truncated": mm.truncated,
            }),
            fix: Some(serde_json::to_value(Cmd::NodeSet { id: n.id.clone(), patch }).unwrap_or(Value::Null)),
        });
    }

    fn font_issues(&mut self, n: &Node, board_id: &str) {
        let spec = match &n.text {
            Some(s) => s,
            None => return,
        };
        let want = spec.family.clone().unwrap_or_default();
        if want.trim().is_empty() {
            return;
        }
        let want = self.doc.tokens.expand(&want);
        let got = self.reg.resolve(&want);
        // Fell back when none of the requested families actually exist.
        let any_hit = want
            .split(',')
            .map(|f| f.trim().trim_matches('"'))
            .filter(|f| !f.is_empty())
            .any(|f| self.reg.has(f));
        if !any_hit {
            self.push(Issue {
                sev: "warn",
                code: "missing-font",
                node: Some(n.id.clone()),
                board: Some(board_id.to_string()),
                msg: format!("\"{}\" wants font \"{want}\", used \"{got}\"", n.id),
                evidence: json!({ "want": want, "got": got }),
                fix: None,
            });
        }
        if !spec.content.trim().is_empty() && !self.reg.covers(&spec.content, &got, spec.weight.unwrap_or(400)) {
            self.push(Issue {
                sev: "warn",
                code: "tofu-risk",
                node: Some(n.id.clone()),
                board: Some(board_id.to_string()),
                msg: format!("\"{}\" face \"{got}\" lacks glyphs for the content (tofu)", n.id),
                evidence: json!({ "want": want, "got": got, "chars": sample_chars(&spec.content) }),
                fix: None,
            });
        }
    }

    fn contrast(&mut self, n: &Node, ab: &Artboard, order: &[String], idx: usize) {
        let spec = match &n.text {
            Some(s) if !s.content.trim().is_empty() => s,
            _ => return,
        };
        let fg_raw = match spec.color.as_deref() {
            Some(c) => c,
            None => return,
        };
        let fg = self.doc.tokens.expand(fg_raw);
        if doc::parse_color(&fg).is_none() {
            return; // reported as bad-color instead.
        }
        let center = (n.box_.cx(), n.box_.cy());
        let bg = match effective_bg(self.doc, ab, order, idx, center) {
            Some(b) => b,
            None => return, // photo / unknown background — do not guess.
        };
        let ratio = match doc::contrast_ratio(&fg, &bg) {
            Some(r) => r,
            None => return,
        };
        if ratio >= 4.5 {
            return;
        }
        self.push(Issue {
            sev: if ratio < 3.0 { "error" } else { "warn" },
            code: "contrast",
            node: Some(n.id.clone()),
            board: Some(ab.id.clone()),
            msg: format!("\"{}\" contrast {ratio:.2}:1 is below 4.5:1", n.id),
            evidence: json!({ "ratio": util::round(ratio, 3), "need": 4.5, "fg": fg, "bg": bg }),
            fix: None,
        });
    }

    // --- assets / images -----------------------------------------------------

    fn assets(&mut self, n: &Node, board_id: &str) {
        let mut refs: Vec<String> = vec![];
        if let Some(i) = &n.image {
            if !i.asset.is_empty() {
                refs.push(i.asset.clone());
            }
        }
        if let Some(st) = &n.style {
            if let Some(Paint::Image { asset, .. }) = &st.fill {
                if !asset.is_empty() {
                    refs.push(asset.clone());
                }
            }
        }
        if let Some(v) = &n.vector {
            if let Some(a) = &v.asset {
                if !a.is_empty() {
                    refs.push(a.clone());
                }
            }
        }
        for id in refs {
            match self.doc.asset(&id) {
                None => self.push(Issue {
                    sev: "warn",
                    code: "missing-asset",
                    node: Some(n.id.clone()),
                    board: Some(board_id.to_string()),
                    msg: format!("\"{}\" references unknown asset \"{id}\"", n.id),
                    evidence: json!({ "asset": id }),
                    fix: None,
                }),
                Some(a) => {
                    let p = self.dir.join(&a.uri);
                    if !p.exists() {
                        self.push(Issue {
                            sev: "warn",
                            code: "asset-missing",
                            node: Some(n.id.clone()),
                            board: Some(board_id.to_string()),
                            msg: format!("asset \"{id}\" file is gone from disk ({})", a.uri),
                            evidence: json!({ "asset": id, "uri": a.uri }),
                            fix: None,
                        });
                    }
                }
            }
        }
    }

    fn low_res(&mut self, n: &Node, board_id: &str) {
        let i = match &n.image {
            Some(i) if !i.asset.is_empty() => i,
            _ => return,
        };
        let a = match self.doc.asset(&i.asset) {
            Some(a) if a.width > 0.0 && a.height > 0.0 => a,
            _ => return,
        };
        let sx = n.box_.w / a.width;
        let sy = n.box_.h / a.height;
        let scale = sx.max(sy);
        if scale <= 1.6 {
            return;
        }
        self.push(Issue {
            sev: if scale >= 2.0 { "warn" } else { "info" },
            code: "low-res",
            node: Some(n.id.clone()),
            board: Some(board_id.to_string()),
            msg: format!("\"{}\" is upscaled {scale:.2}x beyond its native pixels", n.id),
            evidence: json!({
                "native": [round(a.width), round(a.height)],
                "placed": box_arr(n.box_),
                "scale": util::round(scale, 3),
            }),
            fix: None,
        });
    }

    fn invisible_covered(&mut self, n: &Node, ab: &Artboard, order: &[String], idx: usize, board_rect: Rect) {
        // opacity 0 is invisible for an unambiguous reason — any node kind.
        if n.visible && n.opacity <= 0.001 {
            self.push(Issue {
                sev: "info",
                code: "invisible",
                node: Some(n.id.clone()),
                board: Some(ab.id.clone()),
                msg: format!("\"{}\" has opacity 0", n.id),
                evidence: json!({ "opacity": round(n.opacity), "reason": "opacity-0" }),
                fix: Some(
                    serde_json::to_value(if n.children.is_empty() {
                        Cmd::NodeDel { id: n.id.clone() }
                    } else {
                        Cmd::NodeSet { id: n.id.clone(), patch: json!({ "opacity": 1.0 }) }
                    })
                    .unwrap_or(Value::Null),
                ),
            });
            return;
        }
        // Full coverage only matters for content that must stay legible.
        if !is_coverable(n) {
            return;
        }
        let rb = n.rotated_box();
        // Outside the board is already reported by offboard; only judge coverage.
        for j in (idx + 1)..order.len() {
            let cover = match self.doc.node(&order[j]) {
                Some(c) => c,
                None => continue,
            };
            if !is_opaque(self.doc, cover) {
                continue;
            }
            if cover.rotated_box().contains(&rb, SAFE_TOL) {
                self.push(Issue {
                    sev: "warn",
                    code: "invisible",
                    node: Some(n.id.clone()),
                    board: Some(ab.id.clone()),
                    msg: format!("\"{}\" is fully covered by opaque node \"{}\"", n.id, cover.id),
                    evidence: json!({
                        "by": cover.id,
                        "reason": "covered",
                        "box": rect_val(rb),
                        "board": { "w": round(board_rect.w), "h": round(board_rect.h) },
                    }),
                    fix: None,
                });
                return;
            }
        }
    }
}

fn sample_chars(s: &str) -> String {
    s.chars().take(8).collect()
}

/// Render the flat issue list into the compact report object.
fn summarize(doc: &Document, issues: &[Issue], only_board: Option<&str>) -> Value {
    let mut errors = 0;
    let mut warns = 0;
    let mut infos = 0;
    let mut arr = Vec::with_capacity(issues.len());
    for i in issues {
        match i.sev {
            "error" => errors += 1,
            "warn" => warns += 1,
            _ => infos += 1,
        }
        let mut o = json!({ "s": i.sev, "c": i.code, "m": i.msg, "e": i.evidence });
        if let Some(n) = &i.node {
            o["n"] = json!(n);
        }
        if let Some(b) = &i.board {
            o["b"] = json!(b);
        }
        if let Some(f) = &i.fix {
            o["f"] = f.clone();
        }
        arr.push(o);
    }

    let mut boards = Vec::new();
    for ab in &doc.artboards {
        if !ab.visible {
            continue;
        }
        if let Some(b) = only_board {
            if ab.id != b {
                continue;
            }
        }
        let order = doc.paint_order(&ab.id);
        let area = (ab.width * ab.height).max(1.0);
        let rects: Vec<Rect> = order
            .iter()
            .filter_map(|id| doc.node(id))
            .filter(|n| is_ink(n))
            .map(|n| {
                let rb = n.rotated_box();
                Rect::new(rb.x, rb.y, rb.w.max(0.0), rb.h.max(0.0))
            })
            .collect();
        let ink = util::clamp(union_area(&rects) / area, 0.0, 1.0);
        boards.push(json!({ "b": ab.id, "nodes": order.len(), "ink": util::round(ink, 3) }));
    }

    json!({
        "ok": errors == 0,
        "errors": errors,
        "warns": warns,
        "infos": infos,
        "issues": arr,
        "boards": boards,
    })
}

// ------------------------------------------------------------------ flatten diff

/// Serialize each node to an ordered JSON object for diffing.
fn node_map(doc: &Document) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    for n in &doc.nodes {
        m.insert(n.id.clone(), serde_json::to_value(n).unwrap_or(Value::Null));
    }
    m
}

/// Walk two JSON values and collect dotted key paths whose leaves differ. Objects
/// recurse; arrays and scalars are compared as whole leaves.
fn flatten_diff(a: &Value, b: &Value) -> Vec<Value> {
    let out = RefCell::new(Vec::new());
    walk_diff("", a, b, &out);
    out.into_inner()
}

fn walk_diff(path: &str, a: &Value, b: &Value, out: &RefCell<Vec<Value>>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let np = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                walk_diff(&np, x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null), out);
            }
        }
        _ => {
            if a != b {
                out.borrow_mut().push(json!({ "k": path, "from": a, "to": b }));
            }
        }
    }
}

fn board_diff(a: &Document, b: &Document) -> Value {
    let am: HashMap<&str, &Artboard> = a.artboards.iter().map(|x| (x.id.as_str(), x)).collect();
    let bm: HashMap<&str, &Artboard> = b.artboards.iter().map(|x| (x.id.as_str(), x)).collect();
    let mut added: Vec<&str> = bm.keys().copied().filter(|k| !am.contains_key(k)).collect();
    let mut removed: Vec<&str> = am.keys().copied().filter(|k| !bm.contains_key(k)).collect();
    let mut changed = Vec::new();
    let mut shared: Vec<&str> = am.keys().copied().filter(|k| bm.contains_key(k)).collect();
    added.sort();
    removed.sort();
    shared.sort();
    for id in shared {
        let av = serde_json::to_value(am[id]).unwrap_or(Value::Null);
        let bv = serde_json::to_value(bm[id]).unwrap_or(Value::Null);
        let d = flatten_diff(&av, &bv);
        if !d.is_empty() {
            changed.push(json!({ "id": id, "keys": d }));
        }
    }
    json!({ "added": added, "removed": removed, "changed": changed })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::new(x, y, w, h)
    }

    #[test]
    fn union_area_dedups_overlap() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 0.0, 10.0, 10.0);
        // union is 15x10 = 150, not 200 (naive sum).
        assert!((union_area(&[a, b]) - 150.0).abs() < 0.01);
    }

    #[test]
    fn sev_ranking_and_filter() {
        assert_eq!(Issue::rank("info"), 0);
        assert_eq!(Issue::rank("warn"), 1);
        assert_eq!(Issue::rank("error"), 2);
    }

    #[test]
    fn flatten_diff_reports_dotted_paths() {
        let a = json!({ "box_": { "w": 100.0 }, "name": "x" });
        let b = json!({ "box_": { "w": 120.0 }, "name": "x" });
        let d = flatten_diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0]["k"], "box_.w");
        assert_eq!(d[0]["from"], json!(100.0));
        assert_eq!(d[0]["to"], json!(120.0));
    }
}
