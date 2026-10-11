//! Geometry primitives. All document coordinates are CSS pixels with the origin
//! at the artboard's top-left, y growing downwards, and rotation in degrees
//! clockwise about `Rect::anchor_point(origin)`. One convention, used by the
//! layout solver, the SVG compiler and the inspector alike.

use serde::{Deserialize, Serialize};

pub const DEG: f64 = std::f64::consts::PI / 180.0;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }
    pub fn from_size(w: f64, h: f64) -> Self {
        Self { x: 0.0, y: 0.0, w, h }
    }
    pub fn x2(&self) -> f64 {
        self.x + self.w
    }
    pub fn y2(&self) -> f64 {
        self.y + self.h
    }
    pub fn cx(&self) -> f64 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f64 {
        self.y + self.h / 2.0
    }
    pub fn area(&self) -> f64 {
        self.w.max(0.0) * self.h.max(0.0)
    }
    pub fn ratio(&self) -> f64 {
        if self.h == 0.0 { 1.0 } else { self.w / self.h }
    }
    pub fn left(&self) -> f64 {
        self.x
    }
    pub fn right(&self) -> f64 {
        self.x2()
    }
    pub fn top(&self) -> f64 {
        self.y
    }
    pub fn bottom(&self) -> f64 {
        self.y2()
    }
    pub fn contains(&self, r: &Rect, tol: f64) -> bool {
        r.x >= self.x - tol && r.y >= self.y - tol && r.x2() <= self.x2() + tol && r.y2() <= self.y2() + tol
    }
    pub fn intersects(&self, r: &Rect) -> bool {
        self.x < r.x2() && r.x < self.x2() && self.y < r.y2() && r.y < self.y2()
    }
    pub fn intersection(&self, r: &Rect) -> Option<Rect> {
        let x = self.x.max(r.x);
        let y = self.y.max(r.y);
        let x2 = self.x2().min(r.x2());
        let y2 = self.y2().min(r.y2());
        if x2 > x && y2 > y {
            Some(Rect::new(x, y, x2 - x, y2 - y))
        } else {
            None
        }
    }
    pub fn union(&self, r: &Rect) -> Rect {
        let x = self.x.min(r.x);
        let y = self.y.min(r.y);
        Rect::new(x, y, self.x2().max(r.x2()) - x, self.y2().max(r.y2()) - y)
    }
    pub fn inset(&self, t: f64, r: f64, b: f64, l: f64) -> Rect {
        Rect::new(self.x + l, self.y + t, (self.w - l - r).max(0.0), (self.h - t - b).max(0.0))
    }
    /// Axis-aligned box that encloses this rect rotated `deg` about `origin`.
    pub fn rotated(&self, deg: f64, origin: (f64, f64)) -> Rect {
        if deg.abs() < 1e-9 {
            return *self;
        }
        let (sx, sy) = (deg.sin(), deg.cos());
        let cx = origin.0;
        let cy = origin.1;
        let mut minx = f64::MAX;
        let mut miny = f64::MAX;
        let mut maxx = f64::MIN;
        let mut maxy = f64::MIN;
        for (px, py) in self.corners() {
            let dx = px - cx;
            let dy = py - cy;
            let rx = cx + dx * sy - dy * sx;
            let ry = cy + dx * sx + dy * sy;
            minx = minx.min(rx);
            maxx = maxx.max(rx);
            miny = miny.min(ry);
            maxy = maxy.max(ry);
        }
        Rect::new(minx, miny, maxx - minx, maxy - miny)
    }
    pub fn corners(&self) -> [(f64, f64); 4] {
        [
            (self.x, self.y),
            (self.x2(), self.y),
            (self.x2(), self.y2()),
            (self.x, self.y2()),
        ]
    }
    /// Move so that the box sits at `x,y` while keeping size.
    pub fn with_pos(&self, x: f64, y: f64) -> Rect {
        Rect::new(x, y, self.w, self.h)
    }
    pub fn with_size(&self, w: f64, h: f64) -> Rect {
        Rect::new(self.x, self.y, w, h)
    }
    /// The box's start edge along the given flow direction (`true` = column).
    pub fn main_start(&self, vertical: bool) -> f64 {
        if vertical {
            self.y
        } else {
            self.x
        }
    }
    /// Main-axis extent along the given flow direction.
    pub fn main_size(&self, vertical: bool) -> f64 {
        if vertical {
            self.h
        } else {
            self.w
        }
    }
    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.w.is_finite() && self.h.is_finite()
    }
}

/// Where a node pivots, used for rotation and for `anchor` placement.
/// The aliases are the spellings the handoff schema and the docs use
/// (`top-center`, `bottom-left`, …); canonical output stays the short kebab form.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    #[serde(alias = "nw", alias = "topleft")]
    TopLeft,
    #[serde(alias = "n", alias = "top-center", alias = "north", alias = "topcenter")]
    Top,
    #[serde(alias = "ne", alias = "topright")]
    TopRight,
    #[serde(alias = "w", alias = "center-left", alias = "west", alias = "centerleft")]
    Left,
    #[serde(alias = "c", alias = "middle", alias = "center-center", alias = "centercenter")]
    Center,
    #[serde(alias = "e", alias = "center-right", alias = "east", alias = "centerright")]
    Right,
    #[serde(alias = "sw", alias = "bottomleft")]
    BottomLeft,
    #[serde(alias = "s", alias = "bottom-center", alias = "south", alias = "bottomcenter")]
    Bottom,
    #[serde(alias = "se", alias = "bottomright")]
    BottomRight,
}

impl Default for Origin {
    fn default() -> Self {
        Origin::Center
    }
}

impl Origin {
    pub fn point(self, r: Rect) -> (f64, f64) {
        let (mx, my) = (r.cx(), r.cy());
        match self {
            Origin::TopLeft => (r.x, r.y),
            Origin::Top => (mx, r.y),
            Origin::TopRight => (r.x2(), r.y),
            Origin::Left => (r.x, my),
            Origin::Center => (mx, my),
            Origin::Right => (r.x2(), my),
            Origin::BottomLeft => (r.x, r.y2()),
            Origin::Bottom => (mx, r.y2()),
            Origin::BottomRight => (r.x2(), r.y2()),
        }
    }
    pub fn parse(s: &str) -> Option<Origin> {
        Some(match s {
            "top-left" | "topleft" | "nw" => Origin::TopLeft,
            "top" | "n" | "top-center" | "north" => Origin::Top,
            "top-right" | "topright" | "ne" => Origin::TopRight,
            "left" | "w" | "center-left" | "west" => Origin::Left,
            "center" | "c" | "middle" | "center-center" => Origin::Center,
            "right" | "e" | "center-right" | "east" => Origin::Right,
            "bottom-left" | "bottomleft" | "sw" => Origin::BottomLeft,
            "bottom" | "s" | "bottom-center" | "south" => Origin::Bottom,
            "bottom-right" | "bottomright" | "se" => Origin::BottomRight,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::TopLeft => "top-left",
            Origin::Top => "top",
            Origin::TopRight => "top-right",
            Origin::Left => "left",
            Origin::Center => "center",
            Origin::Right => "right",
            Origin::BottomLeft => "bottom-left",
            Origin::Bottom => "bottom",
            Origin::BottomRight => "bottom-right",
        }
    }
}

/// The nine placement anchors used by `layout.intent.anchor`.
pub type Anchor = Origin;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AlignX {
    Left,
    Center,
    Right,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AlignY {
    Top,
    Middle,
    Bottom,
}

pub fn align_x(v: &str) -> Option<AlignX> {
    Some(match v {
        "left" | "start" => AlignX::Left,
        "center" | "middle" | "c" => AlignX::Center,
        "right" | "end" => AlignX::Right,
        _ => return None,
    })
}

pub fn align_y(v: &str) -> Option<AlignY> {
    Some(match v {
        "top" | "start" => AlignY::Top,
        "middle" | "center" | "c" => AlignY::Middle,
        "bottom" | "end" => AlignY::Bottom,
        _ => return None,
    })
}

/// Snap a value to a step, returning the moved distance only when it is worth it.
pub fn snap(v: f64, step: f64, threshold: f64) -> f64 {
    if step <= 0.0 {
        return v;
    }
    let s = (v / step).round() * step;
    if (s - v).abs() <= threshold {
        s
    } else {
        v
    }
}

/// Common social/print formats, `w x h` in pixels at 1x.
pub fn preset(name: &str) -> Option<(f64, f64, &'static str)> {
    let n = name.trim().to_lowercase().replace(' ', "").replace('_', "-");
    Some(match n.as_str() {
        "poster-3x4" | "poster" | "kv" | "1080x1440" => (1080.0, 1440.0, "海报 3:4"),
        "square" | "1:1" | "ig-square" | "1080x1080" => (1080.0, 1080.0, "方图 1:1"),
        "story" | "9:16" | "1080x1920" | "reel" => (1080.0, 1920.0, "故事 9:16"),
        "portrait-4:5" | "4:5" | "1080x1350" => (1080.0, 1350.0, "竖图 4:5"),
        "banner" | "16:9" | "1920x1080" | "cover" => (1920.0, 1080.0, "横幅 16:9"),
        "x-banner" | "3:4-x" | "1200x1600" => (1200.0, 1600.0, "KV 3:4"),
        "a4" | "a4-portrait" => (2480.0, 3508.0, "A4 竖版"),
        "a4-landscape" => (3508.0, 2480.0, "A4 横版"),
        "a5" => (1748.0, 2480.0, "A5"),
        "wechat" | "wx" | "900x500" => (900.0, 500.0, "公众号头图"),
        "xiaohongshu" | "xhs" | "1242x1660" => (1242.0, 1660.0, "小红书 3:4"),
        "ecom-main" | "800x800" => (800.0, 800.0, "电商主图"),
        "ecom-detail" | "750x1000" => (750.0, 1000.0, "详情竖图"),
        "long" | "long-image" | "750x3000" => (750.0, 3000.0, "图文长图"),
        "card" | "banner-16:5" => (1600.0, 500.0, "卡片横幅"),
        "business-card" => (1050.0, 600.0, "名片"),
        "twitter" | "tweet" => (1600.0, 900.0, "X 卡片"),
        _ => return None,
    })
}

pub fn presets() -> Vec<(&'static str, f64, f64, &'static str)> {
    [
        ("poster", 1080.0, 1440.0, "海报 3:4"),
        ("square", 1080.0, 1080.0, "方图 1:1"),
        ("story", 1080.0, 1920.0, "故事 9:16"),
        ("4:5", 1080.0, 1350.0, "竖图 4:5"),
        ("banner", 1920.0, 1080.0, "横幅 16:9"),
        ("xhs", 1242.0, 1660.0, "小红书 3:4"),
        ("ecom-main", 800.0, 800.0, "电商主图"),
        ("long", 750.0, 3000.0, "图文长图"),
        ("a4", 2480.0, 3508.0, "A4 竖版"),
        ("wechat", 900.0, 500.0, "公众号头图"),
        ("card", 1600.0, 500.0, "卡片横幅"),
        ("business-card", 1050.0, 600.0, "名片"),
    ]
    .to_vec()
}

/// `1:1`, `16:9`, `1080x1440` -> (w, h). `base` is always the **width**, because a
/// series keeps its width and grows downward (1080x1350, 1080x1920), which is what
/// a designer means by "make it a story too".
pub fn parse_ratio(s: &str, base: f64) -> Option<(f64, f64)> {
    let t = s.trim().to_lowercase().replace(' ', "");
    if let Some((a, b)) = t.split_once('x') {
        let (w, h) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
        if w > 0.0 && h > 0.0 {
            return Some((w, h));
        }
        return None;
    }
    let (a, b) = t.split_once(':')?;
    let (ra, rb) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
    if ra <= 0.0 || rb <= 0.0 {
        return None;
    }
    Some((base, base * rb / ra))
}

/// Resize a box to fit another, `contain` keeps it inside, `cover` fills it.
pub fn fit(r: Rect, into: Rect, mode: Fit) -> Rect {
    let (w, h) = (r.w.max(1.0), r.h.max(1.0));
    let sx = into.w / w;
    let sy = into.h / h;
    let s = match mode {
        Fit::Contain => sx.min(sy),
        Fit::Cover => sx.max(sy),
        Fit::Fill => 1.0,
        Fit::Actual => sx.min(sy).min(1.0),
    };
    if matches!(mode, Fit::Fill) {
        return into;
    }
    let nw = w * s;
    let nh = h * s;
    Rect::new(into.cx() - nw / 2.0, into.cy() - nh / 2.0, nw, nh)
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    Contain,
    Cover,
    Fill,
    Actual,
}

impl Default for Fit {
    fn default() -> Self {
        Fit::Cover
    }
}

pub fn fit_of(s: &str) -> Option<Fit> {
    Some(match s {
        "contain" | "fit" => Fit::Contain,
        "cover" => Fit::Cover,
        "fill" | "stretch" => Fit::Fill,
        "actual" | "none" | "orig" => Fit::Actual,
        _ => return None,
    })
}
