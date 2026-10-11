//! Raster export. Everything here consumes the string `svg::compile` produced —
//! the same string the webview paints — so a preview and an exported PNG cannot
//! disagree about where a layer sits.

use crate::doc::Document;
use crate::error::{Fail, R};
use crate::{imageops, pdf, svg, util};
use image::ImageEncoder;
use resvg::tiny_skia;
use resvg::usvg;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// Refuse to allocate beyond this; a 4x export of a 4K board is already ~50 MP.
pub const MAX_PIXELS: u64 = 60_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Png,
    Jpeg,
    Webp,
    Svg,
    Pdf,
}

impl Format {
    pub fn ext(&self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Webp => "webp",
            Format::Svg => "svg",
            Format::Pdf => "pdf",
        }
    }
    pub fn mime(&self) -> &'static str {
        match self {
            Format::Png => "image/png",
            Format::Jpeg => "image/jpeg",
            Format::Webp => "image/webp",
            Format::Svg => "image/svg+xml",
            Format::Pdf => "application/pdf",
        }
    }
    pub fn raster(&self) -> bool {
        matches!(self, Format::Png | Format::Jpeg | Format::Webp)
    }
}

#[derive(Clone, Debug)]
pub struct RenderOpts {
    pub scale: f64,
    pub format: Format,
    pub quality: u8,
    /// `None` = keep the document's own background; `Some("none")` = force transparent.
    pub background: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self { scale: 1.0, format: Format::Png, quality: 92, background: None, width: None, height: None }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Rendered {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub bytes: u64,
    #[serde(rename = "w")]
    pub width: u32,
    #[serde(rename = "h")]
    pub height: u32,
    /// The sha256 of the bytes actually written. `sha` because every agent reading
    /// this pays for the key name too.
    #[serde(rename = "sha")]
    pub sha256: String,
    #[serde(rename = "f")]
    pub format: &'static str,
    pub mime: &'static str,
    /// Degradations the compiler had to accept (unreadable asset, missing font,
    /// backdrop fallback). Empty in the common case, so it costs no tokens.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warns: Vec<String>,
}

/// One system-font database for the process. `usvg` would otherwise rescan
/// `C:\Windows\Fonts` on every export, which is the difference between a
/// 60 ms and a 400 ms preview.
fn fontdb() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        // Our measurement engine may know about fonts registered at runtime
        // (`text.register`) that predate this snapshot; fold them in so the
        // rasteriser and the wrapper agree on which families exist.
        for f in crate::text::Registry::shared().known_files() {
            let _ = db.load_font_file(&f);
        }
        Arc::new(db)
    })
    .clone()
}

fn parse(svg_src: &str, dir: Option<&Path>) -> R<usvg::Tree> {
    let mut o = usvg::Options::default();
    o.fontdb = fontdb();
    o.resources_dir = dir.map(|p| p.to_path_buf());
    o.dpi = 96.0;
    usvg::Tree::from_str(svg_src, &o).map_err(|e| Fail::new("RENDER_FAILED", format!("svg parse: {e}")).hint("render.svg returns the exact string that failed; check it with an XML editor"))
}

/// SVG → RGBA8. `scale` multiplies the intrinsic size.
pub fn raster(svg_src: &str, scale: f64) -> R<(u32, u32, Vec<u8>)> {
    raster_at(svg_src, scale, None)
}

pub fn raster_at(svg_src: &str, scale: f64, dir: Option<&Path>) -> R<(u32, u32, Vec<u8>)> {
    let tree = parse(svg_src, dir)?;
    let size = tree.size();
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let w = (size.width() as f64 * s).round().max(1.0) as u32;
    let h = (size.height() as f64 * s).round().max(1.0) as u32;
    if (w as u64) * (h as u64) > MAX_PIXELS {
        return Err(Fail::new("TOO_LARGE", format!("{w}x{h} exceeds the {} MP limit", MAX_PIXELS / 1_000_000))
            .hint("lower sc, or export one board at a time with b=<id>"));
    }
    let mut pix = tiny_skia::Pixmap::new(w, h).ok_or_else(|| Fail::new("RENDER_FAILED", format!("pixmap {w}x{h}")))?;
    let mut pm = pix.as_mut();
    resvg::render(&tree, tiny_skia::Transform::from_scale(s as f32, s as f32), &mut pm);
    drop(pm);
    Ok((w, h, pix.data().to_vec()))
}

/// Same, but forced to an exact pixel box (what `render.export {w,h}` means).
pub fn raster_fit(svg_src: &str, w: u32, h: u32, dir: Option<&Path>) -> R<(u32, u32, Vec<u8>)> {
    let tree = parse(svg_src, dir)?;
    let size = tree.size();
    if (w as u64) * (h as u64) > MAX_PIXELS {
        return Err(Fail::new("TOO_LARGE", format!("{w}x{h}")));
    }
    let sx = w as f32 / size.width() as f32;
    let sy = h as f32 / size.height() as f32;
    let mut pix = tiny_skia::Pixmap::new(w, h).ok_or_else(|| Fail::new("RENDER_FAILED", "pixmap"))?;
    let mut pm = pix.as_mut();
    resvg::render(&tree, tiny_skia::Transform::from_scale(sx, sy), &mut pm);
    drop(pm);
    Ok((w, h, pix.data().to_vec()))
}

pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> R<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    {
        let cur = std::io::Cursor::new(&mut out);
        image::codecs::png::PngEncoder::new(cur)
            .write_image(rgba, w, h, image::ExtendedColorType::Rgba8)
            .map_err(|e| Fail::new("RENDER_FAILED", format!("png: {e}")))?;
    }
    Ok(out)
}

/// Flatten onto `bg` so a transparent render can become a JPEG or a PDF page.
fn flatten(rgba: &[u8], bg: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 4 * 3);
    for px in rgba.chunks_exact(4) {
        let a = px[3] as u32;
        if a == 255 {
            out.extend_from_slice(&[px[0], px[1], px[2]]);
        } else if a == 0 {
            out.extend_from_slice(&bg);
        } else {
            let mix = |c: u8| (((c as u32 * a) + (bg[0] as u32 * (255 - a))) / 255) as u8;
            out.push(mix(px[0]));
            out.push(mix(px[1]));
            out.push(mix(px[2]));
        }
    }
    out
}

pub fn encode(rgba: &[u8], w: u32, h: u32, f: Format, q: u8) -> R<Vec<u8>> {
    match f {
        Format::Png => encode_png(w, h, rgba),
        Format::Jpeg => {
            let rgb = flatten(rgba, [255, 255, 255]);
            let mut out: Vec<u8> = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q.clamp(1, 100))
                .write_image(&rgb, w, h, image::ExtendedColorType::Rgb8)
                .map_err(|e| Fail::new("RENDER_FAILED", format!("jpeg: {e}")))?;
            Ok(out)
        }
        Format::Webp => {
            // `image` 0.25 only exposes a lossless WebP encoder, so `q` is
            // honestly ignored here rather than silently faked; `image.op
            // optimize` is the path that hits a byte target for webp.
            let mut out: Vec<u8> = Vec::new();
            image::codecs::webp::WebPEncoder::new_lossless(&mut out)
                .write_image(rgba, w, h, image::ExtendedColorType::Rgba8)
                .map_err(|e| Fail::new("RENDER_FAILED", format!("webp: {e}")))?;
            Ok(out)
        }
        Format::Svg | Format::Pdf => Err(Fail::new("INVALID_VALUE", "not a raster format")),
    }
}

fn opts_for(doc: &Document, o: &RenderOpts, boards: &[String]) -> svg::SvgOpts {
    svg::SvgOpts {
        boards: boards.to_vec(),
        inline_assets: true,
        asset_url: None,
        font_url: None,
        embed_fonts: matches!(o.format, Format::Svg),
        show_guides: false,
        omit_ids: vec![],
        scale: 1.0,
    }
}

/// Render one document to bytes plus the artifact record (no path yet).
pub fn to_bytes(doc: &Document, o: &RenderOpts) -> R<(Vec<u8>, Rendered)> {
    to_bytes_dir(doc, o, &[], None)
}

pub fn to_bytes_dir(doc: &Document, o: &RenderOpts, boards: &[String], dir: Option<&Path>) -> R<(Vec<u8>, Rendered)> {
    let bs: Vec<String> = if boards.is_empty() {
        doc.artboards.iter().filter(|a| a.visible).map(|a| a.id.clone()).collect()
    } else {
        boards.to_vec()
    };
    if bs.is_empty() {
        return Err(Fail::new("ARTBOARD_NOT_FOUND", "nothing to render").hint("artboard.create {p,w,h} first"));
    }
    let mut so = opts_for(doc, o, &bs);
    match o.format {
        Format::Svg => {
            let (s, warns) = svg::compile_w(doc, &so, dir, None)?;
            let bytes = s.into_bytes();
            let (w, h) = svg::board_size(doc, &so);
            Ok((bytes.clone(), with_warns(rec(None, &bytes, w as u32, h as u32, o.format), warns)))
        }
        Format::Pdf => {
            let mut pages: Vec<(u32, u32, Vec<u8>)> = vec![];
            let mut wmax = 0u32;
            let mut hmax = 0u32;
            let mut warns: Vec<String> = vec![];
            for b in &bs {
                let one = svg::SvgOpts { boards: vec![b.clone()], ..so.clone() };
                let (s, w) = svg::compile_w(doc, &one, dir, Some(b))?;
                warns.extend(w);
                let (w, h, rgba) = raster_or_fit(&s, o, dir)?;
                pages.push((w, h, rgb_of(&rgba, w, h, o)));
                wmax = wmax.max(w);
                hmax = hmax.max(h);
            }
            let bytes = pdf::write_pdf(&pages, &json!({"title": doc.name, "producer": "AgentCanvas", "pages": pages.len()}))?;
            Ok((bytes.clone(), with_warns(rec(None, &bytes, wmax, hmax, o.format), warns)))
        }
        _ => {
            let (s, warns) = svg::compile_w(doc, &so, dir, None)?;
            let (w, h, rgba) = raster_or_fit(&s, o, dir)?;
            let bytes = encode(&rgba, w, h, o.format, o.quality)?;
            Ok((bytes.clone(), with_warns(rec(None, &bytes, w, h, o.format), warns)))
        }
    }
}

/// A render that quietly dropped an asset is the worst kind of success: the file
/// exists, the bytes are real, and the poster has no photo in it.
fn with_warns(mut r: Rendered, warns: Vec<String>) -> Rendered {
    r.warns = warns;
    r
}

fn raster_or_fit(s: &str, o: &RenderOpts, dir: Option<&Path>) -> R<(u32, u32, Vec<u8>)> {
    match (o.width, o.height) {
        (Some(w), Some(h)) => raster_fit(s, w, h, dir),
        _ => raster_at(s, if o.scale > 0.0 { o.scale } else { 1.0 }, dir),
    }
}

fn rgb_of(rgba: &[u8], w: u32, h: u32, o: &RenderOpts) -> Vec<u8> {
    match o.format {
        Format::Jpeg | Format::Pdf => {
            let bg = o
                .background
                .as_deref()
                .and_then(crate::doc::parse_color)
                .map(|(r, g, b, _)| [r, g, b])
                .unwrap_or([255, 255, 255]);
            flatten(rgba, bg)
        }
        _ => rgba.to_vec(),
    }
}

fn rec(path: Option<&Path>, bytes: &[u8], w: u32, h: u32, f: Format) -> Rendered {
    Rendered {
        path: path.map(|p| util::short(p, None)),
        bytes: bytes.len() as u64,
        width: w,
        height: h,
        sha256: util::sha256_bytes(bytes),
        format: f.ext(),
        mime: f.mime(),
        warns: Vec::new(),
    }
}

/// Write one document out. `dir` is where files go; `proj` is where the *assets*
/// are (they are stored relative to the project, so passing the output directory
/// for both silently produces a picture with every image missing).
pub fn export(doc: &Document, dir: &Path, name: &str, o: &RenderOpts, boards: &[String]) -> R<Vec<Rendered>> {
    export_with(doc, dir, name, o, boards, None)
}

pub fn export_with(doc: &Document, dir: &Path, name: &str, o: &RenderOpts, boards: &[String], proj: Option<&Path>) -> R<Vec<Rendered>> {
    util::ensure_parent(&dir.join("x"))?;
    let bs: Vec<String> = if boards.is_empty() {
        doc.artboards.iter().filter(|a| a.visible).map(|a| a.id.clone()).collect()
    } else {
        boards.to_vec()
    };
    if bs.is_empty() {
        return Err(Fail::new("ARTBOARD_NOT_FOUND", "nothing to export").hint("artboard.create {p,w,h} first"));
    }
    let mut out = vec![];
    let single = bs.len() == 1;
    for b in &bs {
        let one = vec![b.clone()];
        let (bytes, mut r) = to_bytes_dir(doc, o, &one, proj.or(Some(dir)))?;
        let stem = if single { name.to_string() } else { format!("{}-{}", name, board_label(doc, b)) };
        let path = unique(&dir.join(format!("{}.{}", sanitize(&stem), o.format.ext())));
        util::write(&path, &bytes)?;
        r.path = Some(util::short(&path, None));
        out.push(r);
    }
    Ok(out)
}

fn board_label(doc: &Document, b: &str) -> String {
    doc.artboard(b).map(|a| if a.name.is_empty() { a.id.clone() } else { a.name.clone() }).unwrap_or_else(|| b.to_string())
}

/// Board names carry ratios (`1:1`) and users type anything; `:` `\` `|` `*` `?`
/// `"` `<` `>` are illegal in Windows file names and os error 87 is not a useful
/// answer to give an agent, so names are flattened on the way to disk.
fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ':' | '\\' | '/' | '*' | '?' | '"' | '<' | '>' | '|' => out.push('-'),
            c if (c as u32) < 0x20 => out.push('_'),
            c => out.push(c),
        }
    }
    let trimmed = out.trim().trim_matches('.');
    if trimmed.is_empty() { "board".to_string() } else { trimmed.to_string() }
}

/// Never silently overwrite the previous export of the same board.
fn unique(p: &Path) -> PathBuf {    if !p.exists() {
        return p.to_path_buf();
    }
    for i in 2..1000 {
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("out").to_string();
        let cand = p.with_file_name(format!("{stem}-{i}.{}", util::ext(p)));
        if !cand.exists() {
            return cand;
        }
    }
    p.to_path_buf()
}

pub fn load_rgba(p: &Path) -> R<(u32, u32, Vec<u8>)> {
    let img = imageops::load(p)?;
    Ok((img.w, img.h, img.px))
}

/// Truthful image comparison: this is how an agent verifies that its edit did
/// something, so identical inputs must score exactly 100.
pub fn diff(a: &Path, b: &Path) -> R<Value> {
    let ia = imageops::load(a)?;
    let ib = imageops::load(b)?;
    if ia.w != ib.w || ia.h != ib.h {
        return Err(Fail::new("SIZE_MISMATCH", format!("{}x{} vs {}x{}", ia.w, ia.h, ib.w, ib.h))
            .hint("resize one side first: image.op op:resize")
            .details(json!({"a": [ia.w, ia.h], "b": [ib.w, ib.h]})));
    }
    let n = (ia.w as usize) * (ia.h as usize);
    let mut sum = [0u64; 4];
    let mut maxd = 0u32;
    let mut changed = 0u64;
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for i in 0..n {
        let o = i * 4;
        let mut any = false;
        for k in 0..4 {
            let d = ia.px[o + k].abs_diff(ib.px[o + k]) as u64;
            sum[k] += d;
            if d > maxd as u64 {
                maxd = d as u32;
            }
        }
        for k in 0..3 {
            if ia.px[o + k].abs_diff(ib.px[o + k]) > 6 {
                any = true;
            }
        }
        if ia.px[o + 3] != ib.px[o + 3] {
            any = true;
        }
        if any {
            changed += 1;
            let x = (i as u32) % ia.w;
            let y = (i as u32) / ia.w;
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    let mae: Vec<f64> = sum.iter().map(|s| util::round(*s as f64 / n as f64, 3)).collect();
    let score = util::round((1.0 - (mae[0] + mae[1] + mae[2]) / 3.0 / 255.0) * 100.0, 2);
    Ok(json!({
        "score": score,
        "identical": changed == 0,
        "mae": {"r": mae[0], "g": mae[1], "b": mae[2], "a": mae[3]},
        "max_delta": maxd,
        "changed": changed,
        "pixels": n,
        "changed_pct": util::round(changed as f64 * 100.0 / n as f64, 3),
        "box": if changed == 0 { Value::Null } else { json!({"x":x0,"y":y0,"w":x1-x0+1,"h":y1-y0+1}) },
        "a": {"path": util::short(a, None), "w": ia.w, "h": ia.h},
        "b": {"path": util::short(b, None), "w": ib.w, "h": ib.h},
    }))
}

/// A quick similarity number only — for callers that just want "did it change?".
pub fn similarity(a: &Path, b: &Path) -> R<f64> {
    Ok(diff(a, b)?["score"].as_f64().unwrap_or(0.0))
}
