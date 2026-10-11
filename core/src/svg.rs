//! The one paint path: document → deterministic SVG.
//!
//! Everything a human sees in the desktop preview and every pixel an agent exports
//! comes out of [`compile`]. That is the architectural promise, so the rules here
//! are strict:
//!
//! * **Deterministic.** Same document in, byte-identical string out. No hash-set
//!   iteration, no clocks, no random ids: every generated id is derived from the
//!   artboard id plus the node id, and defs are appended in paint order.
//! * **Paint order is `Document::paint_order`** — not registration order, not
//!   z-index arithmetic. `visible:false` and `SvgOpts::omit_ids` are skipped.
//! * **Never rely on the viewer's text layout.** Each line the engine solved is
//!   written as its own `<text>` with an explicit baseline, so a browser and resvg
//!   put glyphs in the same place.
//! * **No untrusted markup passthrough.** Imported SVG is sanitised: no `<!DOCTYPE`
//!   or entity declarations (resvg parses with `allow_dtd`, so entity expansion is a
//!   live risk), no `script`/`foreignObject`/`iframe`/`style`, no `on*` handlers, no
//!   remote or `javascript:` URLs, and its local ids are namespaced so it cannot
//!   shadow the defs generated here.
//!
//! Coordinate conventions match `doc.rs`: artboard space, y down, degrees clockwise,
//! `Node::box_` is the *pre-rotation* box. Vector data inside `VectorSpec` (`d`,
//! `points`, `line_end`, `viewBox`) is authored in the node's **local** space:
//! origin at the box's top-left, spanning `box.w x box.h`. `Clip::rect` and
//! `Artboard::guides` are artboard space, like everything else.
//!
//! Stroke alignment: SVG only strokes centred. `inside` clips the shape and draws a
//! `2 * width` stroke so the outer half is trimmed; `outside` draws a `2 * width`
//! stroke through a luminance mask that is white minus the shape. A clipPath cannot
//! express a complement, hence the mask: it costs one extra paint but stays correct
//! when the fill is translucent, which the cheaper "stroke painted under the fill"
//! trick is not. Both are only meaningful for closed, filled geometry; a stroke on
//! an unfilled line or path stays centred.

use crate::doc::{
    parse_color, Artboard, Asset, Document, Node, NodeType, Paint, Radius, Shadow, Stop, Style,
    Stroke, VectorSpec,
};
use crate::error::{Fail, R};
use crate::geom::{fit, Fit, Rect};
use crate::text::{layout_text, MeasureOpts, Registry};
use crate::util;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Gap between artboards when several are compiled into one canvas.
pub const GUTTER: f64 = 40.0;

/// Budget for the "blurred static copy of the backdrop" trick that makes
/// `fx.backdrop_blur` survive export. Past these limits the node degrades to a
/// translucent fill plus noise, and the degradation is reported as a warning.
const BACKDROP_NODE_BUDGET: usize = 48;
const BACKDROP_COPY_BUDGET: usize = 4;

// ------------------------------------------------------------------ options

#[derive(Clone, Debug)]
pub struct SvgOpts {
    /// Which artboards to emit; empty = every visible one.
    pub boards: Vec<String>,
    /// Base64 `data:` URIs (export) vs `{asset_url}/<id>` (preview).
    pub inline_assets: bool,
    /// Base URL used in preview mode, e.g. `http://127.0.0.1:8899/asset`.
    pub asset_url: Option<String>,
    /// Base URL for `@font-face` in preview mode, e.g. `.../font`.
    pub font_url: Option<String>,
    /// Standalone `.svg` export: embed the font binaries as data URIs.
    pub embed_fonts: bool,
    pub show_guides: bool,
    /// Hidden / excluded layers.
    pub omit_ids: Vec<String>,
    /// Written into `width`/`height`. The `viewBox` stays at 1x, so a raster taken
    /// from this SVG and the preview agree on geometry at any scale.
    pub scale: f64,
}

impl Default for SvgOpts {
    fn default() -> Self {
        Self {
            boards: vec![],
            inline_assets: false,
            asset_url: None,
            font_url: None,
            embed_fonts: false,
            show_guides: false,
            omit_ids: vec![],
            scale: 1.0,
        }
    }
}

impl SvgOpts {
    /// Builds options from RPC params (`render.svg`, `render.export`, previews).
    /// Missing keys fall back to preview defaults.
    pub fn from_value(v: &Value, def_scale: f64) -> SvgOpts {
        let mut o = SvgOpts {
            scale: if def_scale.is_finite() && def_scale > 0.0 {
                def_scale
            } else {
                1.0
            },
            ..Default::default()
        };
        o.boards = strings_of(v, "boards");
        if o.boards.is_empty() {
            if let Some(b) = util::ps(v, "b").filter(|s| *s != "all" && *s != "*") {
                o.boards = vec![b.to_string()];
            }
        }
        o.inline_assets = util::opt_b(v, "inline", false);
        o.asset_url = util::ps(v, "asset_url").map(|s| s.trim_end_matches('/').to_string());
        o.font_url = util::ps(v, "font_url").map(|s| s.trim_end_matches('/').to_string());
        o.embed_fonts = util::opt_b(v, "embed", false);
        o.show_guides = util::opt_b(v, "guides", false);
        o.omit_ids = strings_of(v, "omit");
        o
    }

    /// Compact echo for RPC responses.
    pub fn to_value(&self) -> Value {
        json!({
            "boards": self.boards, "inline": self.inline_assets, "asset_url": self.asset_url,
            "font_url": self.font_url, "embed": self.embed_fonts, "guides": self.show_guides,
            "omit": self.omit_ids, "sc": self.scale,
        })
    }
}

fn strings_of(v: &Value, k: &str) -> Vec<String> {
    util::arr(v, k)
        .iter()
        .filter_map(|x| x.as_str().map(|s| s.to_string()))
        .collect()
}

// ------------------------------------------------------------------ entry points

/// Compiles a document into a standalone, deterministic SVG whose viewBox spans the
/// requested artboards laid out left to right with a [`GUTTER`] between them, so one
/// raster can contain a whole series.
pub fn compile(doc: &Document, o: &SvgOpts) -> R<String> {
    compile_at(doc, o, None)
}

/// [`compile`] plus the project directory, used to resolve `Asset::uri` when
/// `inline_assets` is set.
pub fn compile_at(doc: &Document, o: &SvgOpts, dir: Option<&Path>) -> R<String> {
    Ok(compile_w(doc, o, dir, None)?.0)
}

/// One artboard only, sized to that artboard. There is no XML prolog: the result is
/// a bare `<svg>` root, directly embeddable in the webview and directly rasterisable
/// by resvg — which is what `render.preview` of a single board needs.
pub fn compile_board(doc: &Document, board: &str, o: &SvgOpts) -> R<String> {
    compile_board_at(doc, board, o, None)
}

/// [`compile_board`] plus the project directory.
pub fn compile_board_at(
    doc: &Document,
    board: &str,
    o: &SvgOpts,
    dir: Option<&Path>,
) -> R<String> {
    Ok(compile_w(doc, o, dir, Some(board))?.0)
}

/// The real interface: also returns the degradations the compiler decided on
/// (missing assets, backdrop-blur fallbacks, absent fonts) so an agent can tell why
/// the picture looks nothing like the document.
///
/// `board == Some(id)` paints one artboard; `None` paints the canvas described by
/// `o.boards`.
pub fn compile_w(
    doc: &Document,
    o: &SvgOpts,
    dir: Option<&Path>,
    board: Option<&str>,
) -> R<(String, Vec<String>)> {
    let ids = match board {
        Some(b) => {
            if doc.artboard(b).is_none() {
                return Err(Fail::new("ARTBOARD_NOT_FOUND", format!("artboard \"{b}\""))
                    .hint("doc.boards lists every artboard id"));
            }
            if !doc.artboard(b).map(|a| a.visible).unwrap_or(false) {
                return Err(Fail::new("INVALID_VALUE", format!("artboard \"{b}\" is hidden"))
                    .hint("artboard.set {visible:1} or pick another board"));
            }
            vec![b.to_string()]
        }
        None => selected_boards(doc, o),
    };
    if ids.is_empty() {
        return Err(Fail::new(
            "INVALID_VALUE",
            "svg: the document has no visible artboard to paint",
        )
        .hint("artboard.add, or drop the visible / omit filters"));
    }

    let mut c = C::new(doc, o, dir.map(|p| p.to_path_buf()));
    let mut canvas = String::new();
    let mut x = 0.0f64;
    let multi = ids.len() > 1 && board.is_none();
    for id in &ids {
        let ab = match doc.artboard(id) {
            Some(a) => a.clone(),
            None => continue,
        };
        canvas.push_str(&c.board(&ab, (x, 0.0))?);
        if multi {
            x += ab.width + GUTTER;
        }
    }
    let (w, h) = if board.is_some() || !multi {
        doc.artboard(&ids[0]).map(|a| (a.width, a.height)).unwrap_or((1.0, 1.0))
    } else {
        canvas_size(doc, &ids)
    };
    let (w, h) = (w.max(1.0), h.max(1.0));
    let scale = if o.scale.is_finite() && o.scale > 0.0 { o.scale } else { 1.0 };

    let style = c.font_css();
    let defs = std::mem::take(&mut c.defs);
    let mut out = String::with_capacity(canvas.len() + defs.len() + 256);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\"{cls} width=\"{ww}\" height=\"{hh}\" viewBox=\"0 0 {vw} {vh}\">\n",
        cls = if multi { " class=\"acanvas-canvas\"" } else { "" },
        ww = nf(w * scale),
        hh = nf(h * scale),
        vw = nf(w),
        vh = nf(h),
    ));
    if !style.is_empty() {
        out.push_str("<style><![CDATA[\n");
        out.push_str(&style);
        out.push_str("]]></style>\n");
    }
    if !defs.is_empty() {
        out.push_str("<defs>\n");
        out.push_str(&defs);
        out.push_str("</defs>\n");
    }
    out.push_str(&canvas);
    out.push_str("</svg>\n");
    Ok((out, c.warnings))
}

/// Pixel size of the canvas [`compile`] produces for these options: the boards side
/// by side plus gutters, as tall as the tallest.
pub fn board_size(doc: &Document, o: &SvgOpts) -> (f64, f64) {
    let ids = selected_boards(doc, o);
    canvas_size(doc, &ids)
}

fn canvas_size(doc: &Document, ids: &[String]) -> (f64, f64) {
    let mut w = 0.0f64;
    let mut h = 0.0f64;
    for (i, id) in ids.iter().enumerate() {
        if let Some(a) = doc.artboard(id) {
            if i > 0 {
                w += GUTTER;
            }
            w += a.width;
            h = h.max(a.height);
        }
    }
    (w, h)
}

/// Artboards a compile would emit: the requested ones that exist and are visible,
/// else every visible artboard, in document order.
pub fn selected_boards(doc: &Document, o: &SvgOpts) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for b in &o.boards {
        if doc.artboard(b).map(|a| a.visible).unwrap_or(false) && !out.contains(b) {
            out.push(b.clone());
        }
    }
    if out.is_empty() {
        out = doc.artboards.iter().filter(|a| a.visible).map(|a| a.id.clone()).collect();
    }
    out
}

/// The boards themselves, in the order they are laid out.
pub fn boards_of<'a>(doc: &'a Document, o: &SvgOpts) -> Vec<&'a Artboard> {
    selected_boards(doc, o)
        .iter()
        .filter_map(|id| doc.artboard(id))
        .collect()
}

/// The lines a text node will be painted with. The preview and the solver both read
/// this so nobody re-wraps after the fact.
pub fn solved_lines(doc: &Document, n: &Node) -> Vec<String> {
    let Some(t) = n.text.as_ref() else { return vec![] };
    if !t.lines.is_empty() {
        return t.lines.clone();
    }
    let (lines, _) = solve_lines(doc, n, t, &mut Vec::new());
    lines
}

// ------------------------------------------------------------------ compiler

struct C<'a> {
    doc: &'a Document,
    o: &'a SvgOpts,
    dir: Option<PathBuf>,
    defs: String,
    /// Existence checks only; never iterated, so output stays byte-stable.
    def_ids: Vec<String>,
    warnings: Vec<String>,
    /// (family, weight, italic) needing an `@font-face`, first-seen order.
    fonts: Vec<(String, u16, bool)>,
    asset_cache: Vec<(String, String)>,
    board: String,
}

impl<'a> C<'a> {
    fn new(doc: &'a Document, o: &'a SvgOpts, dir: Option<PathBuf>) -> C<'a> {
        C {
            doc,
            o,
            dir,
            defs: String::new(),
            def_ids: vec![],
            warnings: Vec::new(),
            fonts: Vec::new(),
            asset_cache: Vec::new(),
            board: String::new(),
        }
    }

    fn warn(&mut self, m: String) {
        if !self.warnings.contains(&m) {
            self.warnings.push(m);
        }
    }

    /// Namespaced, deterministic def id: board + node + purpose.
    fn did(&self, node: &str, kind: &str) -> String {
        format!("ac-{}-{}-{}", eid(&self.board), eid(node), kind)
    }

    fn push_def(&mut self, id: &str, body: &str) {
        if self.def_ids.iter().any(|x| x == id) {
            return;
        }
        self.def_ids.push(id.to_string());
        self.defs.push_str(body);
        self.defs.push('\n');
    }

    // ------------------------------------------------------------- artboard

    fn board(&mut self, ab: &Artboard, off: (f64, f64)) -> R<String> {
        self.board = ab.id.clone();
        let clip = self.did(&ab.id, "board-clip");
        self.push_def(
            &clip.clone(),
            &format!(
                "<clipPath id=\"{clip}\"><rect x=\"0\" y=\"0\" width=\"{}\" height=\"{}\"/></clipPath>",
                nf(ab.width),
                nf(ab.height)
            ),
        );

        let mut body = String::new();
        if let Some(bg) = ab.background.as_ref().filter(|p| !p.is_none()) {
            let (fill, extra) =
                self.paint_attrs(&format!("{}-bg", ab.id), bg, Rect::from_size(ab.width, ab.height), 1.0, true);
            body.push_str(&format!(
                "<rect x=\"0\" y=\"0\" width=\"{}\" height=\"{}\"{}{}/>\n",
                nf(ab.width),
                nf(ab.height),
                fill,
                extra
            ));
        }

        let order = self.doc.paint_order(&ab.id);
        let mut done: Vec<String> = Vec::new();
        for id in &order {
            if self.o.omit_ids.iter().any(|o| o == id) {
                continue;
            }
            let Some(top) = self.doc.node(id) else { continue };
            if !top.visible {
                continue;
            }
            let s = self.top(top, ab, &mut done)?;
            body.push_str(&s);
        }
        if self.o.show_guides {
            body.push_str(&self.guides(ab));
        }

        Ok(format!(
            "<g id=\"board-{}\" transform=\"translate({} {})\" clip-path=\"url(#{clip})\">\n{}</g>\n",
            eid(&ab.id),
            nf(off.0),
            nf(off.1),
            body
        ))
    }

    fn guides(&mut self, ab: &Artboard) -> String {
        let mut s = String::from(
            "<g class=\"guides\" fill=\"none\" stroke=\"#00a8ff\" stroke-width=\"1\" stroke-dasharray=\"4 3\" opacity=\"0.75\">\n",
        );
        for g in &ab.guides {
            let (x1, y1, x2, y2) = if g.axis.eq_ignore_ascii_case("v") {
                (g.pos, 0.0, g.pos, ab.height)
            } else {
                (0.0, g.pos, ab.width, g.pos)
            };
            s.push_str(&format!(
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/>\n",
                nf(x1),
                nf(y1),
                nf(x2),
                nf(y2)
            ));
            if let Some(l) = &g.label {
                s.push_str(&format!(
                    "<text x=\"{}\" y=\"{}\" stroke=\"none\" fill=\"#00a8ff\" font-family=\"sans-serif\" font-size=\"10\">{}</text>\n",
                    nf(g.pos + 2.0),
                    nf(12.0),
                    esc(l)
                ));
            }
        }
        if let Some(m) = ab.safe_margin.filter(|m| *m > 0.0) {
            s.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" stroke=\"#ff5c8a\"/>\n",
                nf(m),
                nf(m),
                nf((ab.width - 2.0 * m).max(0.0)),
                nf((ab.height - 2.0 * m).max(0.0))
            ));
        }
        s.push_str("</g>\n");
        s
    }

    // ------------------------------------------------------------- node
    //
    // `done` is the running list of finished chunks in paint order *for this level*;
    // it is what a backdrop blur copies. Chunks are only appended once complete, so a
    // snapshot is always well-formed markup.

    fn top(&mut self, n: &Node, ab: &Artboard, done: &mut Vec<String>) -> R<String> {
        let prefix: Vec<String> = done.clone();
        let s = self.node(n, ab, &prefix)?;
        if !s.is_empty() {
            done.push(s.clone());
        }
        Ok(s)
    }

    fn node(&mut self, n: &Node, ab: &Artboard, behind: &[String]) -> R<String> {
        if n.box_.w <= 0.0 || n.box_.h <= 0.0 || !n.box_.is_finite() {
            self.warn(format!(
                "node \"{}\" has a zero or non-finite box and was skipped",
                n.id
            ));
            return Ok(String::new());
        }

        // Own geometry, then children (children see this node's shape as backdrop too).
        let mut own_shape: Vec<String> = Vec::new();
        if !matches!(n.kind, NodeType::Group | NodeType::Frame) {
            let s = self.shape(n, ab)?;
            if !s.is_empty() {
                own_shape.push(s);
            }
        }
        // Everything painted so far inside this branch, in back-to-front order: the
        // backdrop sampler reads it to build the glass effect.
        let mut behind_all: Vec<String> = own_shape.clone();
        let mut kids = String::new();
        for cid in &n.children {
            if cid == &n.id || self.o.omit_ids.iter().any(|o| o == cid) {
                continue;
            }
            let Some(child) = self.doc.node(cid) else { continue };
            if !child.visible {
                continue;
            }
            let mut b: Vec<String> = Vec::with_capacity(behind_all.len() + 1);
            b.extend_from_slice(behind_all.as_slice());
            b.extend_from_slice(behind);
            let s = self.node(child, ab, &b)?;
            if !s.is_empty() {
                behind_all.push(s.clone());
                kids.push_str(&s);
            }
        }

        let mut content = String::new();
        if let Some(glass) = self.backdrop(n, &behind_all, behind) {
            content.push_str(&glass);
        }
        for s in &own_shape {
            content.push_str(s);
            content.push('\n');
        }
        content.push_str(&kids);

        let mut gattrs = String::new();
        if let Some(f) = self.filter_def(n) {
            gattrs.push_str(&format!(" filter=\"url(#{f})\""));
        }
        if let Some(cl) = &n.clip {
            if let Some(id) = self.clip_def(n, cl) {
                gattrs.push_str(&format!(" clip-path=\"url(#{id})\""));
            }
        } else if n.kind == NodeType::Frame {
            let id = self.did(&n.id, "frame-clip");
            let geom = self.geometry(n, "").unwrap_or_default();
            if !geom.is_empty() {
                self.push_def(&id.clone(), &format!("<clipPath id=\"{id}\">{geom}</clipPath>"));
                gattrs.push_str(&format!(" clip-path=\"url(#{id})\""));
            }
        }

        let mut tattrs = String::new();
        let (cx, cy) = n.origin.point(n.box_);
        let mut tf: Vec<String> = Vec::new();
        if n.rotation.abs() > 1e-9 {
            tf.push(format!("rotate({} {} {})", nf(n.rotation), nf(cx), nf(cy)));
        }
        if n.flip_x {
            tf.push(format!("translate({} 0) scale(-1 1)", nf(2.0 * cx)));
        }
        if n.flip_y {
            tf.push(format!("translate(0 {}) scale(1 -1)", nf(2.0 * cy)));
        }
        if !tf.is_empty() {
            tattrs.push_str(&format!(" transform=\"{}\"", tf.join(" ")));
        }
        let op = util::clamp(n.opacity, 0.0, 1.0);
        if op < 1.0 {
            tattrs.push_str(&format!(" opacity=\"{}\"", nf(op)));
        }
        if let Some(b) = &n.blend {
            let b = b.trim().to_lowercase().replace('_', "-");
            const OK: &[&str] = &[
                "multiply", "screen", "overlay", "darken", "lighten", "color-dodge", "color-burn",
                "hard-light", "soft-light", "difference", "exclusion", "hue", "saturation", "color",
                "luminosity",
            ];
            if OK.contains(&b.as_str()) {
                tattrs.push_str(&format!(" style=\"mix-blend-mode:{b}\""));
            } else {
                self.warn(format!("node \"{}\": unknown blend \"{b}\" ignored", n.id));
            }
        }

        if content.is_empty() {
            return Ok(String::new());
        }
        // One wrapper for the filter/clip (applied in the unrotated space, so a
        // shadow rotates with the node the way Figma draws it), one for the transform.
        if !gattrs.is_empty() {
            content = format!("<g id=\"n-{}\"{}>\n{content}</g>\n", eid(&n.id), gattrs);
        }
        if tattrs.is_empty() && gattrs.is_empty() {
            return Ok(content);
        }
        Ok(format!(
            "<g id=\"n-{}-t\"{}>\n{content}</g>\n",
            eid(&n.id),
            tattrs
        ))
    }

    /// A blurred static copy of everything underneath, clipped to this box.
    ///
    /// Static rather than `<use href>` of an ancestor because `<use>` of the parent
    /// chain is a cycle (resvg drops it), and because a copy is painted identically
    /// by the browser and by resvg — which is the whole point.
    fn backdrop(&mut self, n: &Node, own: &[String], behind: &[String]) -> Option<String> {
        let Some(fx) = n.fx.as_ref() else { return None };
        let r = fx.backdrop_blur.filter(|r| *r > 0.0)?;
        let mut flat = String::new();
        for s in behind.iter().chain(own.iter()) {
            flat.push_str(s);
        }
        let nodes = flat.matches("<g ").count();
        let copies = self.def_ids.iter().filter(|d| d.ends_with("-bd-blur")).count();
        if nodes > BACKDROP_NODE_BUDGET || copies >= BACKDROP_COPY_BUDGET {
            self.warn(format!(
                "node \"{}\": backdrop_blur {}px degraded to a translucent tint plus noise \
                 ({} nodes behind it, budget {} nodes / {} copies)",
                n.id,
                nf(r),
                nodes,
                BACKDROP_NODE_BUDGET,
                BACKDROP_COPY_BUDGET
            ));
            return Some(self.glass_tint(n));
        }
        let b = n.box_;
        let clip = self.did(&n.id, "bd-clip");
        let geom = self.geometry(n, "").unwrap_or_else(|| {
            format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
                nf(b.x), nf(b.y), nf(b.w), nf(b.h)
            )
        });
        self.push_def(&clip.clone(), &format!("<clipPath id=\"{clip}\">{geom}</clipPath>"));
        let fid = self.did(&n.id, "bd-blur");
        self.push_def(
            &fid.clone(),
            &format!(
                "<filter id=\"{fid}\" filterUnits=\"userSpaceOnUse\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\">\
<feGaussianBlur stdDeviation=\"{}\"/></filter>",
                nf(b.x), nf(b.y), nf(b.w), nf(b.h), nf(r / 2.0)
            ),
        );
        Some(format!(
            "<g clip-path=\"url(#{clip})\" filter=\"url(#{fid})\">{flat}</g>\n",
            flat = flat
        ))
    }

    /// The fallback glass look: a translucent plate with a little grain.
    fn glass_tint(&mut self, n: &Node) -> String {
        let b = n.box_;
        let id = self.did(&n.id, "glass");
        self.push_def(
            &id.clone(),
            &format!(
                "<filter id=\"{id}\" filterUnits=\"userSpaceOnUse\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\">\
<feTurbulence type=\"fractalNoise\" baseFrequency=\"0.9\" numOctaves=\"2\" seed=\"0\" result=\"n\"/>\
<feColorMatrix in=\"n\" type=\"saturate\" values=\"0\" result=\"g\"/>\
<feComponentTransfer in=\"g\" result=\"a\"><feFuncA type=\"linear\" slope=\"0.05\"/></feComponentTransfer>\
<feComposite in=\"a\" in2=\"SourceGraphic\" operator=\"atop\"/></filter>",
                nf(b.x), nf(b.y), nf(b.w), nf(b.h)
            ),
        );
        format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#ffffff\" fill-opacity=\"0.16\" filter=\"url(#{id})\"/>\n",
            nf(b.x), nf(b.y), nf(b.w), nf(b.h)
        )
    }

    // ------------------------------------------------------------- shapes

    /// The node's geometry as markup with `attrs` spliced in, in artboard space on
    /// the unrotated box. `None` when the kind has no path geometry.
    fn geometry(&mut self, n: &Node, attrs: &str) -> Option<String> {
        let b = n.box_;
        let v: Option<&VectorSpec> = n.vector.as_ref();
        Some(match n.kind {
            NodeType::Rect => {
                let rad = n.style.as_ref().and_then(|s| s.radius.as_ref());
                match rad {
                    Some(r) if r.of().iter().any(|x| *x > 0.0) && !uniform(r) => {
                        format!("<path d=\"{}\"{}/>", corner_path(b, r), attrs)
                    }
                    Some(r) if r.all > 0.0 => format!(
                        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\"{}/>",
                        nf(b.x), nf(b.y), nf(b.w), nf(b.h), nf(r.all.min(b.w.min(b.h) / 2.0)), attrs
                    ),
                    _ => format!(
                        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}/>",
                        nf(b.x), nf(b.y), nf(b.w), nf(b.h), attrs
                    ),
                }
            }
            NodeType::Ellipse => format!(
                "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\"{}/>",
                nf(b.cx()), nf(b.cy()), nf(b.w / 2.0), nf(b.h / 2.0), attrs
            ),
            NodeType::Line => {
                let (x1, y1, x2, y2) = line_ends(n, b);
                format!(
                    "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"{}/>",
                    nf(b.x + x1), nf(b.y + y1), nf(b.x + x2), nf(b.y + y2), attrs
                )
            }
            NodeType::Polygon => {
                let pts = polygon_points(n, b);
                if pts.len() < 3 {
                    return None;
                }
                let s: Vec<String> = pts.iter().map(|(x, y)| format!("{},{}", nf(*x), nf(*y))).collect();
                format!("<polygon points=\"{}\"{}/>", s.join(" "), attrs)
            }
            NodeType::Path => {
                let d = v.and_then(|x| x.d.as_deref()).unwrap_or("").trim();
                if d.is_empty() {
                    return None;
                }
                format!("<path d=\"{}\"{}/>", esc(d), attrs)
            }
            _ => return None,
        })
    }

    fn shape(&mut self, n: &Node, ab: &Artboard) -> R<String> {
        let style = n.style.clone().unwrap_or_default();
        let b = n.box_;
        let out = match n.kind {
            NodeType::Text => self.text(n, &style)?,
            NodeType::Image => self.image(n, &style, ab)?,
            NodeType::Svg => self.svg_node(n)?.unwrap_or_default(),
            NodeType::Group | NodeType::Frame => {
                match style.fill.filter(|p| !p.is_none()) {
                    Some(p) => match self.geometry(n, "") {
                        Some(g) if !g.is_empty() => {
                            let (fill, extra) = self.paint_attrs(&n.id, &p, b, 1.0, true);
                            self.painted(n, &g, &p, &format!("{fill}{extra}"), None)?
                        }
                        _ => {
                            // A group with a fill but no geometry paints a plate.
                            let (fill, extra) = self.paint_attrs(&n.id, &p, b, 1.0, true);
                            format!(
                                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}{}/>\n",
                                nf(b.x), nf(b.y), nf(b.w), nf(b.h), fill, extra
                            )
                        }
                    },
                    None => String::new(),
                }
            }
            _ => {
                let geom = self.geometry(n, "").unwrap_or_default();
                if geom.is_empty() {
                    self.warn(format!(
                        "node \"{}\": no geometry for a {} node (path data or points missing)",
                        n.id,
                        n.kind.as_str()
                    ));
                    return Ok(String::new());
                }
                let fill = style.fill.clone().unwrap_or(Paint::None);
                let (fill_attrs, extra) = if fill.is_none() {
                    (" fill=\"none\"".to_string(), String::new())
                } else {
                    self.paint_attrs(&n.id, &fill, b, 1.0, true)
                };
                self.painted(n, &geom, &fill, &format!("{fill_attrs}{extra}"), style.stroke.as_ref())?
            }
        };
        Ok(out)
    }

    /// One geometry plus its fill and, when asked for, an inside/outside stroke.
    fn painted(
        &mut self,
        n: &Node,
        geom: &str,
        fill: &Paint,
        fill_attrs: &str,
        stroke: Option<&Stroke>,
    ) -> R<String> {
        let b = n.box_;
        let Some(st) = stroke.filter(|s| s.width > 0.0) else {
            return Ok(format!("{}\n", splice(geom, fill_attrs)));
        };
        let mut sa = String::new();
        if !fill.is_none() {
            // inside/outside only mean something when there is a shape to trim
            // against; an unfilled stroke has no inner half.
            sa = format!( " stroke=\"{}\" stroke-width=\"{}\"", esc(&self.color(&st.color).0), nf(st.width));
            let align = st.align.clone().unwrap_or_else(|| "center".into()).to_lowercase();
            match align.as_str() {
                "inside" => {
                    let cid = self.did(&n.id, "in-clip");
                    self.push_def(&cid.clone(), &format!("<clipPath id=\"{cid}\">{geom}</clipPath>"));
                    return Ok(format!(
                        "{}\n",
                        splice(
                            geom,
                            &format!(
                                "{fill_attrs}{} stroke-width=\"{}\" clip-path=\"url(#{cid})\"",
                                self.stroke_rest(st),
                                nf(st.width * 2.0)
                            )
                        )
                    ));
                }
                "outside" => {
                    let pad = st.width;
                    let r = Rect::new(b.x - pad, b.y - pad, b.w + 2.0 * pad, b.h + 2.0 * pad);
                    let mid = self.did(&n.id, "out-mask");
                    self.push_def(
                        &mid.clone(),
                        &format!(
                            "<mask id=\"{mid}\" maskUnits=\"userSpaceOnUse\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\">\
<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#ffffff\"/>\
{geom_black}\
</mask>",
                            nf(r.x), nf(r.y), nf(r.w), nf(r.h),
                            nf(r.x), nf(r.y), nf(r.w), nf(r.h),
                            geom_black = black_geom(geom),
                        ),
                    );
                    return Ok(format!(
                        "{}\n{}\n",
                        splice(geom, fill_attrs),
                        splice(
                            geom,
                            &format!(
                                " fill=\"none\"{} stroke-width=\"{}\" mask=\"url(#{mid})\"",
                                self.stroke_rest(st),
                                nf(st.width * 2.0)
                            )
                        )
                    ));
                }
                _ => {}
            }
        } else {
            sa = String::new();
        }
        let attrs = format!(
            "{fill_attrs}{sa}{}{}",
            self.stroke_rest(st),
            self.stroke_opacity(st)
        );
        Ok(format!("{}\n", splice(geom, &attrs)))
    }

    fn stroke_rest(&mut self, st: &Stroke) -> String {
        let mut s = String::new();
        if let Some(c) = &st.cap {
            let c = c.to_lowercase();
            if matches!(c.as_str(), "butt" | "round" | "square") {
                s.push_str(&format!(" stroke-linecap=\"{c}\""));
            } else {
                self.warn(format!("stroke cap \"{c}\" is not an SVG cap; using butt"));
            }
        }
        if !st.dash.is_empty() {
            let d: Vec<String> = st.dash.iter().map(|v| nf(*v)).collect();
            s.push_str(&format!(" stroke-dasharray=\"{}\"", d.join(" ")));
        }
        s
    }

    fn stroke_opacity(&mut self, st: &Stroke) -> String {
        let a = self.color(&st.color).1;
        if a < 1.0 {
            format!(" stroke-opacity=\"{}\"", nf(a))
        } else {
            String::new()
        }
    }

    // ------------------------------------------------------------- paint

    /// Expands `$token`, then normalises any accepted colour form to `#rrggbb` plus
    /// a separate alpha, which is what SVG wants (fill + fill-opacity).
    fn color(&self, s: &str) -> (String, f64) {
        let expanded = self.doc.tokens.expand(s);
        match parse_color(expanded.trim()) {
            Some((r, g, b, a)) => (format!("#{r:02x}{g:02x}{b:02x}"), a),
            None => {
                let t = s.trim().to_lowercase();
                if t.is_empty() {
                    ("#000000".to_string(), 0.0)
                } else if matches!(t.as_str(), "none" | "transparent") {
                    ("#000000".to_string(), 0.0)
                } else {
                    (t, 1.0)
                }
            }
        }
    }

    /// Returns `(" fill=\"…\"", " fill-opacity=\"…\"")` (or the stroke forms), so
    /// callers only ever splice strings onto an element.
    fn paint_attrs(
        &mut self,
        node: &str,
        p: &Paint,
        b: Rect,
        opacity: f64,
        is_fill: bool,
    ) -> (String, String) {
        let key = if is_fill { "fill" } else { "stroke" };
        let op_key = if is_fill { "fill-opacity" } else { "stroke-opacity" };
        // The shared opacity tail every non-solid paint needs: a gradient or a
        // pattern already carries per-stop / per-tile alpha, so this is only the
        // multiplier on top of it, and it is written when it dims anything at all.
        let op_all = |alpha: f64| -> String {
            let a = util::clamp(alpha, 0.0, 1.0);
            if a < 1.0 {
                format!(" {op_key}=\"{}\"", nf(a))
            } else {
                String::new()
            }
        };
        match p {
            Paint::None => (format!(" {key}=\"none\""), String::new()),
            Paint::Solid { color } => {
                let (hex, a) = self.color(color);
                let alpha = util::clamp(a * opacity, 0.0, 1.0);
                (
                    format!(" {key}=\"{hex}\""),
                    if alpha < 1.0 {
                        format!(" {op_key}=\"{}\"", nf(alpha))
                    } else {
                        String::new()
                    },
                )
            }
            Paint::Linear { stops, angle } => {
                let id = self.did(node, "lg");
                let (x1, y1, x2, y2) = linear_endpoints(b, *angle);
                let body = format!(
                    "<linearGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">{}</linearGradient>",
                    nf(x1), nf(y1), nf(x2), nf(y2), self.stops(stops)
                );
                self.push_def(&id.clone(), &body);
                (format!(" {key}=\"url(#{id})\""), op_all(opacity))
            }
            Paint::Radial { stops, cx, cy, r } => {
                let id = self.did(node, "rg");
                let gx = b.x + util::clamp(*cx, 0.0, 1.0) * b.w;
                let gy = b.y + util::clamp(*cy, 0.0, 1.0) * b.h;
                // `r` is a fraction of half the longer side, so 1.0 reaches the edge.
                let rr = r.max(0.001) * b.w.max(b.h) / 2.0;
                let body = format!(
                    "<radialGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" cx=\"{gx}\" cy=\"{gy}\" r=\"{rr}\" fx=\"{gx}\" fy=\"{gy}\">{}</radialGradient>",
                    self.stops(stops)
                );
                self.push_def(&id.clone(), &body);
                (format!(" {key}=\"url(#{id})\""), op_all(opacity))
            }
            Paint::Image { asset, fit: f, opacity: po } => {
                let Some(uri) = self.asset_href(asset) else {
                    self.warn(format!("asset \"{asset}\" cannot be inlined; {key} dropped"));
                    return (format!(" {key}=\"none\""), String::new());
                };
                let id = self.did(node, "pat");
                let body = format!(
                    "<pattern id=\"{id}\" patternUnits=\"userSpaceOnUse\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\">\
<image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" href=\"{uri}\" preserveAspectRatio=\"{}\"/>\
</pattern>",
                    nf(b.x), nf(b.y), nf(b.w), nf(b.h),
                    nf(b.x), nf(b.y), nf(b.w), nf(b.h), par(*f)
                );
                self.push_def(&id.clone(), &body);
                (format!(" {key}=\"url(#{id})\""), op_all(opacity * po))
            }
        }
    }

    fn stops(&mut self, stops: &[Stop]) -> String {
        let mut v: Vec<&Stop> = stops.iter().collect();
        v.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
        let mut out = String::new();
        for s in &v {
            let (hex, a) = self.color(&s.color);
            out.push_str(&format!(
                "<stop offset=\"{}\" stop-color=\"{hex}\"{}/>",
                nf(util::clamp(s.at, 0.0, 1.0)),
                if a < 1.0 { format!(" stop-opacity=\"{}\"", nf(a)) } else { String::new() }
            ));
        }
        if v.is_empty() {
            self.warn("a gradient has no stops; it will paint nothing".to_string());
        }
        out
    }

    /// `data:` URI when inlining, `{asset_url}/<id>` in preview mode.
    fn asset_href(&mut self, id: &str) -> Option<String> {
        if !self.o.inline_assets {
            let base = self.o.asset_url.clone().unwrap_or_else(|| "asset".to_string());
            return Some(format!("{base}/{}", eid(id)));
        }
        if let Some((_, u)) = self.asset_cache.iter().find(|(k, _)| k == id) {
            return Some(u.clone());
        }
        let a = self.doc.asset(id)?;
        let p = self.asset_path(a)?;
        let bytes = match std::fs::read(&p) {
            Ok(b) => b,
            Err(e) => {
                // Painting nothing silently is how a poster ships without its photo.
                self.warn(format!("asset \"{}\" cannot be read from {}: {e}", a.name, p.display()));
                return None;
            }
        };
        let mime = a.mime.clone().unwrap_or_else(|| mime_from_path(&p));
        let uri = format!("data:{mime};base64,{}", b64(&bytes));
        self.asset_cache.push((id.to_string(), uri.clone()));
        Some(uri)
    }

    fn asset_path(&self, a: &Asset) -> Option<PathBuf> {
        resolve_asset_path(&a.uri, self.dir.as_deref())
    }

    // ------------------------------------------------------------- filters

    fn filter_def(&mut self, n: &Node) -> Option<String> {
        let fx = n.fx.as_ref()?;
        let blur = fx.blur.filter(|v| *v > 0.0);
        let shadows: Vec<&Shadow> = fx.shadows.iter().collect();
        if blur.is_none() && fx.saturate.is_none() && fx.inner_glow.is_none() && shadows.is_empty() {
            return None;
        }
        let id = self.did(&n.id, "fx");
        let b = n.box_;
        let mut pad = 0.0f64;
        let mut prims = String::new();
        let mut base = "SourceGraphic".to_string();

        if let Some(v) = blur {
            // A CSS blur radius is about twice feGaussianBlur's stdDeviation.
            prims.push_str(&format!(
                "<feGaussianBlur in=\"{base}\" stdDeviation=\"{}\" result=\"f-blur\"/>",
                nf(v / 2.0)
            ));
            base = "f-blur".into();
            pad = pad.max(v * 1.5);
        }
        if let Some(s) = fx.saturate {
            prims.push_str(&format!(
                "<feColorMatrix in=\"{base}\" type=\"saturate\" values=\"{}\" result=\"f-sat\"/>",
                nf(s)
            ));
            base = "f-sat".into();
        }
        if let Some(c) = &fx.inner_glow {
            // Glow lives inside the edge: take the inverted alpha and flood it.
            let (hex, a) = self.color(c);
            prims.push_str(&format!(
                "<feComponentTransfer in=\"SourceAlpha\" result=\"f-glow-a\"><feFuncA type=\"table\" tableValues=\"0 0.2 0.55 1\"/></feComponentTransfer>\
<feComposite in=\"f-glow-a\" in2=\"{base}\" operator=\"in\" result=\"f-glow-m\"/>\
<feFlood flood-color=\"{hex}\" flood-opacity=\"{}\" result=\"f-glow-c\"/>\
<feComposite in=\"f-glow-c\" in2=\"f-glow-m\" operator=\"in\" result=\"f-glow\"/>\
<feMerge><feMergeNode in=\"{base}\"/><feMergeNode in=\"f-glow\"/></feMerge>",
                nf(a)
            ));
            base = "f-glow".into();
        }

        let mut outer: Vec<(String, String, &Shadow, String, f64)> = Vec::new();
        for (i, sh) in shadows.iter().enumerate() {
            let (hex, a) = self.color(&sh.color);
            let sd = (sh.blur / 2.0).max(0.0);
            let spread = sh.spread.unwrap_or(0.0);
            pad = pad.max(sd * 3.0 + spread.abs() + sh.x.abs().max(sh.y.abs()));
            if sh.inset.unwrap_or(false) {
                let r = format!("f-in{i}");
                prims.push_str(&format!(
                    "<feOffset in=\"SourceAlpha\" dx=\"{}\" dy=\"{}\" result=\"{r}-o\"/>\
<feGaussianBlur in=\"{r}-o\" stdDeviation=\"{}\" result=\"{r}-b\"/>\
<feComposite in=\"SourceAlpha\" in2=\"{r}-b\" operator=\"out\" result=\"{r}-i\"/>\
<feFlood flood-color=\"{hex}\" flood-opacity=\"{}\" result=\"{r}-c\"/>\
<feComposite in=\"{r}-c\" in2=\"{r}-i\" operator=\"in\" result=\"{r}-s\"/>\
<feComposite in=\"{r}-s\" in2=\"{base}\" operator=\"over\" result=\"{r}\"/>",
                    nf(sh.x), nf(sh.y), nf(sd), nf(a)
                ));
                base = r;
            } else {
                let r = format!("f-out{i}");
                if spread.abs() < 0.001 && i == 0 && shadows.len() == 1 && base == "SourceGraphic" {
                    // The one honest shorthand: feDropShadow already merges the
                    // source on top of its shadow.
                    prims.push_str(&format!(
                        "<feDropShadow dx=\"{}\" dy=\"{}\" stdDeviation=\"{}\" flood-color=\"{hex}\" flood-opacity=\"{}\"/>",
                        nf(sh.x), nf(sh.y), nf(sd), nf(a)
                    ));
                    break;
                }
                if spread.abs() > 0.001 {
                    prims.push_str(&format!(
                        "<feMorphology in=\"SourceAlpha\" operator=\"dilate\" radius=\"{}\" result=\"{r}-d\"/>\
<feGaussianBlur in=\"{r}-d\" stdDeviation=\"{sd}\" result=\"{r}-b\"/>",
                        nf(spread)
                    ));
                    let src = format!("{r}-b");
                    outer.push((src, r.clone(), sh, hex.clone(), a));
                } else {
                    prims.push_str(&format!(
                        "<feGaussianBlur in=\"SourceAlpha\" stdDeviation=\"{sd}\" result=\"{r}-b\"/>"
                    ));
                    let src = format!("{r}-b");
                    outer.push((src, r.clone(), sh, hex.clone(), a));
                }
            }
        }
        // Complete each outer shadow, then merge them under the (already modified)
        // source. Order: the first shadow is the outermost, like CSS.
        let mut merge_names: Vec<String> = Vec::new();
        for (src, name, sh, hex, a) in outer {
            prims.push_str(&format!(
                "<feOffset in=\"{src}\" dx=\"{}\" dy=\"{}\" result=\"{name}-o\"/>\
<feFlood flood-color=\"{hex}\" flood-opacity=\"{}\" result=\"{name}-c\"/>\
<feComposite in=\"{name}-c\" in2=\"{name}-o\" operator=\"in\" result=\"{name}\"/>",
                nf(sh.x), nf(sh.y), nf(a)
            ));
            merge_names.push(name);
        }
        if !merge_names.is_empty() {
            let mut m = String::from("<feMerge>");
            for name in &merge_names {
                m.push_str(&format!("<feMergeNode in=\"{name}\"/>"));
            }
            m.push_str(&format!("<feMergeNode in=\"{base}\"/></feMerge>"));
            prims.push_str(&m);
        }

        let region = Rect::new(b.x - pad, b.y - pad, b.w + 2.0 * pad, b.h + 2.0 * pad);
        self.push_def(
            &id.clone(),
            &format!(
                "<filter id=\"{id}\" filterUnits=\"userSpaceOnUse\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\">{}</filter>",
                nf(region.x), nf(region.y), nf(region.w), nf(region.h), prims
            ),
        );
        Some(id)
    }

    fn clip_def(&mut self, n: &Node, cl: &crate::doc::Clip) -> Option<String> {
        let id = self.did(&n.id, "clip");
        let geom = match cl.kind.as_str() {
            "rect" => {
                let r = cl.rect.unwrap_or(n.box_);
                format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}/>",
                    nf(r.x), nf(r.y), nf(r.w), nf(r.h),
                    cl.radius.map(|x| format!(" rx=\"{}\"", nf(x))).unwrap_or_default()
                )
            }
            _ => {
                let by = cl.by.as_deref()?;
                let src = self.doc.node(by)?;
                if matches!(src.kind, NodeType::Text | NodeType::Image | NodeType::Svg) {
                    self.warn(format!(
                        "node \"{}\": cannot clip by \"{}\" ({} geometry has no outline)",
                        n.id, by, src.kind.as_str()
                    ));
                    return None;
                }
                self.geometry(src, "")?
            }
        };
        self.push_def(&id.clone(), &format!("<clipPath id=\"{id}\">{geom}</clipPath>"));
        Some(id)
    }

    // ------------------------------------------------------------- text

    fn text(&mut self, n: &Node, style: &Style) -> R<String> {
        let Some(t) = n.text.as_ref().cloned() else {
            self.warn(format!(
                "node \"{}\" is a text node with no text spec",
                n.id
            ));
            return Ok(String::new());
        };
        let b = n.box_;
        let pad = t.padding.unwrap_or(0.0).max(0.0);
        let size = t.size.unwrap_or(16.0);
        let weight = t.weight.unwrap_or(400);
        let italic = t.style_italic.unwrap_or(false);
        let ls = t.letter_spacing.unwrap_or(0.0);
        let line_h = t.line_h();
        let align = t.align.clone().unwrap_or_else(|| "left".into()).to_lowercase();
        let valign = t.valign.clone().unwrap_or_else(|| "top".into()).to_lowercase();

        let reg = Registry::shared();
        let want = t
            .family
            .clone()
            .or_else(|| {
                t.font_ref
                    .as_ref()
                    .and_then(|r| self.doc.tokens.font(r).map(|f| f.family.clone()))
            })
            .unwrap_or_else(|| "sans-serif".into());
        let resolved = reg.resolve(&want);
        if !reg.has(&resolved) {
            self.warn(format!(
                "node \"{}\": font \"{want}\" is not installed, resvg will use its default face",
                n.id
            ));
        } else if resolved != want {
            self.warn(format!("node \"{}\": font \"{want}\" resolved to \"{resolved}\"", n.id));
        }
        if t.rotate_each.unwrap_or(0.0).abs() > 1e-6 {
            self.warn(format!(
                "node \"{}\": rotate_each needs per-glyph placement, which SVG <text> cannot do; \
                 the line is written straight",
                n.id
            ));
        }

        let content = if t.uppercase.unwrap_or(false) {
            t.content.to_uppercase()
        } else {
            t.content.clone()
        };
        // A Chinese-first product must not paint tofu just because the caller left
        // the family generic: prefer a face that actually carries these glyphs.
        let resolved = if reg.tofu_risk(&content, &resolved, weight) {
            let cjk = reg.cjk_family();
            if !cjk.is_empty() && cjk != resolved && reg.has(&cjk) {
                self.warn(format!(
                    "node \"{}\": \"{resolved}\" has no glyphs for this text, painted with \"{cjk}\"",
                    n.id
                ));
                cjk
            } else {
                resolved
            }
        } else {
            resolved
        };
        let mut lines = if t.lines.is_empty() {
            let mut w = Vec::new();
            let (l, metrics) = solve_lines_with(self.doc, n, &t, &mut w, &content);
            for x in w {
                self.warn(x);
            }
            metrics.map(|m| m.lines).unwrap_or(l)
        } else {
            t.lines.clone()
        };
        if lines.is_empty() {
            lines.push(String::new());
        }

        let block_h = line_h * lines.len() as f64;
        // Baseline of the first line, per valign. The engine's metrics are used when
        // they are there; 0.8em is the conventional fallback.
        let asc = size * 0.8;
        let base_y = match valign.as_str() {
            "middle" | "center" => b.y + (b.h - block_h) / 2.0 + asc,
            "bottom" | "end" => b.y2() - block_h + asc - pad,
            _ => b.y + pad + asc,
        };
        let anchor = match align.as_str() {
            "center" | "middle" => "middle",
            "right" | "end" => "end",
            _ => "start",
        };
        let tx = match anchor {
            "middle" => b.cx(),
            "end" => b.x2() - pad,
            _ => b.x + pad,
        };

        let mut out = String::new();
        if let Some(bg) = t.bg.as_ref().filter(|p| !p.is_none()) {
            let (bf, bx) = self.paint_attrs(&format!("{}-bg", n.id), bg, b, 1.0, true);
            out.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}{}/>\n",
                nf(b.x), nf(b.y), nf(b.w), nf(b.h), bf, bx
            ));
        }

        let (fill, fextra) = match t.paint.filter(|p| !p.is_none()) {
            Some(p) => self.paint_attrs(&n.id, &p, b, 1.0, true),
            None => {
                let col = t
                    .color
                    .clone()
                    .or_else(|| style.color.clone())
                    .unwrap_or_else(|| "#111111".into());
                let (hex, a) = self.color(&col);
                (
                    format!(" fill=\"{hex}\""),
                    if a < 1.0 { format!(" fill-opacity=\"{}\"", nf(a)) } else { String::new() },
                )
            }
        };
        let stroke_attrs = match t.stroke.filter(|s| s.width > 0.0) {
            Some(s) => format!(
                " stroke=\"{}\" stroke-width=\"{}\" paint-order=\"stroke\"",
                esc(&self.color(&s.color).0),
                nf(s.width)
            ),
            None => String::new(),
        };
        self.note_font(&resolved, weight, italic);
        let common = format!(
            " font-family=\"{}\" font-size=\"{}\" font-weight=\"{}\" letter-spacing=\"{}\"{}",
            esc(&font_list(&resolved, &want, t.fallback.as_deref().unwrap_or(""))),
            nf(size),
            weight,
            nf(ls),
            if italic { " font-style=\"italic\"" } else { "" }
        );
        for (i, line) in lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            out.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" text-anchor=\"{anchor}\"{}{}{}{}>{}</text>\n",
                nf(tx),
                nf(base_y + i as f64 * line_h),
                common,
                fill,
                fextra,
                stroke_attrs,
                esc(line)
            ));
        }
        Ok(out)
    }

    fn note_font(&mut self, family: &str, weight: u16, italic: bool) {
        if self.o.font_url.is_none() && !self.o.embed_fonts {
            return;
        }
        let key = (family.to_string(), weight, italic);
        if !self.fonts.contains(&key) {
            self.fonts.push(key);
        }
    }

    /// The `@font-face` rules, in first-seen order. resvg's CSS subset skips at-rules
    /// entirely, so this block exists only so the browser in the desktop preview
    /// fetches the *same binary* resvg shapes with — which is what keeps preview and
    /// export line-for-line identical.
    fn font_css(&mut self) -> String {
        if self.fonts.is_empty() {
            return String::new();
        }
        let reg = Registry::shared();
        let mut out = String::new();
        for (family, weight, italic) in self.fonts.clone() {
            let style = if italic { "italic" } else { "normal" };
            let src = if self.o.embed_fonts {
                match reg.face_bytes(&family, weight, italic) {
                    Some(bytes) => format!(
                        "url(data:font/truetype;base64,{}) format(\"truetype\")",
                        b64(&bytes)
                    ),
                    None => {
                        self.warn(format!("font \"{family}\" {weight} {style}: no bytes to embed"));
                        continue;
                    }
                }
            } else {
                match &self.o.font_url {
                    Some(base) => format!(
                        "url(\"{}?family={}&weight={}&style={}\") format(\"truetype\")",
                        base,
                        urlenc(&family),
                        weight,
                        style
                    ),
                    None => continue,
                }
            };
            out.push_str(&format!(
                "@font-face{{font-family:\"{family}\";font-style:{style};font-weight:{weight};src:{src};}}\n",
            ));
        }
        out
    }

    // ------------------------------------------------------------- image

    fn image(&mut self, n: &Node, style: &Style, _ab: &Artboard) -> R<String> {
        let Some(spec) = n.image.as_ref().cloned() else {
            self.warn(format!("node \"{}\" is an image node with no image spec", n.id));
            return Ok(String::new());
        };
        let Some(asset) = self.doc.asset(&spec.asset).cloned() else {
            self.warn(format!("node \"{}\": asset \"{}\" is not in the document", n.id, spec.asset));
            return Ok(String::new());
        };
        let Some(uri) = self.asset_href(&asset.id) else {
            self.warn(format!(
                "node \"{}\": asset \"{}\" ({}) cannot be read{}",
                n.id,
                asset.id,
                asset.uri,
                if self.o.inline_assets {
                    " from disk; pass the project dir to compile_at"
                } else {
                    ""
                }
            ));
            return Ok(String::new());
        };
        let b = n.box_;
        let aw = if asset.width > 0.0 { asset.width } else { b.w };
        let ah = if asset.height > 0.0 { asset.height } else { b.h };
        let crop = spec.crop.unwrap_or(Rect::from_size(aw, ah));
        let src = Rect::from_size(crop.w.max(0.001), crop.h.max(0.001));

        if let Some(tile) = spec.tile.filter(|t| *t > 0.0) {
            let id = self.did(&n.id, "tile");
            self.push_def(
                &id.clone(),
                &format!(
                    "<pattern id=\"{id}\" patternUnits=\"userSpaceOnUse\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\">\
<image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" href=\"{uri}\" preserveAspectRatio=\"none\"/>\
</pattern>",
                    nf(b.x), nf(b.y), nf(tile), nf(tile), nf(b.x), nf(b.y), nf(tile), nf(tile)
                ),
            );
            let fid = self.image_filter(n, &spec);
            let fattr = fid.map(|f| format!(" filter=\"url(#{f})\"")).unwrap_or_default();
            return Ok(format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"url(#{id})\" opacity=\"{}\"{fattr}/>\n",
                nf(b.x), nf(b.y), nf(b.w), nf(b.h), nf(img_opacity(&spec, style))
            ));
        }

        // `fit` picks where the *crop* lands inside the box; the whole asset is then
        // drawn at that same scale and the box clips it. `Actual` means no scaling.
        let dst = if spec.fit == Fit::Actual {
            Rect::new(b.cx() - src.w / 2.0, b.cy() - src.h / 2.0, src.w, src.h)
        } else {
            fit(src, b, spec.fit)
        };
        let k = if src.w > 0.0 { dst.w / src.w } else { 1.0 };
        let whole = Rect::new(
            dst.x - crop.x * k,
            dst.y - crop.y * k,
            aw * k,
            ah * (if src.h > 0.0 { dst.h / src.h } else { k }),
        );
        let clip = self.did(&n.id, "img-clip");
        self.push_def(
            &clip.clone(),
            &format!(
                "<clipPath id=\"{clip}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath>",
                nf(b.x), nf(b.y), nf(b.w), nf(b.h)
            ),
        );
        let fid = self.image_filter(n, &spec);
        let fattr = fid.map(|f| format!(" filter=\"url(#{f})\"")).unwrap_or_default();
        let op = img_opacity(&spec, style);
        let oattr = if op < 1.0 { format!(" opacity=\"{}\"", nf(op)) } else { String::new() };
        Ok(format!(
            "<image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" href=\"{uri}\" xlink:href=\"{uri}\" preserveAspectRatio=\"none\" clip-path=\"url(#{clip})\"{oattr}{fattr}/>\n",
            nf(whole.x), nf(whole.y), nf(whole.w), nf(whole.h)
        ))
    }

    /// brightness / contrast / saturate / hue / grayscale / blur / tint as a real
    /// SVG filter chain, so a browser and resvg apply the same maths.
    fn image_filter(&mut self, n: &Node, spec: &crate::doc::ImageSpec) -> Option<String> {
        let b = n.box_;
        let mut prims = String::new();
        let mut base = "SourceGraphic".to_string();
        let mut pad = 0.0f64;

        if let Some(v) = spec.blur.filter(|v| *v > 0.0) {
            prims.push_str(&format!(
                "<feGaussianBlur in=\"{base}\" stdDeviation=\"{}\" result=\"i-blur\"/>",
                nf(v / 2.0)
            ));
            base = "i-blur".into();
            pad = pad.max(v * 1.5);
        }
        let br = spec.brightness.filter(|v| *v != 1.0);
        let co = spec.contrast.filter(|v| *v != 1.0);
        if br.is_some() || co.is_some() {
            // CSS filter(x) == linear transfer: out = slope*in + intercept, with
            // contrast's pivot at mid grey.
            let slope = br.unwrap_or(1.0);
            let c = co.unwrap_or(1.0);
            let intercept = 0.5 * (1.0 - c) * slope;
            let f = format!("type=\"linear\" slope=\"{}\" intercept=\"{}\"", nf(slope * c), nf(intercept));
            prims.push_str(&format!(
                "<feComponentTransfer in=\"{base}\" result=\"i-lum\">\
<feFuncR {f}/><feFuncG {f}/><feFuncB {f}/><feFuncA type=\"identity\"/></feComponentTransfer>"
            ));
            base = "i-lum".into();
        }
        if let Some(s) = spec.saturate.filter(|v| *v != 1.0) {
            prims.push_str(&format!(
                "<feColorMatrix in=\"{base}\" type=\"saturate\" values=\"{}\" result=\"i-sat\"/>",
                nf(s)
            ));
            base = "i-sat".into();
        }
        if let Some(h) = spec.hue.filter(|v| v.abs() > 1e-6) {
            prims.push_str(&format!(
                "<feColorMatrix in=\"{base}\" type=\"hueRotate\" values=\"{}\" result=\"i-hue\"/>",
                nf(h)
            ));
            base = "i-hue".into();
        }
        if spec.grayscale.unwrap_or(false) {
            prims.push_str(&format!(
                "<feColorMatrix in=\"{base}\" type=\"saturate\" values=\"0\" result=\"i-gray\"/>"
            ));
            base = "i-gray".into();
        }
        if let Some(t) = &spec.tint {
            // Tint = luminance remapped into the tint colour, alpha preserved.
            let (hex, a) = self.color(t);
            let (r, g, bl) = hex_channels(&hex);
            let (lr, lg, lb) = (0.2126, 0.7152, 0.0722);
            prims.push_str(&format!(
                "<feColorMatrix in=\"{base}\" type=\"matrix\" values=\"\
{} {} {} 0 0 {} {} {} 0 0 {} {} {} 0 0 0 0 0 {} 0\" result=\"i-tint\"/>",
                nf(r as f64 / 255.0 * lr), nf(r as f64 / 255.0 * lg), nf(r as f64 / 255.0 * lb),
                nf(g as f64 / 255.0 * lr), nf(g as f64 / 255.0 * lg), nf(g as f64 / 255.0 * lb),
                nf(bl as f64 / 255.0 * lr), nf(bl as f64 / 255.0 * lg), nf(bl as f64 / 255.0 * lb),
                nf(a)
            ));
            base = "i-tint".into();
        }
        if prims.is_empty() {
            return None;
        }
        let id = self.did(&n.id, "img-fx");
        let r = Rect::new(b.x - pad, b.y - pad, b.w + 2.0 * pad, b.h + 2.0 * pad);
        self.push_def(
            &id.clone(),
            &format!(
                "<filter id=\"{id}\" filterUnits=\"userSpaceOnUse\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\">{}</filter>",
                nf(r.x), nf(r.y), nf(r.w), nf(r.h), prims
            ),
        );
        Some(id)
    }

    // ------------------------------------------------------------- svg node

    fn svg_node(&mut self, n: &Node) -> R<Option<String>> {
        let Some(v) = n.vector.as_ref().cloned() else {
            self.warn(format!("node \"{}\": svg node with no vector spec", n.id));
            return Ok(None);
        };
        let raw = match (&v.inline, &v.asset) {
            (Some(s), _) => Some(s.clone()),
            (None, Some(id)) => {
                let asset = match self.doc.asset(id) {
                    Some(a) => a.clone(),
                    None => {
                        self.warn(format!("node \"{}\": svg asset \"{id}\" is unknown", n.id));
                        return Ok(None);
                    }
                };
                match self.asset_path(&asset).and_then(|p| std::fs::read_to_string(p).ok()) {
                    Some(s) => Some(s),
                    None => {
                        self.warn(format!(
                            "node \"{}\": svg asset \"{}\" ({}) is missing from disk",
                            n.id, id, asset.uri
                        ));
                        return Ok(None);
                    }
                }
            }
            _ => None,
        };
        let Some(raw) = raw else {
            self.warn(format!("node \"{}\": no svg markup to inline", n.id));
            return Ok(None);
        };
        let clean = sanitize_svg(&raw);
        let Some((attrs, inner)) = root_element(&clean) else {
            self.warn(format!("node \"{}\": imported svg has no readable root <svg>", n.id));
            return Ok(None);
        };
        let b = n.box_;
        let vb = v.viewBox.map(|a| (a[0], a[1], a[2], a[3])).or_else(|| {
            attrs
                .iter()
                .find(|(k, _)| k == "viewbox")
                .and_then(|(_, val)| parse_four(val))
        });
        let (_, inner) = rename_ids(inner, &self.did(&n.id, "i"));
        let (vx, vy, vw, vh) = vb.unwrap_or((0.0, 0.0, b.w, b.h));
        let (vw, vh) = (if vw > 0.0 { vw } else { b.w }, if vh > 0.0 { vh } else { b.h });
        Ok(Some(format!(
            "<svg x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" viewBox=\"{} {} {} {}\" preserveAspectRatio=\"xMidYMid meet\" overflow=\"hidden\">{}</svg>\n",
            nf(b.x), nf(b.y), nf(b.w), nf(b.h), nf(vx), nf(vy), nf(vw), nf(vh), inner
        )))
    }
}

/// Solves the line breaks. `doc` is here so a `$font` token can supply the family;
/// `warnings` collects what the solver said about overflow and fallback.
fn solve_lines_with(
    doc: &Document,
    n: &Node,
    t: &crate::doc::TextSpec,
    warnings: &mut Vec<String>,
    content: &str,
) -> (Vec<String>, Option<crate::text::TextMetrics>) {
    let size = t.size.unwrap_or(16.0);
    let m = MeasureOpts {
        family: t
            .family
            .clone()
            .or_else(|| t.font_ref.as_ref().and_then(|r| doc.tokens.font(r).map(|f| f.family.clone())))
            .unwrap_or_else(|| "sans-serif".into()),
        weight: t.weight.unwrap_or(400),
        italic: t.style_italic.unwrap_or(false),
        size,
        ls: t.letter_spacing.unwrap_or(0.0),
        line_h: t.line_h(),
        max_w: (n.box_.w - 2.0 * t.padding.unwrap_or(0.0).max(0.0)).max(0.0),
        align: t.align.clone().unwrap_or_else(|| "left".into()),
        valign: t.valign.clone().unwrap_or_else(|| "top".into()),
    };
    let mt = layout_text(content, &m, &t.overflow.clone().unwrap_or_else(|| "clip".into()));
    if mt.overflow {
        warnings.push(format!(
            "node \"{}\": text overflows its box (lines are {}px tall in a {}px box)",
            n.id, nf(mt.height), nf(n.box_.h)
        ));
    }
    if mt.truncated {
        warnings.push(format!("node \"{}\": text was truncated by the solver", n.id));
    }
    if mt.fell_back {
        warnings.push(format!("node \"{}\": font fell back to \"{}\"", n.id, mt.font));
    }
    (mt.lines.clone(), Some(mt))
}

fn solve_lines(doc: &Document, n: &Node, t: &crate::doc::TextSpec, w: &mut Vec<String>) -> (Vec<String>, Option<crate::text::TextMetrics>) {
    solve_lines_with(doc, n, t, w, &t.content)
}

// ------------------------------------------------------------------ helpers

fn uniform(r: &Radius) -> bool {
    r.uniform()
}

/// Geometry inside a `<mask>` must be black; strip whatever paint the caller added.
fn black_geom(geom: &str) -> String {
    let mut s = geom.replace(" fill=\"none\"", " fill=\"#000000\"");
    if !s.contains("fill=") {
        s = splice(&s, " fill=\"#000000\"");
    }
    s
}

/// Splices attributes into a self-closed geometry element.
fn splice(geom: &str, attrs: &str) -> String {
    let g = geom.trim_end();
    match g.strip_suffix("/>") {
        Some(head) => format!("{}{attrs}/>", head.trim_end()),
        None => g.to_string(),
    }
}

fn img_opacity(spec: &crate::doc::ImageSpec, style: &Style) -> f64 {
    let mut o = spec.opacity.unwrap_or(1.0);
    if let Some(Paint::Image { opacity, .. }) = &style.fill {
        o *= *opacity;
    }
    util::clamp(o, 0.0, 1.0)
}

fn hex_channels(hex: &str) -> (u8, u8, u8) {
    parse_color(hex).map(|(r, g, b, _)| (r, g, b)).unwrap_or((128, 128, 128))
}

fn parse_four(s: &str) -> Option<(f64, f64, f64, f64)> {
    let v: Vec<f64> = s
        .replace(',', " ")
        .split_whitespace()
        .filter_map(|x| x.parse::<f64>().ok())
        .collect();
    if v.len() == 4 {
        Some((v[0], v[1], v[2], v[3]))
    } else {
        None
    }
}

/// A line's endpoints in local box space: two explicit points, then `line_end` plus
/// the box's left-middle, then a sensible divider across the box.
fn line_ends(n: &Node, b: Rect) -> (f64, f64, f64, f64) {
    if let Some(p) = n.vector.as_ref().filter(|p| p.points.len() >= 2) {
        return (p.points[0][0], p.points[0][1], p.points[1][0], p.points[1][1]);
    }
    if let Some(e) = n.vector.as_ref().and_then(|x| x.line_end) {
        return (0.0, b.h / 2.0, e[0], e[1]);
    }
    if b.h > b.w {
        (b.w / 2.0, 0.0, b.w / 2.0, b.h)
    } else {
        (0.0, b.h / 2.0, b.w, b.h / 2.0)
    }
}

/// Regular polygon inscribed in the box (first vertex at 12 o'clock), or the author's
/// explicit points, offset from the box's top-left.
fn polygon_points(n: &Node, b: Rect) -> Vec<(f64, f64)> {
    if let Some(p) = n.vector.as_ref().filter(|p| p.points.len() >= 3) {
        return p.points.iter().map(|x| (b.x + x[0], b.y + x[1])).collect();
    }
    let sides = n
        .vector
        .as_ref()
        .and_then(|x| x.sides)
        .filter(|s| *s >= 3)
        .unwrap_or(3) as usize;
    let (cx, cy) = (b.cx(), b.cy());
    let (rx, ry) = (b.w / 2.0, b.h / 2.0);
    (0..sides)
        .map(|i| {
            let a = (-90.0 + i as f64 * 360.0 / sides as f64).to_radians();
            (cx + rx * a.cos(), cy + ry * a.sin())
        })
        .collect()
}

/// Rounded-rect path with independent corners (arcs, so it survives any renderer).
fn corner_path(b: Rect, r: &Radius) -> String {
    let mut rad = r.of();
    let cap = (b.w.min(b.h) / 2.0).max(0.0);
    for x in rad.iter_mut() {
        *x = x.clamp(0.0, cap);
    }
    let [tl, tr, br, bl] = rad;
    let (x, y, w, h) = (b.x, b.y, b.w, b.h);
    let arc = |v: f64, to: String| {
        if v <= 0.0 {
            to
        } else {
            format!("a {} {} 0 0 1 {}", nf(v), nf(v), to)
        }
    };
    let p = |v: f64| nf(v);
    // Clockwise from the top edge: each corner is `L <edge end>` followed by its arc,
    // and `arc` degrades to the bare end point when the radius is 0.
    format!(
        "M {tstart} {top} L {tend} {top} {a_tr} L {right} {rstart} {a_br} \
         L {bend} {bot} {a_bl} L {left} {lstart} {a_tl} Z",
        tstart = p(x + tl),
        top = p(y),
        tend = p(x + w - tr),
        a_tr = arc(tr, format!("{} {}", p(x + w), p(y + tr))),
        right = p(x + w),
        rstart = p(y + h - br),
        a_br = arc(br, format!("{} {}", p(x + w - br), p(y + h))),
        bend = p(x + bl),
        bot = p(y + h),
        a_bl = arc(bl, format!("{} {}", p(x), p(y + h - bl))),
        left = p(x),
        lstart = p(y + tl),
        a_tl = arc(tl, format!("{} {}", p(x + tl), p(y))),
    )
}

/// CSS convention: `0deg` points to the top and angles grow clockwise. The gradient
/// line is the projection of the box onto that direction, so a corner-to-corner
/// diagonal is 45deg like a browser would draw it.
fn linear_endpoints(b: Rect, angle: f64) -> (f64, f64, f64, f64) {
    let a = angle.to_radians();
    let (dx, dy) = (a.sin(), -a.cos());
    let t = 0.5 * (b.w * dx.abs() + b.h * dy.abs());
    let (cx, cy) = (b.cx(), b.cy());
    (cx - dx * t, cy - dy * t, cx + dx * t, cy + dy * t)
}

fn par(f: Fit) -> &'static str {
    match f {
        Fit::Contain => "xMidYMid meet",
        Fit::Cover => "xMidYMid slice",
        Fit::Fill => "none",
        Fit::Actual => "xMidYMid meet",
    }
}

fn font_list(resolved: &str, want: &str, fallback: &str) -> String {
    let mut v: Vec<String> = Vec::new();
    for f in [resolved, want, fallback]
        .iter()
        .flat_map(|s| s.split(','))
        .map(|s| s.trim().trim_matches('"').trim_matches('\''))
        .filter(|s| !s.is_empty() && !matches!(*s, "sans-serif" | "serif" | "monospace" | "cursive" | "fantasy"))
    {
        if !v.iter().any(|x| x == f) {
            v.push(f.to_string());
        }
    }
    v.push("sans-serif".to_string());
    v.join(", ")
}

/// Ids are namespaced by board + node, and foreign characters are folded away so the
/// SVG stays valid and byte-stable no matter what an agent named a layer.
pub fn eid(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else {
            out.push('-');
        }
    }
    let t = out.trim_matches('-');
    if t.is_empty() || t.as_bytes()[0].is_ascii_digit() {
        format!("n-{t}")
    } else {
        t.to_string()
    }
}

/// XML-escapes text and attribute values: `& < > " '`.
pub fn esc(s: &str) -> String {
    if !s.contains(['&', '<', '>', '"', '\'']) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

fn urlenc(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Numbers on the wire: three decimals, no trailing zeros, never `-0`.
pub fn nf(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    let r = (v * 1000.0).round() / 1000.0;
    if r == 0.0 {
        return "0".to_string();
    }
    if r.fract() == 0.0 {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

/// An asset uri is project-relative, but absolute paths are honoured so a document
/// can point at a shared library.
fn resolve_asset_path(uri: &str, dir: Option<&Path>) -> Option<PathBuf> {
    let p = Path::new(uri);
    if p.is_absolute() {
        return if p.exists() { Some(p.to_path_buf()) } else { None };
    }
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Some(d) = dir {
        cands.push(d.join(uri));
    }
    cands.push(util::root(None).join(uri));
    cands.push(PathBuf::from(uri));
    cands.into_iter().find(|c| c.exists())
}

fn mime_from_path(p: &Path) -> String {
    let m = match util::ext(p).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    };
    m.to_string()
}

// ------------------------------------------------------------------ sanitiser

/// Elements whose whole subtree is dropped: they can run code, fetch, or restyle.
const DROP_SUBTREE: &[&str] = &[
    "script",
    "foreignobject",
    "iframe",
    "object",
    "embed",
    "audio",
    "video",
    "style",
    "handler",
];
/// Animation elements: dropped at the tag only, they are leaf-ish and a static
/// export must not depend on them.
const DROP_TAGS: &[&str] = &["animate", "animatemotion", "animatetransform", "set"];

/// Makes imported SVG safe to inline.
pub fn sanitize_svg(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut i = 0usize;
    while i < src.len() {
        let at = src[i..].find('<').map(|k| i + k);
        let Some(at) = at else {
            out.push_str(&src[i..]);
            break;
        };
        out.push_str(&src[i..at]);
        let rest = &src[at..];
        if rest.starts_with("<!--") {
            i = rest.find("-->").map(|k| at + k + 3).unwrap_or(src.len());
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            // <!DOCTYPE ...>, <!ENTITY ...>, <?xml ...?>: a DTD resvg would honour
            // is an entity-expansion bomb, so none of it survives.
            i = rest.find('>').map(|k| at + k + 1).unwrap_or(src.len());
            continue;
        }
        if rest.starts_with("</") {
            let rel = rest.find('>').map(|k| k + 1).unwrap_or(rest.len());
            let name = tag_name(&rest[2..]);
            if !is_dropped(name, DROP_SUBTREE) && !is_dropped(name, DROP_TAGS) {
                out.push_str(&rest[..rel]);
            }
            i = at + rel;
            continue;
        }
        let Some(rel_end) = find_tag_end(rest) else {
            // Unterminated tag: stop rather than invent structure.
            break;
        };
        let raw = &rest[..rel_end];
        let end = at + rel_end;
        let name = tag_name(&raw[1..]);
        if is_dropped(name, DROP_SUBTREE) {
            // skip_element answers relative to `rest`, the loop index is absolute.
            i = at + skip_element(rest, name, rel_end);
            continue;
        }
        if is_dropped(name, DROP_TAGS) {
            i = end;
            continue;
        }
        out.push_str(&clean_attrs(raw));
        i = end;
    }
    out
}

fn is_dropped(name: &str, list: &[&str]) -> bool {
    let n = name.to_lowercase();
    let n = n.split(':').next_back().unwrap_or(&n);
    list.contains(&n)
}

fn tag_name(s: &str) -> &str {
    let t = s.trim_start_matches(['/', ' ', '\t', '\n', '\r']);
    let end = t
        .find(|c: char| c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '/' || c == '>')
        .unwrap_or(t.len());
    &t[..end]
}

/// Index just past this tag's `>`, honouring quoted attribute values.
fn find_tag_end(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut q: Option<u8> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        match q {
            Some(c) => {
                if bytes[i] == c {
                    q = None;
                }
            }
            None => match bytes[i] {
                b'"' | b'\'' => q = Some(bytes[i]),
                b'>' => return Some(i + 1),
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// Index just past the matching close tag of an element that starts at 0 of `s` and
/// whose open tag ends at `open_end`.
fn skip_element(s: &str, name: &str, open_end: usize) -> usize {
    let lower = name.to_lowercase();
    let mut depth = 1usize;
    let mut i = open_end;
    while i < s.len() {
        let Some(at) = s[i..].find('<').map(|k| i + k) else {
            return s.len();
        };
        let rest = &s[at..];
        let Some(rel) = find_tag_end(rest) else {
            return s.len();
        };
        let raw = &rest[..rel];
        if raw.starts_with("</") {
            if tag_name(&raw[2..]).to_lowercase() == lower {
                depth -= 1;
                if depth == 0 {
                    return at + rel;
                }
            }
        } else if tag_name(&raw[1..]).to_lowercase() == lower && !raw.trim_end().ends_with("/>") {
            depth += 1;
        }
        i = at + rel;
    }
    s.len()
}

/// Rebuilds a start tag keeping only safe attributes.
fn clean_attrs(raw: &str) -> String {
    let self_closed = raw.trim_end().ends_with("/>");
    let inner = &raw[1..raw.len() - if self_closed { 2 } else { 1 }];
    let name = tag_name(inner);
    let mut out = format!("<{name}");
    for (k, v) in parse_attrs(&inner[name.len()..]) {
        let lk = k.to_lowercase();
        let local = lk.split(':').next_back().unwrap_or(&lk).to_string();
        let lval = v.trim().to_lowercase();
        let remote_ref = matches!(local.as_str(), "href" | "xlink:href" | "src" | "background")
            && !lval.starts_with('#')
            && !lval.starts_with("data:image");
        let keep = !local.starts_with("on")
            && !matches!(local.as_str(), "formaction" | "srcdoc" | "xlink:href" | "href" | "src" | "background")
            && !remote_ref
            && !(local == "style" && (lval.contains("@import") || lval.contains("expression(")))
            && !lval.starts_with("javascript:");
        // href is legitimate on `use`/`a` inside an icon; keep only local refs there.
        let keep = keep
            || ((local == "href" || local == "xlink:href") && lval.starts_with('#'));
        if keep {
            out.push_str(&format!(" {k}=\"{}\"", esc(v.trim())));
        }
    }
    out.push_str(if self_closed { "/>" } else { ">" });
    out
}

fn parse_attrs(s: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        i += whitespace(&b[i..]);
        let start = i;
        while i < b.len() && b[i] != b'=' && b[i] != b'>' && whitespace(&b[i..]) == 0 {
            i += 1;
        }
        let key = &s[start..i];
        if key.is_empty() {
            break;
        }
        i += whitespace(&b[i..]);
        if b.get(i) == Some(&b'=') {
            i += 1;
            i += whitespace(&b.get(i..).unwrap_or(b""));
            match b.get(i) {
                Some(q @ (b'"' | b'\'')) => {
                    i += 1;
                    let vs = i;
                    while i < b.len() && b[i] != *q {
                        i += 1;
                    }
                    out.push((key, &s[vs..i]));
                    i += 1;
                }
                _ => {
                    let vs = i;
                    while i < b.len() && b[i] != b'>' && whitespace(&b[i..]) == 0 {
                        i += 1;
                    }
                    out.push((key, &s[vs..i]));
                }
            }
        } else {
            out.push((key, ""));
        }
    }
    out
}

fn whitespace(b: &[u8]) -> usize {
    let mut n = 0usize;
    while n < b.len() && (b[n] as char).is_whitespace() {
        n += 1;
    }
    n
}

/// Namespaces ids inside an imported fragment so it cannot collide with the defs
/// generated here, and rewrites every internal reference to follow.
fn rename_ids(inner: String, ns: &str) -> (Option<(f64, f64, f64, f64)>, String) {
    let prefix = format!("{}-", eid(ns));
    let mut map: Vec<(String, String)> = Vec::new();
    let mut i = 0usize;
    while i < inner.len() {
        let at = match inner[i..].find("id=\"").map(|k| i + k) {
            Some(k) => k,
            None => break,
        };
        let vs = at + 4;
        let ve = inner[vs..].find('"').map(|k| vs + k).unwrap_or(inner.len());
        let old = &inner[vs..ve];
        if !old.is_empty() && !map.iter().any(|(o, _)| o == old) {
            map.push((old.to_string(), format!("{prefix}{}", eid(old))));
        }
        i = ve + 1;
    }
    let mut out = inner;
    // Longest first, so a name that prefixes another cannot half-match.
    let mut sorted = map.clone();
    sorted.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    for (old, new) in &sorted {
        for (pat, repl) in [
            (format!("id=\"{old}\""), format!("id=\"{new}\"")),
            (format!("href=\"#{old}\""), format!("href=\"#{new}\"")),
            (format!("xlink:href=\"#{old}\""), format!("xlink:href=\"#{new}\"")),
            (format!("url(#{old})"), format!("url(#{new})")),
            (format!("url(#{old} )"), format!("url(#{new})")),
        ] {
            out = out.replace(&pat, &repl);
        }
    }
    (None, out)
}

/// First `<svg>` element of a fragment: its attributes and its inner markup.
fn root_element(src: &str) -> Option<(Vec<(String, String)>, String)> {
    let at = find_tag_start(src, "svg")?;
    let rest = &src[at..];
    let open_end = find_tag_end(rest)?;
    let raw = &rest[..open_end];
    let self_closed = raw.trim_end().ends_with("/>");
    let inner_span = &raw[4..raw.len() - if self_closed { 2 } else { 1 }];
    let attrs: Vec<(String, String)> = parse_attrs(inner_span)
        .into_iter()
        .map(|(k, v)| (k.to_lowercase(), v.to_string()))
        .collect();
    if self_closed {
        return Some((attrs, String::new()));
    }
    let name = tag_name(&raw[1..]);
    let close = skip_element(rest, name, open_end);
    // `close` is past `</svg>`; walk back to the start of the close tag.
    let body = &rest[open_end..close];
    let cut = body.rfind("</").unwrap_or(body.len());
    Some((attrs, body[..cut].to_string()))
}

fn find_tag_start(s: &str, name: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'<' && bytes[i + 1] != b'/' && bytes[i + 1] != b'!' && bytes[i + 1] != b'?' {
            if tag_name(&s[i + 1..]).eq_ignore_ascii_case(name) {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// A stable, compact description of the compile options, handy in RPC responses.
pub fn opts_value(o: &SvgOpts) -> Value {
    o.to_value()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rounded-rect path is written by hand, so its edges and arcs are checked
    /// one by one: clockwise from the top edge, each corner an `L` then its arc.
    #[test]
    fn corner_path_writes_every_edge_and_arc() {
        let b = Rect::new(10.0, 20.0, 100.0, 50.0);
        let r = Radius {
            all: 0.0,
            tl: Some(5.0),
            tr: Some(10.0),
            br: Some(15.0),
            bl: Some(20.0),
        };
        assert_eq!(
            corner_path(b, &r),
            "M 15 20 L 100 20 a 10 10 0 0 1 110 30 L 110 55 a 15 15 0 0 1 95 70 \
             L 30 70 a 20 20 0 0 1 10 50 L 10 25 a 5 5 0 0 1 15 20 Z"
        );
        // a zero radius keeps the corner as a plain point on the edge, not an arc
        let sharp = Radius { all: 0.0, tl: None, tr: None, br: None, bl: None };
        assert_eq!(
            corner_path(b, &sharp),
            "M 10 20 L 110 20 110 20 L 110 70 110 70 L 10 70 10 70 L 10 20 10 20 Z"
        );
    }

    /// A gradient, pattern or translucent colour all report their alpha the same
    /// way: the second half of the pair is the matching `-opacity` attribute.
    #[test]
    fn every_paint_reports_opacity_on_the_right_key() {
        let doc = Document::default();
        let o = SvgOpts::default();
        let mut c = C::new(&doc, &o, None);
        c.board = "b1".to_string();
        let b = Rect::from_size(200.0, 100.0);
        let stops = vec![
            Stop { at: 0.0, color: "#ffffff".into() },
            Stop { at: 1.0, color: "#000000".into() },
        ];
        let grad = Paint::Linear { stops, angle: 90.0 };
        let (fill, extra) = c.paint_attrs("n1", &grad, b, 0.5, true);
        assert!(fill.starts_with(" fill=\"url(#"), "{fill}");
        assert_eq!(extra, " fill-opacity=\"0.5\"");
        let (stroke, sextra) = c.paint_attrs("n2", &grad, b, 0.25, false);
        assert!(stroke.starts_with(" stroke=\"url(#"), "{stroke}");
        assert_eq!(sextra, " stroke-opacity=\"0.25\"");
        // full opacity writes nothing at all, and a translucent solid behaves the same
        let (_, none) = c.paint_attrs("n3", &grad, b, 1.0, true);
        assert_eq!(none, "");
        let (_, solid) = c.paint_attrs("n4", &Paint::Solid { color: "#ff000080".into() }, b, 1.0, true);
        assert_eq!(solid, " fill-opacity=\"0.502\"");
        // the gradient def itself landed in <defs>, namespaced by board and node
        assert!(c.defs.contains("<linearGradient id=\"ac-b1-n1-lg\""), "{}", c.defs);
    }

    /// A real imported file has prose before every tag, so the scanner's indices are
    /// relative to the slice after `<`. These cases each put multi-byte text in front
    /// of a construct the sanitizer handles differently, which is what used to panic.
    #[test]
    fn sanitize_keeps_structure_and_drops_executable_content() {
        let src = "\u{feff}<!-- 品牌标识 -->\n<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 200 60\">\
                   <script>alert(1)</script>\n  文字\
                   <image href=\"https://evil.example/x.png\" width=\"10\"/>\
                   <text x=\"16\" y=\"40\" onclick=\"steal()\">品牌</text>\
                   </svg>";
        let out = sanitize_svg(src);
        assert!(out.contains("</svg>"), "close tags survive: {out}");
        assert!(!out.contains("alert"), "the script subtree is gone: {out}");
        assert!(!out.contains("evil.example"), "a remote href is stripped: {out}");
        assert!(!out.contains("onclick"), "an event handler is stripped: {out}");
        assert!(out.contains("品牌"), "the text content is kept: {out}");
        assert_eq!(out, sanitize_svg(&out), "sanitising twice changes nothing");
    }

    /// Truncated and mismatched markup must not index off a character boundary.
    #[test]
    fn sanitize_survives_broken_markup() {
        for src in [
            "文字文字<",
            "文字</",
            "文字</svg",
            "文字<!-- 未闭合",
            "文字<!DOCTYPE svg",
            "文字<?xml version=\"1.0\"",
            "文字<script><script>x</script>",
            "文字<text>未闭合",
            "文字<rect/",
        ] {
            let out = sanitize_svg(src);
            assert!(out.starts_with("文字"), "leading text is never eaten: {out}");
        }
    }
}
