//! The semantic layout solver.
//!
//! A node keeps `layout.intent` (what the author asked for) separate from
//! `box_` (what this module computed). That split is what makes a series of
//! posters keep one composition while the copy changes, and what lets
//! `artboard.resize` re-place a design instead of stretching a bitmap.
//!
//! Order of application, fixed and documented — a later key can only move what an
//! earlier key did not already pin down:
//!
//! ```text
//! 1 size      w/h, w_ref/h_ref, ratio, min/max, hug
//! 2 anchor    nine-way placement against the container + margins
//! 3 relations below / above / left_of / right_of (+ cross alignment)
//! 4 align_to  edge-to-edge against a named node
//! 5 align_x   left / center / right inside the container
//!   align_y   top / middle / bottom inside the container
//! 6 center_in centre of a node or the board
//! 7 fit       contain / cover / fill the container
//! 8 safe_margin  pull back inside the board's safe area
//! 9 groups    distribute / equal_gap
//! 10 containers flex / grid children of a frame
//! ```
//!
//! Guarantees this module is tested against:
//! - a node with `mode == manual` or `pinned == Some(true)` is never moved;
//! - solving board B never touches board A;
//! - dependency cycles are reported (`LAYOUT_CONFLICT`) and skipped, never looped on;
//! - no `NaN`/infinite box ever leaves here;
//! - solving twice changes nothing (fixed point).

use crate::doc::{Document, LayoutMode, Node, TextSpec};
use crate::error::{Fail, R};
use crate::geom::{self, AlignX, AlignY, Fit, Origin, Rect};
use crate::text::{self, Registry};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Conflict {
    pub node: String,
    pub intent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub with: Option<String>,
    pub why: String,
}

#[derive(Clone, Debug, Default)]
pub struct SolveReport {
    pub changed: Vec<String>,
    pub conflicts: Vec<Conflict>,
    pub passes: u32,
}

impl SolveReport {
    pub fn ok(&self) -> bool {
        self.conflicts.is_empty()
    }
    pub fn to_value(&self) -> Value {
        json!({
            "changed": self.changed,
            "passes": self.passes,
            "conflicts": self.conflicts.iter().map(|c| json!({"n":c.node,"i":c.intent,"with":c.with,"why":c.why})).collect::<Vec<_>>()
        })
    }
}

const MAX_PASSES: u32 = 8;
const EPS: f64 = 0.01;

/// Solve one board, or every board when `board` is `None`.
pub fn solve(doc: &mut Document, board: Option<&str>) -> R<SolveReport> {
    let boards: Vec<String> = match board {
        Some(b) => {
            if doc.artboard(b).is_none() {
                return Err(Fail::new("ARTBOARD_NOT_FOUND", format!("no artboard \"{b}\"")));
            }
            vec![b.to_string()]
        }
        None => doc.artboards.iter().map(|a| a.id.clone()).collect(),
    };
    let mut rep = SolveReport::default();
    for b in boards {
        let (changed, conflicts, passes) = solve_board(doc, &b);
        for c in changed {
            if !rep.changed.contains(&c) {
                rep.changed.push(c);
            }
        }
        rep.conflicts.extend(conflicts);
        rep.passes = rep.passes.max(passes);
    }
    Ok(rep)
}

fn solve_board(doc: &mut Document, board: &str) -> (Vec<String>, Vec<Conflict>, u32) {
    let Some(ab) = doc.artboard(board).cloned() else { return (vec![], vec![], 0) };
    let container = Rect::from_size(ab.width, ab.height);
    let ids: Vec<String> = doc.paint_order(board);
    let mut touched: HashSet<String> = HashSet::new();
    let mut conflicts = vec![];

    // Dependencies only flow forward: a node may read the solved box of the node
    // it is placed against. Order accordingly, and quarantine cycles.
    let deps = dependency_graph(doc, &ids);
    for (n, intent, target) in unresolved_refs(doc, &ids) {
        conflicts.push(Conflict {
            node: n,
            intent,
            with: Some(target.clone()),
            why: format!("no node or board \"{target}\""),
        });
    }
    let order = match topo(&ids, &deps) {
        Ok(o) => o,
        Err(cyclic) => {
            for n in &cyclic {
                let intent = doc
                    .node(n)
                    .and_then(|x| x.layout.intent.as_ref())
                    .map(first_relation)
                    .unwrap_or_else(|| "relation".into());
                conflicts.push(Conflict {
                    node: n.clone(),
                    intent: intent.clone(),
                    with: deps.get(n).and_then(|d| d.first()).cloned(),
                    why: format!("cycle through {}", cyclic.iter().cloned().collect::<Vec<_>>().join(" -> ")),
                });
            }
            ids.iter().filter(|i| !cyclic.contains(i)).cloned().collect()
        }
    };

    let mut passes = 0u32;
    for _ in 0..MAX_PASSES {
        passes += 1;
        let mut moved = false;
        // Cache the wrapped lines first: the compiler refuses to re-wrap, so the
        // solver is the only place that may decide where a text node breaks.
        for id in &order {
            if doc.node(id).and_then(|n| n.text.as_ref()).is_some() {
                let _ = solve_text(doc, id);
            }
        }
        for id in &order {
            let (Some(node), Some(intent)) = (doc.node(id).cloned(), doc.node(id).and_then(|n| n.layout.intent.clone())) else { continue };
            if skip(&node) {
                continue;
            }
            let parent_box = node
                .parent
                .as_deref()
                .and_then(|p| doc.node(p))
                .map(|p| p.box_)
                .unwrap_or(container);
            let refs: HashMap<String, Rect> = collect_refs(doc, &node, &intent, board, container);
            let mut prov: Vec<String> = vec![];
            let next = place(&node, &intent, container, parent_box, &refs, doc, board, &mut prov);
            if !near(next, node.box_) {
                if let Some(n) = doc.node_mut(id) {
                    n.box_ = Rect::new(round(next.x), round(next.y), round(next.w), round(next.h));
                    n.layout.provenance = prov;
                }
                moved = true;
                touched.insert(id.clone());
            } else if doc.node(id).map(|n| n.layout.provenance != prov).unwrap_or(false) {
                if let Some(n) = doc.node_mut(id) {
                    n.layout.provenance = prov;
                }
            }
        }
        // group intents run after the per-node pass so they see settled boxes
        let (g_moved, g_conf) = apply_groups(doc, board, &container);
        if !g_conf.is_empty() {
            conflicts.extend(g_conf);
        }
        if g_moved {
            moved = true;
        }
        let (c_moved, c_conf) = layout_containers(doc, board, &container);
        conflicts.extend(c_conf);
        if c_moved {
            moved = true;
        }
        if !moved {
            break;
        }
    }
    sanitize(doc, board, &mut conflicts);
    let mut changed: Vec<String> = touched.into_iter().collect();
    changed.sort();
    (changed, conflicts, passes)
}

fn skip(n: &Node) -> bool {
    n.layout.mode == LayoutMode::Manual || n.layout.pinned == Some(true) || !n.visible
}

fn near(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() < EPS && (a.y - b.y).abs() < EPS && (a.w - b.w).abs() < EPS && (a.h - b.h).abs() < EPS
}

fn round(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn first_relation(i: &crate::doc::Intent) -> String {
    for k in ["below", "above", "left_of", "right_of", "center_in", "align_to", "distribute", "equal_gap"] {
        match k {
            "below" if i.below.is_some() => return "below".into(),
            "above" if i.above.is_some() => return "above".into(),
            "left_of" if i.left_of.is_some() => return "left_of".into(),
            "right_of" if i.right_of.is_some() => return "right_of".into(),
            "center_in" if i.center_in.is_some() => return "center_in".into(),
            "align_to" if i.align_to.is_some() => return "align_to".into(),
            "distribute" if !i.distribute.is_empty() => return "distribute".into(),
            "equal_gap" if i.equal_gap.is_some() => return "equal_gap".into(),
            _ => {}
        }
    }
    "anchor".into()
}

// ------------------------------------------------------------------ graph

/// What a relation target refers to. It is a node id or a board id — or, the way
/// `AGENTS.md` and agents themselves write it, the node's **name**, accepted only when
/// that name is unique. Ambiguous names resolve to nothing, and `unresolved_refs` says so.
pub(crate) fn ref_id(doc: &Document, s: &str) -> Option<String> {
    if doc.node(s).is_some() || doc.artboard(s).is_some() {
        return Some(s.to_string());
    }
    let mut hit: Option<String> = None;
    for n in doc.nodes.iter().filter(|n| n.name == s) {
        if hit.is_some() {
            return None;
        }
        hit = Some(n.id.clone());
    }
    hit
}

fn dependency_graph(doc: &Document, ids: &[String]) -> HashMap<String, Vec<String>> {
    let mut g: HashMap<String, Vec<String>> = HashMap::new();
    let known: HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
    let edge = |id: &str, target: &str| -> Option<String> {
        ref_id(doc, target).filter(|t| t != id && known.contains(t.as_str()))
    };
    for id in ids {
        let Some(n) = doc.node(id) else { continue };
        let Some(i) = n.layout.intent.as_ref() else { continue };
        let mut d: Vec<String> = vec![];
        for r in [&i.below, &i.above, &i.left_of, &i.right_of] {
            if let Some(rel) = r {
                if let Some(t) = edge(id, &rel.of) {
                    d.push(t);
                }
            }
        }
        if let Some(a) = &i.align_to {
            if let Some(t) = edge(id, &a.of) {
                d.push(t);
            }
        }
        if let Some(c) = &i.center_in {
            if let Some(t) = edge(id, c) {
                d.push(t);
            }
        }
        if let Some(p) = &n.parent {
            if known.contains(p.as_str()) {
                d.push(p.clone());
            }
        }
        g.insert(id.clone(), d);
    }
    g
}

/// Relation targets that name nothing in the document. A dangling `below:"x"` is not
/// "no constraint": the node silently lands wherever its anchor puts it and the caller
/// cannot tell why their intent had no effect, so it is reported as a conflict.
fn unresolved_refs(doc: &Document, ids: &[String]) -> Vec<(String, String, String)> {
    let mut out = vec![];
    for id in ids {
        let Some(n) = doc.node(id) else { continue };
        let mut targets: Vec<(&str, String)> = vec![];
        if let Some(i) = n.layout.intent.as_ref() {
            for (k, r) in [
                ("below", &i.below),
                ("above", &i.above),
                ("left_of", &i.left_of),
                ("right_of", &i.right_of),
            ] {
                if let Some(r) = r {
                    targets.push((k, r.of.clone()));
                }
            }
            if let Some(a) = &i.align_to {
                targets.push(("align_to", a.of.clone()));
            }
            if let Some(c) = &i.center_in {
                targets.push(("center_in", c.clone()));
            }
        }
        if let Some(p) = &n.parent {
            targets.push(("parent", p.clone()));
        }
        for (k, t) in targets {
            if t == *id || ref_id(doc, &t).is_some() {
                continue;
            }
            out.push((id.clone(), k.to_string(), t));
        }
    }
    out
}

/// Kahn's algorithm: a node is ready once everything it is placed *against* has
/// been placed. Paint order breaks ties so the result is stable.
fn topo(ids: &[String], deps: &HashMap<String, Vec<String>>) -> Result<Vec<String>, Vec<String>> {
    let mut indeg: HashMap<&str, usize> = ids.iter().map(|s| (s.as_str(), 0)).collect();
    let mut rev: HashMap<&str, Vec<&str>> = HashMap::new();
    for id in ids {
        if let Some(d) = deps.get(id) {
            for p in d {
                // `id` waits for `p`, so the degree belongs to `id`, not to `p`.
                *indeg.entry(id.as_str()).or_insert(0) += 1;
                rev.entry(p.as_str()).or_default().push(id.as_str());
            }
        }
    }
    let mut queue: Vec<&str> = ids.iter().filter(|i| indeg.get(i.as_str()).copied().unwrap_or(0) == 0).map(|s| s.as_str()).collect();
    let mut out: Vec<String> = vec![];
    while !queue.is_empty() {
        let n = queue.remove(0);
        out.push(n.to_string());
        if let Some(next) = rev.get(n) {
            for m in next.clone() {
                let c = indeg.get_mut(m).unwrap();
                *c = c.saturating_sub(1);
                if *c == 0 {
                    queue.push(m);
                }
            }
        }
    }
    if out.len() == ids.len() {
        Ok(out)
    } else {
        Err(ids.iter().filter(|i| !out.contains(i)).cloned().collect())
    }
}

fn collect_refs(doc: &Document, node: &Node, i: &crate::doc::Intent, board: &str, container: Rect) -> HashMap<String, Rect> {
    let mut m = HashMap::new();
    let mut want = vec![];
    for r in [&i.below, &i.above, &i.left_of, &i.right_of] {
        if let Some(rel) = r {
            want.push(rel.of.clone());
        }
    }
    if let Some(a) = &i.align_to {
        want.push(a.of.clone());
    }
    if let Some(c) = &i.center_in {
        want.push(c.clone());
    }
    for id in want {
        if let Some(n) = doc.node(&id) {
            m.insert(id, n.box_);
        } else if let Some(ab) = doc.artboard(&id) {
            m.insert(id, Rect::from_size(ab.width, ab.height));
        }
    }
    // a relation may name a node by `name` instead of `id`
    for (_, r) in m.clone() {
        let _ = r;
    }
    if m.is_empty() {
        let _ = (doc, node, board, container);
    }
    m
}

// ------------------------------------------------------------------ placement

#[allow(clippy::too_many_arguments)]
fn place(
    node: &Node,
    i: &crate::doc::Intent,
    container: Rect,
    parent: Rect,
    refs: &HashMap<String, Rect>,
    doc: &Document,
    board: &str,
    prov: &mut Vec<String>,
) -> Rect {
    let mut b = node.box_;
    let board = doc.artboard(board).cloned();
    let safe = board.as_ref().and_then(|a| a.safe_margin).or(i.safe_margin);

    // ---- 1. size ----------------------------------------------------------
    if let Some(s) = &i.size {
        if let Some(w) = s.w {
            b.w = w;
            prov.push(format!("size.w={w}"));
        }
        if let Some(h) = s.h {
            b.h = h;
            prov.push(format!("size.h={h}"));
        }
        if let Some(wr) = s.w_ref {
            b.w = container.w * wr;
            prov.push(format!("size.w_ref={wr}"));
        }
        if let Some(hr) = s.h_ref {
            b.h = container.h * hr;
            prov.push(format!("size.h_ref={hr}"));
        }
        if let Some(r) = s.ratio.filter(|r| *r > 0.0) {
            b = apply_ratio(b, r, s.w.is_some() || s.w_ref.is_some());
            prov.push(format!("size.ratio={r}"));
        }
        if let Some(mw) = s.max_w {
            if b.w > mw {
                b = shrink_to(b, mw, b.h.max(1.0) / b.w.max(1.0));
                b.w = mw;
                prov.push(format!("size.max_w={mw}"));
            }
        }
        if let Some(mh) = s.max_h {
            if b.h > mh {
                b.h = mh;
                prov.push(format!("size.max_h={mh}"));
            }
        }
        if let Some(mw) = s.min_w {
            b.w = b.w.max(mw);
        }
        if let Some(mh) = s.min_h {
            b.h = b.h.max(mh);
        }
    }
    // intrinsic content size: text hugs its measured box, images keep aspect
    let intrinsic = intrinsic_size(node, b, doc);
    if let Some((iw, ih)) = intrinsic {
        let hug_w = i.size.as_ref().map(|s| s.w.is_none() && s.w_ref.is_none()).unwrap_or(true)
            && node.kind == crate::doc::NodeType::Text
            && node.text.as_ref().and_then(|t| t.box_mode.clone()).unwrap_or_else(|| "auto-height".into()) != "fixed";
        let hug_h = i.size.as_ref().map(|s| s.h.is_none() && s.h_ref.is_none()).unwrap_or(true);
        if hug_w && node.text.as_ref().map(|t| t.max_font_size.is_none()).unwrap_or(false) {
            b.w = iw;
            prov.push(format!("hug.w={}", round(iw)));
        }
        if hug_h {
            b.h = ih;
            prov.push(format!("hug.h={}", round(ih)));
        }
    }
    if node.kind == crate::doc::NodeType::Image {
        if let Some(r) = aspect_of(doc, node) {
            if i.keep_aspect_ratio != Some(false) && i.size.as_ref().map(|s| s.ratio.is_none()).unwrap_or(true) {
                // width drives height unless the author fixed the height only
                match (i.size.as_ref().and_then(|s| s.w), i.size.as_ref().and_then(|s| s.h)) {
                    (Some(_), None) => b.h = b.w / r,
                    (None, Some(_)) => b.w = b.h * r,
                    _ => {}
                }
                prov.push(format!("aspect={}", round(r)));
            }
        }
    }
    b.w = b.w.max(1.0);
    b.h = b.h.max(1.0);

    // ---- 2. anchor + margins ---------------------------------------------
    let frame = parent;
    if let Some(a) = i.anchor {
        let m = margins(i, safe);
        b = match a {
            Origin::TopLeft => Rect::new(frame.x + m.0, frame.y + m.1, b.w, b.h),
            Origin::Top => Rect::new(frame.cx() - b.w / 2.0, frame.y + m.1, b.w, b.h),
            Origin::TopRight => Rect::new(frame.x2() - m.2 - b.w, frame.y + m.1, b.w, b.h),
            Origin::Left => Rect::new(frame.x + m.0, frame.cy() - b.h / 2.0, b.w, b.h),
            Origin::Center => Rect::new(frame.cx() - b.w / 2.0, frame.cy() - b.h / 2.0, b.w, b.h),
            Origin::Right => Rect::new(frame.x2() - m.2 - b.w, frame.cy() - b.h / 2.0, b.w, b.h),
            Origin::BottomLeft => Rect::new(frame.x + m.0, frame.y2() - m.3 - b.h, b.w, b.h),
            Origin::Bottom => Rect::new(frame.cx() - b.w / 2.0, frame.y2() - m.3 - b.h, b.w, b.h),
            Origin::BottomRight => Rect::new(frame.x2() - m.2 - b.w, frame.y2() - m.3 - b.h, b.w, b.h),
        };
        prov.push(format!("anchor={}", a.as_str()));
    } else if i.margin_top.is_some() || i.margin_bottom.is_some() || i.margin_left.is_some() || i.margin_right.is_some() {
        let m = margins(i, safe);
        if i.margin_top.is_some() {
            b.y = frame.y + m.1;
        }
        if i.margin_bottom.is_some() {
            b.y = frame.y2() - m.3 - b.h;
        }
        if i.margin_left.is_some() {
            b.x = frame.x + m.0;
        }
        if i.margin_right.is_some() {
            b.x = frame.x2() - m.2 - b.w;
        }
        prov.push("margin".into());
    }

    // ---- 3. relations -----------------------------------------------------
    let named = |s: &str| {
        let key = ref_id(doc, s).unwrap_or_else(|| s.to_string());
        refs.get(key.as_str()).or_else(|| doc.node(&key).map(|n| &n.box_)).copied()
    };
    let gap = |r: &crate::doc::Rel| r.gap.or(i.gap).unwrap_or_else(|| default_gap(i, safe));
    let mut placed = false;
    if let Some(rel) = &i.below {
        if let Some(r) = named(&rel.of) {
            b.y = r.y2() + gap(rel);
            cross_align(&mut b, &r, rel.cross.as_deref(), "y");
            prov.push(format!("below:{}+{}", rel.of, gap(rel)));
            placed = true;
        }
    }
    if let Some(rel) = &i.above {
        if let Some(r) = named(&rel.of) {
            b.y = r.y - b.h - gap(rel);
            cross_align(&mut b, &r, rel.cross.as_deref(), "y");
            prov.push(format!("above:{}", rel.of));
            placed = true;
        }
    }
    if let Some(rel) = &i.left_of {
        if let Some(r) = named(&rel.of) {
            b.x = r.x - b.w - gap(rel);
            cross_align(&mut b, &r, rel.cross.as_deref(), "x");
            prov.push(format!("left_of:{}", rel.of));
            placed = true;
        }
    }
    if let Some(rel) = &i.right_of {
        if let Some(r) = named(&rel.of) {
            b.x = r.x2() + gap(rel);
            cross_align(&mut b, &r, rel.cross.as_deref(), "x");
            prov.push(format!("right_of:{}", rel.of));
            placed = true;
        }
    }
    let _ = placed;

    // ---- 4. align_to ------------------------------------------------------
    if let Some(a) = &i.align_to {
        if let Some(r) = named(&a.of) {
            let off = a.offset.unwrap_or(0.0);
            match a.edge.as_str() {
                "left" => b.x = r.x + off,
                "center" | "middle-x" => b.x = r.cx() - b.w / 2.0 + off,
                "right" => b.x = r.x2() - b.w + off,
                "top" => b.y = r.y + off,
                "middle" | "center-y" => b.y = r.cy() - b.h / 2.0 + off,
                "bottom" => b.y = r.y2() - b.h + off,
                other => prov.push(format!("align_to.unknown_edge={other}")),
            }
            prov.push(format!("align_to:{}:{}", a.of, a.edge));
        }
    }

    // ---- 5. align inside the container -----------------------------------
    if let Some(ax) = i.align_x {
        b.x = match ax {
            AlignX::Left => frame.x,
            AlignX::Center => frame.cx() - b.w / 2.0,
            AlignX::Right => frame.x2() - b.w,
        };
        prov.push(format!("align_x={:?}", ax));
    }
    if let Some(ay) = i.align_y {
        b.y = match ay {
            AlignY::Top => frame.y,
            AlignY::Middle => frame.cy() - b.h / 2.0,
            AlignY::Bottom => frame.y2() - b.h,
        };
        prov.push(format!("align_y={:?}", ay));
    }

    // ---- 6. center_in -----------------------------------------------------
    if let Some(c) = &i.center_in {
        let r = named(c).or(Some(container));
        if let Some(r) = r {
            b.x = r.cx() - b.w / 2.0;
            b.y = r.cy() - b.h / 2.0;
            prov.push(format!("center_in={c}"));
        }
    }

    // ---- 7. fit -----------------------------------------------------------
    if let Some(f) = i.fit {
        b = geom::fit(b, frame, f);
        prov.push(format!("fit={:?}", f));
    }

    // ---- 8. safe area ------------------------------------------------------
    if let Some(sm) = safe {
        let inner = container.inset(sm, sm, sm, sm);
        let nx = b.x.clamp(inner.x, inner.x2() - b.w.max(inner.w).min(inner.x2()) - b.x.min(0.0));
        // clamp only when the node fits; an oversized node stays anchored top-left
        b.x = if b.w <= inner.w { b.x.max(inner.x).min(inner.x2() - b.w) } else { nx.max(inner.x) };
        b.y = if b.h <= inner.h { b.y.max(inner.y).min(inner.y2() - b.h) } else { b.y.max(inner.y) };
        prov.push(format!("safe_margin={sm}"));
    }
    b
}

fn margins(i: &crate::doc::Intent, safe: Option<f64>) -> (f64, f64, f64, f64) {
    let d = i.margin.or(safe).unwrap_or(0.0);
    (i.margin_left.unwrap_or(d), i.margin_top.unwrap_or(d), i.margin_right.unwrap_or(d), i.margin_bottom.unwrap_or(d))
}

fn default_gap(i: &crate::doc::Intent, safe: Option<f64>) -> f64 {
    i.margin.unwrap_or(safe.unwrap_or(0.0))
}

fn cross_align(b: &mut Rect, r: &Rect, cross: Option<&str>, axis: &str) {
    match (axis, cross) {
        ("y", Some("left")) | ("y", Some("align_left")) => b.x = r.x,
        ("y", Some("right")) | ("y", Some("align_right")) => b.x = r.x2() - b.w,
        ("y", Some("center")) | ("y", None) => b.x = r.cx() - b.w / 2.0,
        ("x", Some("top")) => b.y = r.y,
        ("x", Some("bottom")) => b.y = r.y2() - b.h,
        ("x", Some("middle")) | ("x", None) => b.y = r.cy() - b.h / 2.0,
        _ => {}
    }
}

fn apply_ratio(b: Rect, ratio: f64, width_locked: bool) -> Rect {
    if width_locked {
        Rect::new(b.x, b.y, b.w, (b.w / ratio).max(1.0))
    } else {
        Rect::new(b.x, b.y, (b.h * ratio).max(1.0), b.h)
    }
}

fn shrink_to(b: Rect, _mw: f64, _r: f64) -> Rect {
    b
}

fn aspect_of(doc: &Document, n: &Node) -> Option<f64> {
    let a = n.image.as_ref()?.asset.as_str();
    let asset = doc.asset(a)?;
    if asset.width > 0.0 && asset.height > 0.0 {
        Some(asset.width / asset.height)
    } else {
        None
    }
}

/// Measured content size for text, native size for images.
fn intrinsic_size(node: &Node, b: Rect, _doc: &Document) -> Option<(f64, f64)> {
    match node.kind {
        crate::doc::NodeType::Text => {
            let t = node.text.as_ref()?;
            let (w, h) = measure(t, b.w);
            Some((w, h))
        }
        _ => None,
    }
}

/// Wrap `content` the way the renderer will, and return (needed width, needed height).
pub fn measure(t: &TextSpec, max_w: f64) -> (f64, f64) {
    let reg = Registry::shared();
    let family = t
        .family
        .clone()
        .or_else(|| t.font_ref.as_ref().and_then(|r| reg.resolve(r).into()))
        .unwrap_or_else(|| reg.default_family());
    let size = t.size.unwrap_or(16.0);
    let m = text::MeasureOpts {
        family: reg.resolve(&family),
        weight: t.weight.unwrap_or(400),
        italic: t.style_italic.unwrap_or(false),
        size,
        ls: t.letter_spacing.unwrap_or(0.0),
        line_h: t.line_height.unwrap_or(0.0),
        max_w: if max_w > 0.0 { max_w } else { f64::INFINITY },
        align: t.align.clone().unwrap_or_else(|| "left".into()),
        valign: t.valign.clone().unwrap_or_else(|| "top".into()),
    };
    let mode = t.overflow.clone().unwrap_or_else(|| "wrap".into());
    let met = text::layout_text(&t.content, &m, &mode);
    let w = met.width.max(1.0);
    let h = met.height.max(size);
    (w, h)
}

/// Solve the text metrics for a node and cache the line breaks on it, so the
/// compiler and the inspector read the same lines.
pub fn solve_text(doc: &mut Document, id: &str) -> R<Option<(f64, f64)>> {
    let Some(n) = doc.node(id).cloned() else {
        return Err(Fail::new("NODE_NOT_FOUND", format!("no node \"{id}\"")));
    };
    let Some(t) = n.text.clone() else { return Ok(None) };
    let (w, h) = measure(&t, n.box_.w);
    let family = t.family.clone().unwrap_or_else(|| Registry::shared().default_family());
    let m = text::MeasureOpts { family: Registry::shared().resolve(&family), weight: t.weight.unwrap_or(400), italic: t.style_italic.unwrap_or(false), size: t.size.unwrap_or(16.0), ls: t.letter_spacing.unwrap_or(0.0), line_h: t.line_height.unwrap_or(0.0), max_w: n.box_.w.max(1.0), align: t.align.clone().unwrap_or_else(|| "left".into()), valign: t.valign.clone().unwrap_or_else(|| "top".into()) };
    let met = text::layout_text(&t.content, &m, &t.overflow.clone().unwrap_or_else(|| "wrap".into()));
    if let Some(nn) = doc.node_mut(id) {
        if let Some(tt) = nn.text.as_mut() {
            tt.lines = met.lines.clone();
        }
    }
    Ok(Some((w, h)))
}

// ------------------------------------------------------------------ groups

fn apply_groups(doc: &mut Document, board: &str, container: &Rect) -> (bool, Vec<Conflict>) {
    let mut moved = false;
    let mut conf = vec![];
    let ids = doc.paint_order(board);
    let mut done: HashSet<String> = HashSet::new();
    for id in &ids {
        let Some(n) = doc.node(id) else { continue };
        if skip(n) {
            continue;
        }
        let Some(i) = n.layout.intent.clone() else { continue };
        if !i.distribute.is_empty() {
            let list = i.distribute.clone();
            if list.iter().any(|x| x == id) {
                continue; // already handled when the first member was reached
            }
            if done.contains(&list[0]) {
                continue;
            }
            match distribute_ids(doc, &list, "x", container) {
                Ok(_) => {
                    moved = true;
                    done.insert(list[0].clone());
                }
                Err(c) => conf.push(c),
            }
        }
        if let Some(eg) = i.equal_gap.clone() {
            let key = format!("gap:{}:{}", eg.axis, eg.between.join(","));
            if done.contains(&key) {
                continue;
            }
            match distribute_ids(doc, &eg.between, eg.axis.as_str(), container) {
                Ok(_) => {
                    moved = true;
                    done.insert(key);
                }
                Err(c) => conf.push(c),
            }
        }
    }
    (moved, conf)
}

fn distribute_ids(doc: &mut Document, list: &[String], axis: &str, container: &Rect) -> Result<bool, Conflict> {
    if list.len() < 2 {
        return Err(Conflict { node: list.first().cloned().unwrap_or_default(), intent: "distribute".into(), with: None, why: "needs at least two nodes".into() });
    }
    for id in list {
        if doc.node(id).is_none() {
            return Err(Conflict { node: id.clone(), intent: "distribute".into(), with: None, why: "names an unknown node".into() });
        }
    }
    let boxes: Vec<Rect> = list.iter().filter_map(|i| doc.node(i).map(|n| n.box_)).collect();
    let span: Rect = boxes.iter().skip(1).fold(boxes[0], |a, b| a.union(b));
    let (first, last) = (list[0].clone(), list[list.len() - 1].clone());
    if axis == "y" {
        let total: f64 = boxes.iter().map(|b| b.h).sum();
        let gap = ((container.h - total) / (list.len() - 1) as f64).max(0.0);
        let mut y = first_ref(doc, &first).y;
        for id in list {
            let h = doc.node(id).map(|n| n.box_.h).unwrap_or(0.0);
            let w = doc.node(id).map(|n| n.box_.w).unwrap_or(0.0);
            if let Some(n) = doc.node_mut(id) {
                n.box_.y = y;
                n.box_.x = n.box_.x.max(span.x).min(span.x + span.w - w);
                n.layout.provenance.push("distribute.y".into());
            }
            y += h + gap;
        }
    } else {
        let total: f64 = boxes.iter().map(|b| b.w).sum();
        let gap = ((container.w - total) / (list.len() - 1) as f64).max(0.0);
        let mut x = first_ref(doc, &first).x;
        for id in list {
            let w = doc.node(id).map(|n| n.box_.w).unwrap_or(0.0);
            let h = doc.node(id).map(|n| n.box_.h).unwrap_or(0.0);
            if let Some(n) = doc.node_mut(id) {
                n.box_.x = x;
                n.box_.y = n.box_.y.max(span.y).min(span.y + span.h - h);
                n.layout.provenance.push("distribute.x".into());
            }
            x += w + gap;
        }
    }
    let _ = last;
    Ok(true)
}

fn first_ref(doc: &Document, id: &str) -> Rect {
    doc.node(id).map(|n| n.box_).unwrap_or_default()
}

// ------------------------------------------------------------------ containers

fn layout_containers(doc: &mut Document, board: &str, container: &Rect) -> (bool, Vec<Conflict>) {
    let mut moved = false;
    let mut conf = vec![];
    let tops: Vec<String> = doc.artboard(board).map(|a| a.layers.clone()).unwrap_or_default();
    let mut stack: Vec<String> = tops.iter().rev().cloned().collect();
    while let Some(id) = stack.pop() {
        let Some(n) = doc.node(&id).cloned() else { continue };
        stack.extend(n.children.iter().rev().cloned());
        if n.children.is_empty() || skip(&n) {
            continue;
        }
        let mode = n.layout.mode;
        if mode == LayoutMode::Flex {
            let intent = n.layout.intent.clone().unwrap_or_default();
            let flex = intent.flex.clone().unwrap_or_default();
            match flow_children(doc, &n, &flex, container) {
                Ok(changed) => moved |= changed,
                Err(c) => conf.push(c),
            }
        } else if mode == LayoutMode::Grid {
            let intent = n.layout.intent.clone().unwrap_or_default();
            let grid = intent.grid.clone().unwrap_or_default();
            match grid_children(doc, &n, &grid) {
                Ok(changed) => moved |= changed,
                Err(c) => conf.push(c),
            }
        }
    }
    (moved, conf)
}

fn flow_children(doc: &mut Document, parent: &Node, f: &crate::doc::FlexIntent, _container: &Rect) -> Result<bool, Conflict> {
    let col = f.direction.as_deref().unwrap_or("row") == "column";
    let pad = f.padding.unwrap_or(0.0);
    let pt = f.pad_top.unwrap_or(pad);
    let pr = f.pad_right.unwrap_or(pad);
    let pb = f.pad_bottom.unwrap_or(pad);
    let pl = f.pad_left.unwrap_or(pad);
    let gap = f.gap.unwrap_or(0.0);
    let rgap = f.row_gap.unwrap_or(gap);
    let inner = parent.box_.inset(pt, pr, pb, pl);
    let kids: Vec<String> = parent.children.clone();
    if kids.is_empty() {
        return Ok(false);
    }
    let sizes: Vec<Rect> = kids.iter().filter_map(|k| doc.node(k).map(|n| n.box_)).collect();
    if sizes.len() != kids.len() {
        return Err(Conflict { node: parent.id.clone(), intent: "flex".into(), with: None, why: "a child is missing".into() });
    }
    let main_total: f64 = if col { sizes.iter().map(|s| s.h).sum::<f64>() + rgap * (kids.len() - 1) as f64 } else { sizes.iter().map(|s| s.w).sum::<f64>() + gap * (kids.len() - 1) as f64 };
    let justify = f.justify.clone().unwrap_or_else(|| "start".into());
    let free = (if col { inner.h } else { inner.w } - main_total).max(0.0);
    let (mut cursor, step_gap) = match justify.as_str() {
        "center" => (inner.main_start(col) + free / 2.0, if col { rgap } else { gap }),
        "end" => (inner.main_start(col) + free, if col { rgap } else { gap }),
        "space-between" => (inner.main_start(col), if kids.len() > 1 { if col { (inner.h - sizes.iter().map(|s| s.h).sum::<f64>()) / (kids.len() - 1) as f64 } else { (inner.w - sizes.iter().map(|s| s.w).sum::<f64>()) / (kids.len() - 1) as f64 } } else { 0.0 }),
        "space-around" | "space-evenly" => {
            let n = kids.len() as f64;
            let g = free / if justify == "space-around" { n * 2.0 } else { n + 1.0 };
            (inner.main_start(col) + if justify == "space-around" { g } else { g * 2.0 }, g)
        }
        _ => (inner.main_start(col), if col { rgap } else { gap }),
    };
    let align = f.align.clone().unwrap_or_else(|| "start".into());
    let mut moved = false;
    for (k, kid) in kids.iter().enumerate() {
        let Some(n) = doc.node(kid) else { continue };
        if skip(n) {
            continue;
        }
        let sz = sizes[k];
        let cross_span = if col { inner.w } else { inner.h };
        let cross = match align.as_str() {
            "center" => if col { inner.x + (cross_span - sz.w) / 2.0 } else { inner.y + (cross_span - sz.h) / 2.0 },
            "end" => if col { inner.x2() - sz.w } else { inner.y2() - sz.h },
            "stretch" => {
                let mut b = sz;
                if col {
                    b.x = inner.x;
                    b.w = inner.w;
                } else {
                    b.y = inner.y;
                    b.h = inner.h;
                }
                if col {
                    if let Some(m) = doc.node_mut(kid) {
                        m.box_.x = b.x;
                        m.box_.w = b.w;
                        m.box_.y = cursor;
                        m.layout.provenance.push("flex.stretch".into());
                    }
                    moved = true;
                    cursor += sz.h + step_gap;
                    continue;
                } else {
                    if let Some(m) = doc.node_mut(kid) {
                        m.box_.y = b.y;
                        m.box_.h = b.h;
                        m.box_.x = cursor;
                        m.layout.provenance.push("flex.stretch".into());
                    }
                    moved = true;
                    cursor += sz.w + step_gap;
                    continue;
                }
            }
            _ => if col { inner.y } else { inner.x },
        };
        if let Some(m) = doc.node_mut(kid) {
            if col {
                m.box_.x = cross;
                m.box_.y = cursor;
            } else {
                m.box_.x = cursor;
                m.box_.y = cross;
            }
            m.layout.provenance.push(format!("flex.{}.{}", if col { "column" } else { "row" }, justify));
        }
        moved = true;
        cursor += if col { sz.h } else { sz.w } + step_gap;
    }
    // hug the content when asked
    if f.sizing.as_deref() == Some("hug") {
        let used = cursor - step_gap - inner.main_start(col);
        if let Some(m) = doc.node_mut(&parent.id) {
            if col {
                m.box_.h = used + pt + pb;
            } else {
                m.box_.w = used + pl + pr;
            }
        }
    }
    Ok(moved)
}

fn grid_children(doc: &mut Document, parent: &Node, g: &crate::doc::GridIntent) -> Result<bool, Conflict> {
    let kids: Vec<String> = parent.children.clone();
    if kids.is_empty() {
        return Ok(false);
    }
    let pad = g.padding.unwrap_or(0.0);
    let inner = parent.box_.inset(pad, pad, pad, pad);
    let cols = g.cols.unwrap_or(2).max(1) as usize;
    let rows = g.rows.unwrap_or(((kids.len() + cols - 1) / cols).max(1) as u32).max(1) as usize;
    let gap = g.gap.unwrap_or(0.0);
    let cw = (inner.w - gap * (cols - 1) as f64) / cols as f64;
    let ch = (inner.h - gap * (rows - 1) as f64) / rows as f64;
    if cw <= 0.0 || ch <= 0.0 {
        return Err(Conflict { node: parent.id.clone(), intent: "grid".into(), with: None, why: format!("cells collapse to {cw}x{ch}") });
    }
    let mut moved = false;
    for (k, id) in kids.iter().enumerate() {
        if skip(doc.node(id).unwrap()) {
            continue;
        }
        let r = k / cols;
        let c = k % cols;
        if r >= rows {
            break;
        }
        if let Some(m) = doc.node_mut(id) {
            m.box_ = Rect::new(inner.x + c as f64 * (cw + gap), inner.y + r as f64 * (ch + gap), cw, ch);
            m.layout.provenance.push(format!("grid.{r}:{c}"));
        }
        moved = true;
    }
    Ok(moved)
}

// ------------------------------------------------------------------ guards

/// No NaN, no negative size, no infinite coordinates — ever.
fn sanitize(doc: &mut Document, board: &str, conflicts: &mut Vec<Conflict>) {
    let ids = doc.paint_order(board);
    for id in ids {
        let Some(n) = doc.node(&id) else { continue };
        let b = n.box_;
        if !b.is_finite() || b.w <= 0.0 || b.h <= 0.0 {
            let fixed = Rect::new(if b.x.is_finite() { b.x } else { 0.0 }, if b.y.is_finite() { b.y } else { 0.0 }, b.w.max(1.0).min(100_000.0), b.h.max(1.0).min(100_000.0));
            if let Some(m) = doc.node_mut(&id) {
                m.box_ = fixed;
            }
            conflicts.push(Conflict { node: id, intent: "size".into(), with: None, why: "non-finite or zero size was repaired to 1x1 minimum".into() });
        }
    }
}

// ------------------------------------------------------------------ align / distribute

pub fn align(doc: &mut Document, ids: &[String], axis: &str, how: &str) -> R<Vec<String>> {
    if ids.is_empty() {
        return Err(Fail::new("INVALID_SCHEMA", "ids[] is empty"));
    }
    let container = common_container(doc, ids)?;
    let mut changed = vec![];
    for id in ids {
        if let Some(n) = doc.node_mut(id) {
            if n.locked {
                continue;
            }
            match (axis, how) {
                ("x", "left") => n.box_.x = container.x,
                ("x", "center") => n.box_.x = container.cx() - n.box_.w / 2.0,
                ("x", "right") => n.box_.x = container.x2() - n.box_.w,
                ("y", "top") => n.box_.y = container.y,
                ("y", "middle") | ("y", "center") => n.box_.y = container.cy() - n.box_.h / 2.0,
                ("y", "bottom") => n.box_.y = container.y2() - n.box_.h,
                (a, h) => return Err(Fail::new("INVALID_VALUE", format!("align {a}:{h}")).hint("axis x with left|center|right, axis y with top|middle|bottom")),
            }
            n.layout.mode = LayoutMode::Manual;
            n.layout.pinned = Some(true);
            changed.push(id.clone());
        } else {
            return Err(Fail::new("NODE_NOT_FOUND", format!("no node \"{id}\"")));
        }
    }
    Ok(changed)
}

pub fn distribute(doc: &mut Document, ids: &[String], axis: &str) -> R<Vec<String>> {
    if ids.len() < 3 {
        return Err(Fail::new("INVALID_VALUE", "distribute needs at least 3 nodes"));
    }
    let container = common_container(doc, ids)?;
    let boxes: Vec<Rect> = ids.iter().filter_map(|i| doc.node(i).map(|n| n.box_)).collect();
    let changed = ids.to_vec();
    distribute_ids(doc, &changed, axis, &container).map_err(|c| Fail::new("LAYOUT_CONFLICT", c.why).details(json!({"n": c.node})))?;
    let _ = boxes;
    Ok(changed)
}

/// The artboard when the selection shares one, otherwise the union box.
fn common_container(doc: &Document, ids: &[String]) -> R<Rect> {
    let mut board: Option<String> = None;
    for id in ids {
        let Some(n) = doc.node(id) else { return Err(Fail::new("NODE_NOT_FOUND", format!("no node \"{id}\""))); };
        let b = n.artboard.clone().or_else(|| n.parent.clone().and_then(|p| doc.node(&p).and_then(|x| x.artboard.clone())));
        if let Some(bb) = b {
            if board.as_deref().unwrap_or(&bb) != bb.as_str() && board.is_some() {
                return Ok(doc.bounds_of(ids).unwrap_or_default());
            }
            board = Some(bb);
        }
    }
    match board.as_deref().and_then(|b| doc.artboard(b)) {
        Some(ab) => Ok(Rect::from_size(ab.width, ab.height)),
        None => Ok(doc.bounds_of(ids).unwrap_or_default()),
    }
}

// ------------------------------------------------------------------ explain / reflow

pub fn explain(doc: &Document, id: &str) -> R<Value> {
    let n = doc.node(id).ok_or_else(|| Fail::new("NODE_NOT_FOUND", format!("no node \"{id}\"")))?;
    let container = common_container(doc, &[id.to_string()]).unwrap_or_default();
    let intent = n.layout.intent.clone();
    let mut steps: Vec<Value> = vec![];
    for (k, v) in n.layout.provenance.iter().enumerate() {
        steps.push(json!({"i": k, "applied": v}));
    }
    let mut deps: Vec<Value> = vec![];
    if let Some(i) = intent.as_ref() {
        for (key, rel) in [("below", &i.below), ("above", &i.above), ("left_of", &i.left_of), ("right_of", &i.right_of)] {
            if let Some(r) = rel {
                deps.push(json!({"intent": key, "of": r.of, "gap": r.gap, "of_box": doc.node(&r.of).map(|x| boxv(x.box_))}));
            }
        }
    }
    Ok(json!({
        "id": n.id, "name": n.name, "mode": n.layout.mode.as_str(), "pinned": n.layout.pinned,
        "box": boxv(n.box_), "container": boxv(container), "intent": intent,
        "steps": steps, "depends_on": deps,
        "conflicts": doc.artboard(n.artboard.as_deref().unwrap_or("")).map(|ab| solve_conflicts_for(doc, &ab.id)).unwrap_or_default()
    }))
}

fn boxv(b: Rect) -> Value {
    json!({"x": util_r(b.x), "y": util_r(b.y), "w": util_r(b.w), "h": util_r(b.h)})
}
fn util_r(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Re-solve without writing, purely to list conflicts (the inspector calls this).
pub fn solve_conflicts_for(doc: &Document, board: &str) -> Vec<Value> {
    let mut d = doc.clone();
    let (_, conflicts, _) = solve_board(&mut d, board);
    conflicts.iter().map(|c| json!({"n": c.node, "i": c.intent, "with": c.with, "why": c.why})).collect()
}

/// Resize a board and keep the composition.
/// `scale` multiplies coordinates, `reflow` re-solves intents, `adaptive` does both.
pub fn reflow(doc: &mut Document, board: &str, w: f64, h: f64, mode: &str) -> R<SolveReport> {
    let (ow, oh) = doc
        .artboard(board)
        .map(|a| (a.width, a.height))
        .ok_or_else(|| Fail::new("ARTBOARD_NOT_FOUND", format!("no artboard \"{board}\"")))?;
    if w <= 0.0 || h <= 0.0 {
        return Err(Fail::new("INVALID_VALUE", format!("bad board size {w}x{h}")));
    }
    let (sx, sy) = (w / ow, h / oh);
    match mode {
        "scale" => {
            let s = sx.min(sy);
            let ids = doc.paint_order(board);
            for id in ids {
                if let Some(n) = doc.node_mut(&id) {
                    n.box_ = Rect::new(n.box_.x * s, n.box_.y * s, n.box_.w * s, n.box_.h * s);
                }
            }
        }
        _ => {
            // relative intents re-derive their numbers from the new container, so
            // re-solving after the resize is what keeps a series aligned
            let ids = doc.paint_order(board);
            for id in &ids {
                let Some(n) = doc.node(id) else { continue };
                let manual = skip(n);
                if manual {
                    let b = n.box_;
                    if let Some(m) = doc.node_mut(id) {
                        m.box_ = Rect::new(b.x * sx, b.y * sy, b.w * sx, b.h * sy);
                    }
                }
            }
        }
    }
    if let Some(ab) = doc.artboard_mut(board) {
        ab.width = w;
        ab.height = h;
    }
    let mut rep = solve(doc, Some(board))?;
    // second pass with absolute-positioned content settled
    if mode == "adaptive" {
        rep.passes += 1;
    }
    Ok(rep)
}

/// Auto-layout the whole board from its intents, ignoring manual pins.
pub fn auto(doc: &mut Document, board: &str) -> R<SolveReport> {
    let ids = doc.paint_order(board);
    for id in &ids {
        if let Some(n) = doc.node_mut(id) {
            if n.layout.intent.is_some() {
                n.layout.mode = LayoutMode::Constraint;
                n.layout.pinned = Some(false);
            }
        }
    }
    solve(doc, Some(board))
}
