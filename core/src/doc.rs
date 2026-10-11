//! The versioned scene graph. This is the single source of truth shared by the
//! desktop UI, the HTTP API and MCP: every entry point reads and writes *this*
//! structure, and every visual is a projection of it.
//!
//! Conventions worth knowing before editing a document by hand:
//! - Units are CSS pixels. Colours are `#RRGGBB`, `#RRGGBBAA` or `rgb()/rgba()`.
//! - `nodes` is a flat map keyed by `id`; parent/child order lives in `children`
//!   and in each artboard's `layers`. Index 0 is the *bottom* of the stack.
//! - `layout.intent` is what the author asked for; `layout.box` is the solved
//!   result. Manual edits set `mode = "manual"` so a later reflow cannot silently
//!   move something a human placed.

use crate::geom::{AlignX, AlignY, Fit, Origin, Rect};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: &str = "1.1.0";

// ------------------------------------------------------------------ document

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Document {
    #[serde(default = "default_version")]
    pub schema_version: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub tokens: Tokens,
    #[serde(default)]
    pub assets: Vec<Asset>,
    #[serde(default)]
    pub artboards: Vec<Artboard>,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub meta: Meta,
}

fn default_version() -> String {
    SCHEMA_VERSION.to_string()
}

impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION.into(),
            project_id: String::new(),
            revision: 0,
            name: "Untitled".into(),
            tokens: Tokens::default(),
            assets: vec![],
            artboards: vec![],
            nodes: vec![],
            meta: Meta::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Meta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Which client last touched it: `desktop`, `mcp`, `http`, `cli`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dirty: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Tokens {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<ColorToken>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fonts: Vec<FontToken>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spacing: Vec<NumToken>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub radii: Vec<NumToken>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ColorToken {
    pub name: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NumToken {
    pub name: String,
    pub value: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FontToken {
    pub name: String,
    pub family: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub letter_spacing: Option<f64>,
}

impl Tokens {
    pub fn color(&self, ref_or_value: &str) -> Option<String> {
        let t = ref_or_value.trim();
        if !t.starts_with('$') {
            return None;
        }
        let key = &t[1..];
        self.colors.iter().find(|c| c.name == key).map(|c| c.value.clone())
    }
    pub fn font(&self, ref_or_value: &str) -> Option<&FontToken> {
        let t = ref_or_value.trim();
        if !t.starts_with('$') {
            return None;
        }
        let key = &t[1..];
        self.fonts.iter().find(|f| f.name == key)
    }
    /// Substitute `$token` references anywhere inside a JSON-ish value string.
    pub fn expand(&self, s: &str) -> String {
        if !s.contains('$') {
            return s.to_string();
        }
        let mut out = s.to_string();
        for _ in 0..4 {
            let Some(a) = out.find('$') else { break };
            let rest = &out[a + 1..];
            let end = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.'))
                .unwrap_or(rest.len());
            if end == 0 {
                break;
            }
            let key = &rest[..end];
            let repl = self
                .colors
                .iter()
                .find(|c| c.name == key)
                .map(|c| c.value.clone())
                .or_else(|| {
                    self.spacing.iter().find(|n| n.name == key).map(|n| n.value.to_string())
                })
                .or_else(|| self.radii.iter().find(|n| n.name == key).map(|n| n.value.to_string()));
            match repl {
                Some(v) => out.replace_range(a..a + 1 + end, &v),
                None => break,
            }
        }
        out
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Asset {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Path relative to the project directory, e.g. `assets/product.png`.
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub height: f64,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orientation: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Artboard {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub width: f64,
    pub height: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Paint>,
    #[serde(default = "default_true")]
    pub visible: bool,
    /// Top-level node ids, bottom first.
    #[serde(default)]
    pub layers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_margin: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guides: Vec<Guide>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    /// Variables filled from a template; used by `template.apply_variables`.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub vars: serde_json::Map<String, serde_json::Value>,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Guide {
    /// "v" or "h"
    pub axis: String,
    pub pos: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

// ------------------------------------------------------------------ nodes

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    Text,
    Image,
    Rect,
    Ellipse,
    Line,
    Polygon,
    Path,
    Group,
    Frame,
    Svg,
}

impl NodeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeType::Text => "text",
            NodeType::Image => "image",
            NodeType::Rect => "rect",
            NodeType::Ellipse => "ellipse",
            NodeType::Line => "line",
            NodeType::Polygon => "polygon",
            NodeType::Path => "path",
            NodeType::Group => "group",
            NodeType::Frame => "frame",
            NodeType::Svg => "svg",
        }
    }
    pub fn parse(s: &str) -> Option<NodeType> {
        Some(match s.to_lowercase().as_str() {
            "text" | "t" => NodeType::Text,
            "image" | "img" | "i" => NodeType::Image,
            "rect" | "r" | "box" => NodeType::Rect,
            "ellipse" | "circle" | "e" | "oval" => NodeType::Ellipse,
            "line" | "l" | "rule" | "divider" => NodeType::Line,
            "polygon" | "triangle" | "poly" => NodeType::Polygon,
            "path" | "p" => NodeType::Path,
            "group" | "g" => NodeType::Group,
            "frame" | "container" | "f" => NodeType::Frame,
            "svg" => NodeType::Svg,
            _ => return None,
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Node {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: NodeType,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artboard: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,

    /// Solved geometry in artboard coordinates (pre-rotation box). Serialised as
    /// `box`, because that is the key every caller — UI, agent, template — writes.
    /// `box_` stays readable so documents written before the rename still load.
    #[serde(rename = "box", alias = "box_")]
    pub box_: Rect,
    #[serde(default)]
    pub rotation: f64,
    #[serde(default)]
    pub origin: Origin,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub flip_y: bool,

    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default)]
    pub locked: bool,
    /// 0..1
    #[serde(default = "default_one")]
    pub opacity: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// `normal` | `background` | `decor` | `content` | `label`: how the inspector
    /// should judge overlaps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z_role: Option<String>,

    #[serde(default)]
    pub layout: Layout,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<Style>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<TextSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector: Option<VectorSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<Clip>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx: Option<Effects>,
    /// Free-form tag bag, carried through untouched. Handy for agent bookkeeping.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub data: serde_json::Map<String, serde_json::Value>,
}

fn default_one() -> f64 {
    1.0
}

impl Node {
    pub fn new(id: &str, kind: NodeType, r: Rect) -> Self {
        Self {
            id: id.into(),
            kind,
            name: kind.as_str().to_string(),
            artboard: None,
            parent: None,
            children: vec![],
            box_: r,
            rotation: 0.0,
            origin: Origin::Center,
            flip_x: false,
            flip_y: false,
            visible: true,
            locked: false,
            opacity: 1.0,
            blend: None,
            role: None,
            z_role: None,
            layout: Layout::default(),
            style: None,
            text: None,
            image: None,
            vector: None,
            clip: None,
            fx: None,
            data: serde_json::Map::new(),
        }
    }
    pub fn rotated_box(&self) -> Rect {
        if self.rotation.abs() < 1e-9 {
            self.box_
        } else {
            let o = self.origin.point(self.box_);
            self.box_.rotated(self.rotation, o)
        }
    }
}

// ------------------------------------------------------------------ layout

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LayoutMode {
    /// Coordinates are literal; reflow leaves them alone.
    #[default]
    Manual,
    /// Semantic constraints against the artboard or sibling nodes.
    Constraint,
    /// Flex-like container (only meaningful on `frame`).
    Flex,
    /// Grid container.
    Grid,
}

impl LayoutMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            LayoutMode::Manual => "manual",
            LayoutMode::Constraint => "constraint",
            LayoutMode::Flex => "flex",
            LayoutMode::Grid => "grid",
        }
    }
    pub fn parse(s: &str) -> Option<LayoutMode> {
        Some(match s.to_lowercase().as_str() {
            "manual" | "free" | "absolute" | "none" => LayoutMode::Manual,
            "constraint" | "constrain" | "semantic" | "auto" => LayoutMode::Constraint,
            "flex" | "stack" | "row" | "column" => LayoutMode::Flex,
            "grid" => LayoutMode::Grid,
            _ => return None,
        })
    }
}

/// What the author asked for. Every field is optional; the solver applies the
/// ones that are present in a fixed, documented priority order.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Intent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Origin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_top: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_right: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_bottom: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_left: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin: Option<f64>,
    /// Gap used by whichever relation is present (`{"below":"t","gap":90}`), which
    /// is the shorthand the CLI and `AGENTS.md` teach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<f64>,
    /// Relative relations: place this node relative to another node.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "de_rel")]
    pub above: Option<Rel>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "de_rel")]
    pub below: Option<Rel>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "de_rel")]
    pub left_of: Option<Rel>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "de_rel")]
    pub right_of: Option<Rel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center_in: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align_x: Option<AlignX>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align_y: Option<AlignY>,
    /// Align an edge to another node's edge (`{"of":"logo","edge":"left"}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align_to: Option<AlignTo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<SizeIntent>,
    /// Fill the parent/artboard minus padding, optionally `ratio` locked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<Fit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_margin: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub distribute: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equal_gap: Option<EqualGap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_aspect_ratio: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flex: Option<FlexIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<GridIntent>,
    /// Higher wins when two intents disagree. Default 50.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Rel {
    pub of: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<f64>,
    /// `align` the moved node to the reference along the cross axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross: Option<String>,
}

/// `below` accepts either the shorthand `{"below":"headline"}` — which is what
/// `AGENTS.md` teaches agents to write, and what a bare gap-free relation means —
/// or the full object form a template uses.
fn de_rel<'d, D>(d: D) -> Result<Option<Rel>, D::Error>
where
    D: serde::Deserializer<'d>,
{
    let v = serde_json::Value::deserialize(d)?;
    if v.is_null() {
        return Ok(None);
    }
    if let Some(s) = v.as_str() {
        return Ok(Some(Rel { of: s.to_string(), gap: None, cross: None }));
    }
    serde_json::from_value::<Rel>(v).map(Some).map_err(serde::de::Error::custom)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AlignTo {
    pub of: String,
    /// left | center | right | top | middle | bottom
    pub edge: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SizeIntent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<f64>,
    /// Percentage of the artboard, 0..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w_ref: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h_ref: Option<f64>,
    /// `16:9` style ratio lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<f64>,
    /// fixed | fill | hug | min
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_h: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_h: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EqualGap {
    /// x | y
    pub axis: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub between: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct FlexIntent {
    /// row | column
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_gap: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad_top: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad_right: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad_bottom: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad_left: Option<f64>,
    /// start | center | end | space-between | space-around | space-evenly
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub justify: Option<String>,
    /// start | center | end | stretch
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrap: Option<bool>,
    /// hug | fill | fixed container sizing
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sizing: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct GridIntent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Layout {
    #[serde(default)]
    pub mode: LayoutMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<Intent>,
    /// Which intents produced the current box. `layout.explain` reads it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<String>,
    /// Set when a human dragged something: the solver must not move it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
}

// ------------------------------------------------------------------ style

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Paint {
    None,
    Solid { color: String },
    Linear { stops: Vec<Stop>, angle: f64 },
    Radial { stops: Vec<Stop>, cx: f64, cy: f64, r: f64 },
    Image { asset: String, fit: Fit, opacity: f64 },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Stop {
    pub at: f64,
    pub color: String,
}

impl Paint {
    pub fn solid(c: &str) -> Paint {
        Paint::Solid { color: c.to_string() }
    }
    pub fn is_none(&self) -> bool {
        matches!(self, Paint::None)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Style {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Paint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Stroke>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<Radius>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Stroke {
    pub color: String,
    #[serde(default = "default_one")]
    pub width: f64,
    /// inside | center | outside
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dash: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Radius {
    #[serde(default)]
    pub all: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tl: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tr: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub br: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bl: Option<f64>,
}

impl Radius {
    pub fn of(&self) -> [f64; 4] {
        [
            self.tl.unwrap_or(self.all),
            self.tr.unwrap_or(self.all),
            self.br.unwrap_or(self.all),
            self.bl.unwrap_or(self.all),
        ]
    }
    pub fn uniform(&self) -> bool {
        self.tl.is_none() && self.tr.is_none() && self.br.is_none() && self.bl.is_none()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Effects {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shadows: Vec<Shadow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur: Option<f64>,
    /// backdrop blur of what sits *behind* this node, in px (glass look).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backdrop_blur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_glow: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Shadow {
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub blur: f64,
    #[serde(default = "default_shadow_color")]
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inset: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spread: Option<f64>,
}

fn default_shadow_color() -> String {
    "#00000033".into()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Clip {
    /// `node` clips by another node's shape, `rect` by an inset box.
    #[serde(default = "default_clip_kind")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<Rect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
}

fn default_clip_kind() -> String {
    "node".into()
}

// ------------------------------------------------------------------ text

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct TextSpec {
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Absolute px, or use `line_ratio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_ratio: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub letter_spacing: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
    /// top | middle | bottom, inside the box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valign: Option<String>,
    /// fixed | auto-height | auto
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub box_mode: Option<String>,
    /// clip | shrink | scale | overflow
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overflow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_font_size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_font_size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uppercase: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate_each: Option<f64>,
    /// Solved line breaks, written by the engine so the browser and resvg agree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paint: Option<Paint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Stroke>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<Paint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding: Option<f64>,
}

impl TextSpec {
    pub fn line_h(&self) -> f64 {
        let s = self.size.unwrap_or(16.0);
        if let Some(lh) = self.line_height {
            lh
        } else {
            s * self.line_ratio.unwrap_or(1.35)
        }
    }
}

// ------------------------------------------------------------------ image / vector

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImageSpec {
    pub asset: String,
    #[serde(default)]
    pub fit: Fit,
    /// Crop rect in *asset* pixels, applied before fit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<Rect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contrast: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hue: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grayscale: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VectorSpec {
    /// For `path`/`svg`: raw path data or a complete svg fragment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d: Option<String>,
    /// For `polygon`: number of sides (3 = triangle) or explicit point list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sides: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<[f64; 2]>,
    /// Native svg document (asset id or inline markup) for `svg` nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewBox: Option<[f64; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<[f64; 2]>,
}

// ------------------------------------------------------------------ accessors

impl Document {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    pub fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
    pub fn artboard(&self, id: &str) -> Option<&Artboard> {
        self.artboards.iter().find(|a| a.id == id)
    }
    pub fn artboard_mut(&mut self, id: &str) -> Option<&mut Artboard> {
        self.artboards.iter_mut().find(|a| a.id == id)
    }
    pub fn first_artboard(&self) -> Option<&Artboard> {
        self.artboards.iter().find(|a| a.visible)
    }
    pub fn asset(&self, id: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.id == id || a.name == id)
    }
    pub fn artboard_of(&self, n: &Node) -> Option<&Artboard> {
        if let Some(a) = n.artboard.as_deref() {
            if let Some(ab) = self.artboard(a) {
                return Some(ab);
            }
        }
        if let Some(p) = n.parent.as_deref().and_then(|id| self.node(id)) {
            return self.artboard_of(p);
        }
        None
    }
    /// Depth-first paint order for an artboard: bottom first.
    pub fn paint_order(&self, board_id: &str) -> Vec<String> {
        let mut out = vec![];
        let Some(ab) = self.artboard(board_id) else { return out };
        for id in &ab.layers {
            self.walk(id, &mut out);
        }
        out
    }
    fn walk(&self, id: &str, out: &mut Vec<String>) {
        out.push(id.to_string());
        if let Some(n) = self.node(id) {
            for c in &n.children {
                self.walk(c, out);
            }
        }
    }
    pub fn subtree(&self, id: &str) -> Vec<String> {
        let mut out = vec![];
        self.walk(id, &mut out);
        out
    }
    pub fn children_of(&self, parent: Option<&str>) -> Vec<&Node> {
        self.nodes.iter().filter(|n| n.parent.as_deref() == parent).collect()
    }
    /// Union of a node set's rotated boxes.
    pub fn bounds_of(&self, ids: &[String]) -> Option<Rect> {
        let mut acc: Option<Rect> = None;
        for id in ids {
            if let Some(n) = self.node(id) {
                let b = n.rotated_box();
                acc = Some(match acc {
                    None => b,
                    Some(a) => a.union(&b),
                });
            }
        }
        acc
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }
    pub fn to_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }
    pub fn from_json(s: &str) -> crate::R<Document> {
        serde_json::from_str(s).map_err(|e| {
            crate::error::Fail::new("INVALID_SCHEMA", format!("document: {e}"))
                .hint("run project.validate to see the offending node")
        })
    }
}

/// Colour helpers shared by the SVG compiler and the inspector.
/// Accepts `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()` and four names.
pub fn parse_color(s: &str) -> Option<(u8, u8, u8, f64)> {
    let t = s.trim();
    if let Some(h) = t.strip_prefix('#') {
        let d = |c: char| c.to_digit(16).map(|v| v as u8);
        let c: Vec<u8> = h.chars().filter_map(d).collect();
        if c.len() != h.len() {
            return None;
        }
        let pair = |i: usize| c[i] * 16 + c[i + 1];
        return match c.len() {
            3 => Some((c[0] * 17, c[1] * 17, c[2] * 17, 1.0)),
            4 => Some((c[0] * 17, c[1] * 17, c[2] * 17, c[3] as f64 / 255.0)),
            6 => Some((pair(0), pair(2), pair(4), 1.0)),
            8 => Some((pair(0), pair(2), pair(4), pair(6) as f64 / 255.0)),
            _ => None,
        };
    }
    let nums = |inner: &str| -> Vec<f64> {
        inner
            .split(|c| c == ',' || c == '/')
            .filter_map(|x| {
                let y = x.trim();
                y.strip_suffix('%').and_then(|p| p.parse::<f64>().ok()).map(|v| v * 255.0 / 100.0)
                    .or_else(|| y.parse::<f64>().ok())
            })
            .collect()
    };
    if let Some(inner) = t.strip_prefix("rgb(").and_then(|x| x.strip_suffix(')')) {
        let p = nums(inner);
        if p.len() >= 3 {
            return Some((p[0] as u8, p[1] as u8, p[2] as u8, 1.0));
        }
        return None;
    }
    if let Some(inner) = t.strip_prefix("rgba(").and_then(|x| x.strip_suffix(')')) {
        let p = nums(inner);
        if p.len() >= 4 {
            return Some((p[0] as u8, p[1] as u8, p[2] as u8, p[3]));
        }
        return None;
    }
    let named = match t.to_lowercase().as_str() {
        "white" => "#ffffff",
        "black" => "#000000",
        "red" => "#ff0000",
        "transparent" => "#00000000",
        _ => return None,
    };
    parse_color(named)
}

pub fn hex_with_alpha(hex: &str, alpha: f64) -> String {
    match parse_color(hex) {
        Some((r, g, b, a)) => {
            let na = ((a * alpha).clamp(0.0, 1.0) * 255.0).round() as u8;
            format!("#{r:02x}{g:02x}{b:02x}{na:02x}")
        }
        None => hex.to_string(),
    }
}

pub fn contrast_ratio(a: &str, b: &str) -> Option<f64> {
    let l = |c: &str| {
        parse_color(c).map(|(r, g, b, _)| {
            let f = |v: u8| {
                let v = v as f64 / 255.0;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
        })
    };
    let (la, lb) = (l(a)?, l(b)?);
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    Some((hi + 0.05) / (lo + 0.05))
}
