//! Raster image processing — the "图片处理" half of the engine.
//!
//! Everything in here is a pure function of `(Img, args)`: no global state, no
//! document access, no I/O outside the explicit `load`/`save`/`probe` helpers.
//! That is what lets `image.batch` run whole chains under rayon and what makes an
//! op reproducible from an agent's tool call alone.
//!
//! Conventions (shared by every op, documented in [`ops`]):
//! - Colours are the document's colour syntax (`#rgb`, `#rrggbb`, `#rrggbbaa`,
//!   `rgb()/rgba()`, `white|black|red|transparent`) parsed by [`crate::doc::parse_color`].
//! - `args` may be an object, or a compact string in the same shape as the chain
//!   string form (`"400x400"`, `"center 1:1"`), which is the cheapest thing for a
//!   language model to emit.
//! - Sizes are pixels; angles are degrees, clockwise; `opacity`/`amount` are 0..1
//!   unless a range is stated in the op description.
//! - Analysis ops (`palette`, `dhash`, `ahash`, `trim`, `optimize`, `strip`) pass
//!   the image through untouched and put their findings in the step report, which
//!   [`run_chain_report`] merges into the result JSON.

use crate::doc::parse_color;
use crate::error::{Fail, R};
use crate::geom::{self, Fit, Origin, Rect};
use crate::util;
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader,
    RgbaImage,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------ limits

/// A file bigger than this is refused before any decoding happens.
pub const MAX_FILE_BYTES: u64 = 200 * 1024 * 1024;
/// Longest accepted side (pixels) of a decoded image.
pub const MAX_DIM: u32 = 20_000;
/// Largest accepted pixel count (60 MP) — guards against decode bombs.
pub const MAX_PIXELS: u64 = 60_000_000;

// ------------------------------------------------------------------ Img

/// RGBA8 pixel buffer, row-major, 4 bytes per pixel. Always RGBA, whatever the
/// source format was; that keeps every op free of colour-type special cases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Img {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

pub const TRANSPARENT: [u8; 4] = [0, 0, 0, 0];
pub const WHITE: [u8; 4] = [255, 255, 255, 255];
pub const BLACK: [u8; 4] = [0, 0, 0, 255];

impl Img {
    /// Transparent canvas.
    pub fn new(w: u32, h: u32) -> R<Img> {
        check_size(w, h)?;
        Ok(Self { w, h, px: vec![0u8; (w as usize) * (h as usize) * 4] })
    }
    pub fn filled(w: u32, h: u32, c: [u8; 4]) -> R<Img> {
        check_size(w, h)?;
        let n = (w as usize) * (h as usize);
        let mut px = Vec::with_capacity(n * 4);
        for _ in 0..n {
            px.extend_from_slice(&c);
        }
        Ok(Self { w, h, px })
    }
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.w as usize + x) * 4;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, p: [u8; 4]) {
        let i = (y * self.w as usize + x) * 4;
        self.px[i..i + 4].copy_from_slice(&p);
    }
    /// Alpha bounds, used by a lot of the mask math.
    #[inline]
    fn a(&self, x: usize, y: usize) -> u8 {
        self.px[(y * self.w as usize + x) * 4 + 3]
    }
    pub fn len(&self) -> usize {
        (self.w as usize) * (self.h as usize)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// True when any pixel is not fully opaque.
    pub fn has_alpha(&self) -> bool {
        self.px.chunks_exact(4).any(|p| p[3] != 255)
    }
    pub fn to_image(&self) -> R<RgbaImage> {
        RgbaImage::from_raw(self.w, self.h, self.px.clone())
            .ok_or_else(|| Fail::new("INTERNAL", "pixel buffer length does not match size"))
    }
    pub fn from_image(im: RgbaImage) -> Img {
        let (w, h) = im.dimensions();
        Img { w, h, px: im.into_raw() }
    }
}

fn check_size(w: u32, h: u32) -> R<()> {
    if w == 0 || h == 0 {
        return Err(Fail::new("INVALID_VALUE", "image size must be at least 1x1")
            .hint("crop/resize produced an empty canvas: check w/h against the source size"));
    }
    if w > MAX_DIM || h > MAX_DIM || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(Fail::new("TOO_LARGE", format!("output would be {w}x{h}"))
            .hint(format!("limits are {MAX_DIM}px per side and {MAX_PIXELS} pixels; scale down first")));
    }
    Ok(())
}

// ------------------------------------------------------------------ arg helpers

fn bad(msg: impl Into<String>) -> Fail {
    Fail::new("INVALID_SCHEMA", msg)
}
fn inval(msg: impl Into<String>) -> Fail {
    Fail::new("INVALID_VALUE", msg)
}

/// First present key, as f64.
fn af(args: &Value, keys: &[&str], def: f64) -> f64 {
    for k in keys {
        if let Some(v) = util::f(args, k) {
            return v;
        }
    }
    def
}
fn rf(args: &Value, keys: &[&str]) -> R<f64> {
    for k in keys {
        if let Some(v) = util::f(args, k) {
            return Ok(v);
        }
    }
    Err(bad(format!("missing number argument \"{}\"", keys[0])))
}
fn au(args: &Value, keys: &[&str], def: u32) -> u32 {
    af(args, keys, f64::from(def)).max(0.0).round() as u32
}
fn ai(args: &Value, keys: &[&str], def: i64) -> i64 {
    af(args, keys, def as f64).round() as i64
}
fn asv<'a>(args: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| util::ps(args, k)).filter(|s| !s.is_empty())
}
/// Boolean argument: the first present key wins, otherwise `def`.
/// Accepts `true/false`, `1/0` and `"yes"/"no"` through [`util::opt_b`].
fn ab(args: &Value, keys: &[&str], def: bool) -> bool {
    for k in keys {
        if args.get(*k).is_some() {
            return util::opt_b(args, k, def);
        }
    }
    def
}
/// Colour argument -> `[r,g,b,a]`; unparsable strings are treated as absent.
fn col(args: &Value, keys: &[&str]) -> Option<[u8; 4] > {
    let s = asv(args, keys)?;
    let (r, g, b, a) = parse_color(s)?;
    Some([r, g, b, (a * 255.0).round() as u8])
}
fn col_or(args: &Value, keys: &[&str], def: [u8; 4]) -> [u8; 4] {
    col(args, keys).unwrap_or(def)
}
fn col_req(args: &Value, keys: &[&str]) -> R<[u8; 4]> {
    match col(args, keys) {
        Some(c) => Ok(c),
        None => Err(bad(format!("argument \"{}\" must be a colour", keys[0]))
            .hint("use \"#ff0000\", \"#ff000080\", \"rgb(255,0,0)\" or white|black|red|transparent")),
    }
}
fn anchor_of(args: &Value) -> Origin {
    if let Some(s) = asv(args, &["anchor", "pos", "position", "origin", "gravity", "at"]) {
        if let Some(o) = Origin::parse(s) {
            return o;
        }
        if matches!(s, "centre" | "Centre") {
            return Origin::Center;
        }
    }
    Origin::Center
}

fn u8c(v: f64) -> u8 {
    if v.is_nan() {
        0
    } else if v <= 0.0 {
        0
    } else if v >= 255.0 {
        255
    } else {
        v as u8
    }
}
fn f8(v: f64) -> f64 {
    v.max(0.0).min(255.0)
}

/// Pixel (x,y) -> (0..1) float rgb + alpha, out of bounds gives `bg`.
fn sample_bil(img: &Img, fx: f64, fy: f64, bg: [f64; 4]) -> [f64; 4] {
    let w = img.w as f64;
    let h = img.h as f64;
    let x0f = fx - 0.5;
    let y0f = fy - 0.5;
    let x0 = x0f.floor() as i64;
    let y0 = y0f.floor() as i64;
    let tx = (x0f - x0 as f64).clamp(0.0, 1.0);
    let ty = (y0f - y0 as f64).clamp(0.0, 1.0);
    let mut acc = [0.0f64; 4];
    let pts = [
        (x0, y0, (1.0 - tx) * (1.0 - ty)),
        (x0 + 1, y0, tx * (1.0 - ty)),
        (x0, y0 + 1, (1.0 - tx) * ty),
        (x0 + 1, y0 + 1, tx * ty),
    ];
    for (sx, sy, wt) in pts {
        if sx < 0 || sy < 0 || sx as f64 >= w || sy as f64 >= h {
            for c in 0..4 {
                acc[c] += bg[c] * wt;
            }
            continue;
        }
        let p = img.at(sx as usize, sy as usize);
        for c in 0..4 {
            acc[c] += p[c] as f64 * wt;
        }
    }
    acc
}

/// Where a `iw x ih` box sits inside an `ow x oh` canvas at the given anchor.
fn place(ow: u32, oh: u32, iw: u32, ih: u32, a: Origin) -> (u32, u32) {
    let outer = Rect::from_size(f64::from(ow), f64::from(oh));
    let (ax, ay) = anchor_frac(a);
    let p = a.point(outer);
    let x = (p.0 - f64::from(iw) * ax).round().max(0.0);
    let y = (p.1 - f64::from(ih) * ay).round().max(0.0);
    (
        x.min((ow.saturating_sub(iw)) as f64) as u32,
        y.min((oh.saturating_sub(ih)) as f64) as u32,
    )
}

// ------------------------------------------------------------------ decode / encode

pub fn mime_of(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

pub fn fmt_of_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "png",
        "jpg" | "jpeg" | "jpe" => "jpeg",
        "webp" => "webp",
        "gif" => "gif",
        "bmp" => "bmp",
        "tif" | "tiff" => "tiff",
        "svg" => "svg",
        _ => return None,
    })
}

/// Decode any supported format to RGBA8. EXIF orientation is applied so what you
/// get is what a viewer would show. Refuses oversized inputs and decode bombs.
pub fn load(p: &Path) -> R<Img> {
    let md = std::fs::metadata(p)
        .map_err(|e| Fail::new("NOT_FOUND", format!("{}: {e}", p.display()))
            .hint("image.probe first, or pass a path under the project root"))?;
    if md.len() > MAX_FILE_BYTES {
        return Err(Fail::new("TOO_LARGE", format!("{} is {}", p.display(), util::human_bytes(md.len())))
            .hint(format!("files must be under {}; downscale or crop upstream", util::human_bytes(MAX_FILE_BYTES))));
    }
    let ext = util::ext(p);
    if ext == "svg" || looks_svg(p) {
        let img = raster_svg(&std::fs::read(p)?, None)?;
        return Ok(img);
    }
    let (dw, dh) = dimensions(p)?;
    if dw > MAX_DIM || dh > MAX_DIM || u64::from(dw) * u64::from(dh) > MAX_PIXELS {
        return Err(Fail::new("TOO_LARGE", format!("{dw}x{dh} exceeds the decode limits"))
            .hint(format!("limits are {MAX_DIM}px per side / {} megapixels; use `crop` on a smaller source", MAX_PIXELS / 1_000_000)));
    }
    let mut reader = ImageReader::open(p)
        .map_err(|e| Fail::new("NOT_FOUND", format!("{}: {e}", p.display())))?;
    reader = reader
        .with_guessed_format()
        .map_err(|e| decode_fail(image::ImageError::from(e)))?;
    reader.no_limits();
    let mut dec = reader.into_decoder().map_err(decode_fail)?;
    let orient = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut di = DynamicImage::from_decoder(dec).map_err(decode_fail)?;
    if orient != image::metadata::Orientation::NoTransforms {
        di.apply_orientation(orient);
    }
    let rgba = di.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    check_size(w, h)?;
    Ok(Img { w, h, px: rgba.into_raw() })
}

fn decode_fail(e: image::ImageError) -> Fail {
    match e {
        image::ImageError::Unsupported(_) => Fail::new(
            "UNSUPPORTED",
            "this image format cannot be decoded in this build",
        )
        .hint("supported: png jpeg webp bmp gif tiff svg; convert the file first"),
        other => Fail::new("DECODE_FAILED", other.to_string())
            .hint("the file is truncated or not an image; run image.probe to confirm"),
    }
}

fn looks_svg(p: &Path) -> bool {
    let Ok(b) = std::fs::read(p) else { return false };
    let head = &b[..b.len().min(512)];
    let s = String::from_utf8_lossy(head);
    s.trim_start().starts_with("<svg") || (s.contains("<svg") && s.contains("xmlns"))
}

fn dimensions(p: &Path) -> R<(u32, u32)> {
    let r = ImageReader::open(p).map_err(|e| Fail::new("NOT_FOUND", format!("{}: {e}", p.display())))?;
    let r = r
        .with_guessed_format()
        .map_err(|e| decode_fail(image::ImageError::from(e)))?;
    let (w, h) = r.into_dimensions().map_err(decode_fail)?;
    Ok((w, h))
}

/// Encode to bytes; `fmt` is one of png|jpeg|webp|gif|bmp|tiff.
/// JPEG flattens alpha onto `bg` (default white). PNG keeps alpha.
/// WebP is lossless in this build (the `image` crate ships no VP8 lossy encoder);
/// the chosen quality is reported so callers can be honest about it.
pub fn encode(img: &Img, fmt: &str, q: u8, bg: [u8; 3]) -> R<(Vec<u8>, Value)> {
    let q = q.clamp(1, 100);
    let mut buf: Vec<u8> = Vec::new();
    let mut note: Option<String> = None;
    match fmt {
        "jpeg" | "jpg" => {
            let rgb = flatten(img, bg);
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q);
            enc.write_image(&rgb, img.w, img.h, ExtendedColorType::Rgb8)
                .map_err(|e| Fail::new("RENDER_FAILED", format!("jpeg: {e}")))?;
        }
        "png" => {
            use image::codecs::png::{CompressionType, FilterType, PngEncoder};
            // `quality` maps onto the deflate level: high quality = smaller file.
            let comp = if q >= 90 {
                CompressionType::Best
            } else if q >= 50 {
                CompressionType::Default
            } else if q >= 20 {
                CompressionType::Fast
            } else {
                CompressionType::Uncompressed
            };
            let enc = PngEncoder::new_with_quality(&mut buf, comp, FilterType::Adaptive);
            enc.write_image(&img.px, img.w, img.h, ExtendedColorType::Rgba8)
                .map_err(|e| Fail::new("RENDER_FAILED", format!("png: {e}")))?;
        }
        "webp" => {
            let enc = image::codecs::webp::WebPEncoder::new_lossless(&mut buf);
            enc.write_image(&img.px, img.w, img.h, ExtendedColorType::Rgba8)
                .map_err(|e| Fail::new("RENDER_FAILED", format!("webp: {e}")))?;
            if q < 100 {
                note = Some(format!(
                    "webp is lossless in this build: quality {q} did not change the bytes"
                ));
            }
        }
        "gif" | "bmp" | "tiff" => {
            let di = DynamicImage::ImageRgba8(img.to_image()?);
            let f = match fmt {
                "gif" => ImageFormat::Gif,
                "bmp" => ImageFormat::Bmp,
                _ => ImageFormat::Tiff,
            };
            di.write_to(&mut std::io::Cursor::new(&mut buf), f)
                .map_err(|e| Fail::new("RENDER_FAILED", format!("{fmt}: {e}")))?;
            if fmt == "gif" {
                note = Some("gif is written as a single still frame".into());
            }
        }
        other => {
            return Err(Fail::new("UNSUPPORTED", format!("cannot encode \"{other}\""))
                .hint("use png, jpeg, webp, gif, bmp or tiff"));
        }
    }
    let mut rep = json!({ "fmt": fmt, "quality": q, "bytes": buf.len() });
    if let Some(n) = note {
        rep["note"] = json!(n);
    }
    Ok((buf, rep))
}

/// RGBA -> RGB with alpha flattened onto `bg`.
pub fn flatten(img: &Img, bg: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity((img.w as usize) * (img.h as usize) * 3);
    for p in img.px.chunks_exact(4) {
        let a = p[3] as f64 / 255.0;
        let ia = 1.0 - a;
        for c in 0..3 {
            out.push(u8c(p[c] as f64 * a + bg[c] as f64 * ia));
        }
    }
    out
}

/// Write `img` to `p`; the encoder is picked from the extension.
pub fn save(img: &Img, p: &Path, q: u8) -> R<()> {
    save_ex(img, p, &json!({ "q": q }))
        .map(|_| ())
}

/// [`save`] plus an options object (`q`, `bg`, `fmt`) and a result report.
pub fn save_ex(img: &Img, p: &Path, opts: &Value) -> R<Value> {
    let ext = match asv(opts, &["fmt", "f", "format"]) {
        Some(f) => norm_fmt(f)?.to_string(),
        None => {
            let e = util::ext(p);
            match fmt_of_ext(&e) {
                Some(f) => f.to_string(),
                None => {
                    return Err(Fail::new("UNSUPPORTED", format!("unknown output extension \".{e}\""))
                        .hint("name the file .png .jpg .webp .gif .bmp .tif, or pass fmt"))
                }
            }
        }
    };
    let q = au(opts, &["q", "quality"], 92).clamp(1, 100) as u8;
    let bg = col(opts, &["bg", "background", "matte"]).unwrap_or(WHITE);
    let (bytes, mut rep) = encode(img, &ext, q, [bg[0], bg[1], bg[2]])?;
    util::ensure_parent(p)?;
    std::fs::write(p, &bytes).map_err(|e| Fail::new("IO", format!("{}: {e}", p.display())))?;
    rep["out"] = json!(disp(p));
    rep["w"] = json!(img.w);
    rep["h"] = json!(img.h);
    rep["sha"] = json!(util::sha256_bytes(&bytes));
    rep["mime"] = json!(mime_of(&ext));
    rep["size"] = json!(util::human_bytes(bytes.len() as u64));
    Ok(rep)
}

fn norm_fmt(f: &str) -> R<&'static str> {
    let t = f.to_lowercase();
    let t = t.trim_start_matches('.');
    fmt_of_ext(match t {
        "jpeg" | "jpg" => "jpg",
        other => other,
    })
    .ok_or_else(|| Fail::new("UNSUPPORTED", format!("unknown format \"{f}\"")))
}

pub fn disp(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn resolve(s: &str, ctx: Option<&Path>) -> R<PathBuf> {
    util::inp(s, ctx)
}

/// File-level probe: `{w,h,fmt,alpha,bytes,sha,orientation,mime,...}`.
pub fn probe(p: &Path) -> R<Value> {
    let md = std::fs::metadata(p)
        .map_err(|e| Fail::new("NOT_FOUND", format!("{}: {e}", p.display())))?;
    let img = load(p)?;
    let ext = util::ext(p);
    let sha = util::sha256_file(p)?;
    let mut v = json!({
        "path": disp(p),
        "w": img.w, "h": img.h,
        "fmt": fmt_of_ext(&ext).unwrap_or(if ext.is_empty() { "unknown" } else { ext.as_str() }),
        "alpha": img.has_alpha(),
        "bytes": md.len(),
        "size": util::human_bytes(md.len()),
        "sha": sha,
        "orientation": orientation_label(img.w, img.h),
        "mime": mime_of(&ext),
    });
    if ext == "svg" {
        v["kind"] = json!("vector");
    } else {
        if let Some(o) = exif_orientation(p) {
            v["exif_orientation"] = json!(orientation_name(o));
        }
    }
    Ok(v)
}

/// EXIF orientation tag, when the container carries one.
fn exif_orientation(p: &Path) -> Option<image::metadata::Orientation> {
    let r = ImageReader::open(p).ok()?;
    let r = r.with_guessed_format().ok()?;
    let mut dec = r.into_decoder().ok()?;
    dec.orientation().ok()
}

pub fn orientation_label(w: u32, h: u32) -> &'static str {
    match w.cmp(&h) {
        std::cmp::Ordering::Greater => "landscape",
        std::cmp::Ordering::Less => "portrait",
        std::cmp::Ordering::Equal => "square",
    }
}

fn orientation_name(o: image::metadata::Orientation) -> &'static str {
    use image::metadata::Orientation as O;
    match o {
        O::NoTransforms => "normal",
        O::Rotate90 => "rotate90",
        O::Rotate180 => "rotate180",
        O::Rotate270 => "rotate270",
        O::FlipHorizontal => "flip-h",
        O::FlipVertical => "flip-v",
        O::Rotate90FlipH => "rotate90-flip-h",
        O::Rotate270FlipH => "rotate270-flip-h",
    }
}

// ------------------------------------------------------------------ rasterising SVG

/// Shared system-font database, loaded once. `resvg`/`usvg` need it to turn
/// `<text>` into outlines; without fonts, text ops report `FONT_MISSING`.
pub fn fontdb() -> std::sync::Arc<resvg::usvg::fontdb::Database> {
    static DB: std::sync::OnceLock<std::sync::Arc<resvg::usvg::fontdb::Database>> = std::sync::OnceLock::new();
    DB.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_system_fonts();
        std::sync::Arc::new(db)
    })
    .clone()
}

/// True when at least one system font was found (text ops need this).
pub fn has_fonts() -> bool {
    !fontdb().is_empty()
}

/// Rasterise an SVG document to RGBA8. `scale` multiplies the intrinsic size.
pub fn raster_svg(src: &[u8], scale: Option<f64>) -> R<Img> {
    let fonts = fontdb();
    if fonts.is_empty() {
        // Still fine for font-free documents; only text nodes would be dropped.
        log_none_fonts();
    }
    let opts = resvg::usvg::Options {
        fontdb: fonts,
        resources_dir: None,
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_data(src, &opts)
        .map_err(|e| Fail::new("DECODE_FAILED", format!("svg: {e}")))?;
    let sz = tree.size();
    let s = scale.unwrap_or(1.0).clamp(0.01, 64.0) as f32;
    let w = (sz.width() * s).round().max(1.0);
    let h = (sz.height() * s).round().max(1.0);
    let (iw, ih) = (w as u32, h as u32);
    check_size(iw, ih)?;
    let mut pm = resvg::tiny_skia::Pixmap::new(iw, ih)
        .ok_or_else(|| Fail::new("RENDER_FAILED", "svg: pixmap"))?;
    let tr = if (s - 1.0).abs() < 1e-6 {
        resvg::tiny_skia::Transform::identity()
    } else {
        resvg::tiny_skia::Transform::from_scale(s, s)
    };
    resvg::render(&tree, tr, &mut pm.as_mut());
    Ok(unpremultiply(pm.data(), iw, ih))
}

fn log_none_fonts() {}

/// tiny-skia hands back premultiplied RGBA; the engine stores straight RGBA.
fn unpremultiply(data: &[u8], w: u32, h: u32) -> Img {
    let mut px = vec![0u8; (w as usize) * (h as usize) * 4];
    for i in 0..(px.len() / 4) {
        let a = data[i * 4 + 3] as u32;
        if a == 0 {
            px[i * 4..i * 4 + 4].copy_from_slice(&[0, 0, 0, 0]);
        } else if a >= 255 {
            px[i * 4..i * 4 + 4].copy_from_slice(&data[i * 4..i * 4 + 4]);
        } else {
            for c in 0..3 {
                let v = (data[i * 4 + c] as u32 * 255 + a / 2) / a;
                px[i * 4 + c] = v.min(255) as u8;
            }
            px[i * 4 + 3] = 255;
        }
    }
    Img { w, h, px }
}

/// `svg` text -> RGBA8 at 1x, used by the text watermark.
fn raster_svg_str(svg: &str) -> R<Img> {
    raster_svg(svg.as_bytes(), None)
}

// ------------------------------------------------------------------ compositing

/// Straight-alpha source-over of one pixel.
#[inline]
fn over(dst: &mut [u8; 4], src: [u8; 4], opacity: f64) {
    let sa = (src[3] as f64 / 255.0) * opacity.clamp(0.0, 1.0);
    if sa <= 0.0 {
        return;
    }
    let da = dst[3] as f64 / 255.0;
    if sa >= 1.0 {
        *dst = src;
        return;
    }
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        *dst = TRANSPARENT;
        return;
    }
    for k in 0..3 {
        let oc = (f64::from(src[k]) * sa + f64::from(dst[k]) * da * (1.0 - sa)) / oa;
        dst[k] = u8c(oc);
    }
    dst[3] = u8c(oa * 255.0);
}

/// Paste `src` onto `dst` at (x,y) with opacity; parts outside `dst` are dropped.
fn paste(dst: &mut Img, x: i64, y: i64, src: &Img, opacity: f64) {
    for sy in 0..src.h as i64 {
        let dy = y + sy;
        if dy < 0 || dy >= dst.h as i64 {
            continue;
        }
        for sx in 0..src.w as i64 {
            let dx = x + sx;
            if dx < 0 || dx >= dst.w as i64 {
                continue;
            }
            let i = ((dy * dst.w as i64 + dx) * 4) as usize;
            let mut p = [dst.px[i], dst.px[i + 1], dst.px[i + 2], dst.px[i + 3]];
            over(&mut p, src.at(sx as usize, sy as usize), opacity);
            dst.px[i..i + 4].copy_from_slice(&p);
        }
    }
}

fn blend_fn(mode: &str) -> Option<fn(f64, f64) -> f64> {
    Some(match mode {
        "normal" => |_, s| s,
        "multiply" => |b, s| b * s,
        "screen" => |b, s| 1.0 - (1.0 - b) * (1.0 - s),
        "overlay" => |b, s| {
            if b <= 0.5 {
                2.0 * b * s
            } else {
                1.0 - 2.0 * (1.0 - b) * (1.0 - s)
            }
        },
        "hard-light" => |b, s| {
            if s <= 0.5 {
                2.0 * b * s
            } else {
                1.0 - 2.0 * (1.0 - b) * (1.0 - s)
            }
        },
        "soft-light" => |b, s| {
            let d = if b <= 0.25 { ((6.0 * b - 12.0) * b + 4.0) * b } else { b.sqrt() };
            if s < 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                b + (2.0 * s - 1.0) * (d - b)
            }
        },
        "color-dodge" => |b, s| {
            if s >= 1.0 {
                1.0
            } else if b <= 0.0 {
                0.0
            } else {
                (b / (1.0 - s)).min(1.0)
            }
        },
        "color-burn" => |b, s| {
            if s <= 0.0 {
                0.0
            } else if b >= 1.0 {
                1.0
            } else {
                1.0 - ((1.0 - b) / s).min(1.0)
            }
        },
        "difference" => |b, s| (b - s).abs(),
        "exclusion" => |b, s| b + s - 2.0 * b * s,
        "lighten" => |b, s| b.max(s),
        "darken" => |b, s| b.min(s),
        "add" | "linear-dodge" => |b, s| (b + s).min(1.0),
        _ => return None,
    })
}

const BLEND_MODES: &str =
    "normal multiply screen overlay soft-light hard-light color-dodge color-burn difference exclusion lighten darken add";

/// Composite `src` over `dst` with a Photoshop-style blend mode.
/// `coverage` = per-pixel alpha of the layer (from its own alpha * opacity).
fn composite(
    dst: &mut Img,
    x: i64,
    y: i64,
    src: &Img,
    opacity: f64,
    mode: &str,
    tile: Option<(u32, u32)>,
) -> R<()> {
    let bf = if mode == "normal" { None } else { Some(blend_fn(mode).ok_or_else(|| {
        Fail::new("INVALID_VALUE", format!("unknown blend mode \"{mode}\""))
            .hint(format!("modes: {BLEND_MODES}"))
    })?) };
    let (tw, th) = match tile {
        Some((tx, ty)) => (tx.max(1) as i64, ty.max(1) as i64),
        None => (i64::from(src.w), i64::from(src.h)),
    };
    let opacity = opacity.clamp(0.0, 1.0);
    let xs: Vec<i64> = {
        let mut v = vec![];
        let start = x.rem_euclid(tw.max(1)) - tw;
        let mut cx = start;
        while cx < dst.w as i64 {
            v.push(cx);
            cx += tw;
        }
        v
    };
    let ys: Vec<i64> = {
        let mut v = vec![];
        let start = y.rem_euclid(th.max(1)) - th;
        let mut cy = start;
        while cy < dst.h as i64 {
            v.push(cy);
            cy += th;
        }
        v
    };
    for oy in &ys {
        for ox in &xs {
            for sy in 0..src.h as i64 {
                let dy = oy + sy;
                if dy < 0 || dy >= dst.h as i64 {
                    continue;
                }
                for sx in 0..src.w as i64 {
                    let dx = ox + sx;
                    if dx < 0 || dx >= dst.w as i64 {
                        continue;
                    }
                    let s = src.at(sx as usize, sy as usize);
                    let di = ((dy * dst.w as i64 + dx) * 4) as usize;
                    let a = (f64::from(s[3]) / 255.0) * opacity;
                    if a <= 0.0 {
                        continue;
                    }
                    if let Some(f) = bf {
                        let mut blended = s;
                        for c in 0..3 {
                            let b = f64::from(dst.px[di + c]) / 255.0;
                            let sc = f64::from(s[c]) / 255.0;
                            blended[c] = u8c(f(b, sc) * 255.0);
                        }
                        let mut d = [dst.px[di], dst.px[di + 1], dst.px[di + 2], dst.px[di + 3]];
                        over(&mut d, blended, opacity);
                        dst.px[di..di + 4].copy_from_slice(&d);
                    } else {
                        let mut d = [dst.px[di], dst.px[di + 1], dst.px[di + 2], dst.px[di + 3]];
                        over(&mut d, s, opacity);
                        dst.px[di..di + 4].copy_from_slice(&d);
                    }
                }
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ resampling

/// The filter set [`resample`] names. Re-exported so a caller can pick one without
/// depending on the `image` crate directly.
pub use image::imageops::FilterType;

/// Fit the image into `w`x`h` with a fit mode (`cover`|`contain`|`fill`|`actual`),
/// the file-level convenience over the `resize` op.
pub fn resize(img: &Img, w: u32, h: u32, fit: &str) -> R<Img> {
    let (o, _) = op_resize(img.clone(), &json!({ "w": w, "h": h, "fit": fit }))?;
    Ok(o)
}

/// Resample with a named filter. Down/upscales that need good quality go through
/// the `image` crate's separable filters; `bilinear`/`nearest` are ours because
/// several ops need exact control over out-of-bounds behaviour.
pub fn resample(src: &Img, w: u32, h: u32, filter: &str) -> R<Img> {
    check_size(w, h)?;
    if src.w == w && src.h == h && matches!(filter, "nearest" | "bilinear" | "copy") {
        return Ok(Img { w, h, px: src.px.clone() });
    }
    let ft = match filter {
        "nearest" => return Ok(resample_nearest(src, w, h)),
        "bilinear" => return Ok(resample_bilinear(src, w, h, TRANSPARENT.map(f64::from))),
        "triangle" | "linear" | "area" => image::imageops::FilterType::Triangle,
        "catmull" | "cubic" | "mitchell" => image::imageops::FilterType::CatmullRom,
        "lanczos" | "lanczos3" => image::imageops::FilterType::Lanczos3,
        "gaussian" | "box" => image::imageops::FilterType::Gaussian,
        "fast" | "thumbnail" => image::imageops::FilterType::Nearest,
        _ => {
            let downscale = u64::from(w) * u64::from(h) < u64::from(src.w) * u64::from(src.h);
            if downscale {
                image::imageops::FilterType::Lanczos3
            } else {
                image::imageops::FilterType::CatmullRom
            }
        }
    };
    let im = src.to_image()?;
    let out = image::imageops::resize(&im, w, h, ft);
    Ok(Img::from_image(out))
}

fn resample_nearest(src: &Img, w: u32, h: u32) -> Img {
    let mut px = vec![0u8; (w as usize) * (h as usize) * 4];
    for y in 0..h as usize {
        let sy = ((y as f64 + 0.5) * f64::from(src.h) / f64::from(h)).floor() as usize;
        let sy = sy.min(src.h as usize - 1);
        for x in 0..w as usize {
            let sx = ((x as f64 + 0.5) * f64::from(src.w) / f64::from(w)).floor() as usize;
            let sx = sx.min(src.w as usize - 1);
            let p = src.at(sx, sy);
            let i = (y * w as usize + x) * 4;
            px[i..i + 4].copy_from_slice(&p);
        }
    }
    Img { w, h, px }
}

fn resample_bilinear(src: &Img, w: u32, h: u32, bg: [f64; 4]) -> Img {
    let mut px = vec![0u8; (w as usize) * (h as usize) * 4];
    for y in 0..h as usize {
        let fy = (f64::from(y as u32) + 0.5) * f64::from(src.h) / f64::from(h);
        for x in 0..w as usize {
            let fx = (f64::from(x as u32) + 0.5) * f64::from(src.w) / f64::from(w);
            let v = sample_bil(src, fx, fy, bg);
            let i = (y * w as usize + x) * 4;
            for c in 0..4 {
                px[i + c] = u8c(v[c]);
            }
        }
    }
    Img { w, h, px }
}

// ------------------------------------------------------------------ blur primitives

fn gauss_weights(sigma: f64) -> Vec<f64> {
    let r = (sigma * 3.0).ceil().clamp(1.0, 48.0) as usize;
    let mut k = vec![0.0f64; 2 * r + 1];
    let mut sum = 0.0;
    for (i, e) in k.iter_mut().enumerate() {
        let d = i as f64 - r as f64;
        *e = (-d * d / (2.0 * sigma * sigma)).exp();
        sum += *e;
    }
    for e in &mut k {
        *e /= sum;
    }
    k
}

/// Separable Gaussian, per channel, in f32 planes. `mode` = gauss|box|auto.
/// `auto` (the default) switches to three stacked box passes when a true
/// convolution would cost more than ~40M taps — the box stack is a Gaussian
/// approximation whose composite variance is matched to sigma.
fn blur_planes(src: &Img, sigma: f64, mode: &str) -> Img {
    let (w, h) = (src.w as usize, src.h as usize);
    let n = w * h * 4;
    let mut plane: Vec<f32> = Vec::with_capacity(n);
    for v in &src.px {
        plane.push(*v as f32);
    }
    let sigma = sigma.max(0.05);
    let cost = (w * h) as f64 * (sigma * 3.0) * 2.0;
    let use_box = mode == "box" || (mode != "gauss" && cost > 40_000_000.0);
    if use_box {
        // three boxes: variance of one box of radius r is r(r+1)/3, three of them
        // give sigma^2 = r(r+1) -> r = (sqrt(1+4 sigma^2) - 1) / 2
        let r = (((1.0 + 4.0 * sigma * sigma).sqrt() - 1.0) / 2.0).round() as i64;
        let r = r.max(1);
        for _ in 0..3 {
            box_pass(&mut plane, w, h, r);
        }
    } else {
        let k = gauss_weights(sigma);
        let rad = (k.len() / 2) as i64;
        let mut tmp = vec![0f32; n];
        // horizontal
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f64; 4];
                for (i, kv) in k.iter().enumerate() {
                    let sx = (x as i64 + i as i64 - rad).clamp(0, w as i64 - 1) as usize;
                    let base = (y * w + sx) * 4;
                    for c in 0..4 {
                        acc[c] += f64::from(plane[base + c]) * *kv;
                    }
                }
                let base = (y * w + x) * 4;
                for c in 0..4 {
                    tmp[base + c] = acc[c] as f32;
                }
            }
        }
        std::mem::swap(&mut plane, &mut tmp);
        // vertical
        for x in 0..w {
            for y in 0..h {
                let mut acc = [0.0f64; 4];
                for (i, kv) in k.iter().enumerate() {
                    let sy = (y as i64 + i as i64 - rad).clamp(0, h as i64 - 1) as usize;
                    let base = (sy * w + x) * 4;
                    for c in 0..4 {
                        acc[c] += f64::from(plane[base + c]) * *kv;
                    }
                }
                let base = (y * w + x) * 4;
                for c in 0..4 {
                    tmp[base + c] = acc[c] as f32;
                }
            }
        }
        plane = tmp;
    }
    let mut px = Vec::with_capacity(n);
    for v in &plane {
        px.push(u8c(f64::from(*v)));
    }
    Img { w: src.w, h: src.h, px }
}

/// Sliding-window box blur of integer radius over 4 interleaved channels.
fn box_pass(plane: &mut [f32], w: usize, h: usize, r: i64) {
    let mut tmp = vec![0f32; plane.len()];
    let win = 2 * r + 1;
    for y in 0..h {
        for c in 0..4usize {
            let row = y * w * 4;
            let mut acc = 0.0f64;
            for i in -r..=r {
                let sx = (i as usize).min(w - 1);
                acc += f64::from(plane[row + sx * 4 + c]);
            }
            for x in 0..w {
                tmp[row + x * 4 + c] = (acc / win as f64) as f32;
                let add = ((x as i64 + r) as usize).clamp(0, w - 1);
                let sub = ((x as i64 - r - 1) as usize).clamp(0, w - 1);
                acc += f64::from(plane[row + add * 4 + c]) - f64::from(plane[row + sub * 4 + c]);
            }
        }
    }
    plane.copy_from_slice(&tmp);
    for x in 0..w {
        for c in 0..4usize {
            let mut acc = 0.0f64;
            for i in -r..=r {
                let sy = (i.max(0) as usize).min(h - 1);
                acc += f64::from(plane[sy * w * 4 + x * 4 + c]);
            }
            for y in 0..h {
                tmp[y * w * 4 + x * 4 + c] = (acc / win as f64) as f32;
                let add = ((y as i64 + r) as usize).clamp(0, h - 1);
                let sub = ((y as i64 - r - 1) as usize).clamp(0, h - 1);
                acc += f64::from(plane[add * w * 4 + x * 4 + c]) - f64::from(plane[sub * w * 4 + x * 4 + c]);
            }
        }
    }
    plane.copy_from_slice(&tmp);
}

/// Premultiplied blur when `premul:1` — correct for edges of semi-transparent art.
fn blur(img: &Img, sigma: f64, mode: &str, premul: bool) -> Img {
    if !premul {
        return blur_planes(img, sigma, mode);
    }
    let mut p = img.clone();
    for q in p.px.chunks_exact_mut(4) {
        let a = q[3] as f64 / 255.0;
        for c in 0..3 {
            q[c] = u8c(f64::from(q[c]) * a);
        }
    }
    let mut b = blur_planes(&p, sigma, mode);
    for q in b.px.chunks_exact_mut(4) {
        let a = q[3] as f64 / 255.0;
        if a > 1e-6 {
            for c in 0..3 {
                q[c] = u8c(f64::from(q[c]) / a);
            }
        }
    }
    b
}

// ------------------------------------------------------------------ geometry ops

/// Resolve a target size from `w`/`h`/`size:"WxH"`/`ratio:"16:9"`.
/// `preserve` fills the missing axis from the source aspect ratio (resize),
/// otherwise the missing axis stays at the current value (pad, extend).
fn wh_from(args: &Value, cur: (u32, u32), preserve: bool) -> R<Option<(u32, u32)>> {
    let sw = f64::from(cur.0);
    let sh = f64::from(cur.1);
    let aw = util::f(args, "w").or_else(|| util::f(args, "width"));
    let ah = util::f(args, "h").or_else(|| util::f(args, "height"));
    if aw.is_some() || ah.is_some() {
        let mut w = aw.unwrap_or(sw);
        let mut h = ah.unwrap_or(sh);
        if let Some(r) = asv(args, &["ratio", "ar"]).and_then(parse_ratio_any) {
            if aw.is_none() {
                w = h * r.0 / r.1;
            } else if ah.is_none() {
                h = w * r.1 / r.0;
            }
        } else if preserve {
            if aw.is_none() {
                w = h * sw / sh;
            } else if ah.is_none() {
                h = w * sh / sw;
            }
        }
        if w <= 0.0 || h <= 0.0 || !w.is_finite() || !h.is_finite() {
            return Err(inval(format!("bad size {w}x{h}")));
        }
        return Ok(Some((w.round().max(1.0) as u32, h.round().max(1.0) as u32)));
    }
    if let Some(s) = asv(args, &["size", "to", "wh", "px", "xy", "target"]) {
        let both = s.contains('x') || s.contains('X') || s.contains(',') || s.contains(':') || s.contains('×');
        if let Some((w, h)) = parse_size(s) {
            if preserve && !both {
                // a bare number is the longest side
                let sc = (f64::from(w) / sw.max(sh)).min(f64::from(h) / sh.max(sw));
                let sc = sc.max(0.0001);
                return Ok(Some((
                    (sw * sc).round().max(1.0) as u32,
                    (sh * sc).round().max(1.0) as u32,
                )));
            }
            return Ok(Some((w, h)));
        }
        if let Some(r) = parse_ratio_any(s) {
            let base = sw.max(sh);
            let (w, h) = geom::parse_ratio(&format!("{}:{}", r.0, r.1), base).unwrap_or((base, base));
            return Ok(Some((w.round().max(1.0) as u32, h.round().max(1.0) as u32)));
        }
        return Err(bad(format!("cannot read a size from \"{s}\""))
            .hint("use \"WxH\" (800x600), \"1:1\", or {w:800,h:600}"));
    }
    if let Some(r) = asv(args, &["ratio", "ar"]).and_then(parse_ratio_any) {
        let base = sw.max(sh);
        let (w, h) = geom::parse_ratio(&format!("{}:{}", r.0, r.1), base).unwrap_or((base, base));
        return Ok(Some((w.round().max(1.0) as u32, h.round().max(1.0) as u32)));
    }
    Ok(None)
}

/// `"400x400"`, `"400"`, `"400,400"` -> explicit pixel size.
fn parse_size(s: &str) -> Option<(u32, u32)> {
    let t = s.trim().to_lowercase().replace(' ', "x").replace(',', "x").replace("×", "x");
    if let Some((a, b)) = t.split_once('x') {
        let (w, h) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
        if w > 0.0 && h > 0.0 {
            return Some((w.round().max(1.0) as u32, h.round().max(1.0) as u32));
        }
        return None;
    }
    let one = t.parse::<f64>().ok()?;
    if one > 0.0 {
        return Some((one.round() as u32, one.round() as u32));
    }
    None
}

/// `"16:9"` -> (16.0, 9.0)
fn parse_ratio_any(s: &str) -> Option<(f64, f64)> {
    let t = s.trim().to_lowercase();
    if let Some((a, b)) = t.split_once(':') {
        let (x, y) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
        if x > 0.0 && y > 0.0 {
            return Some((x, y));
        }
    }
    None
}

fn ratio_of(s: &str) -> Option<f64> {
    parse_ratio_any(s).map(|(a, b)| a / b)
}

fn op_crop(img: Img, args: &Value) -> R<(Img, Value)> {
    let (sw, sh) = (img.w as i64, img.h as i64);
    let mut x = ai(args, &["x", "left"], -1);
    let mut y = ai(args, &["y", "top"], -1);
    let mut cw: i64;
    let mut ch: i64;
    let mut clamped_ratio = false;
    if let Some(s) = asv(args, &["size", "wh", "to"]).and_then(parse_size) {
        cw = i64::from(s.0);
        ch = i64::from(s.1);
    } else if util::f(args, "w").is_some() || util::f(args, "h").is_some() {
        let (w, h) = wh_from(args, (img.w, img.h), false)?.unwrap_or((img.w, img.h));
        cw = i64::from(w);
        ch = i64::from(h);
    } else if let Some(r) = asv(args, &["ratio", "ar", "aspect"]).and_then(ratio_of) {
        // largest region of that ratio, at the anchor
        let cur = sw as f64 / sh as f64;
        if cur > r {
            ch = sh;
            cw = (sh as f64 * r).round().max(1.0) as i64;
        } else {
            cw = sw;
            ch = (sw as f64 / r).round().max(1.0) as i64;
        }
        clamped_ratio = true;
    } else {
        return Err(bad("crop needs a size")
            .hint("crop 400x400, crop 1:1, or {x:10,y:10,w:400,h:400}; clamped to the source bounds"));
    }
    let a = anchor_of(args);
    if x < 0 || y < 0 {
        let (ax, ay) = place(img.w, img.h, cw.max(1) as u32, ch.max(1) as u32, a);
        if x < 0 {
            x = ax as i64;
        }
        if y < 0 {
            y = ay as i64;
        }
    }
    cw = cw.max(1);
    ch = ch.max(1);
    x = x.clamp(0, (sw - 1).max(0));
    y = y.clamp(0, (sh - 1).max(0));
    cw = cw.min(sw - x).max(1);
    ch = ch.min(sh - y).max(1);
    let mut out = Img { w: cw as u32, h: ch as u32, px: vec![0u8; (cw * ch * 4) as usize] };
    for row in 0..ch {
        let s = (((y + row) * sw + x) * 4) as usize;
        let e = s + (cw * 4) as usize;
        let d = (row * cw * 4) as usize;
        out.px[d..d + (cw * 4) as usize].copy_from_slice(&img.px[s..e]);
    }
    let mut rep = json!({ "crop": { "x": x, "y": y, "w": cw, "h": ch } });
    if clamped_ratio {
        rep["clamped"] = json!(true);
    }
    Ok((out, rep))
}

fn op_resize(img: Img, args: &Value) -> R<(Img, Value)> {
    let (tw, th) = match wh_from(args, (img.w, img.h), true)? {
        Some((w, h)) => (w, h),
        None => {
            return Err(bad("resize needs a size")
                .hint("resize 800x600 | resize 800 (keeps aspect) | {w:800,fit:\"cover\",anchor:\"top\"}"))
        }
    };
    check_size(tw, th)?;
    let fitname = asv(args, &["fit", "mode"]).unwrap_or("cover");
    let fm = geom::fit_of(fitname).ok_or_else(|| {
        Fail::new("INVALID_VALUE", format!("unknown fit \"{fitname}\""))
            .hint("fit: contain | cover | fill | actual")
    })?;
    let a = anchor_of(args);
    let bg = col_or(args, &["bg", "color", "fill"], TRANSPARENT);
    let filt = asv(args, &["filter"]).unwrap_or(if matches!(fm, Fit::Contain | Fit::Actual) { "lanczos" } else { "catmull" });
    if fm == Fit::Actual {
        // never enlarge: fit inside the requested canvas at 1:1 maximum
        let s = (f64::from(tw) / f64::from(img.w)).min(f64::from(th) / f64::from(img.h)).min(1.0);
        let nw = (f64::from(img.w) * s).round().max(1.0) as u32;
        let nh = (f64::from(img.h) * s).round().max(1.0) as u32;
        let scaled = resample(&img, nw, nh, filt)?;
        if nw == tw && nh == th {
            return Ok((scaled, json!({ "fit": "actual", "w": tw, "h": th })));
        }
        let mut out = Img::filled(tw, th, bg)?;
        let (x, y) = place(tw, th, nw, nh, a);
        paste(&mut out, i64::from(x), i64::from(y), &scaled, 1.0);
        return Ok((out, json!({ "fit": "actual", "w": tw, "h": th, "inner": [nw, nh] })));
    }
    // geometry::fit does the scaling; the anchor only decides where it lands.
    let placed = geom::fit(
        Rect::from_size(f64::from(img.w), f64::from(img.h)),
        Rect::from_size(f64::from(tw), f64::from(th)),
        fm,
    );
    let (nw, nh) = if fm == Fit::Fill {
        (tw, th)
    } else {
        (
            placed.w.round().max(1.0) as u32,
            placed.h.round().max(1.0) as u32,
        )
    };
    let scaled = resample(&img, nw, nh, filt)?;
    if nw == tw && nh == th {
        return Ok((scaled, json!({ "fit": fit_name(fm), "w": tw, "h": th })));
    }
    let out = if fm == Fit::Cover {
        // the scaled image overflows the canvas: cut the excess at the anchor
        let cx = ((nw as i64 - tw as i64) as f64 * anchor_frac(a).0).round().max(0.0) as u32;
        let cy = ((nh as i64 - th as i64) as f64 * anchor_frac(a).1).round().max(0.0) as u32;
        let (o, _) = op_crop(scaled, &json!({ "x": cx, "y": cy, "w": tw, "h": th }))?;
        o
    } else {
        let mut canvas = Img::filled(tw, th, bg)?;
        let (x, y) = place(tw, th, nw, nh, a);
        paste(&mut canvas, i64::from(x), i64::from(y), &scaled, 1.0);
        canvas
    };
    Ok((out, json!({ "fit": fit_name(fm), "w": tw, "h": th, "inner": [nw, nh] })))
}

fn fit_name(f: Fit) -> &'static str {
    match f {
        Fit::Contain => "contain",
        Fit::Cover => "cover",
        Fit::Fill => "fill",
        Fit::Actual => "actual",
    }
}

fn anchor_frac(a: Origin) -> (f64, f64) {
    match a {
        Origin::TopLeft => (0.0, 0.0),
        Origin::Top => (0.5, 0.0),
        Origin::TopRight => (1.0, 0.0),
        Origin::Left => (0.0, 0.5),
        Origin::Center => (0.5, 0.5),
        Origin::Right => (1.0, 0.5),
        Origin::BottomLeft => (0.0, 1.0),
        Origin::Bottom => (0.5, 1.0),
        Origin::BottomRight => (1.0, 1.0),
    }
}

fn crop_area(img: &Img, x: u32, y: u32, w: u32, h: u32) -> R<Img> {
    let (o, _) = op_crop(img.clone(), &json!({ "x": x, "y": y, "w": w, "h": h }))?;
    Ok(o)
}

fn op_scale(img: Img, args: &Value) -> R<(Img, Value)> {
    let mut f = af(args, &["by", "factor", "scale", "amount", "s"], 0.0);
    if f == 0.0 {
        if let Some(s) = asv(args, &["by", "factor", "scale"]) {
            let t = s.trim().to_lowercase();
            if let Some(p) = t.strip_suffix('%') {
                f = p.trim().parse::<f64>().unwrap_or(0.0) / 100.0;
            } else if let Some(x) = t.strip_suffix('x') {
                f = x.trim().parse::<f64>().unwrap_or(0.0);
            } else {
                f = t.parse::<f64>().unwrap_or(0.0);
            }
        }
    }
    if f <= 0.0 {
        // `scale` also accepts an explicit width/height, like resize
        if let Some(v) = util::f(args, "w") {
            f = v / f64::from(img.w);
        } else if let Some(v) = util::f(args, "h") {
            f = v / f64::from(img.h);
        }
    }
    if f <= 0.0 || !f.is_finite() {
        return Err(bad("scale needs a positive factor").hint("scale 0.5, scale 150%, or {by:2}"));
    }
    let filt = if f >= 1.0 { "catmull" } else { "lanczos" };
    let nw = (f64::from(img.w) * f).round().max(1.0) as u32;
    let nh = (f64::from(img.h) * f).round().max(1.0) as u32;
    let out = resample(&img, nw, nh, filt)?;
    Ok((out, json!({ "factor": util::round(f, 4), "w": nw, "h": nh })))
}

fn op_pad(img: Img, args: &Value) -> R<(Img, Value)> {
    let uniform = au(args, &["px", "all", "size", "pad"], 0);
    let mut t = au(args, &["t", "top", "pt"], uniform);
    let mut r = au(args, &["r", "right", "pr"], uniform);
    let mut b = au(args, &["b", "bottom", "pb"], uniform);
    let mut l = au(args, &["l", "left", "pl"], uniform);
    if let Some((w, h)) = wh_from(args, (img.w, img.h), false)? {
        // "pad into a WxH canvas": the leftover goes to the edges at the anchor
        let a = anchor_of(args);
        if w > img.w {
            let (x, _) = place(w, img.h.max(h), img.w, img.h, a);
            l = x;
            r = w - img.w - l;
        }
        if h > img.h {
            let (_, y) = place(img.w.max(w), h, img.w, img.h, a);
            t = y;
            b = h - img.h - t;
        }
    }
    let (nw, nh) = (img.w + l + r, img.h + t + b);
    check_size(nw, nh)?;
    let mode = asv(args, &["mode", "fill_mode"]).unwrap_or("color").to_lowercase();
    let c = if mode == "transparent" {
        TRANSPARENT
    } else {
        col_or(args, &["color", "c", "bg", "fill"], TRANSPARENT)
    };
    let mut out = Img::filled(nw, nh, c)?;
    paste(&mut out, i64::from(l), i64::from(t), &img, 1.0);
    let (w, h) = (nw as usize, nh as usize);
    let (il, it) = (l as usize, t as usize);
    let (iw, ih) = (img.w as usize, img.h as usize);
    let mirror = mode == "mirror" || mode == "reflect";
    let edge = mode == "edge" || mode == "stretch";
    if mirror || edge {
        // fill the padded band by clamping (edge) or folding (mirror) the source
        for y in 0..h {
            let v = y as i64 - it as i64;
            let sv = if edge {
                v.clamp(0, ih as i64 - 1) as usize
            } else {
                fold(v, ih as i64)
            };
            for x in 0..w {
                let u = x as i64 - il as i64;
                let su = if edge {
                    u.clamp(0, iw as i64 - 1) as usize
                } else {
                    fold(u, iw as i64)
                };
                out.set(x, y, img.at(su, sv));
            }
        }
    }
    Ok((out, json!({ "pad": [l, t, r, b], "w": nw, "h": nh })))
}

/// Mirror-fold an out-of-range index into `0..n`.
fn fold(v: i64, n: i64) -> usize {
    if n <= 0 {
        return 0;
    }
    let per = 2 * n;
    let mut m = v.rem_euclid(per);
    if m >= n {
        m = per - 1 - m;
    }
    m.clamp(0, (n - 1).max(0)) as usize
}

fn op_flip(img: Img, args: &Value) -> R<(Img, Value)> {
    let dir = asv(args, &["dir", "axis", "direction", "to"]).unwrap_or("h").to_lowercase();
    let (fh, fv) = match dir.as_str() {
        "h" | "horizontal" | "x" | "left-right" => (true, false),
        "v" | "vertical" | "y" | "up-down" => (false, true),
        "both" | "b" | "hv" | "180" => (true, true),
        other => {
            return Err(bad(format!("flip: unknown direction \"{other}\""))
                .hint("flip h | v | both"))
        }
    };
    let (w, h) = (img.w as usize, img.h as usize);
    let mut px = img.px.clone();
    for y in 0..h {
        for x in 0..w / 2 {
            let ox = if fh { w - 1 - x } else { x };
            let oy = if fv { h - 1 - y } else { y };
            let a = (y * w + x) * 4;
            let b = (oy * w + ox) * 4;
            if a == b {
                continue;
            }
            px.swap(a, b);
            px.swap(a + 1, b + 1);
            px.swap(a + 2, b + 2);
            px.swap(a + 3, b + 3);
        }
    }
    Ok((Img { w: img.w, h: img.h, px }, json!({ "flip": dir })))
}

fn op_rotate(img: Img, args: &Value) -> R<(Img, Value)> {
    let mut deg = af(args, &["angle", "deg", "rotation", "rot"], 0.0);
    if deg == 0.0 {
        if let Some(s) = asv(args, &["angle", "to", "by"]) {
            deg = s.trim().trim_end_matches("deg").trim_end_matches('°').parse().unwrap_or(0.0);
        }
    }
    let norm = deg.rem_euclid(360.0);
    if norm.abs() < 1e-9 {
        return Ok((img, json!({ "rotate": deg })));
    }
    for q in [90.0, 180.0, 270.0] {
        if (norm - q).abs() < 1e-6 {
            return Ok(rotate_quarter(img, q as u32));
        }
    }
    let expand = ab(args, &["expand", "fit"], true) && !ab(args, &["crop"], false);
    let bg = col_or(args, &["bg", "color", "fill"], TRANSPARENT);
    let rad = norm * std::f64::consts::PI / 180.0;
    let (sin, cos) = (rad.sin().abs(), rad.cos().abs());
    let (sw, sh) = (f64::from(img.w), f64::from(img.h));
    let (nw, nh) = if expand {
        (
            (sw * cos + sh * sin).ceil().max(1.0) as u32,
            (sw * sin + sh * cos).ceil().max(1.0) as u32,
        )
    } else {
        (img.w, img.h)
    };
    check_size(nw, nh)?;
    let mut out = Img::filled(nw, nh, bg)?;
    let scx = sw / 2.0;
    let scy = sh / 2.0;
    let dcx = f64::from(nw) / 2.0;
    let dcy = f64::from(nh) / 2.0;
    let (cs, sn) = (rad.cos(), rad.sin());
    let bgf = [f64::from(bg[0]), f64::from(bg[1]), f64::from(bg[2]), f64::from(bg[3])];
    let nearest = matches!(asv(args, &["filter", "mode"]), Some("nearest"));
    for y in 0..nh as usize {
        for x in 0..nw as usize {
            let dx = f64::from(x as u32) + 0.5 - dcx;
            let dy = f64::from(y as u32) + 0.5 - dcy;
            let sx = dx * cs + dy * sn + scx;
            let sy = -dx * sn + dy * cs + scy;
            let p = if nearest {
                let ix = sx.floor() as i64;
                let iy = sy.floor() as i64;
                if ix < 0 || iy < 0 || ix >= img.w as i64 || iy >= img.h as i64 {
                    bg
                } else {
                    img.at(ix as usize, iy as usize)
                }
            } else {
                let v = sample_bil(&img, sx, sy, bgf);
                [u8c(v[0]), u8c(v[1]), u8c(v[2]), u8c(v[3])]
            };
            out.set(x, y, p);
        }
    }
    Ok((out, json!({ "rotate": util::round(deg, 3), "expand": expand })))
}

fn rotate_quarter(img: Img, q: u32) -> (Img, Value) {
    let (w, h) = (img.w as usize, img.h as usize);
    let (nw, nh) = if q == 180 { (w, h) } else { (h, w) };
    let mut px = vec![0u8; nw * nh * 4];
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = match q {
                90 => (h - 1 - y, x),
                180 => (w - 1 - x, h - 1 - y),
                _ => (y, w - 1 - x),
            };
            let s = (y * w + x) * 4;
            let d = (dy * nw + dx) * 4;
            px[d..d + 4].copy_from_slice(&img.px[s..s + 4]);
        }
    }
    (Img { w: nw as u32, h: nh as u32, px }, json!({ "rotate": q }))
}

/// Anti-aliased rounded-rectangle coverage at (px,py): 1.0 inside, 0.0 outside,
/// blended over one pixel across the corner arcs. Radii are `[tl, tr, br, bl]`.
fn corner_alpha(px: f64, py: f64, x0: f64, y0: f64, x1: f64, y1: f64, r: [f64; 4]) -> f64 {
    if px < x0 || py < y0 || px >= x1 || py >= y1 {
        return 0.0;
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let mx = w.min(h) / 2.0;
    let (lx, ly) = (px - x0, py - y0);
    let (tl, tr, br, bl) = (r[0].min(mx).max(0.0), r[1].min(mx).max(0.0), r[2].min(mx).max(0.0), r[3].min(mx).max(0.0));
    let corners = [
        (tl, tl, tl, lx <= tl, ly <= tl),
        (tr, w - tr, tr, w - lx <= tr, ly <= tr),
        (br, w - br, h - br, w - lx <= br, h - ly <= br),
        (bl, bl, h - bl, lx <= bl, h - ly <= bl),
    ];
    for (rad, ccx, ccy, inx, iny) in corners {
        if rad <= 0.0 || !inx || !iny {
            continue;
        }
        let d = ((lx - ccx).powi(2) + (ly - ccy).powi(2)).sqrt();
        return (0.5 + (rad - d)).clamp(0.0, 1.0);
    }
    1.0
}

fn round_alpha_grid(img: &Img, radii: [f64; 4]) -> Img {
    let mut out = img.clone();
    let (w, h) = (img.w as usize, img.h as usize);
    for y in 0..h {
        for x in 0..w {
            let a = corner_alpha(
                x as f64 + 0.5,
                y as f64 + 0.5,
                0.0,
                0.0,
                w as f64,
                h as f64,
                radii,
            );
            if a >= 1.0 {
                continue;
            }
            let i = (y * w + x) * 4;
            out.px[i + 3] = u8c(f64::from(out.px[i + 3]) * a);
        }
    }
    out
}

fn radii_for(img: &Img, args: &Value) -> [f64; 4] {
    let (w, h) = (f64::from(img.w), f64::from(img.h));
    let mut base = 0.0;
    if let Some(p) = util::f(args, "pct").or_else(|| util::f(args, "percent")) {
        // 50% of the short side is a full circle; 0..1 is read as a fraction
        let pc = if p <= 1.0 { p * 100.0 } else { p };
        base = w.min(h) * pc / 100.0;
    }
    if let Some(r) = util::f(args, "radius").or_else(|| util::f(args, "r")).or_else(|| util::f(args, "rx")) {
        base = r;
    }
    let mut rr = [base, base, base, base];
    for (i, k) in ["tl", "tr", "br", "bl"].iter().enumerate() {
        if let Some(v) = util::f(args, k) {
            rr[i] = v;
        }
    }
    if let Some(s) = asv(args, &["corners"]) {
        let parts: Vec<f64> = s
            .split(|c| c == ',' || c == ' ' || c == '/')
            .filter_map(|p| p.trim().parse::<f64>().ok())
            .collect();
        match parts.len() {
            1 => rr = [parts[0]; 4],
            2 => rr = [parts[0], parts[1], parts[0], parts[1]],
            4 => rr = [parts[0], parts[1], parts[2], parts[3]],
            3 => rr = [parts[0], parts[1], parts[2], parts[1]],
            _ => {}
        }
    }
    rr
}

fn op_round(img: Img, args: &Value) -> R<(Img, Value)> {
    let rr = radii_for(&img, args);
    if rr.iter().all(|v| *v <= 0.0) {
        return Err(bad("round needs a radius").hint("round 24  or  {pct:50}  (pct is of the short side)"));
    }
    let out = round_alpha_grid(&img, rr);
    Ok((out, json!({ "radius": rr.map(|v| util::round(v, 2)) })))
}

fn op_border(img: Img, args: &Value) -> R<(Img, Value)> {
    let width = af(args, &["width", "w", "px", "size"], 2.0).max(0.5);
    let color = col_or(args, &["color", "c", "stroke", "bg"], BLACK);
    let side = asv(args, &["side", "align", "position"]).unwrap_or("inside");
    let radius = af(args, &["radius", "r"], 0.0);
    let per = if radius > 0.0 { [radius; 4] } else { [0.0; 4] };
    let opacity = af(args, &["opacity", "o"], 1.0).clamp(0.0, 1.0);
    let (grow, inset) = match side {
        "outside" | "out" => (width.ceil() as u32, 0.0f64),
        "center" | "middle" => ((width / 2.0).ceil() as u32, width / 2.0),
        _ => (0, width),
    };
    let nw = img.w + grow * 2;
    let nh = img.h + grow * 2;
    check_size(nw, nh)?;
    let mut out = Img::new(nw, nh)?;
    if grow > 0 {
        paste(&mut out, i64::from(grow), i64::from(grow), &img, 1.0);
    } else {
        out = img.clone();
    }
    // ring coverage = outer rounded mask minus inner rounded mask
    let (w, h) = (nw as usize, nh as usize);
    let inner = Rect::new(
        f64::from(grow) + inset,
        f64::from(grow) + inset,
        (f64::from(img.w) - 2.0 * inset).max(0.0),
        (f64::from(img.h) - 2.0 * inset).max(0.0),
    );
    let rr_in = per.map(|v| (v - inset).max(0.0));
    for y in 0..h {
        for x in 0..w {
            let o = corner_alpha(
                x as f64 + 0.5,
                y as f64 + 0.5,
                f64::from(grow),
                f64::from(grow),
                f64::from(grow) + f64::from(img.w),
                f64::from(grow) + f64::from(img.h),
                per,
            );
            if o <= 0.0 {
                continue;
            }
            let i = corner_alpha(
                x as f64 + 0.5,
                y as f64 + 0.5,
                inner.x,
                inner.y,
                inner.x2(),
                inner.y2(),
                rr_in,
            );
            let cov = (o - i).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            let idx = (y * w + x) * 4;
            let mut d = [out.px[idx], out.px[idx + 1], out.px[idx + 2], out.px[idx + 3]];
            let mut s = color;
            s[3] = u8c(f64::from(color[3]) * cov);
            over(&mut d, s, opacity);
            out.px[idx..idx + 4].copy_from_slice(&d);
        }
    }
    Ok((out, json!({ "border": { "width": width, "side": side }, "w": nw, "h": nh })))
}

fn op_trim(img: Img, args: &Value) -> R<(Img, Value)> {
    let fuzz = af(args, &["fuzz", "tolerance", "tol", "threshold"], 8.0).clamp(0.0, 255.0);
    let key = match col(args, &["color", "c", "bg"]) {
        Some(c) => c,
        None => corner_colour(&img),
    };
    let (w, h) = (img.w as usize, img.h as usize);
    let mut minx = w;
    let mut miny = h;
    let mut maxx = 0usize;
    let mut maxy = 0usize;
    let transparent_mode = ab(args, &["alpha"], true);
    for y in 0..h {
        for x in 0..w {
            let p = img.at(x, y);
            let hit = if transparent_mode && p[3] == 0 {
                false
            } else {
                colour_dist(p, key) > fuzz
            };
            if hit {
                minx = minx.min(x);
                maxx = maxx.max(x);
                miny = miny.min(y);
                maxy = maxy.max(y);
            }
        }
    }
    if minx > maxx || miny > maxy {
        return Err(Fail::new("INVALID_VALUE", "trim: the whole image is the border colour")
            .hint("lower `fuzz` or pass an explicit {color:\"#rrggbb\"}"));
    }
    let cw = (maxx - minx + 1) as u32;
    let ch = (maxy - miny + 1) as u32;
    let (cropped, _) = op_crop(
        img,
        &json!({ "x": minx, "y": miny, "w": cw, "h": ch }),
    )?;
    Ok((cropped, json!({ "trim": { "l": minx, "t": miny, "r": w - maxx - 1, "b": h - maxy - 1 }, "w": cw, "h": ch })))
}

/// Median of the four corner pixels: the classic "this is the background" guess.
fn corner_colour(img: &Img) -> [u8; 4] {
    let (w, h) = (img.w as usize, img.h as usize);
    let pts = [
        img.at(0, 0),
        img.at(w - 1, 0),
        img.at(0, h - 1),
        img.at(w - 1, h - 1),
    ];
    let mut out = [0u8; 4];
    for c in 0..4 {
        let mut v: Vec<u8> = pts.iter().map(|p| p[c]).collect();
        v.sort_unstable();
        out[c] = v[v.len() / 2];
    }
    out
}

/// Weighted RGB distance (0..~441): closer to perceived difference than Euclid.
fn colour_dist(a: [u8; 4], b: [u8; 4]) -> f64 {
    let (dr, dg, db) = (
        f64::from(a[0]) - f64::from(b[0]),
        f64::from(a[1]) - f64::from(b[1]),
        f64::from(a[2]) - f64::from(b[2]),
    );
    (2.0 * dr * dr + 4.0 * dg * dg + 3.0 * db * db).sqrt()
}

fn op_border_trim(img: Img, args: &Value) -> R<(Img, Value)> {
    if let Some(s) = asv(args, &["side"]) {
        let w = au(args, &["width", "px", "w"], 1);
        let (mut x, mut y, mut cw, mut ch) = (0i64, 0i64, img.w as i64, img.h as i64);
        match s {
            "t" | "top" => {
                y = w as i64;
                ch -= w as i64;
            }
            "b" | "bottom" => ch -= w as i64,
            "l" | "left" => {
                x = w as i64;
                cw -= w as i64;
            }
            "r" | "right" => cw -= w as i64,
            other => {
                return Err(bad(format!("border-trim side \"{other}\""))
                    .hint("side: top|right|bottom|left, or omit for all four"))
            }
        }
        if cw < 1 || ch < 1 {
            return Err(inval("border-trim would remove the whole image"));
        }
        let (o, _) = op_crop(img, &json!({ "x": x, "y": y, "w": cw, "h": ch }))?;
        return Ok((o, json!({ "trimmed": { s: w } })));
    }
    let u = au(args, &["width", "px", "all"], 0);
    let mut t = au(args, &["t", "top"], u);
    let mut r = au(args, &["r", "right"], u);
    let mut b = au(args, &["b", "bottom"], u);
    let mut l = au(args, &["l", "left"], u);
    if t + r + b + l == 0 {
        // auto: measure how many outer rows/cols are uniform within fuzz, then cut them
        let fuzz = af(args, &["fuzz", "tolerance", "f"], 6.0);
        let key = col_or(args, &["color", "c"], corner_colour(&img));
        let (w, h) = (img.w as usize, img.h as usize);
        let row_uniform = |y: usize| {
            (0..w).all(|x| colour_dist(img.at(x, y), key) <= fuzz)
        };
        let col_uniform = |x: usize| {
            (0..h).all(|y| colour_dist(img.at(x, y), key) <= fuzz)
        };
        let (mut ti, mut ri, mut bi, mut li) = (t as usize, r as usize, b as usize, l as usize);
        while ti < h && row_uniform(ti) {
            ti += 1;
        }
        while bi + ti < h && row_uniform(h - 1 - bi) {
            bi += 1;
        }
        while li < w && col_uniform(li) {
            li += 1;
        }
        while ri + li < w && col_uniform(w - 1 - ri) {
            ri += 1;
        }
        t = ti as u32;
        r = ri as u32;
        b = bi as u32;
        l = li as u32;
    }
    let cw = img.w as i64 - l as i64 - r as i64;
    let ch = img.h as i64 - t as i64 - b as i64;
    if cw < 1 || ch < 1 {
        return Err(inval("border-trim would remove the whole image").hint("lower the width / fuzz"));
    }
    let (o, _) = op_crop(img, &json!({ "x": l, "y": t, "w": cw, "h": ch }))?;
    Ok((o, json!({ "border_trim": [l, t, r, b] })))
}

// ------------------------------------------------------------------ colour math

fn s2l(v: f64) -> f64 {
    let x = v / 255.0;
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}
fn l2s(v: f64) -> f64 {
    let x = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.max(0.0).powf(1.0 / 2.4) - 0.055 };
    (x * 255.0).clamp(0.0, 255.0)
}
/// Rec.709 luma on gamma-encoded values — what a UI slider means by "brightness".
#[inline]
fn luma(r: u8, g: u8, b: u8) -> f64 {
    0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b)
}
/// Relative luminance (linear-light), used for contrast-sensitive ops.
#[inline]
fn luma_lin(r: u8, g: u8, b: u8) -> f64 {
    0.2126 * s2l(f64::from(r)) + 0.7152 * s2l(f64::from(g)) + 0.0722 * s2l(f64::from(b))
}

fn rgb2hsl(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    let (rf, gf, bf) = (f64::from(r) / 255.0, f64::from(g) / 255.0, f64::from(b) / 255.0);
    let mx = rf.max(gf).max(bf);
    let mn = rf.min(gf).min(bf);
    let l = (mx + mn) / 2.0;
    let d = mx - mn;
    if d <= 1e-9 {
        return (0.0, 0.0, l);
    }
    let s = if l > 0.5 { d / (2.0 - mx - mn) } else { d / (mx + mn) };
    let h = if (mx - rf).abs() < 1e-9 {
        (gf - bf) / d
    } else if (mx - gf).abs() < 1e-9 {
        2.0 + (bf - rf) / d
    } else {
        4.0 + (rf - gf) / d
    };
    let mut hh = h * 60.0;
    if hh < 0.0 {
        hh += 360.0;
    }
    (hh / 360.0, s, l)
}

fn hsl1(p: f64, q: f64, mut t: f64) -> f64 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

fn hsl2rgb(h: f64, s: f64, l: f64) -> [f64; 3] {
    if s <= 1e-9 {
        return [l * 255.0, l * 255.0, l * 255.0];
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    [
        hsl1(p, q, h + 1.0 / 3.0) * 255.0,
        hsl1(p, q, h) * 255.0,
        hsl1(p, q, h - 1.0 / 3.0) * 255.0,
    ]
}

/// Tanner Helland black-body approximation: the colour of a light at `k` kelvin,
/// normalised to 0..1 per channel. 6500 K lands on neutral white.
fn kelvin_colour(k: f64) -> [f64; 3] {
    let t = (k.clamp(1000.0, 40_000.0)) / 100.0;
    let r = if t <= 66.0 {
        255.0
    } else {
        329.698_727_446 * (t - 60.0).powf(-0.133_204_759_2)
    };
    let g = if t <= 66.0 {
        99.470_802_586_1 * t.ln() - 161.119_568_166_1
    } else {
        288.122_169_528_3 * (t - 60.0).powf(-0.075_514_849_2)
    };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7
    };
    [r.clamp(0.0, 255.0) / 255.0, g.clamp(0.0, 255.0) / 255.0, b.clamp(0.0, 255.0) / 255.0]
}

/// Linear-light multipliers that render the picture as if lit at `k` kelvin.
/// Normalised so the geometric mean is 1 (no accidental brightness change) and
/// 6500 K is exactly neutral.
fn kelvin_multipliers(k: f64) -> [f64; 3] {
    let c = kelvin_colour(k);
    let n = kelvin_colour(6500.0);
    let m = [c[0] / n[0], c[1] / n[1], c[2] / n[2]];
    let g = (m[0] * m[1] * m[2]).max(1e-9).powf(1.0 / 3.0);
    [m[0] / g, m[1] / g, m[2] / g]
}

// ------------------------------------------------------------------ adjust

fn op_adjust(img: Img, args: &Value) -> R<(Img, Value)> {
    let brightness = af(args, &["brightness", "bright", "b"], 0.0);
    let exposure = af(args, &["exposure", "ev"], 0.0);
    let contrast = af(args, &["contrast", "c"], 1.0);
    let sat = af(args, &["saturation", "sat", "s"], 1.0);
    let vibrance = af(args, &["vibrance", "vib"], 0.0);
    let hue = af(args, &["hue", "hue_deg", "rotate"], 0.0);
    let gamma = af(args, &["gamma", "g"], 1.0);
    let kelvin = af(args, &["temperature", "temp", "kelvin", "k"], 6500.0);
    let tint = af(args, &["tint", "green_magenta", "gm"], 0.0);
    let posterize = au(args, &["posterize", "levels", "poster"], 0);
    let sepia = ab(args, &["sepia"], false) || af(args, &["sepia_amount", "sepiam"], 0.0) > 0.0;
    let sepia_amt = if ab(args, &["sepia"], false) { 1.0 } else { af(args, &["sepia_amount", "sepiam"], 0.0) };
    let invert = ab(args, &["invert"], false);
    let mono = ab(args, &["mono", "grayscale"], false);
    let black = norm_0_255(args, &["black", "black_point", "cutoff", "lo"])?;
    let white = norm_0_255(args, &["white", "white_point", "capped", "hi"])?;
    let mid = af(args, &["midtone", "midtone_point", "pivot"], 128.0);
    let preserve_alpha = !ab(args, &["alpha"], false);
    if (kelvin < 1000.0 || kelvin > 40_000.0) && (kelvin - 6500.0).abs() > 0.01 {
        return Err(inval(format!("temperature {kelvin} K is out of range"))
            .hint("temperature is 1000..40000 K; 6500 is neutral"));
    }
    let km = kelvin_multipliers(kelvin);
    let tint_g = 1.0 + tint.clamp(-1.0, 1.0);
    let exp_mul = 2f64.powf(exposure);
    let span = (white - black).max(1.0);
    let mut out = img.clone();
    for p in out.px.chunks_exact_mut(4) {
        let (mut r, mut g, mut b) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        // 1. level remap (black / white point)
        if black > 0.0 || white < 255.0 {
            r = ((r - black) * 255.0 / span).clamp(0.0, 255.0);
            g = ((g - black) * 255.0 / span).clamp(0.0, 255.0);
            b = ((b - black) * 255.0 / span).clamp(0.0, 255.0);
        }
        // 2. brightness lifts in display space, 3. contrast pivots around `mid`
        if brightness != 0.0 {
            let add = brightness * 255.0;
            r += add;
            g += add;
            b += add;
        }
        if (contrast - 1.0).abs() > 1e-9 {
            r = mid + (r - mid) * contrast;
            g = mid + (g - mid) * contrast;
            b = mid + (b - mid) * contrast;
        }
        r = r.clamp(0.0, 255.0);
        g = g.clamp(0.0, 255.0);
        b = b.clamp(0.0, 255.0);
        // 4. gamma
        if (gamma - 1.0).abs() > 1e-9 {
            let gm = 1.0 / gamma.max(0.01);
            r = 255.0 * (r / 255.0).powf(gm);
            g = 255.0 * (g / 255.0).powf(gm);
            b = 255.0 * (b / 255.0).powf(gm);
        }
        // 5. exposure, temperature and tint happen in linear light
        if (exposure - 0.0).abs() > 1e-9
            || (kelvin - 6500.0).abs() > 1e-9
            || tint.abs() > 1e-9
        {
            let (mut lr, mut lg, mut lb) = (s2l(r), s2l(g), s2l(b));
            lr *= exp_mul * km[0];
            lg *= exp_mul * km[1] * tint_g;
            lb *= exp_mul * km[2];
            r = l2s(lr);
            g = l2s(lg);
            b = l2s(lb);
        }
        // 6. hue rotation, saturation and vibrance in HSL around luma
        if hue.abs() > 1e-9 || (sat - 1.0).abs() > 1e-9 || vibrance.abs() > 1e-9 {
            let (h0, s0, l0) = rgb2hsl(u8c(r), u8c(g), u8c(b));
            let mut h = h0;
            let mut s = s0;
            if hue.abs() > 1e-9 {
                h = (h0 + hue / 360.0).rem_euclid(1.0);
            }
            if (sat - 1.0).abs() > 1e-9 {
                s = (s0 * sat).clamp(0.0, 1.0);
            }
            if vibrance.abs() > 1e-9 {
                // muted pixels get the boost, already-saturated ones get almost none
                s = (s + vibrance * (1.0 - s) * s * 2.0).clamp(0.0, 1.0);
            }
            let v = hsl2rgb(h, s, l0);
            r = v[0];
            g = v[1];
            b = v[2];
        }
        let (mut rr, mut gg, mut bb) = (u8c(r), u8c(g), u8c(b));
        if mono {
            let y = u8c(luma(rr, gg, bb));
            (rr, gg, bb) = (y, y, y);
        }
        if sepia {
            let sr = u8c(0.393 * f64::from(rr) + 0.769 * f64::from(gg) + 0.189 * f64::from(bb));
            let sg = u8c(0.349 * f64::from(rr) + 0.686 * f64::from(gg) + 0.168 * f64::from(bb));
            let sb = u8c(0.272 * f64::from(rr) + 0.534 * f64::from(gg) + 0.131 * f64::from(bb));
            let t = sepia_amt.clamp(0.0, 1.0);
            rr = u8c(f64::from(rr) * (1.0 - t) + f64::from(sr) * t);
            gg = u8c(f64::from(gg) * (1.0 - t) + f64::from(sg) * t);
            bb = u8c(f64::from(bb) * (1.0 - t) + f64::from(sb) * t);
        }
        if posterize >= 2 {
            let step = 255.0 / (f64::from(posterize) - 1.0);
            rr = u8c((f64::from(rr) / step).round() * step);
            gg = u8c((f64::from(gg) / step).round() * step);
            bb = u8c((f64::from(bb) / step).round() * step);
        }
        if invert {
            rr = 255 - rr;
            gg = 255 - gg;
            bb = 255 - bb;
        }
        p[0] = rr;
        p[1] = gg;
        p[2] = bb;
        if invert && !preserve_alpha {
            p[3] = 255 - p[3];
        }
    }
    Ok((out, json!({ "adjusted": true })))
}

/// Accept a 0..1 or 0..255 level and normalise to 0..255.
fn norm_0_255(args: &Value, keys: &[&str]) -> R<f64> {
    for k in keys {
        if let Some(v) = util::f(args, k) {
            if v < 0.0 || v > 255.0 {
                return Err(inval(format!("{k} must be 0..255 (or 0..1)")));
            }
            return Ok(if v <= 1.0 { v * 255.0 } else { v });
        }
    }
    Ok(if keys[0].contains("black") || keys[0].contains("lo") || keys[0].contains("cutoff") {
        0.0
    } else {
        255.0
    })
}

fn op_gray(img: Img, args: &Value) -> R<(Img, Value)> {
    let linear = ab(args, &["linear"], false);
    let mut out = img.clone();
    for p in out.px.chunks_exact_mut(4) {
        let y = if linear {
            l2s(luma_lin(p[0], p[1], p[2]))
        } else {
            luma(p[0], p[1], p[2])
        };
        let y = u8c(y);
        p[0] = y;
        p[1] = y;
        p[2] = y;
    }
    Ok((out, json!({ "gray": if linear { "linear-luma" } else { "rec709" } })))
}

fn op_invert(img: Img, args: &Value) -> R<(Img, Value)> {
    let with_alpha = ab(args, &["alpha", "invert_alpha"], false);
    let mut out = img.clone();
    for p in out.px.chunks_exact_mut(4) {
        p[0] = 255 - p[0];
        p[1] = 255 - p[1];
        p[2] = 255 - p[2];
        if with_alpha {
            p[3] = 255 - p[3];
        }
    }
    Ok((out, json!({ "invert": true })))
}

fn op_sepia(img: Img, args: &Value) -> R<(Img, Value)> {
    op_adjust(img, &merged(args, json!({ "sepia": true })))
}

fn merged(base: &Value, extra: Value) -> Value {
    let mut m = match base.as_object() {
        Some(o) => o.clone(),
        None => serde_json::Map::new(),
    };
    if let Value::Object(e) = extra {
        for (k, v) in e {
            m.insert(k, v);
        }
    }
    Value::Object(m)
}

/// Otsu's threshold on a 256-bin luma histogram.
fn otsu(img: &Img) -> f64 {
    let mut hist = [0u64; 256];
    for p in img.px.chunks_exact(4) {
        hist[luma(p[0], p[1], p[2]) as usize] += 1;
    }
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return 128.0;
    }
    let mut sum_all = 0.0f64;
    for (i, h) in hist.iter().enumerate() {
        sum_all += i as f64 * (*h as f64);
    }
    let (mut sum_b, mut w_b) = (0.0f64, 0.0f64);
    let mut best = -1.0f64;
    let mut thr = 128.0;
    for (i, h) in hist.iter().enumerate() {
        w_b += *h as f64;
        if w_b == 0.0 {
            continue;
        }
        let w_f = total as f64 - w_b;
        if w_f == 0.0 {
            break;
        }
        sum_b += i as f64 * (*h as f64);
        let m_b = sum_b / w_b;
        let m_f = (sum_all - sum_b) / w_f;
        let between = w_b * w_f * (m_b - m_f).powi(2);
        if between > best {
            best = between;
            thr = i as f64;
        }
    }
    thr
}

fn op_threshold(img: Img, args: &Value) -> R<(Img, Value)> {
    let auto = ab(args, &["otsu", "auto"], false);
    let value = if auto {
        otsu(&img).round()
    } else {
        af(args, &["value", "v", "threshold", "t"], 128.0)
    };
    let v = if value <= 1.0 && !auto && util::f(args, "value").map(|x| x <= 1.0).unwrap_or(false) {
        value * 255.0
    } else {
        value
    };
    let low = col_or(args, &["low", "below", "under"], BLACK);
    let high = col_or(args, &["high", "above", "over"], WHITE);
    let channel = asv(args, &["channel", "ch"]).unwrap_or("luma");
    let mut out = Img { w: img.w, h: img.h, px: vec![0u8; img.px.len()] };
    for (i, p) in img.px.chunks_exact(4).enumerate() {
        let m = match channel {
            "r" | "red" => f64::from(p[0]),
            "g" | "green" => f64::from(p[1]),
            "b" | "blue" => f64::from(p[2]),
            "a" | "alpha" => f64::from(p[3]),
            _ => luma(p[0], p[1], p[2]),
        };
        let c = if m > v { high } else { low };
        out.px[i * 4..i * 4 + 4].copy_from_slice(&c);
    }
    Ok((out, json!({ "threshold": util::round(v, 1), "otsu": auto })))
}

fn op_dither(img: Img, args: &Value) -> R<(Img, Value)> {
    let levels = au(args, &["levels", "n", "colors", "depth"], 2).max(2);
    let bits = util::f(args, "bits").map(|b| 1u32 << b.clamp(1.0, 8.0) as u32).unwrap_or(0);
    let lv = if bits > 0 { bits } else { levels };
    let mode = asv(args, &["mode", "to"]).unwrap_or("gray");
    let serpentine = ab(args, &["serpentine"], true);
    let step = (255.0 / (f64::from(lv) - 1.0)) as f32;
    let (w, h) = (img.w as usize, img.h as usize);
    // working buffer in f32 so the diffusion error survives
    let mut buf: Vec<f32> = Vec::with_capacity(w * h * 3);
    for p in img.px.chunks_exact(4) {
        let y = luma(p[0], p[1], p[2]);
        if mode == "gray" || mode == "luma" || mode == "mono" {
            buf.extend_from_slice(&[y as f32, y as f32, y as f32]);
        } else {
            buf.extend_from_slice(&[f64::from(p[0]) as f32, f64::from(p[1]) as f32, f64::from(p[2]) as f32]);
        }
    }
    let quant = |v: f32| -> f32 { (v / step).round().clamp(0.0, 255.0 / step) * step };
    let spread = |x: usize, y: usize, err: [f32; 3], buf: &mut [f32], weights: &[(i64, i64, f32)]| {
        for (dx, dy, k) in weights {
            let nx = x as i64 + dx;
            let ny = y as i64 + dy;
            if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                continue;
            }
            let i = (ny as usize * w + nx as usize) * 3;
            for c in 0..3 {
                buf[i + c] += err[c] * k;
            }
        }
    };
    for y in 0..h {
        let reverse = serpentine && y % 2 == 1;
        let xs: Box<dyn Iterator<Item = usize>> = if reverse {
            Box::new((0..w).rev())
        } else {
            Box::new(0..w)
        };
        for x in xs {
            let i = (y * w + x) * 3;
            let old = [buf[i], buf[i + 1], buf[i + 2]];
            let new = [quant(old[0]), quant(old[1]), quant(old[2])];
            let err = [old[0] - new[0], old[1] - new[1], old[2] - new[2]];
            buf[i] = new[0];
            buf[i + 1] = new[1];
            buf[i + 2] = new[2];
            let dir: i64 = if reverse { -1 } else { 1 };
            let fwd: [(i64, i64, f32); 4] = [
                (dir, 0, 7.0 / 16.0),
                (-dir, 1, 3.0 / 16.0),
                (0, 1, 5.0 / 16.0),
                (dir, 1, 1.0 / 16.0),
            ];
            spread(x, y, err, buf.as_mut_slice(), &fwd);
        }
    }
    let mut out = Img { w: img.w, h: img.h, px: vec![0u8; w * h * 4] };
    for i in 0..w * h {
        out.px[i * 4] = u8c(f64::from(buf[i * 3]));
        out.px[i * 4 + 1] = u8c(f64::from(buf[i * 3 + 1]));
        out.px[i * 4 + 2] = u8c(f64::from(buf[i * 3 + 2]));
        out.px[i * 4 + 3] = img.px[i * 4 + 3];
    }
    Ok((out, json!({ "dither": { "levels": lv, "mode": mode, "algorithm": "floyd-steinberg" } })))
}

/// Duotone / tritone: map luma across two or three colours.
fn op_tint(img: Img, args: &Value) -> R<(Img, Value)> {
    let a = col_or(args, &["shadow", "from", "a", "low", "color"], BLACK);
    let b = col_or(args, &["highlight", "to", "b", "high", "color2", "second"], WHITE);
    let mid = col(args, &["mid", "midtone", "color3"]);
    let keep_alpha = img.has_alpha();
    let mut out = Img { w: img.w, h: img.h, px: vec![0u8; img.px.len()] };
    for (i, p) in img.px.chunks_exact(4).enumerate() {
        let t = (luma(p[0], p[1], p[2]) / 255.0).clamp(0.0, 1.0);
        let [r, g, bl] = match mid {
            Some(m) => {
                if t < 0.5 {
                    let k = t * 2.0;
                    [
                        a[0] as f64 + (m[0] as f64 - a[0] as f64) * k,
                        a[1] as f64 + (m[1] as f64 - a[1] as f64) * k,
                        a[2] as f64 + (m[2] as f64 - a[2] as f64) * k,
                    ]
                } else {
                    let k = (t - 0.5) * 2.0;
                    [
                        m[0] as f64 + (f64::from(b[0]) - f64::from(m[0])) * k,
                        m[1] as f64 + (f64::from(b[1]) - f64::from(m[1])) * k,
                        m[2] as f64 + (f64::from(b[2]) - f64::from(m[2])) * k,
                    ]
                }
            }
            None => [
                a[0] as f64 + (f64::from(b[0]) - f64::from(a[0])) * t,
                a[1] as f64 + (f64::from(b[1]) - f64::from(a[1])) * t,
                a[2] as f64 + (f64::from(b[2]) - f64::from(a[2])) * t,
            ],
        };
        out.px[i * 4] = u8c(r);
        out.px[i * 4 + 1] = u8c(g);
        out.px[i * 4 + 2] = u8c(bl);
        out.px[i * 4 + 3] = if keep_alpha { p[3] } else { 255 };
    }
    Ok((out, json!({ "tint": [hex(a), hex(b)] })))
}

pub fn hex(c: [u8; 4]) -> String {
    if c[3] == 255 {
        format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3])
    }
}

fn op_blur(img: Img, args: &Value) -> R<(Img, Value)> {
    let radius = af(args, &["radius", "r", "px", "size", "amount"], 4.0);
    if radius <= 0.0 {
        return Err(inval("blur radius must be > 0").hint("blur 4, or {radius:8,sigma:3}"));
    }
    let sigma = af(args, &["sigma", "std"], radius / 2.0);
    let mode = asv(args, &["mode", "filter"]).unwrap_or("auto");
    let premul = ab(args, &["premul"], true);
    let out = blur(&img, sigma.max(0.05), mode, premul);
    Ok((out, json!({ "blur": { "radius": radius, "sigma": util::round(sigma, 2), "mode": mode } })))
}

/// Unsharp mask: `out = in + amount * (in - blur(in))`, skipping changes smaller
/// than `threshold` (0..255) so grain is not amplified.
fn op_sharpen(img: Img, args: &Value) -> R<(Img, Value)> {
    let amount = af(args, &["amount", "a", "strength", "sharpen"], 1.0);
    let radius = af(args, &["radius", "r", "px"], 2.0).max(0.5);
    let sigma = af(args, &["sigma"], radius / 2.0).max(0.3);
    let threshold = af(args, &["threshold", "t"], 0.0);
    let soft = blur(&img, sigma, "auto", false);
    let mut out = img.clone();
    for i in 0..out.px.len() / 4 {
        for c in 0..4 {
            let o = f64::from(img.px[i * 4 + c]);
            let b = f64::from(soft.px[i * 4 + c]);
            let d = o - b;
            if d.abs() < threshold {
                continue;
            }
            out.px[i * 4 + c] = u8c(o + amount * d);
        }
    }
    Ok((out, json!({ "sharpen": { "amount": amount, "radius": radius, "threshold": threshold } })))
}

/// Deterministic xorshift64* so `noise` is a pure function of (Img, args).
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed })
    }
    fn next_f64(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        let v = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (v >> 11) as f64 / ((1u64 << 53) as f64)
    }
    /// Standard normal by Box-Muller.
    fn gauss(&mut self) -> f64 {
        let u1 = (self.next_f64()).max(1e-9);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

fn op_noise(img: Img, args: &Value) -> R<(Img, Value)> {
    let amount = af(args, &["amount", "a", "sigma", "intensity"], 12.0);
    if amount <= 0.0 {
        return Err(inval("noise amount must be > 0").hint("noise 12  (sigma in 8-bit levels)"));
    }
    let mono = ab(args, &["mono", "gray", "luma"], false);
    let seed = au(args, &["seed"], 0) as u64 ^ (af(args, &["seedf"], 0.0) as u64);
    let salt = ai(args, &["salt", "offset"], 0) as u64;
    let mut rng = Rng::new(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(salt));
    let mut out = img.clone();
    for p in out.px.chunks_exact_mut(4) {
        let n = rng.gauss() * amount;
        if mono {
            p[0] = u8c(f64::from(p[0]) + n);
            p[1] = u8c(f64::from(p[1]) + n);
            p[2] = u8c(f64::from(p[2]) + n);
        } else {
            p[0] = u8c(f64::from(p[0]) + n);
            p[1] = u8c(f64::from(p[1]) + n);
            p[2] = u8c(f64::from(p[2]) + rng.gauss() * amount);
        }
        if ab(args, &["alpha_noise"], false) {
            p[3] = u8c(f64::from(p[3]) + rng.gauss() * amount);
        }
    }
    Ok((out, json!({ "noise": { "amount": amount, "mono": mono, "seed": seed } })))
}

/// Percentile clip: stretch the histogram so the `lo`/`hi` tails land on 0/255.
fn op_normalize(img: Img, args: &Value) -> R<(Img, Value)> {
    let lo_pct = af(args, &["lo", "low", "cutoff_lo", "lo_pct", "cutoff"], 0.5).max(0.0);
    let hi_pct = af(args, &["hi", "high", "cutoff_hi", "hi_pct"], 0.5).max(0.0);
    if lo_pct + hi_pct >= 100.0 {
        return Err(inval("normalize: lo+hi percentiles must be under 100"));
    }
    let per_channel = matches!(asv(args, &["mode", "channel"]), Some("rgb") | Some("channels"));
    let (mut lo, mut hi) = if per_channel {
        (0.0, 255.0)
    } else {
        let mut hist = [0u64; 256];
        for p in img.px.chunks_exact(4) {
            hist[luma(p[0], p[1], p[2]) as usize] += 1;
        }
        percentile_bounds(&hist, lo_pct, hi_pct)
    };
    let mut out = img.clone();
    if per_channel {
        for c in 0..3 {
            let mut hist = [0u64; 256];
            for p in img.px.chunks_exact(4) {
                hist[p[c] as usize] += 1;
            }
            let (l, hi2) = percentile_bounds(&hist, lo_pct, hi_pct);
            for i in (c..out.px.len()).step_by(4) {
                let v = (f64::from(out.px[i]) - l) * 255.0 / (hi2 - l).max(1.0);
                out.px[i] = u8c(v);
            }
            if c == 0 {
                lo = l;
                hi = hi2;
            }
        }
    } else {
        let span = (hi - lo).max(1.0);
        for p in out.px.chunks_exact_mut(4) {
            for c in 0..3 {
                p[c] = u8c((f64::from(p[c]) - lo) * 255.0 / span);
            }
        }
    }
    Ok((out, json!({ "normalize": { "lo": util::round(lo, 1), "hi": util::round(hi, 1) } })))
}

fn percentile_bounds(hist: &[u64; 256], lo_pct: f64, hi_pct: f64) -> (f64, f64) {
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return (0.0, 255.0);
    }
    let want_lo = total as f64 * lo_pct / 100.0;
    let want_hi = total as f64 * (1.0 - hi_pct / 100.0);
    let mut acc = 0.0f64;
    let mut lo = 0.0;
    let mut hi = 255.0;
    for (i, h) in hist.iter().enumerate() {
        acc += *h as f64;
        if lo == 0.0 && acc >= want_lo {
            lo = i as f64;
        }
        if acc >= want_hi {
            hi = i as f64;
            break;
        }
    }
    if hi <= lo {
        (0.0, 255.0)
    } else {
        (lo, hi)
    }
}

fn op_equalize(img: Img, args: &Value) -> R<(Img, Value)> {
    let per_channel = !matches!(asv(args, &["mode"]), Some("luma") | Some("gray"));
    let mut out = img.clone();
    let build = |ch: usize| -> [u8; 256] {
        let mut hist = [0u64; 256];
        for p in img.px.chunks_exact(4) {
            hist[p[ch] as usize] += 1;
        }
        let total: u64 = hist.iter().sum();
        let mut lut = [0u8; 256];
        if total == 0 {
            for (i, e) in lut.iter_mut().enumerate() {
                *e = i as u8;
            }
            return lut;
        }
        let mut acc = 0u64;
        for (i, h) in hist.iter().enumerate() {
            acc += *h;
            lut[i] = ((acc as f64 / total as f64) * 255.0).round() as u8;
        }
        lut
    };
    if per_channel {
        let luts = [build(0), build(1), build(2)];
        for p in out.px.chunks_exact_mut(4) {
            for c in 0..3 {
                p[c] = luts[c][p[c] as usize];
            }
        }
    } else {
        let mut hist = [0u64; 256];
        for p in img.px.chunks_exact(4) {
            hist[luma(p[0], p[1], p[2]) as usize] += 1;
        }
        let lut = build(0);
        let _ = hist;
        for p in out.px.chunks_exact_mut(4) {
            for c in 0..3 {
                p[c] = lut[p[c] as usize];
            }
        }
    }
    Ok((out, json!({ "equalize": if per_channel { "rgb" } else { "luma" } })))
}

fn op_vignette(img: Img, args: &Value) -> R<(Img, Value)> {
    let strength = af(args, &["strength", "amount", "a"], 0.5).clamp(0.0, 1.0);
    let radius = af(args, &["radius", "r"], 0.75).clamp(0.01, 4.0);
    let feather = af(args, &["feather", "f"], 0.5).clamp(0.001, 4.0);
    let color = col_or(args, &["color", "c"], BLACK);
    let cx = af(args, &["cx", "x"], 0.5);
    let cy = af(args, &["cy", "y"], 0.5);
    let (w, h) = (f64::from(img.w), f64::from(img.h));
    let maxr = (w * w + h * h).sqrt() / 2.0;
    let inner = radius * maxr;
    let outer = inner + feather * maxr * 0.5;
    let mut out = img.clone();
    for y in 0..h as usize {
        for x in 0..w as usize {
            let dx = x as f64 + 0.5 - cx * w;
            let dy = y as f64 + 0.5 - cy * h;
            let d = (dx * dx + dy * dy).sqrt();
            let t = util::clamp((d - inner) / (outer - inner).max(1.0), 0.0, 1.0);
            if t <= 0.0 {
                continue;
            }
            let k = t.powf(1.4) * strength;
            let i = (y * w as usize + x) * 4;
            for c in 0..3 {
                out.px[i + c] = u8c(f64::from(out.px[i + c]) * (1.0 - k) + f64::from(color[c]) * k);
            }
        }
    }
    Ok((out, json!({ "vignette": { "strength": strength, "radius": radius } })))
}

fn op_pixelate(img: Img, args: &Value) -> R<(Img, Value)> {
    let mut size = au(args, &["size", "px", "block", "cell"], 8).max(1);
    if let Some(p) = util::f(args, "pct") {
        size = ((f64::from(img.w).min(f64::from(img.h))) * p / 100.0).round().max(1.0) as u32;
    }
    let nearest = matches!(asv(args, &["mode"]), Some("nearest") | Some("sample"));
    let (w, h) = (img.w as usize, img.h as usize);
    let mut out = img.clone();
    let mut y = 0usize;
    while y < h {
        let bh = (size as usize).min(h - y);
        let mut x = 0usize;
        while x < w {
            let bw = (size as usize).min(w - x);
            let p = if nearest {
                img.at(x, y)
            } else {
                let (mut s0, mut s1, mut s2, mut s3, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
                for yy in y..y + bh {
                    for xx in x..x + bw {
                        let q = img.at(xx, yy);
                        s0 += u64::from(q[0]);
                        s1 += u64::from(q[1]);
                        s2 += u64::from(q[2]);
                        s3 += u64::from(q[3]);
                        n += 1;
                    }
                }
                [
                    u8c(s0 as f64 / n as f64),
                    u8c(s1 as f64 / n as f64),
                    u8c(s2 as f64 / n as f64),
                    u8c(s3 as f64 / n as f64),
                ]
            };
            for yy in y..y + bh {
                for xx in x..x + bw {
                    out.set(xx, yy, p);
                }
            }
            x += bw;
        }
        y += bh;
    }
    Ok((out, json!({ "pixelate": size })))
}

/// 3x3 directional emboss kernel; `angle` picks one of eight directions.
fn op_emboss(img: Img, args: &Value) -> R<(Img, Value)> {
    let amount = af(args, &["amount", "a", "strength"], 1.0);
    let angle = af(args, &["angle", "deg", "dir"], 315.0);
    let dir = ((angle.rem_euclid(360.0)) / 45.0).round() as i64 % 8;
    let (dx, dy): (i64, i64) = match dir {
        0 => (1, 0),
        1 => (1, 1),
        2 => (0, 1),
        3 => (-1, 1),
        4 => (-1, 0),
        5 => (-1, -1),
        6 => (0, -1),
        _ => (1, -1),
    };
    let (w, h) = (img.w as usize, img.h as usize);
    let mut out = Img { w: img.w, h: img.h, px: vec![0u8; w * h * 4] };
    for y in 0..h {
        for x in 0..w {
            let sx = x as i64 + dx;
            let sy = y as i64 + dy;
            let src = if sx < 0 || sy < 0 || sx >= w as i64 || sy >= h as i64 {
                [128, 128, 128, 255]
            } else {
                img.at(sx as usize, sy as usize)
            };
            let cur = img.at(x, y);
            let mut v = [0u8; 4];
            for c in 0..3 {
                v[c] = u8c(128.0 + (f64::from(cur[c]) - f64::from(src[c])) * amount);
            }
            v[3] = cur[3];
            if ab(args, &["gray", "mono"], true) {
                let g = u8c(luma(v[0], v[1], v[2]));
                v = [g, g, g, v[3]];
            }
            out.set(x, y, v);
        }
    }
    Ok((out, json!({ "emboss": { "angle": angle, "amount": amount } })))
}

/// Sobel gradient magnitude.
fn op_edge(img: Img, args: &Value) -> R<(Img, Value)> {
    let amount = af(args, &["amount", "scale", "a"], 1.0);
    let mode = asv(args, &["mode", "axis"]).unwrap_or("magnitude");
    let invert = ab(args, &["invert", "dark"], false);
    let (w, h) = (img.w as usize, img.h as usize);
    let mut out = Img { w: img.w, h: img.h, px: vec![0u8; w * h * 4] };
    let g = |x: i64, y: i64| -> f64 {
        let cx = x.clamp(0, w as i64 - 1) as usize;
        let cy = y.clamp(0, h as i64 - 1) as usize;
        let p = img.at(cx, cy);
        luma(p[0], p[1], p[2])
    };
    for y in 0..h {
        for x in 0..w {
            let (ix, iy) = (x as i64, y as i64);
            let gx = g(ix - 1, iy - 1) + 2.0 * g(ix - 1, iy) + g(ix - 1, iy + 1)
                - g(ix + 1, iy - 1) - 2.0 * g(ix + 1, iy) - g(ix + 1, iy + 1);
            let gy = g(ix - 1, iy - 1) + 2.0 * g(ix, iy - 1) + g(ix + 1, iy - 1)
                - g(ix - 1, iy + 1) - 2.0 * g(ix, iy + 1) - g(ix + 1, iy + 1);
            let m = match mode {
                "x" | "horizontal" => gx.abs(),
                "y" | "vertical" => gy.abs(),
                _ => (gx * gx + gy * gy).sqrt(),
            };
            let mut v = (m / 4.0 * amount).clamp(0.0, 255.0);
            if invert {
                v = 255.0 - v;
            }
            let b = u8c(v);
            let i = (y * w + x) * 4;
            out.px[i] = b;
            out.px[i + 1] = b;
            out.px[i + 2] = b;
            out.px[i + 3] = img.px[i + 3];
        }
    }
    Ok((out, json!({ "edge": mode })))
}

/// Drop shadow of the alpha silhouette, composited under the image.
fn op_shadow(img: Img, args: &Value) -> R<(Img, Value)> {
    let dx = ai(args, &["x", "ox", "dx"], 6);
    let dy = ai(args, &["y", "oy", "dy"], 6);
    let radius = af(args, &["blur", "radius", "r", "size"], 12.0).max(0.5);
    let color = col_or(args, &["color", "c"], [0, 0, 0, 255]);
    let opacity = af(args, &["opacity", "o", "a"], 0.5).clamp(0.0, 1.0);
    let pad = au(args, &["pad", "margin"], (radius * 2.0).ceil() as u32 + 4);
    let grow = pad + (dx.abs().max(dy.abs())) as u32;
    let nw = img.w + grow * 2;
    let nh = img.h + grow * 2;
    check_size(nw, nh)?;
    // silhouette: alpha drives the shadow colour
    let mut sil = Img::filled(nw, nh, TRANSPARENT)?;
    for y in 0..img.h as usize {
        for x in 0..img.w as usize {
            let a = img.a(x, y);
            sil.set(x + grow as usize, y + grow as usize, [color[0], color[1], color[2], a]);
        }
    }
    let mut shadow = blur(&sil, radius / 2.0, "auto", true);
    for p in shadow.px.chunks_exact_mut(4) {
        p[3] = u8c(f64::from(p[3]) * opacity);
    }
    let mut out = Img::new(nw, nh)?;
    paste(&mut out, dx + grow as i64, dy + grow as i64, &shadow, 1.0);
    paste(&mut out, grow as i64, grow as i64, &img, 1.0);
    Ok((out, json!({ "shadow": { "offset": [dx, dy], "blur": radius }, "w": nw, "h": nh })))
}

// ------------------------------------------------------------------ background removal

/// Most common colour among the border pixels, quantised to 5 bits per channel so
/// JPEG noise does not scatter the vote. This is the auto-detected chroma key.
fn border_colour(img: &Img) -> [u8; 4] {
    let (w, h) = (img.w as usize, img.h as usize);
    let mut best: (u64, u32) = (0, 0);
    let mut tally: std::collections::HashMap<u32, (u64, u64, u64, u64, u64)> = Default::default();
    let mut visit = |x: usize, y: usize| {
        let p = img.at(x, y);
        let key = ((p[0] as u32 >> 3) << 10) | ((p[1] as u32 >> 3) << 5) | (p[2] as u32 >> 3);
        let e = tally.entry(key).or_insert((0, 0, 0, 0, 0));
        e.0 += 1;
        e.1 += u64::from(p[0]);
        e.2 += u64::from(p[1]);
        e.3 += u64::from(p[2]);
        e.4 += u64::from(p[3]);
    };
    for x in 0..w {
        visit(x, 0);
        visit(x, h - 1);
    }
    for y in 0..h {
        visit(0, y);
        visit(w - 1, y);
    }
    for (k, v) in &tally {
        if v.0 > best.0 {
            best = (v.0, *k);
        }
    }
    match tally.get(&best.1) {
        Some(v) if v.0 > 0 => [
            u8c(v.1 as f64 / v.0 as f64),
            u8c(v.2 as f64 / v.0 as f64),
            u8c(v.3 as f64 / v.0 as f64),
            u8c(v.4 as f64 / v.0 as f64),
        ],
        _ => corner_colour(img),
    }
}

fn op_remove_bg(img: Img, args: &Value) -> R<(Img, Value)> {
    let mode = asv(args, &["mode", "method", "by"]).unwrap_or("chroma").to_lowercase();
    if matches!(mode.as_str(), "ai" | "ml" | "smart" | "segment" | "segmentation" | "person") {
        return Err(Fail::new(
            "PROVIDER_NOT_CONFIGURED",
            "AI background removal needs a segmentation provider and none is configured in this build",
        )
        .hint("use the deterministic keyer instead: mode:\"chroma\" with color:\"#rrggbb\" (or omit color to auto-pick the border colour) plus tolerance/feather, or mode:\"edge\" to flood-fill the connected background from the borders"));
    }
    let key = match col(args, &["color", "key", "bg", "c"]) {
        Some(c) => c,
        None => border_colour(&img),
    };
    let tol = af(args, &["tolerance", "tol", "t"], 40.0).clamp(0.0, 442.0);
    let soft = af(args, &["feather", "soft", "ramp"], (tol * 0.5).max(1.0)).max(0.0);
    let want_mask = ab(args, &["mask", "maskonly", "matte"], false);
    let spill = ab(args, &["spill", "despill"], false);
    let defringe = ab(args, &["defringe", "shrink"], false);
    let premul = ab(args, &["premul"], false);
    let (w, h) = (img.w as usize, img.h as usize);
    let mut alpha = vec![255u8; w * h];
    match mode.as_str() {
        "edge" | "flood" | "fill" | "floodfill" => {
            // flood fill the region connected to the border that stays within the
            // tolerance of the key colour: holes inside the subject survive.
            let mut seen = vec![false; w * h];
            let mut stack: Vec<usize> = Vec::new();
            let mut push = |x: usize, y: usize, stack: &mut Vec<usize>, seen: &mut Vec<bool>| {
                let i = y * w + x;
                if !seen[i] && colour_dist(img.at(x, y), key) <= tol {
                    seen[i] = true;
                    stack.push(i);
                }
            };
            for x in 0..w {
                push(x, 0, &mut stack, &mut seen);
                push(x, h - 1, &mut stack, &mut seen);
            }
            for y in 0..h {
                push(0, y, &mut stack, &mut seen);
                push(w - 1, y, &mut stack, &mut seen);
            }
            while let Some(i) = stack.pop() {
                alpha[i] = 0;
                let (x, y) = (i % w, i / w);
                for (nx, ny) in [
                    (x.wrapping_sub(1), y),
                    (x + 1, y),
                    (x, y.wrapping_sub(1)),
                    (x, y + 1),
                ] {
                    if nx < w && ny < h {
                        push(nx, ny, &mut stack, &mut seen);
                    }
                }
            }
        }
        "chroma" | "key" | "color" | "colour" | "" => {
            for i in 0..w * h {
                let d = colour_dist(img.px[i * 4..i * 4 + 4].try_into().unwrap_or([0, 0, 0, 0]), key);
                let a = if d <= tol {
                    0.0
                } else {
                    util::clamp((d - tol) / soft.max(0.001), 0.0, 1.0)
                };
                alpha[i] = u8c(a * 255.0);
            }
        }
        other => {
            return Err(bad(format!("remove_bg mode \"{other}\" is not available"))
                .hint("mode: chroma (default) | edge | ai (needs a provider)"));
        }
    }
    let mut work = img.clone();
    for i in 0..w * h {
        work.px[i * 4 + 3] = alpha[i];
    }
    // feather the matte by blurring the alpha channel alone
    if let Some(f) = util::f(args, "blur") {
        let _ = f;
    }
    let feather_px = af(args, &["blur", "edge_blur"], 0.0);
    let mut work = if feather_px > 0.0 {
        let m = blur(&work, feather_px / 2.0, "auto", false);
        let mut o = work.clone();
        for i in 0..w * h {
            o.px[i * 4 + 3] = m.px[i * 4 + 3];
        }
        o
    } else {
        work
    };
    if defringe {
        // erode the matte by one pixel so the halo of mixed pixels goes away
        let mut o = work.clone();
        for y in 0..h {
            for x in 0..w {
                let mut mn = work.a(x, y);
                for (dx, dy) in [(-1i64, 0), (1, 0), (0, -1), (0, 1)] {
                    let nx = x as i64 + dx;
                    let ny = y as i64 + dy;
                    if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                        mn = mn.min(work.a(nx as usize, ny as usize));
                    } else {
                        mn = mn.min(0);
                    }
                }
                o.px[(y * w + x) * 4 + 3] = mn;
            }
        }
        work = o;
    }
    if spill {
        // kill the key colour's cast on the surviving pixels (green screen classic)
        let dom = dominant_channel(key);
        for p in work.px.chunks_exact_mut(4) {
            if p[3] == 0 {
                continue;
            }
            let others: Vec<f64> = (0..3).filter(|c| *c != dom).map(|c| f64::from(p[c])).collect();
            let cap = others.iter().cloned().fold(0.0f64, f64::max);
            let cur = f64::from(p[dom]);
            if cur > cap {
                let mix = util::clamp((cur - cap) / 64.0, 0.0, 1.0) * af(args, &["spill_strength", "spill_amount"], 0.85);
                p[dom] = u8c(cur - (cur - cap) * mix);
            }
        }
    }
    if premul {
        for p in work.px.chunks_exact_mut(4) {
            let a = f64::from(p[3]) / 255.0;
            for c in 0..3 {
                p[c] = u8c(f64::from(p[c]) * a);
            }
        }
    }
    let kept: u64 = alpha.iter().filter(|a| **a > 127).count() as u64;
    let mut rep = json!({
        "remove_bg": {
            "mode": mode,
            "key": hex(key),
            "tolerance": util::round(tol, 1),
            "feather": util::round(soft, 1),
            "heuristic": true,
            "coverage": util::round(kept as f64 * 100.0 / (w * h) as f64, 2)
        },
        "fmt": if want_mask { json!("png") } else { Value::Null }
    });
    let out = if want_mask {
        // the matte itself, as an opaque black/white image
        let mut m = Img { w: img.w, h: img.h, px: vec![0u8; w * h * 4] };
        for i in 0..w * h {
            let v = work.px[i * 4 + 3];
            m.px[i * 4] = v;
            m.px[i * 4 + 1] = v;
            m.px[i * 4 + 2] = v;
            m.px[i * 4 + 3] = 255;
        }
        rep["mask"] = json!(true);
        m
    } else {
        work
    };
    Ok((out, rep))
}

fn dominant_channel(c: [u8; 4]) -> usize {
    if c[0] >= c[1] && c[0] >= c[2] {
        0
    } else if c[1] >= c[2] {
        1
    } else {
        2
    }
}

/// Metadata-free output: re-encode losslessly in memory so nothing but pixels
/// survives the container, and prove the frame is encodable before writing.
fn op_strip(img: Img, _args: &Value) -> R<(Img, Value)> {
    let (bytes, _) = encode(&img, "png", 92, [WHITE[0], WHITE[1], WHITE[2]])?;
    let back = image::load_from_memory(&bytes)
        .map_err(|e| Fail::new("RENDER_FAILED", format!("strip: re-decode failed: {e}")))?
        .to_rgba8();
    if back.dimensions() != (img.w, img.h) {
        return Err(Fail::new("INTERNAL", "strip: round-trip size changed"));
    }
    Ok((
        Img { w: img.w, h: img.h, px: img.px.clone() },
        json!({ "strip": { "metadata": "dropped", "lossless_roundtrip": bytes.len() } }),
    ))
}

// ------------------------------------------------------------------ composites

fn op_overlay(img: Img, args: &Value, ctx: Option<&Path>) -> R<(Img, Value)> {
    let mode = asv(args, &["mode", "blend", "mix"])
        .unwrap_or("normal")
        .to_lowercase()
        .replace(' ', "-");
    if mode != "normal" && blend_fn(&mode).is_none() {
        return Err(bad(format!("unknown blend mode \"{mode}\""))
            .hint(format!("modes: {BLEND_MODES}")));
    }
    let opacity = af(args, &["opacity", "o", "alpha"], 1.0).clamp(0.0, 1.0);
    let layer = load_layer(args, ctx, &img)?;
    // size the layer to the canvas when asked
    let layer = if asv(args, &["size", "wh"]).is_some()
        || util::f(args, "w").is_some()
        || util::f(args, "h").is_some()
        || util::f(args, "scale").is_some()
    {
        let (tw, th) = match wh_from(args, (layer.w, layer.h), true)? {
            Some(v) => v,
            None => (layer.w, layer.h),
        };
        resample(&layer, tw, th, "lanczos")?
    } else {
        layer
    };
    let a = anchor_of(args);
    let fitmode = asv(args, &["fit"]).and_then(geom::fit_of);
    let layer = match fitmode {
        Some(Fit::Cover) | Some(Fit::Contain) | Some(Fit::Fill) => {
            let (o, _) = op_resize(
                layer,
                &merged(args, json!({ "w": img.w, "h": img.h, "anchor": a.as_str() })),
            )?;
            o
        }
        _ => layer,
    };
    let tx = match asv(args, &["tile"]).map(|s| s.to_lowercase()).as_deref() {
        Some("1") | Some("true") | Some("yes") | Some("x") => Some((
            au(args, &["tile_w", "gapx", "step"], layer.w),
            au(args, &["tile_h", "gapy", "stepy"], layer.h),
        )),
        _ if ab(args, &["tile"], false) => Some((layer.w, layer.h)),
        _ => None,
    };
    let margin = af(args, &["margin", "m"], 0.0);
    let (bx, by) = place(img.w, img.h, layer.w, layer.h, a);
    let dx = bx as i64 + ai(args, &["dx", "ox"], 0) + margin as i64;
    let dy = by as i64 + ai(args, &["dy", "oy"], 0) + margin as i64;
    let mut out = img.clone();
    composite(&mut out, dx, dy, &layer, opacity, &mode, tx)?;
    Ok((
        out,
        json!({ "overlay": { "mode": mode, "opacity": opacity, "at": [dx, dy], "tile": tx.is_some() } }),
    ))
}

/// Load the second image for overlay / watermark / grid / stack.
fn load_layer(args: &Value, ctx: Option<&Path>, base: &Img) -> R<Img> {
    if let Some(s) = asv(args, &["src", "path", "img", "image", "file", "over", "on"]) {
        let p = resolve(s, ctx)?;
        return load(&p);
    }
    if let Some(o) = obj_any(args, &["img", "image", "layer"]) {
        // an inline Img-shaped object
        if let (Some(w), Some(h), Some(px)) = (
            util::f(o, "w"),
            util::f(o, "h"),
            o.get("px").and_then(|v| v.as_array()),
        ) {
            // `as_f64` because JSON from a caller may carry `255.0`; `as_u64` would
            // drop those and the length check below would then reject the image.
            let bytes: Vec<u8> = px
                .iter()
                .filter_map(|v| v.as_f64().map(|x| (x.round() as i64).clamp(0, 255) as u8))
                .collect();
            let (w, h) = (w as u32, h as u32);
            if bytes.len() == (w as usize) * (h as usize) * 4 {
                return Ok(Img { w, h, px: bytes });
            }
        }
    }
    if let Some(b64) = asv(args, &["base64", "b64", "data"]) {
        let raw = b64
            .split_once(',')
            .map(|x| x.1)
            .unwrap_or(b64);
        let bytes = base64_decode(raw)?;
        return decode_bytes(&bytes);
    }
    if asv(args, &["text", "t", "s"]).is_some() {
        return text_image(args);
    }
    // a flat colour layer over the same canvas
    if let Some(c) = col(args, &["color", "c", "fill", "bg"]) {
        return Img::filled(base.w, base.h, c);
    }
    Err(bad("overlay needs a second image")
        .hint("pass src:\"path.png\", text:\"ACME\", color:\"#00000080\" or base64:\"...\""))
}

fn obj_any<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| v.get(*k)).filter(|x| x.is_object())
}

pub fn base64_decode(s: &str) -> R<Vec<u8>> {
    use base64::Engine;
    let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(clean.as_bytes())
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(clean.as_bytes()))
        .map_err(|e| bad(format!("base64: {e}")))
}

pub fn base64_encode(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// Decode an in-memory image (png/jpeg/webp/bmp/gif/tiff) to RGBA8.
pub fn decode_bytes(bytes: &[u8]) -> R<Img> {
    let di = image::load_from_memory(bytes).map_err(|e| {
        Fail::new("DECODE_FAILED", e.to_string()).hint("supported: png jpeg webp bmp gif tiff")
    })?;
    let (w, h) = (di.width(), di.height());
    check_size(w, h)?;
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(Fail::new("TOO_LARGE", format!("{w}x{h} exceeds the decode limits")));
    }
    let rgba = di.to_rgba8();
    Ok(Img { w, h, px: rgba.into_raw() })
}

// ------------------------------------------------------------------ text

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Text arguments shared by `watermark` and `grid` labels.
pub struct TextOpts {
    pub text: String,
    pub size: f64,
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub ls: f64,
    pub line_gap: f64,
    pub color: [u8; 4],
}

fn text_opts(args: &Value, base: Option<&Img>) -> R<TextOpts> {
    let text = asv(args, &["text", "t", "label", "content", "string"])
        .ok_or_else(|| bad("missing text").hint("watermark \"ACME\" / {text:\"ACME\",size:32}"))?
        .to_string();
    if text.is_empty() {
        return Err(bad("empty text"));
    }
    let default = base
        .map(|i| (f64::from(i.w.min(i.h)) / 18.0).clamp(10.0, 96.0))
        .unwrap_or(24.0);
    Ok(TextOpts {
        size: af(args, &["size", "font_size", "fs"], default).clamp(1.0, 4096.0),
        family: asv(args, &["family", "font", "face"])
            .unwrap_or("sans-serif")
            .to_string(),
        weight: au(args, &["weight", "bold"], if ab(args, &["bold"], false) { 700 } else { 400 }) as u16,
        italic: ab(args, &["italic", "oblique"], false),
        ls: af(args, &["ls", "letter_spacing", "tracking"], 0.0),
        line_gap: af(args, &["line_height", "lh", "leading", "line_gap"], 1.25),
        color: col_or(args, &["color", "c", "fill"], WHITE),
        text,
    })
}

/// Rasterise text to an RGBA image trimmed to its ink box. This is a real font
/// rasteriser: the string becomes an SVG `<text>` and resvg turns it into
/// outlines with the system fonts, so the watermark matches the app's own paint path.
pub fn text_image(args: &Value) -> R<Img> {
    text_image_opts(&text_opts(args, None)?)
}

fn text_image_opts(t: &TextOpts) -> R<Img> {
    if !has_fonts() {
        return Err(Fail::new("FONT_MISSING", "no system fonts are installed, so text cannot be drawn")
            .hint("use watermark with src:\"logo.png\" instead of text, or install a font"));
    }
    let lines: Vec<&str> = t.text.split('\n').collect();
    let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1) as f64;
    let est_w = (longest * t.size * 0.72 + t.ls * longest + t.size * 2.0).ceil().max(8.0);
    let est_h = (lines.len() as f64 * t.size * t.line_gap + t.size * 1.6).ceil().max(8.0);
    if est_w > 12_000.0 || est_h > 12_000.0 || est_w * est_h > 40_000_000.0 {
        return Err(inval(format!("text box {est_w}x{est_h} is too large")).hint("lower `size` or shorten the string"));
    }
    let (cw, ch) = (est_w as u32, est_h as u32);
    let col_attr = format!("#{:02x}{:02x}{:02x}", t.color[0], t.color[1], t.color[2]);
    let mut body = String::new();
    for (i, line) in lines.iter().enumerate() {
        let y = t.size * (0.30 + t.line_gap * i as f64) + t.size * 0.5;
        body.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" font-family=\"{}\" font-size=\"{}\" font-weight=\"{}\"{} fill=\"{}\"{} xml:space=\"preserve\">{}</text>",
            t.size * 0.5,
            util::round(y, 2),
            esc(&t.family),
            util::round(t.size, 2),
            t.weight,
            if t.italic { " font-style=\"italic\"" } else { "" },
            col_attr,
            if t.ls != 0.0 {
                format!(" letter-spacing=\"{}\"", util::round(t.ls, 2))
            } else {
                String::new()
            },
            esc(line),
        ));
    }
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{cw}\" height=\"{ch}\" viewBox=\"0 0 {cw} {ch}\">{body}</svg>"
    );
    let img = raster_svg_str(&svg)?;
    // the alpha the renderer produced already carries the fill colour's opacity
    let trimmed = trim_ink(&img)?;
    if trimmed.w == 0 || trimmed.h == 0 {
        return Err(Fail::new(
            "FONT_MISSING",
            format!("nothing was drawn for family \"{}\" — no face matched", t.family),
        )
        .hint("check `text.fonts` for an installed family, or use watermark with src:\"logo.png\""));
    }
    Ok(trimmed)
}

/// Crop to the box that has any non-transparent pixel.
fn trim_ink(img: &Img) -> R<Img> {
    let (w, h) = (img.w as usize, img.h as usize);
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if img.a(x, y) > 8 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x0 > x1 || y0 > y1 {
        return Ok(Img::new(1, 1)?);
    }
    let (ow, oh) = ((x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32);
    let mut out = Img { w: ow, h: oh, px: vec![0u8; ow as usize * oh as usize * 4] };
    for y in 0..oh as usize {
        let s = ((y0 + y) * w + x0) * 4;
        let e = s + ow as usize * 4;
        let d = y * ow as usize * 4;
        out.px[d..d + ow as usize * 4].copy_from_slice(&img.px[s..e]);
    }
    Ok(out)
}

fn op_watermark(img: Img, args: &Value, ctx: Option<&Path>) -> R<(Img, Value)> {
    let opacity = af(args, &["opacity", "o", "alpha"], 0.6).clamp(0.0, 1.0);
    let angle = af(args, &["rotate", "angle", "deg"], 0.0);
    let a = anchor_of(args);
    if asv(args, &["pos", "anchor", "position", "origin", "at"]).is_none()
        && a == Origin::Center
        && asv(args, &["tile"]).is_none()
    {
        // a watermark with no placement asked for sits bottom-right, which is what
        // every photo tool does
    }
    let layer = if asv(args, &["text", "t", "label", "content"]).is_some() {
        let mut to = text_opts(args, Some(&img))?;
        if to.color[3] == 255 && util::f(args, "color").is_none() {
            to.color = WHITE;
        }
        text_image_opts(&to)?
    } else {
        load_layer(args, ctx, &img)?
    };
    // scale the mark relative to the photo
    let target_w = if let Some(w) = util::f(args, "w").or_else(|| util::f(args, "width")) {
        w
    } else if let Some(f) = util::f(args, "scale").or_else(|| util::f(args, "by")) {
        f64::from(img.w) * f
    } else if let Some(p) = util::f(args, "pct") {
        f64::from(img.w.min(img.h)) * p / 100.0
    } else {
        f64::from(img.w.min(img.h)) * 0.25
    };
    let layer = if (target_w - f64::from(layer.w)).abs() > 0.5 && target_w > 1.0 {
        let s = target_w / f64::from(layer.w);
        resample(
            &layer,
            (f64::from(layer.w) * s).round().max(1.0) as u32,
            (f64::from(layer.h) * s).round().max(1.0) as u32,
            if s < 1.0 { "lanczos" } else { "catmull" },
        )?
    } else {
        layer
    };
    let layer = if angle.abs() > 1e-6 {
        let (r, _) = op_rotate(
            layer,
            &json!({ "angle": angle, "expand": true, "filter": "bilinear" }),
        )?;
        r
    } else {
        layer
    };
    let margin = af(args, &["margin", "m", "padding"], 0.0).max(0.0) as i64;
    let dx = ai(args, &["dx", "x", "ox"], 0);
    let dy = ai(args, &["dy", "y", "oy"], 0);
    let mut out = img.clone();
    let tiled = ab(args, &["tile", "repeat"], false);
    if tiled {
        let step_x = au(args, &["step", "gapx", "spacing", "tile_w"], layer.w + margin as u32 * 2);
        let step_y = au(args, &["stepy", "gapy", "tile_h"], layer.h + margin as u32 * 2);
        let (bx, by) = place(img.w, img.h, layer.w, layer.h, a);
        composite(
            &mut out,
            bx as i64 + dx - margin,
            by as i64 + dy - margin,
            &layer,
            opacity,
            "normal",
            Some((step_x.max(1), step_y.max(1))),
        )?;
    } else {
        let (bx, by) = place(img.w, img.h, layer.w, layer.h, a);
        let x = bx as i64 + dx + if matches!(a, Origin::TopRight | Origin::BottomRight | Origin::Right) {
            -margin
        } else if matches!(a, Origin::TopLeft | Origin::BottomLeft | Origin::Left) {
            margin
        } else {
            0
        };
        let y = by as i64 + dy + if matches!(a, Origin::BottomLeft | Origin::Bottom | Origin::BottomRight) {
            -margin
        } else if matches!(a, Origin::TopLeft | Origin::Top | Origin::TopRight) {
            margin
        } else {
            0
        };
        if ab(args, &["shadow", "outline"], false) {
            let mut sh = Img::filled(layer.w, layer.h, [0, 0, 0, 255])?;
            // use the mark's own alpha as the silhouette, then blur it
            for i in 0..(layer.w as usize * layer.h as usize) {
                sh.px[i * 4 + 3] = layer.px[i * 4 + 3];
            }
            sh = blur(&sh, 1.5, "auto", true);
            composite(&mut out, x + 2, y + 2, &sh, (opacity * 0.55).min(1.0), "normal", None)?;
        }
        composite(&mut out, x, y, &layer, opacity, "normal", None)?;
    }
    Ok((
        out,
        json!({ "watermark": { "at": a.as_str(), "opacity": opacity, "w": layer.w, "h": layer.h, "tiled": tiled } }),
    ))
}

// ------------------------------------------------------------------ multi-image layouts

fn cell_images(args: &Value, ctx: Option<&Path>, base: &Img) -> R<Vec<Img>> {
    let mut list: Vec<Img> = vec![];
    let keys = ["paths", "images", "files", "srcs", "inputs", "items"];
    let mut raw: Vec<String> = vec![];
    for k in keys {
        let a = util::arr(args, k);
        if !a.is_empty() {
            raw.extend(a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())));
            break;
        }
    }
    if raw.is_empty() {
        if let Some(s) = asv(args, &["paths", "images", "src", "dir"]) {
            if let Ok(listing) = read_dir_sorted(s, ctx) {
                raw = listing;
            }
        }
    }
    if ab(args, &["self", "include", "prepend"], false) || raw.is_empty() {
        raw.insert(0, "<current>".to_string());
    }
    for (i, p) in raw.iter().enumerate() {
        if p == "<current>" {
            list.push(base.clone());
            continue;
        }
        let path = resolve(p, ctx)
            .map_err(|e| e.details(json!({ "index": i, "path": p })))?;
        list.push(load(&path)?);
    }
    if list.is_empty() {
        return Err(bad("no images to place")
            .hint("pass paths:[\"a.png\",\"b.png\"] or set self:1 to include the current image"));
    }
    Ok(list)
}

fn read_dir_sorted(dir: &str, ctx: Option<&Path>) -> R<Vec<String>> {
    let p = resolve(dir, ctx)?;
    let mut v: Vec<String> = std::fs::read_dir(p)?
        .filter_map(|e| e.ok())
        .map(|e| disp(&e.path()))
        .filter(|s| {
            fmt_of_ext(&util::ext(Path::new(s))).is_some() && !s.to_lowercase().ends_with(".svg")
        })
        .collect();
    v.sort();
    Ok(v)
}

fn op_grid(img: Img, args: &Value, ctx: Option<&Path>, default_cols: u32) -> R<(Img, Value)> {
    let cells = cell_images(args, ctx, &img)?;
    let n = cells.len();
    let mut cols = au(args, &["cols", "columns", "c"], 0);
    let mut rows = au(args, &["rows", "r"], 0);
    if cols == 0 && rows == 0 {
        cols = if default_cols > 0 { default_cols } else { (n as f64).sqrt().ceil() as u32 }.max(1);
        rows = ((n as f64) / f64::from(cols)).ceil() as u32;
    } else if cols == 0 {
        cols = ((n as f64) / f64::from(rows.max(1))).ceil() as u32;
    } else if rows == 0 {
        rows = ((n as f64) / f64::from(cols)).ceil() as u32;
    }
    cols = cols.max(1);
    rows = rows.max(1);
    if u64::from(cols) * u64::from(rows) < n as u64 {
        return Err(inval(format!("grid {cols}x{rows} cannot hold {n} images")));
    }
    let (cell_w, cell_h) = match wh_from(args, (img.w, img.h), false)? {
        Some((w, h)) if asv(args, &["cell", "size", "wh"]).is_some() || util::f(args, "cell_w").is_some() => (w, h),
        _ => (
            au(args, &["cell", "cell_w"], img.w).max(1),
            au(args, &["cell_h", "tile_h", "tile"], img.h).max(1),
        ),
    };
    let cell = (
        au(args, &["cell_w"], cell_w).max(1),
        au(args, &["cell_h"], cell_h).max(1),
    );
    let gap = au(args, &["gap", "g", "spacing"], 0);
    let pad = au(args, &["pad", "padding", "margin"], gap);
    let bg = col_or(args, &["bg", "background", "color"], TRANSPARENT);
    let fit = asv(args, &["fit"]).unwrap_or("cover");
    let by_col = matches!(asv(args, &["order"]), Some("col") | Some("column") | Some("vertical"));
    let label = ab(args, &["label", "numbers", "index"], false);
    let (nw, nh) = (
        cell.0 * cols + gap * (cols - 1) + pad * 2,
        cell.1 * rows + gap * (rows - 1) + pad * 2,
    );
    check_size(nw, nh)?;
    let mut out = Img::filled(nw, nh, bg)?;
    for (i, c) in cells.iter().enumerate().take(n) {
        let (cx, cy) = if by_col {
            (((i as u32) / rows) % cols, (i as u32) % rows)
        } else {
            ((i as u32) % cols, (i as u32) / cols)
        };
        let x = pad as i64 + cx as i64 * (cell.0 + gap) as i64;
        let y = pad as i64 + cy as i64 * (cell.1 + gap) as i64;
        let (placed, _) = op_resize(
            c.clone(),
            &json!({ "w": cell.0, "h": cell.1, "fit": fit, "bg": hex(bg), "anchor": "center" }),
        )?;
        paste(&mut out, x, y, &placed, 1.0);
        if label {
            let mut la = serde_json::Map::new();
            la.insert("text".into(), json!((i + 1).to_string()));
            la.insert("size".into(), json!((cell.1 as f64 * 0.14).clamp(9.0, 64.0)));
            la.insert("color".into(), json!("#ffffff"));
            let t = text_image_opts(&text_opts(&Value::Object(la), None)?)?;
            let mut m = Img::new(cell.0, cell.1)?;
            paste(&mut m, 4, 4, &t, 0.85);
            composite(&mut out, x, y, &m, 1.0, "normal", None)?;
        }
    }
    Ok((
        out,
        json!({ "grid": { "cols": cols, "rows": rows, "cell": [cell.0, cell.1], "gap": gap, "count": n }, "w": nw, "h": nh }),
    ))
}

/// Justified-row mosaic: every image keeps its aspect, rows are scaled to fit.
fn op_collage(img: Img, args: &Value, ctx: Option<&Path>) -> R<(Img, Value)> {
    let cells = cell_images(args, ctx, &img)?;
    let canvas_w = au(args, &["w", "width"], img.w * 2).max(16);
    let row_h = au(args, &["row", "row_h", "h"], img.h / 2).max(8);
    let gap = au(args, &["gap", "g"], 4);
    let pad = au(args, &["pad", "padding"], gap);
    let bg = col_or(args, &["bg", "background"], TRANSPARENT);
    let mut rows: Vec<Vec<(u32, u32)>> = vec![];
    let mut cur: Vec<(u32, u32)> = vec![];
    let mut used = 0u32;
    for c in &cells {
        let aspect = f64::from(c.w) / f64::from(c.h);
        let w = (aspect * f64::from(row_h)).round().max(1.0) as u32;
        if used + w > canvas_w.saturating_sub(pad * 2) && !cur.is_empty() {
            rows.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.push((w, row_h));
        used += w + gap;
    }
    if !cur.is_empty() {
        rows.push(cur);
    }
    // scale each row so it exactly fills the content width
    let content = canvas_w.saturating_sub(pad * 2).max(1);
    let mut plan: Vec<Vec<(u32, u32)>> = vec![];
    for r in &rows {
        let natural: u32 = r.iter().map(|(w, _)| *w).sum::<u32>() + gap * (r.len() as u32 - 1);
        let k = f64::from(content.max(1)) / f64::from(natural.max(1));
        plan.push(
            r.iter()
                .map(|(w, h)| {
                    ((f64::from(*w) * k).round().max(1.0) as u32, (f64::from(*h) * k).round().max(1.0) as u32)
                })
                .collect(),
        );
    }
    let total_h: u32 = plan.iter().map(|r| r.iter().map(|(_, h)| *h).max().unwrap_or(0)).sum::<u32>()
        + gap * plan.len().saturating_sub(1) as u32
        + pad * 2;
    let mut out = Img::filled(canvas_w, total_h.max(1), bg)?;
    let mut y = pad as i64;
    let mut idx = 0usize;
    for r in &plan {
        let mut x = pad as i64;
        let rh = r.iter().map(|(_, h)| *h).max().unwrap_or(1);
        for (w, h) in r {
            if idx >= cells.len() {
                break;
            }
            let (placed, _) = op_resize(
                cells[idx].clone(),
                &json!({ "w": w, "h": h, "fit": "cover", "anchor": "center" }),
            )?;
            paste(&mut out, x, y, &placed, 1.0);
            x += i64::from(*w) + i64::from(gap);
            idx += 1;
        }
        y += i64::from(rh) + i64::from(gap);
    }
    Ok((
        out,
        json!({ "collage": { "rows": plan.len(), "count": cells.len(), "gap": gap }, "w": canvas_w, "h": total_h }),
    ))
}

fn op_stack(img: Img, args: &Value, ctx: Option<&Path>) -> R<(Img, Value)> {
    let mut cells = cell_images(args, ctx, &img)?;
    let axis = asv(args, &["axis", "dir", "direction", "by"]).unwrap_or("v").to_lowercase();
    let horiz = matches!(axis.as_str(), "h" | "horizontal" | "row" | "x");
    let gap = au(args, &["gap", "g", "spacing"], 0);
    let pad = au(args, &["pad", "padding", "margin"], 0);
    let bg = col_or(args, &["bg", "background", "color"], TRANSPARENT);
    let equalise = ab(args, &["equalise", "equalize", "match"], true);
    let cross = if horiz {
        au(args, &["h", "height", "cross"], cells.iter().map(|c| c.h).max().unwrap_or(1))
    } else {
        au(args, &["w", "width", "cross"], cells.iter().map(|c| c.w).max().unwrap_or(1))
    };
    let cross_fit = asv(args, &["fit"]).unwrap_or("cover");
    if equalise {
        let mut out = Vec::with_capacity(cells.len());
        for c in &cells {
            let (tw, th) = if horiz {
                let main = (f64::from(c.w) * f64::from(cross) / f64::from(c.h)).round().max(1.0) as u32;
                (main, cross)
            } else {
                let main = (f64::from(c.h) * f64::from(cross) / f64::from(c.w)).round().max(1.0) as u32;
                (cross, main)
            };
            let (r, _) = op_resize(
                c.clone(),
                &json!({ "w": tw, "h": th, "fit": cross_fit, "anchor": "center" }),
            )?;
            out.push(r);
        }
        cells = out;
    } else {
        // still clamp anything wildly larger than the requested cross size
        let mut out = Vec::with_capacity(cells.len());
        for c in &cells {
            let sc = if horiz {
                (f64::from(cross) / f64::from(c.h)).min(1.0)
            } else {
                (f64::from(cross) / f64::from(c.w)).min(1.0)
            };
            out.push(if sc < 1.0 {
                resample(c, (f64::from(c.w) * sc).round() as u32, (f64::from(c.h) * sc).round() as u32, "lanczos")?
            } else {
                c.clone()
            });
        }
        cells = out;
    }
    let (main_sum, cross_max) = if horiz {
        (
            cells.iter().map(|c| c.w).sum::<u32>() + gap * cells.len().saturating_sub(1) as u32,
            cells.iter().map(|c| c.h).max().unwrap_or(1),
        )
    } else {
        (
            cells.iter().map(|c| c.h).sum::<u32>() + gap * cells.len().saturating_sub(1) as u32,
            cells.iter().map(|c| c.w).max().unwrap_or(1),
        )
    };
    let (nw, nh) = if horiz {
        (main_sum + pad * 2, cross_max + pad * 2)
    } else {
        (cross_max + pad * 2, main_sum + pad * 2)
    };
    check_size(nw, nh)?;
    let mut out = Img::filled(nw, nh, bg)?;
    let align = anchor_of(args);
    let mut cursor = pad as i64;
    for c in &cells {
        let (x, y) = if horiz {
            let yy = if matches!(align, Origin::Top | Origin::TopLeft | Origin::TopRight) {
                pad as i64
            } else if matches!(align, Origin::Bottom | Origin::BottomLeft | Origin::BottomRight) {
                (nh - pad - c.h) as i64
            } else {
                (pad + (cross_max - c.h) / 2) as i64
            };
            (cursor, yy)
        } else {
            let xx = if matches!(align, Origin::Left | Origin::TopLeft | Origin::BottomLeft) {
                pad as i64
            } else if matches!(align, Origin::Right | Origin::TopRight | Origin::BottomRight) {
                (nw - pad - c.w) as i64
            } else {
                (pad + (cross_max - c.w) / 2) as i64
            };
            (xx, cursor)
        };
        paste(&mut out, x, y, c, 1.0);
        cursor += if horiz { i64::from(c.w) } else { i64::from(c.h) } + i64::from(gap);
    }
    Ok((
        out,
        json!({ "stack": { "axis": axis, "count": cells.len(), "gap": gap, "equalise": equalise }, "w": nw, "h": nh }),
    ))
}

// ------------------------------------------------------------------ background / extend

fn stops_of(args: &Value) -> Vec<(f64, [u8; 4])> {
    let raw = util::arr(args, "stops");
    if !raw.is_empty() {
        let mut v: Vec<(f64, [u8; 4])> = raw
            .iter()
            .filter_map(|s| {
                let at = util::f(s, "at").or_else(|| util::f(s, "p")).unwrap_or(0.0);
                let c = util::ps(s, "color").or_else(|| util::ps(s, "c")).and_then(parse_color4)?;
                Some((at, c))
            })
            .collect();
        v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        if !v.is_empty() {
            return v;
        }
    }
    let colors: Vec<[u8; 4]> = util::arr(args, "colors")
        .iter()
        .filter_map(|v| v.as_str().and_then(parse_color4))
        .collect();
    if colors.len() > 1 {
        let last = colors.len() - 1;
        return colors
            .into_iter()
            .enumerate()
            .map(|(i, c)| (i as f64 / last as f64, c))
            .collect();
    }
    let a = col_or(args, &["from", "start", "a", "c0"], BLACK);
    let b = col_or(args, &["to", "end", "b", "c1"], WHITE);
    vec![(0.0, a), (1.0, b)]
}

fn parse_color4(s: &str) -> Option<[u8; 4]> {
    let (r, g, b, a) = parse_color(s)?;
    Some([r, g, b, (a * 255.0).round() as u8])
}

fn ramp_at(stops: &[(f64, [u8; 4])], t: f64) -> [u8; 4] {
    if stops.is_empty() {
        return TRANSPARENT;
    }
    if t <= stops[0].0 {
        return stops[0].1;
    }
    let n = stops.len();
    if t >= stops[n - 1].0 {
        return stops[n - 1].1;
    }
    for i in 0..n - 1 {
        let (a0, c0) = stops[i];
        let (a1, c1) = stops[i + 1];
        if t >= a0 && t <= a1 {
            let k = if a1 > a0 { (t - a0) / (a1 - a0) } else { 0.0 };
            let r0 = s2l(f64::from(c0[0]));
            let g0 = s2l(f64::from(c0[1]));
            let b0 = s2l(f64::from(c0[2]));
            let r1 = s2l(f64::from(c1[0]));
            let g1 = s2l(f64::from(c1[1]));
            let b1 = s2l(f64::from(c1[2]));
            return [
                u8c(l2s(r0 + (r1 - r0) * k)),
                u8c(l2s(g0 + (g1 - g0) * k)),
                u8c(l2s(b0 + (b1 - b0) * k)),
                u8c(f64::from(c0[3]) + (f64::from(c1[3]) - f64::from(c0[3])) * k),
            ];
        }
    }
    stops[n - 1].1
}

/// Paint a `kind` gradient over the whole canvas.
fn paint(
    canvas: &mut Img,
    kind: &str,
    stops: &[(f64, [u8; 4])],
    angle: f64,
    cx: f64,
    cy: f64,
    rad: f64,
) {
    let (w, h) = (f64::from(canvas.w), f64::from(canvas.h));
    let diag = (w * w + h * h).sqrt();
    match kind {
        "solid" => {
            let c = if stops.is_empty() { TRANSPARENT } else { ramp_at(stops, 0.5) };
            for p in canvas.px.chunks_exact_mut(4) {
                p.copy_from_slice(&c);
            }
        }
        "linear" => {
            let a = angle * std::f64::consts::PI / 180.0;
            let (vx, vy) = (a.sin(), -a.cos());
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let px = x as f64 + 0.5 - w / 2.0;
                    let py = y as f64 + 0.5 - h / 2.0;
                    let proj = px * vx + py * vy;
                    let t = util::clamp(proj / diag + 0.5, 0.0, 1.0);
                    let i = (y * w as usize + x) * 4;
                    canvas.px[i..i + 4].copy_from_slice(&ramp_at(stops, t));
                }
            }
        }
        "radial" => {
            let (ox, oy) = (cx * w, cy * h);
            let rr = if rad <= 0.0 { diag / 2.0 } else { rad * diag / 2.0 };
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let d = ((x as f64 + 0.5 - ox).powi(2) + (y as f64 + 0.5 - oy).powi(2)).sqrt();
                    let t = util::clamp(d / rr.max(1e-6), 0.0, 1.0);
                    let i = (y * w as usize + x) * 4;
                    canvas.px[i..i + 4].copy_from_slice(&ramp_at(stops, t));
                }
            }
        }
        _ => {}
    }
}

fn op_background(img: Img, args: &Value) -> R<(Img, Value)> {
    let (tw, th) = match wh_from(args, (img.w, img.h), false)? {
        Some((w, h)) => (w, h),
        None => (img.w, img.h),
    };
    check_size(tw, th)?;
    let kind: String = if col(args, &["color", "c", "fill", "bg"]).is_some()
        && asv(args, &["gradient"]).is_none()
    {
        "solid".to_string()
    } else {
        match asv(args, &["gradient", "g", "kind", "type"]) {
            Some(s) => s.to_lowercase(),
            None => "linear".to_string(),
        }
    };
    let stops: Vec<(f64, [u8; 4])> = if kind == "solid" {
        let c = col_or(args, &["color", "c", "fill", "bg"], WHITE);
        vec![(0.0, c), (1.0, c)]
    } else {
        stops_of(args)
    };
    let mut canvas = Img::new(tw, th)?;
    paint(
        &mut canvas,
        &kind,
        &stops,
        af(args, &["angle", "deg", "a"], 180.0),
        af(args, &["cx", "x"], 0.5),
        af(args, &["cy", "y"], 0.5),
        af(args, &["r", "radius", "rad"], 0.7),
    );
    let mut rep = json!({ "background": { "kind": kind, "w": tw, "h": th } });
    if ab(args, &["bg_only", "only"], false) {
        return Ok((canvas, rep));
    }
    let blur_bg = af(args, &["blur", "blur_bg", "background_blur"], 0.0);
    let mut back = None;
    if blur_bg > 0.0 {
        // a blurred, cover-scaled copy of the photo behind it (story/poster look)
        let (cov, _) = op_resize(img.clone(), &json!({ "w": tw, "h": th, "fit": "cover" }))?;
        let mut b = blur(&cov, blur_bg / 2.0, "auto", false);
        let dim = af(args, &["dim", "darken"], 0.0).clamp(0.0, 1.0);
        if dim > 0.0 {
            for p in b.px.chunks_exact_mut(4) {
                for c in 0..3 {
                    p[c] = u8c(f64::from(p[c]) * (1.0 - dim));
                }
            }
        }
        back = Some(b);
    }
    if let Some(b) = back {
        canvas = b;
        rep["background"]["blurred_copy"] = json!(true);
    }
    let fit = asv(args, &["fit", "mode"]).unwrap_or("cover");
    let pad = au(args, &["pad", "padding", "margin"], 0);
    let (fw, fh) = (tw.saturating_sub(pad * 2).max(1), th.saturating_sub(pad * 2).max(1));
    let (placed, meta) = op_resize(
        img,
        &merged(args, json!({ "w": fw, "h": fh, "fit": fit, "anchor": anchor_of(args).as_str() })),
    )?;
    let (x, y) = if fw == placed.w && fh == placed.h {
        (pad as i64, pad as i64)
    } else {
        let (px, py) = place(fw, fh, placed.w, placed.h, anchor_of(args));
        (px as i64 + pad as i64, py as i64 + pad as i64)
    };
    let opacity = af(args, &["opacity", "o"], 1.0).clamp(0.0, 1.0);
    composite(&mut canvas, x, y, &placed, opacity, "normal", None)?;
    rep["place"] = meta["fit"].clone();
    rep["w"] = json!(tw);
    rep["h"] = json!(th);
    Ok((canvas, rep))
}

/// Blur-extend the canvas to a target ratio: the photo stays intact in the
/// middle, the extra area is filled with a blurred enlarged copy of itself.
/// This is the photo-to-story conversion.
fn op_extend(img: Img, args: &Value, force_blur: f64) -> R<(Img, Value)> {
    let ratio = asv(args, &["ratio", "ar", "to"])
        .and_then(ratio_of)
        .or_else(|| util::f(args, "ratio_n"))
        .or_else(|| util::f(args, "ratio"))
        .unwrap_or(0.0);
    let (tw, th) = match wh_from(args, (img.w, img.h), false)? {
        Some((w, h)) => (w, h),
        None => {
            if ratio <= 0.0 {
                return Err(bad("extend needs a target ratio or size")
                    .hint("extend 9:16  or  {ratio:\"16:9\"}  or  {w:1080,h:1920}"));
            }
            let cur = f64::from(img.w) / f64::from(img.h);
            if ratio > cur {
                ((f64::from(img.h) * ratio).round() as u32, img.h)
            } else {
                (img.w, (f64::from(img.w) / ratio).round() as u32)
            }
        }
    };
    let tw = tw.max(1);
    let th = th.max(1);
    if tw < img.w && th < img.h && !ab(args, &["downscale"], false) {
        // never silently throw away pixels: shrink the canvas to the photo instead
        let (o, r) = op_resize(img, &json!({ "w": tw, "h": th, "fit": "contain" }))?;
        let mut rep = r;
        rep["note"] = json!("target is smaller than the source; fitted without extending");
        return Ok((o, rep));
    }
    let blur_px = af(args, &["blur", "radius", "r"], force_blur).max(0.5);
    let dim = af(args, &["dim", "darken"], 0.18).clamp(0.0, 1.0);
    let zoom = af(args, &["zoom", "scale_back"], 1.05).max(1.0);
    let (cov, _) = op_resize(img.clone(), &json!({ "w": tw, "h": th, "fit": "cover" }))?;
    let zw = ((f64::from(cov.w) * zoom).round() as u32).max(1);
    let zh = ((f64::from(cov.h) * zoom).round() as u32).max(1);
    let z = resample(&cov, zw.min(MAX_DIM), zh.min(MAX_DIM), "lanczos")?;
    let back = blur(&z, blur_px / 2.0, "auto", false);
    let (bx, by) = place(tw, th, back.w, back.h, Origin::Center);
    let mut canvas = Img::new(tw, th)?;
    paste(&mut canvas, bx as i64, by as i64, &back, 1.0);
    for p in canvas.px.chunks_exact_mut(4) {
        for c in 0..3 {
            p[c] = u8c(f64::from(p[c]) * (1.0 - dim));
        }
    }
    let pad = au(args, &["pad", "padding"], 0);
    let (fit, _) = op_resize(
        img,
        &json!({
            "w": tw.saturating_sub(pad * 2).max(1),
            "h": th.saturating_sub(pad * 2).max(1),
            "fit": asv(args, &["fit"]).unwrap_or("contain"),
            "anchor": anchor_of(args).as_str()
        }),
    )?;
    let (px, py) = place(tw, th, fit.w, fit.h, anchor_of(args));
    paste(&mut canvas, px as i64, py as i64, &fit, 1.0);
    Ok((
        canvas,
        json!({ "extend": { "ratio": util::round(f64::from(tw) / f64::from(th), 4), "blur": blur_px, "dim": dim }, "w": tw, "h": th }),
    ))
}

// ------------------------------------------------------------------ analysis

/// Median-cut quantiser, the default for `palette` and asset import.
pub fn palette_of(img: &Img, n: usize) -> Vec<(String, f64)> {
    palette_mode(img, n, "median")
}

pub fn palette_mode(img: &Img, n: usize, mode: &str) -> Vec<(String, f64)> {
    let want = n.clamp(1, 32);
    // sample at most 200k pixels: palettes do not need every pixel
    let total = img.px.len() / 4;
    let stride = (total / 200_000).max(1);
    let mut pts: Vec<[u8; 3]> = Vec::with_capacity(total / stride);
    for (i, p) in img.px.chunks_exact(4).enumerate() {
        if i % stride != 0 {
            continue;
        }
        if p[3] < 8 {
            // transparent pixels say nothing about the palette
            continue;
        }
        pts.push([p[0], p[1], p[2]]);
    }
    if pts.is_empty() {
        return vec![("#00000000".to_string(), 1.0)];
    }
    let mut boxes = median_cut(&pts, want);
    if mode == "kmeans" || mode == "k-means" || mode == "means" {
        boxes = kmeans(&pts, boxes.clone(), 8);
    }
    let all = pts.len() as f64;
    boxes.sort_by(|a, b| b.1.cmp(&a.1));
    boxes
        .into_iter()
        .map(|(c, count)| {
            (format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]), util::round(count as f64 / all, 4))
        })
        .collect()
}

/// Classic median cut: split the box with the widest channel range at its
/// median until we have `n` boxes, then report each box's mean colour and size.
fn median_cut(pts: &[[u8; 3]], n: usize) -> Vec<([u8; 3], usize)> {
    struct Box_ {
        pts: Vec<[u8; 3]>,
    }
    let mut boxes = vec![Box_ { pts: pts.to_vec() }];
    while boxes.len() < n {
        // pick the box whose widest channel range is the largest
        let mut pick: Option<(usize, usize, i64)> = None;
        for (bi, b) in boxes.iter().enumerate() {
            if b.pts.len() < 2 {
                continue;
            }
            for ax in 0..3 {
                let mn = b.pts.iter().map(|p| p[ax] as i64).min().unwrap_or(0);
                let mx = b.pts.iter().map(|p| p[ax] as i64).max().unwrap_or(0);
                let range = mx - mn;
                if pick.as_ref().map(|(_, _, best)| range > *best).unwrap_or(true) {
                    pick = Some((bi, ax, range));
                }
            }
        }
        let Some((bi, ax, range)) = pick else { break };
        if range <= 0 {
            break;
        }
        let b = boxes.remove(bi);
        let mut left = b.pts;
        left.sort_by_key(|p| p[ax]);
        let mid = left.len() / 2;
        let right: Vec<[u8; 3]> = left.drain(mid..).collect();
        boxes.push(Box_ { pts: left });
        boxes.push(Box_ { pts: right });
    }
    boxes.retain(|b| !b.pts.is_empty());
    if boxes.is_empty() {
        boxes.push(Box_ { pts: pts.to_vec() });
    }
    boxes.iter().map(|b| (mean_of(&b.pts), b.pts.len())).collect()
}

fn mean_of(pts: &[[u8; 3]]) -> [u8; 3] {
    if pts.is_empty() {
        return [0, 0, 0];
    }
    let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
    for p in pts {
        r += u64::from(p[0]);
        g += u64::from(p[1]);
        b += u64::from(p[2]);
    }
    let n = pts.len() as u64;
    [u8c(r as f64 / n as f64), u8c(g as f64 / n as f64), u8c(b as f64 / n as f64)]
}

fn kmeans(pts: &[[u8; 3]], mut centers: Vec<([u8; 3], usize)>, iters: usize) -> Vec<([u8; 3], usize)> {
    if centers.is_empty() {
        return centers;
    }
    for _ in 0..iters {
        let mut acc = vec![[0u64; 4]; centers.len()];
        for p in pts {
            let mut bi = 0usize;
            let mut bd = f64::MAX;
            for (i, (c, _)) in centers.iter().enumerate() {
                let d = colour_dist([p[0], p[1], p[2], 255], [c[0], c[1], c[2], 255]);
                if d < bd {
                    bd = d;
                    bi = i;
                }
            }
            acc[bi][0] += u64::from(p[0]);
            acc[bi][1] += u64::from(p[1]);
            acc[bi][2] += u64::from(p[2]);
            acc[bi][3] += 1;
        }
        let mut moved = false;
        for (i, a) in acc.iter().enumerate() {
            if a[3] == 0 {
                continue;
            }
            let c = [
                u8c(a[0] as f64 / a[3] as f64),
                u8c(a[1] as f64 / a[3] as f64),
                u8c(a[2] as f64 / a[3] as f64),
            ];
            if c != centers[i].0 {
                centers[i].0 = c;
                moved = true;
            }
            centers[i].1 = a[3] as usize;
        }
        if !moved {
            break;
        }
    }
    centers
}

fn op_palette(img: Img, args: &Value) -> R<(Img, Value)> {
    let n = au(args, &["n", "count", "colors", "k"], 6).clamp(1, 32) as usize;
    let mode = asv(args, &["mode", "algo"]).unwrap_or("median");
    let p = palette_mode(&img, n, mode);
    let list: Vec<Value> = p
        .iter()
        .map(|(hex, cov)| json!({ "hex": hex, "count": (cov * img.len() as f64) as u64, "pct": util::round(cov * 100.0, 2) }))
        .collect();
    Ok((
        img,
        json!({ "palette": list, "colors": list.iter().filter_map(|v| v["hex"].as_str()).collect::<Vec<_>>(), "algo": mode }),
    ))
}

/// 8x8 difference hash -> 16 hex chars.
pub fn dhash(img: &Img) -> String {
    let g = resample(img, 9, 8, "bilinear").unwrap_or_else(|_| img.clone());
    let lum = |x: usize, y: usize| luma(g.at(x, y)[0], g.at(x, y)[1], g.at(x, y)[2]);
    let mut bits = 0u64;
    let mut i = 0;
    for y in 0..8 {
        for x in 0..8 {
            if lum(x, y) > lum(x + 1, y) {
                bits |= 1 << i;
            }
            i += 1;
        }
    }
    format!("{bits:016x}")
}

/// 8x8 average hash -> 16 hex chars.
pub fn ahash(img: &Img) -> String {
    let g = resample(img, 8, 8, "bilinear").unwrap_or_else(|_| img.clone());
    let mut sum = 0.0f64;
    for i in 0..64 {
        let p = g.at(i % 8, i / 8);
        sum += luma(p[0], p[1], p[2]);
    }
    let mean = sum / 64.0;
    let mut bits = 0u64;
    for i in 0..64 {
        let p = g.at(i % 8, i / 8);
        if luma(p[0], p[1], p[2]) >= mean {
            bits |= 1 << i;
        }
    }
    format!("{bits:016x}")
}

pub fn hamming(a: &str, b: &str) -> u32 {
    let x = u64::from_str_radix(a, 16).unwrap_or(0);
    let y = u64::from_str_radix(b, 16).unwrap_or(0);
    (x ^ y).count_ones()
}

/// Binary-search the JPEG quality that fits `kb`, shrinking the image only if no
/// quality does. Returns the resized image plus the chosen quality.
fn op_optimize(img: Img, args: &Value) -> R<(Img, Value)> {
    let kb = af(args, &["kb", "size_kb", "target", "max_kb", "under"], 200.0);
    if kb <= 0.0 {
        return Err(inval("optimize needs a target size in KB").hint("optimize 200  or  {kb:150}"));
    }
    let target = (kb * 1024.0) as u64;
    let fmt = asv(args, &["fmt", "format", "f"]).unwrap_or("jpeg");
    let qmin = au(args, &["qmin", "min"], 25).clamp(1, 99) as u8;
    let qmax = au(args, &["qmax", "max"], 92).clamp(u32::from(qmin) + 1, 100) as u8;
    let allow_down = ab(args, &["downscale", "shrink"], true);
    let bg = col_or(args, &["bg", "background"], WHITE);
    let mut cur = img.clone();
    let mut tries = 0u32;
    let chosen: u8 = loop {
        tries += 1;
        if fmt == "png" {
            // PNG has no quality knob: try the compression levels, then shrink
            let mut best: Option<u8> = None;
            for q in [92u8, 70, 50, 30, 10] {
                let (b, _) = encode(&cur, "png", q, [bg[0], bg[1], bg[2]])?;
                if b.len() as u64 <= target {
                    best = Some(q);
                    break;
                }
            }
            if let Some(q) = best {
                break q;
            }
        } else if fmt == "webp" {
            let (b, _) = encode(&cur, "webp", 100, [bg[0], bg[1], bg[2]])?;
            if b.len() as u64 <= target {
                break 100;
            }
        } else {
            let (hi_b, _) = encode(&cur, "jpeg", qmax, [bg[0], bg[1], bg[2]])?;
            if hi_b.len() as u64 <= target {
                break qmax;
            }
            let (lo_b, _) = encode(&cur, "jpeg", qmin, [bg[0], bg[1], bg[2]])?;
            if lo_b.len() as u64 > target {
                // too big even at the floor quality: fall through to downscaling
            } else {
                let (mut lo, mut hi) = (qmin, qmax);
                let mut best = qmin;
                while lo <= hi.saturating_sub(1) {
                    let mid = lo + (hi - lo) / 2;
                    let (b, _) = encode(&cur, "jpeg", mid, [bg[0], bg[1], bg[2]])?;
                    tries += 1;
                    if b.len() as u64 <= target {
                        best = mid;
                        lo = mid + 1;
                    } else if mid == 1 {
                        break;
                    } else {
                        hi = mid - 1;
                    }
                    if hi <= lo {
                        break;
                    }
                }
                break best;
            }
        }
        if !allow_down {
            let (b, _) = encode(&cur, fmt, qmin, [bg[0], bg[1], bg[2]])?;
            return Err(Fail::new(
                "TOO_LARGE",
                format!("cannot reach {} KB at {}x{}", kb, cur.w, cur.h),
            )
            .details(json!({ "bytes": b.len(), "quality": qmin }))
            .hint("allow downscale:1 or raise the target"));
        }
        let (nw, nh) = (
            ((f64::from(cur.w) * 0.85).round() as u32).max(1),
            ((f64::from(cur.h) * 0.85).round() as u32).max(1),
        );
        cur = resample(&cur, nw, nh, "lanczos")?;
        if tries > 40 {
            let (b, _) = encode(&cur, fmt, qmin, [bg[0], bg[1], bg[2]])?;
            return Err(Fail::new("TOO_LARGE", format!("cannot reach {} KB", kb))
                .details(json!({ "bytes": b.len(), "w": cur.w, "h": cur.h })));
        }
    };
    let (bytes, _) = encode(&cur, fmt, chosen, [bg[0], bg[1], bg[2]])?;
    Ok((
        cur.clone(),
        json!({
            "optimize": {
                "kb": util::round(kb, 2),
                "quality": chosen,
                "fmt": fmt,
                "bytes": bytes.len(),
                "size": util::human_bytes(bytes.len() as u64),
                "tries": tries,
                "w": cur.w,
                "h": cur.h,
                "shrunk": cur.w != img.w || cur.h != img.h
            },
            "quality": chosen,
            "fmt": fmt
        }),
    ))
}

// ------------------------------------------------------------------ dispatcher

/// Every op name this build knows, in the order `caps` should print them.
const OP_TABLE: &[(&str, &str, &str)] = &[
    ("crop", "geometry", "Cut a rectangle: x,y,w,h | ratio | anchor+size, clamped"),
    ("resize", "geometry", "Fit into w,h with fit:contain|cover|fill|actual + anchor"),
    ("scale", "geometry", "Multiply both sides by a factor (0.5, 150%, 2x)"),
    ("pad", "geometry", "Add px or t,r,b,l margins in a colour, transparent or edge"),
    ("extend", "geometry", "Grow to a ratio with a blurred copy filling the extra"),
    ("blurpad", "geometry", "extend with a heavier blur: photo to story in one call"),
    ("rotate", "geometry", "90/180/270 exact, any angle with bilinear + expand|crop"),
    ("flip", "geometry", "Mirror h, v or both"),
    ("round", "geometry", "Rounded corners with anti-aliased alpha edges"),
    ("border", "geometry", "Stroke the frame: width, colour, radius, inside|outside"),
    ("adjust", "colour", "brightness exposure contrast saturation vibrance hue temp tint"),
    ("gray", "colour", "Convert to grayscale using Rec.709 luma"),
    ("invert", "colour", "Invert RGB (alpha optional)"),
    ("sepia", "colour", "Sepia tone, amount 0..1"),
    ("threshold", "colour", "Binarise at a value, or otsu:true for automatic"),
    ("dither", "colour", "Floyd-Steinberg to N levels, gray or per-channel"),
    ("tint", "colour", "Duotone/tritone: map luma across two or three colours"),
    ("blur", "filter", "Separable Gaussian; auto-switches to box passes for big radii"),
    ("sharpen", "filter", "Unsharp mask with amount, radius and threshold"),
    ("noise", "filter", "Gaussian grain, amount in 8-bit levels, mono optional"),
    ("normalize", "colour", "Percentile clip: stretch lo/hi histogram bounds to 0-255"),
    ("autocontrast", "colour", "normalize with a gentler default clip"),
    ("equalize", "colour", "Histogram equalisation per channel"),
    ("vignette", "filter", "Radial darkening towards the edges"),
    ("pixelate", "filter", "Block average or nearest mosaic of size px"),
    ("emboss", "filter", "Directional 3x3 emboss kernel, 8 angles"),
    ("edge", "filter", "Sobel gradient magnitude (x/y/mode selectable)"),
    ("shadow", "filter", "Drop shadow of the alpha silhouette, canvas grows"),
    ("overlay", "composite", "Second image with a blend mode, opacity, position, tile"),
    ("watermark", "composite", "Text (real font outlines) or image mark, tiled or rotated"),
    ("grid", "composite", "N images into cols x rows with gap, bg and padding"),
    ("contact_sheet", "composite", "grid with 4 columns and optional index labels"),
    ("stack", "composite", "Vertical or horizontal composition with equalise"),
    ("collage", "composite", "Justified-row mosaic keeping every image aspect"),
    ("background", "composite", "Solid, linear/radial gradient or blurred copy behind"),
    ("remove_bg", "cutout", "Chroma-key or edge flood-fill background removal (heuristic)"),
    ("strip", "file", "Drop all metadata by re-encoding clean"),
    ("optimize", "file", "Fit a target file size, binary-searching JPEG quality"),
    ("palette", "analysis", "Median-cut or k-means colours with coverage percentages"),
    ("dhash", "analysis", "Perceptual difference hash, 16 hex chars (image passes through)"),
    ("ahash", "analysis", "Perceptual average hash, 16 hex chars (image passes through)"),
    ("trim", "analysis", "Auto-crop uniform borders with fuzz; reports the box"),
    ("border-trim", "geometry", "Cut a band off the edges: width, t/r/b/l or auto-detect"),
];

/// Names accepted as aliases of the above.
const ALIASES: &[(&str, &str)] = &[
    ("trim-border", "border-trim"),
    ("bordertrim", "border-trim"),
    ("unsharp", "sharpen"),
    ("gaussian", "blur"),
    ("gauss", "blur"),
    ("posterize", "adjust"),
    ("levels", "adjust"),
    ("temperature", "adjust"),
    ("saturation", "adjust"),
    ("contrast", "adjust"),
    ("brightness", "adjust"),
    ("duotone", "tint"),
    ("b&w", "gray"),
    ("greyscale", "gray"),
    ("grayscale", "gray"),
    ("fit", "resize"),
    ("cover", "resize"),
    ("contain", "resize"),
    ("matte", "remove_bg"),
    ("cutout", "remove_bg"),
    ("sheet", "contact_sheet"),
    ("mosaic", "collage"),
    ("canvas", "background"),
    ("quality", "optimize"),
    ("compress", "optimize"),
    ("colours", "palette"),
    ("colors", "palette"),
    ("hash", "dhash"),
    ("metadata", "strip"),
    ("clean", "strip"),
    ("orient", "rotate"),
    ("mirror", "flip"),
];

pub fn op_names() -> Vec<&'static str> {
    OP_TABLE.iter().map(|(n, _, _)| *n).collect()
}

/// `name` -> the shipped op, resolving aliases; `None` when unknown.
fn resolve_op(name: &str) -> Option<&'static str> {
    let t = name.trim().to_lowercase().replace(' ', "_").replace('-', "_");
    if let Some((n, _, _)) = OP_TABLE.iter().find(|(n, _, _)| *n == t) {
        return Some(n);
    }
    if let Some((_, to)) = ALIASES.iter().find(|(a, _)| *a == t.replace('_', "-") || *a == t) {
        return OP_TABLE.iter().map(|(n, _, _)| *n).find(|n| *n == *to);
    }
    // tolerant second pass: exact prefix, then edit distance
    let mut near: Vec<(usize, &str)> = OP_TABLE
        .iter()
        .map(|(n, _, _)| (lev(&t, &n.replace('_', "-")), *n))
        .collect();
    near.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    near.first().filter(|d| d.0 <= 3).map(|d| d.1)
}

fn lev(a: &str, b: &str) -> usize {
    let av: Vec<char> = a.chars().collect();
    let bv: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=bv.len()).collect();
    let mut cur = vec![0usize; bv.len() + 1];
    for i in 1..=av.len() {
        cur[0] = i;
        for j in 1..=bv.len() {
            let cost = if av[i - 1] == bv[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[bv.len()]
}

fn closest_five(name: &str) -> Vec<&'static str> {
    let t = name.trim().to_lowercase().replace(' ', "_").replace('-', "_");
    let mut scored: Vec<(usize, &str)> = OP_TABLE
        .iter()
        .map(|(n, _, _)| {
            let alt = n.replace('_', "-");
            let d = lev(&t, &alt).min(lev(&t, n));
            let prefix_bonus = if alt.starts_with(&t) || n.starts_with(&t) { 0 } else { 0 };
            (d + prefix_bonus, *n)
        })
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    scored.into_iter().take(5).map(|(_, n)| n).collect()
}

/// Run one op. `args` may be an object or a compact string of positional tokens.
pub fn op(name: &str, img: Img, args: &Value) -> R<Img> {
    let (o, _) = op_report(name, img, args, None)?;
    Ok(o)
}

/// [`op`] with a path context, for ops that read a second file (overlay, grid...).
pub fn op_ctx(name: &str, img: Img, args: &Value, ctx: Option<&Path>) -> R<Img> {
    let (o, _) = op_report(name, img, args, ctx)?;
    Ok(o)
}

/// [`op`] plus the step report (what analysis ops found, the quality `optimize`
/// chose, the mask hint, ...).
pub fn op_report(name: &str, img: Img, args: &Value, ctx: Option<&Path>) -> R<(Img, Value)> {
    let canon = resolve_op(name).ok_or_else(|| {
        Fail::new("UNSUPPORTED", format!("unknown image op \"{name}\""))
            .hint(format!("did you mean one of: {}", closest_five(name).join(", ")))
            .details(json!({ "op": name, "known": op_names() }))
    })?;
    let a = normalise_args(canon, args)?;
    let out = match canon {
        "crop" => op_crop(img, &a),
        "resize" => op_resize(img, &a),
        "scale" => op_scale(img, &a),
        "pad" => op_pad(img, &a),
        "border-trim" => op_border_trim(img, &a),
        "flip" => op_flip(img, &a),
        "rotate" => op_rotate(img, &a),
        "round" => op_round(img, &a),
        "border" => op_border(img, &a),
        "adjust" => op_adjust(img, &a),
        "gray" => op_gray(img, &a),
        "invert" => op_invert(img, &a),
        "sepia" => op_sepia(img, &a),
        "threshold" => op_threshold(img, &a),
        "dither" => op_dither(img, &a),
        "tint" => op_tint(img, &a),
        "blur" => op_blur(img, &a),
        "sharpen" => op_sharpen(img, &a),
        "noise" => op_noise(img, &a),
        "normalize" => op_normalize(img, &a),
        "autocontrast" => op_normalize(img, &merged(&a, json!({ "lo": 1.0, "hi": 1.0 }))),
        "equalize" => op_equalize(img, &a),
        "vignette" => op_vignette(img, &a),
        "pixelate" => op_pixelate(img, &a),
        "emboss" => op_emboss(img, &a),
        "edge" => op_edge(img, &a),
        "shadow" => op_shadow(img, &a),
        "trim" => op_trim(img, &a),
        "palette" => op_palette(img, &a),
        "strip" => op_strip(img, &a),
        "optimize" => op_optimize(img, &a),
        "dhash" => {
            let h = dhash(&img);
            Ok((img, json!({ "dhash": h, "hash": h })))
        }
        "ahash" => {
            let h = ahash(&img);
            Ok((img, json!({ "ahash": h, "hash": h })))
        }
        "remove_bg" => op_remove_bg(img, &a),
        "overlay" => op_overlay(img, &a, ctx),
        "watermark" => op_watermark(img, &a, ctx),
        "grid" => op_grid(img, &a, ctx, 0),
        "contact_sheet" => op_grid(img, &merged(&a, json!({ "cols": 4 })), ctx, 4),
        "stack" => op_stack(img, &a, ctx),
        "collage" => op_collage(img, &a, ctx),
        "background" => op_background(img, &a),
        "extend" => op_extend(img, &a, 30.0),
        "blurpad" => op_extend(img, &merged(&a, json!({ "blur": af(&a, &["blur"], 60.0) })), 60.0),
        other => unreachable!("op {other} is not dispatched"),
    };
    let (img, rep) = out?;
    if img.px.len() != (img.w as usize) * (img.h as usize) * 4 {
        return Err(Fail::new("INTERNAL", format!("op \"{canon}\" returned a bad buffer")));
    }
    let (w, h) = (img.w, img.h);
    Ok((img, json!({ "op": canon, "w": w, "h": h, "report": rep })))
}

/// Accept object args, null args and the compact string form.
fn normalise_args(canon: &str, args: &Value) -> R<Value> {
    match args {
        Value::Null => Ok(json!({})),
        Value::Object(_) => Ok(args.clone()),
        Value::Bool(b) => Ok(json!({ "on": b })),
        Value::Number(_) => Ok(json!({ "v": args })),
        Value::Array(items) => {
            // ["crop", "400x400"] style: first element names the op
            let strs: Vec<String> = items.iter().map(|v| value_to_string(v)).collect();
            parse_segment(&format!("{} {}", canon, strs.join(" ")))
        }
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                return Ok(json!({}));
            }
            if t.starts_with('{') {
                return serde_json::from_str::<Value>(t)
                    .map_err(|e| bad(format!("args json: {e}")));
            }
            parse_segment(&format!("{canon} {t}"))
        }
    }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

// ------------------------------------------------------------------ chain form

/// `"crop 400x400 > scale 0.5 > sharpen 1.2"` -> step objects.
pub fn parse_chain(s: &str) -> R<Vec<Value>> {
    let segs = split_top(s, '>');
    let mut out = Vec::with_capacity(segs.len());
    for seg in segs {
        let t = seg.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        out.push(parse_segment(t)?);
    }
    if out.is_empty() {
        return Err(bad("empty chain").hint("chain: \"crop 400x400 > scale 0.5 > sharpen 1.2\""));
    }
    Ok(out)
}

/// One step: `op k=v k2=v2` or `op positional tokens`.
pub fn parse_segment(seg: &str) -> R<Value> {
    let toks = tokenize(seg)?;
    if toks.is_empty() {
        return Err(bad("empty step"));
    }
    let head = toks[0].to_lowercase();
    let head = head.replace('-', "_");
    if head == "op" || head.is_empty() {
        return Err(bad("step has no op name"));
    }
    let canon = resolve_op(&head).ok_or_else(|| {
        Fail::new("UNSUPPORTED", format!("unknown image op \"{}\"", toks[0]))
            .hint(format!("did you mean one of: {}", closest_five(&head).join(", ")))
    })?;
    let mut m = serde_json::Map::new();
    let mut pos: Vec<String> = vec![];
    for t in &toks[1..] {
        if let Some((k, v)) = split_kv(t) {
            let key = k.to_string();
            m.insert(key, parse_value(&v));
        } else {
            pos.push(t.clone());
        }
    }
    apply_positional(canon, &pos, &mut m)?;
    m.insert("op".into(), json!(canon));
    Ok(Value::Object(m))
}

/// Map bare positional tokens onto the named arguments each op reads, so the
/// compact chain string means *exactly* the JSON it stands for:
/// `"crop 1:1"` -> `{"op":"crop","ratio":"1:1"}`, `"scale 0.5"` ->
/// `{"op":"scale","by":0.5}`, `"border 8 #FFFFFF"` ->
/// `{"op":"border","width":8,"color":"#FFFFFF"}`, `"watermark 品牌 center"` ->
/// `{"op":"watermark","text":"品牌","anchor":"center"}`.
///
/// Every slot name below is a key the op really looks up (see the `args, &[..]`
/// lists in each `op_*`), which is the whole point: a token routed to a key nobody
/// reads would silently do nothing. `key=value` tokens never come here —
/// [`parse_segment`] already split them out.
fn apply_positional(op: &str, pos: &[String], m: &mut serde_json::Map<String, Value>) -> R<()> {
    if pos.is_empty() {
        return Ok(());
    }
    let slots = pos_slots(op);
    let flags = pos_flags(op);
    // A run of bare numbers is a coordinate list (`crop 400 400` -> w,h), so it
    // fills the numeric slots only and never a word slot like `size`.
    let all_num = pos.len() > 1 && pos.iter().all(|t| is_num_tok(t));
    let mut i = 0usize;
    while i < pos.len() {
        let t = pos[i].trim().to_string();
        if t.is_empty() {
            i += 1;
            continue;
        }
        let low = t.to_lowercase();
        // 1. bare switch: `rotate 30 crop`, `threshold otsu`, `noise mono`
        if flags.contains(&low.as_str()) {
            m.insert(low.clone(), json!(true));
            i += 1;
            continue;
        }
        // 2. `round 50%` is the same mark as `{pct:50}`
        if op == "round" && low.ends_with('%') {
            match low.trim_end_matches('%').trim().parse::<f64>() {
                Ok(_) => {
                    m.insert("pct".into(), parse_value(low.trim_end_matches('%').trim()));
                    i += 1;
                    continue;
                }
                Err(_) => return Err(pos_err(op, &t, slots)),
            }
        }
        // 3. the watermark mark: consecutive plain words are one text string
        if op == "watermark" && !m.contains_key("text") && !m.contains_key("src") {
            if !tok_is_option(op, &t) {
                let mut words: Vec<String> = Vec::new();
                while i < pos.len() && !tok_is_option(op, &pos[i]) {
                    words.push(pos[i].trim().to_string());
                    i += 1;
                }
                m.insert("text".into(), json!(words.join(" ")));
                continue;
            }
        }
        // 4. several images at once: `grid a.png b.png` -> `paths:[..]`
        if multi_paths(op) && !m.contains_key("paths") && looks_like_path(&t) {
            let mut list: Vec<Value> = Vec::new();
            while i < pos.len() && looks_like_path(&pos[i]) {
                list.push(json!(pos[i].trim()));
                i += 1;
            }
            m.insert("paths".into(), Value::Array(list));
            continue;
        }
        // 5. shape-first: a ratio or a size has exactly one home per op
        if is_ratio_tok(&t) {
            if let Some((name, kind)) = slot_named(slots, "ratio") {
                put(m, name, kind, &t);
                i += 1;
                continue;
            }
        }
        if is_size_tok(&t) {
            if let Some((name, kind)) = slot_named(slots, "size") {
                put(m, name, kind, &t);
                i += 1;
                continue;
            }
        }
        // 6. colour, path and anchor words each have one obvious key
        if is_colour_tok(&t) {
            if let Some((name, kind)) = first_free(slots, m, &['c']) {
                put(m, name, kind, &t);
                i += 1;
                continue;
            }
        }
        if looks_like_path(&t) {
            if let Some((name, kind)) = first_free(slots, m, &['p']) {
                put(m, name, kind, &t);
                i += 1;
                continue;
            }
        }
        if takes_anchor(op) && Origin::parse(&low).is_some() {
            m.insert("anchor".into(), json!(low));
            i += 1;
            continue;
        }
        // 7. otherwise fill the next slot this token fits
        let want: &[char] = if is_num_tok(&t) || is_quantity_tok(&t) {
            if all_num {
                &['n']
            } else {
                &['n', 'z']
            }
        } else {
            &['s']
        };
        match first_free(slots, m, want) {
            Some((name, kind)) => {
                put(m, name, kind, &t);
                i += 1;
            }
            None => return Err(pos_err(op, &t, slots)),
        }
    }
    Ok(())
}

/// A slot: the argument name plus what kind of token it takes. `n` is a number,
/// `z` a size (a bare number kept as the word the op parses, `size:"400"`), `s` a
/// word, `c` a colour, `p` a path.
type Slots = &'static [(&'static str, char)];

/// The positional arguments of each op, in the order tokens fill them.
fn pos_slots(op: &str) -> Slots {
    match op {
        "crop" => &[
            ("size", 'z'), ("w", 'n'), ("h", 'n'), ("x", 'n'), ("y", 'n'), ("ratio", 's'),
        ],
        "resize" => &[("size", 'z'), ("w", 'n'), ("h", 'n'), ("fit", 's'), ("ratio", 's')],
        "scale" => &[("by", 'n')],
        "pad" => &[("px", 'n'), ("color", 'c'), ("mode", 's')],
        "border" => &[("width", 'n'), ("radius", 'n'), ("color", 'c'), ("side", 's')],
        "border-trim" => &[("width", 'n'), ("side", 's'), ("color", 'c')],
        // `size` comes last: `extend 9:16 60` means ratio + blur, not ratio + size,
        // and the size shape is matched by name anyway.
        "extend" | "blurpad" => &[("ratio", 'n'), ("blur", 'n'), ("dim", 'n'), ("size", 'z')],
        "rotate" => &[("angle", 'n'), ("filter", 's')],
        "flip" => &[("dir", 's')],
        "round" => &[("radius", 'n')],
        "blur" => &[("radius", 'n')],
        "sharpen" => &[("amount", 'n'), ("radius", 'n'), ("threshold", 'n')],
        "noise" => &[("amount", 'n'), ("seed", 'n')],
        "threshold" => &[("value", 'n'), ("low", 'c'), ("high", 'c')],
        "dither" => &[("levels", 'n'), ("mode", 's')],
        "tint" => &[("shadow", 'c'), ("highlight", 'c'), ("mid", 'c')],
        "normalize" | "autocontrast" => &[("lo", 'n'), ("hi", 'n')],
        "vignette" => &[("strength", 'n'), ("radius", 'n'), ("color", 'c')],
        "pixelate" => &[("size", 'z'), ("mode", 's')],
        "emboss" => &[("amount", 'n'), ("angle", 'n')],
        "edge" => &[("amount", 'n'), ("mode", 's')],
        "shadow" => &[("blur", 'n'), ("opacity", 'n'), ("color", 'c'), ("x", 'n'), ("y", 'n')],
        "trim" => &[("fuzz", 'n'), ("color", 'c')],
        "remove_bg" => &[("tolerance", 'n'), ("color", 'c'), ("mode", 's')],
        "optimize" => &[("kb", 'n'), ("fmt", 's')],
        "overlay" => &[("src", 'p'), ("mode", 's'), ("opacity", 'n')],
        "watermark" => &[
            ("src", 'p'), ("opacity", 'n'), ("size", 'n'), ("rotate", 'n'), ("color", 'c'),
        ],
        "grid" | "contact_sheet" => &[
            ("cols", 'n'), ("rows", 'n'), ("gap", 'n'), ("bg", 'c'), ("paths", 'p'),
        ],
        "stack" => &[("axis", 's'), ("gap", 'n'), ("bg", 'c'), ("paths", 'p')],
        "collage" => &[("w", 'n'), ("row", 'n'), ("gap", 'n'), ("bg", 'c'), ("paths", 'p')],
        "background" => &[
            ("size", 'z'), ("w", 'n'), ("h", 'n'), ("pad", 'n'), ("color", 'c'),
            ("gradient", 's'), ("ratio", 's'),
        ],
        "palette" => &[("n", 'n'), ("mode", 's')],
        // everything else (`adjust`, `gray`, `strip`, `dhash`, ...) reads named
        // keys only: a bare number there would be a coin toss, so it is an error
        _ => &[],
    }
}

/// Bare words that switch a boolean on. Only keys the op actually reads.
fn pos_flags(op: &str) -> &'static [&'static str] {
    match op {
        "rotate" => &["expand", "crop", "fit"],
        "threshold" => &["otsu", "auto"],
        "noise" => &["mono", "gray", "luma"],
        "overlay" => &["tile"],
        "watermark" => &["tile", "repeat", "shadow", "outline", "bold", "italic"],
        "grid" | "contact_sheet" => &["label", "numbers", "index", "self", "include", "prepend"],
        "stack" | "collage" => &["equalise", "equalize", "match"],
        "remove_bg" => &[
            "mask", "maskonly", "matte", "spill", "despill", "defringe", "shrink", "premul",
        ],
        "invert" => &["alpha", "invert_alpha"],
        "gray" => &["linear"],
        "dither" => &["serpentine"],
        "optimize" => &["downscale", "shrink"],
        "extend" | "blurpad" => &["downscale"],
        "edge" => &["invert", "dark"],
        "emboss" => &["gray", "mono"],
        "blur" => &["premul"],
        "adjust" => &["sepia", "invert", "mono", "grayscale", "alpha"],
        "trim" => &["alpha"],
        "background" => &["bg_only", "only"],
        _ => &[],
    }
}

/// Ops that place the result with an anchor (`anchor_of` in each `op_*`).
fn takes_anchor(op: &str) -> bool {
    matches!(
        op,
        "crop" | "resize" | "pad" | "overlay" | "watermark" | "background" | "stack"
            | "extend" | "blurpad"
    )
}

/// Ops that take a list of files positionally.
fn multi_paths(op: &str) -> bool {
    matches!(op, "grid" | "contact_sheet" | "stack" | "collage")
}

fn put(m: &mut serde_json::Map<String, Value>, name: &str, kind: char, t: &str) {
    let v = match kind {
        'n' => parse_value(t),
        _ => json!(t),
    };
    m.insert(name.to_string(), v);
}

fn slot_named(slots: Slots, name: &str) -> Option<(&'static str, char)> {
    slots.iter().find(|(n, _)| *n == name).copied()
}

fn first_free(
    slots: Slots,
    m: &serde_json::Map<String, Value>,
    kinds: &[char],
) -> Option<(&'static str, char)> {
    slots
        .iter()
        .find(|(n, k)| kinds.contains(k) && !m.contains_key(*n))
        .copied()
}

fn tok_is_option(op: &str, t: &str) -> bool {
    let low = t.to_lowercase();
    pos_flags(op).contains(&low.as_str())
        || is_num_tok(t)
        || is_quantity_tok(t)
        || is_ratio_tok(t)
        || is_size_tok(t)
        || is_colour_tok(t)
        || looks_like_path(t)
        || (takes_anchor(op) && Origin::parse(&low).is_some())
}

fn is_num_tok(t: &str) -> bool {
    !t.trim().is_empty() && t.trim().parse::<f64>().is_ok()
}

/// `150%`, `2x`, `90deg`, `45°` are factors and angles the same way `1.5` is: the
/// op parses the suffix itself (`scale 150%`, `rotate 90deg`), so these belong on a
/// numeric slot rather than a word slot.
fn is_quantity_tok(t: &str) -> bool {
    let s = t.trim().to_lowercase();
    for suf in ["%", "x", "deg", "°"] {
        if let Some(p) = s.strip_suffix(suf) {
            let p = p.trim();
            if !p.is_empty() && p.parse::<f64>().is_ok() {
                return true;
            }
        }
    }
    false
}

/// `"9:16"`, `"1:1"` — two numbers around a colon.
fn is_ratio_tok(t: &str) -> bool {
    match t.split_once(':') {
        Some((a, b)) => a.trim().parse::<f64>().ok().is_some() && b.trim().parse::<f64>().ok().is_some(),
        None => false,
    }
}

/// `"400x400"`, `"400,400"`, `"800×600"` — an explicit pixel pair.
fn is_size_tok(t: &str) -> bool {
    let low = t.to_lowercase().replace('×', "x").replace(',', "x");
    match low.split_once('x') {
        Some((a, b)) => !a.trim().is_empty()
            && !b.trim().is_empty()
            && a.trim().parse::<f64>().ok().is_some()
            && b.trim().parse::<f64>().ok().is_some(),
        None => false,
    }
}

fn is_colour_tok(t: &str) -> bool {
    parse_color(t.trim()).is_some() && !is_num_tok(t)
}

/// A file argument rather than a word: a separator or a known image extension.
/// (`"1.5"` has an extension as far as [`std::path::Path`] is concerned, so a
/// number is never a path.)
fn looks_like_path(t: &str) -> bool {
    let s = t.trim();
    if s.is_empty() || is_num_tok(s) {
        return false;
    }
    if s.contains('/') || s.contains('\\') {
        return true;
    }
    match std::path::Path::new(s).extension().and_then(|e| e.to_str()) {
        Some(e) => matches!(
            e.to_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "heic" | "avif"
                | "svg"
        ),
        None => false,
    }
}

fn pos_err(op: &str, t: &str, slots: Slots) -> Fail {
    let takes = slots
        .iter()
        .map(|(n, _)| format!("{n}={t}"))
        .collect::<Vec<_>>()
        .join(", ");
    let hint = if takes.is_empty() {
        format!("\"{op}\" takes no positional arguments; write it as {{\"op\":\"{op}\"}}")
    } else {
        format!("use key=value for {op}, e.g. {takes}")
    };
    bad(format!("unexpected argument \"{t}\" for op {op}")).hint(hint)
}

fn split_kv(t: &str) -> Option<(&str, &str)> {
    let i = t.find('=')?;
    let (k, v) = (&t[..i], &t[i + 1..]);
    if k.is_empty() || !k.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
        return None;
    }
    Some((k, v))
}

fn parse_value(v: &str) -> Value {
    let t = v.trim();
    if t.is_empty() {
        return json!(true);
    }
    if t.contains(',') {
        let parts: Vec<Value> = t.split(',').map(|p| parse_value(p)).collect();
        return Value::Array(parts);
    }
    if t.starts_with('[') || t.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<Value>(t) {
            return parsed;
        }
    }
    if let Ok(n) = t.parse::<i64>() {
        return json!(n);
    }
    if let Ok(n) = t.parse::<f64>() {
        return json!(n);
    }
    match t.to_lowercase().as_str() {
        "true" | "yes" | "on" => return json!(true),
        "false" | "no" | "off" => return json!(false),
        _ => {}
    }
    json!(t)
}

/// Split on `sep` but ignore separators inside quotes or brackets.
fn split_top(s: &str, sep: char) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    for c in s.chars() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
                continue;
            }
            None => {}
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                cur.push(c);
            }
            '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ']' | '}' => {
                depth -= 1;
                cur.push(c);
            }
            _ if c == sep && depth <= 0 => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Whitespace tokens, honouring `"quoted strings"` (including mid-token quotes).
fn tokenize(s: &str) -> R<Vec<String>> {
    let mut out: Vec<String> = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    for c in s.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ']' | '}' => depth = (depth - 1).max(0),
            _ if c.is_whitespace() && depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if quote.is_some() {
        return Err(bad("unbalanced quote in step").hint(format!("\"{s}\"")));
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}



/// Steps from either an array of objects or one `>`-joined string.
pub fn chain_steps(v: &Value) -> R<Vec<Value>> {
    match v {
        Value::String(s) => parse_chain(s),
        Value::Array(a) => {
            let mut out = Vec::with_capacity(a.len());
            for item in a {
                match item {
                    Value::String(s) => out.push(parse_segment(s)?),
                    Value::Object(_) => out.push(item.clone()),
                    Value::Null => {}
                    other => return Err(bad(format!("chain step must be a string or object, got {other}"))),
                }
            }
            if out.is_empty() {
                return Err(bad("empty chain"));
            }
            Ok(out)
        }
        Value::Null => Ok(vec![]),
        Value::Object(_) => Ok(vec![v.clone()]),
        Value::Bool(_) | Value::Number(_) => Err(bad("chain must be an array or a string")),
    }
}

/// Apply each step in order.
pub fn run_chain(img: Img, steps: &[Value]) -> R<Img> {
    let (o, _) = run_chain_report(img, steps, None)?;
    Ok(o)
}

pub fn run_chain_ctx(img: Img, steps: &[Value], ctx: Option<&Path>) -> R<Img> {
    let (o, _) = run_chain_report(img, steps, ctx)?;
    Ok(o)
}

/// [`run_chain`] plus a merged report: per-step findings, the last `quality`,
/// hashes, palettes and the output format hint that `optimize`/`remove_bg` set.
pub fn run_chain_report(img: Img, steps: &[Value], ctx: Option<&Path>) -> R<(Img, Value)> {
    if steps.len() > 64 {
        return Err(inval(format!("chain has {} steps", steps.len())).hint("a chain is capped at 64 steps"));
    }
    let mut cur = img;
    let mut per: Vec<Value> = vec![];
    let mut merged = json!({});
    for (i, s) in steps.iter().enumerate() {
        let (name, args) = split_step(s)?;
        let before = (cur.w, cur.h);
        let (next, rep) = op_report(&name, cur, &args, ctx).map_err(|e| {
            let mut f = e;
            if f.details.is_none() {
                f.details = Some(json!({ "step": i, "op": name }));
            } else if let Some(d) = f.details.as_mut().and_then(|d| d.as_object_mut()) {
                d.insert("step".into(), json!(i));
                d.insert("op".into(), json!(name));
            }
            f
        })?;
        cur = next;
        per.push(rep.clone());
        let r = &rep["report"];
        for key in ["quality", "fmt", "dhash", "ahash", "hash", "palette", "colors", "mask", "note"] {
            if let Some(v) = r.get(key).cloned().or_else(|| rep.get(key).cloned()) {
                if !v.is_null() {
                    merged[key] = v;
                }
            }
        }
        if let Some(rm) = merged.as_object_mut() {
            rm.entry("size".to_string())
                .or_insert(json!({}));
        }
        merged["size"]["w"] = json!(cur.w);
        merged["size"]["h"] = json!(cur.h);
        if (cur.w, cur.h) != before {
            merged["changed_size"] = json!(true);
        }
    }
    merged["steps"] = json!(per);
    Ok((cur, merged))
}

fn split_step(s: &Value) -> R<(String, Value)> {
    match s {
        Value::String(t) => {
            let v = parse_segment(t)?;
            split_step(&v)
        }
        Value::Object(_) => {
            let name = asv(s, &["op", "t", "name", "do", "cmd", "use"])
                .ok_or_else(|| bad("chain step needs an \"op\" key").hint("use {op:\"crop\",w:400,h:400} or \"crop 400x400\""))?
                .to_string();
            let mut rest = s.as_object().cloned().unwrap_or_default();
            let mut args = serde_json::Map::new();
            for key in ["op", "t", "name", "do", "cmd", "use"] {
                rest.remove(key);
            }
            for key in ["args", "a", "params", "p", "with"] {
                if let Some(v) = rest.remove(key) {
                    match v {
                        Value::Object(o) => {
                            for (k, val) in o {
                                args.insert(k, val);
                            }
                        }
                        other => {
                            let n = normalise_args(&name, &other)?;
                            if let Some(o) = n.as_object() {
                                for (k, v) in o {
                                    args.insert(k.clone(), v.clone());
                                }
                            }
                        }
                    }
                }
            }
            for (k, v) in rest {
                args.insert(k, v);
            }
            Ok((name, Value::Object(args)))
        }
        other => Err(bad(format!("chain step must be a string or object, got {other}"))),
    }
}

// ------------------------------------------------------------------ presets

const PRESETS: &[(&str, &str)] = &[
    ("social_1080", "1080x1080 cover, punchy colour, sharpened"),
    ("ecommerce_main", "800x800 white canvas, product contained with 32px margin"),
    ("wechat_cover", "900x500 blur-extended cover for a WeChat header"),
    ("avatar", "200x200 centre crop, circular alpha"),
    ("story_9_16", "1080x1920 story with a blurred padding band"),
    ("strip_4x3", "1600x1200 4:3 strip, blur-padded and sharpened"),
    ("thumbnail_256", "256px longest side, metadata stripped"),
    ("print_a4", "2480x3508 A4 at 300dpi on white, contained"),
    ("web_hero", "1920x1080 cover, darkened and vignetted"),
    ("compress_200k", "JPEG under 200 KB by searching quality"),
];

pub fn presets() -> Vec<(&'static str, &'static str)> {
    PRESETS.to_vec()
}

pub fn preset_chain(name: &str) -> R<Vec<Value>> {
    let n = name.trim().to_lowercase().replace('-', "_").replace(' ', "_");
    let steps: Vec<Value> = match n.as_str() {
        "social_1080" | "social" => vec![
            json!({ "op": "resize", "w": 1080, "h": 1080, "fit": "cover", "anchor": "center" }),
            json!({ "op": "adjust", "contrast": 1.06, "saturation": 1.08, "brightness": 0.01 }),
            json!({ "op": "sharpen", "amount": 0.7, "radius": 1.5, "threshold": 2 }),
        ],
        "ecommerce_main" | "ecom" | "ecom_main" => vec![
            json!({ "op": "background", "w": 800, "h": 800, "gradient": "solid", "color": "#ffffff", "fit": "contain", "pad": 32, "anchor": "center" }),
            json!({ "op": "sharpen", "amount": 0.5, "radius": 1.0 }),
        ],
        "wechat_cover" | "wechat" => vec![
            json!({ "op": "extend", "ratio": "9:5", "blur": 40, "dim": 0.25 }),
            json!({ "op": "resize", "w": 900, "h": 500, "fit": "cover" }),
            json!({ "op": "adjust", "contrast": 1.04 }),
        ],
        "avatar" => vec![
            json!({ "op": "crop", "ratio": "1:1", "anchor": "center" }),
            json!({ "op": "resize", "w": 200, "h": 200, "fit": "cover" }),
            json!({ "op": "round", "pct": 50 }),
            json!({ "op": "strip" }),
        ],
        "story_9_16" | "story" => vec![
            json!({ "op": "extend", "w": 1080, "h": 1920, "ratio": "9:16", "blur": 60, "dim": 0.3 }),
            json!({ "op": "sharpen", "amount": 0.6 }),
        ],
        "strip_4x3" | "strip4x3" | "strip" => vec![
            json!({ "op": "extend", "ratio": "4:3", "blur": 30, "dim": 0.2 }),
            json!({ "op": "resize", "w": 1600, "h": 1200, "fit": "cover" }),
            json!({ "op": "sharpen", "amount": 0.6, "radius": 1.5 }),
        ],
        "thumbnail_256" | "thumb256" | "thumbnail" => vec![
            json!({ "op": "resize", "w": 256, "h": 256, "fit": "contain", "bg": "transparent" }),
            json!({ "op": "strip" }),
        ],
        "print_a4" | "a4" => vec![
            json!({ "op": "background", "w": 2480, "h": 3508, "gradient": "solid", "color": "#ffffff", "fit": "contain", "pad": 96 }),
            json!({ "op": "strip" }),
        ],
        "web_hero" | "hero" => vec![
            json!({ "op": "resize", "w": 1920, "h": 1080, "fit": "cover" }),
            json!({ "op": "adjust", "brightness": -0.04, "contrast": 1.08, "saturation": 1.05 }),
            json!({ "op": "vignette", "strength": 0.35, "radius": 0.8 }),
            json!({ "op": "sharpen", "amount": 0.5 }),
        ],
        "compress_200k" | "compress" => vec![
            json!({ "op": "optimize", "kb": 200, "fmt": "jpeg" }),
        ],
        other => {
            return Err(Fail::new("UNSUPPORTED", format!("unknown preset \"{other}\""))
                .hint(format!("presets: {}", PRESETS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "))));
        }
    };
    Ok(steps)
}

/// Run a named preset over a file pair. Returns the same shape as [`apply_file`].
pub fn apply_preset(name: &str, inp: &Path, out: &Path) -> R<Value> {
    let steps = preset_chain(name)?;
    let mut v = process_one(inp, Some(out), &steps, 0, None, None, None)?;
    v["preset"] = json!(name);
    Ok(v)
}

fn pick_ext(out: &Path, hint: Option<&str>) -> String {
    let e = util::ext(out);
    match hint {
        Some(h) if fmt_of_ext(h).is_some() || fmt_of_ext(&e).is_none() => match h {
            "jpeg" => "jpg".to_string(),
            other => other.to_string(),
        },
        _ if fmt_of_ext(&e).is_some() => e,
        _ => "png".to_string(),
    }
}

// ------------------------------------------------------------------ file level

/// The engine behind `image.op`, `image.chain` and `image.preset`: decode, run
/// the steps, encode, write. Returns the merged step report plus the artifact
/// summary `{out,rel,in,w,h,bytes,size,sha,mime,fmt,quality?}`.
pub fn process_one(
    inp: &Path,
    out: Option<&Path>,
    steps: &[Value],
    q: u8,
    fmt: Option<&str>,
    bg: Option<[u8; 4]>,
    ctx: Option<&Path>,
) -> R<Value> {
    let img = load(inp)?;
    let (res, rep) = run_chain_report(img, steps, ctx)?;
    let fmt_hint = rep.get("fmt").and_then(|v| v.as_str()).map(|s| s.to_string());
    let mut out_path = match out {
        Some(p) => p.to_path_buf(),
        None => util::outp(&format!("{}.png", util::stem(inp)), ctx),
    };
    let ext = match fmt {
        Some(f) => norm_fmt(f)?.to_string(),
        None => pick_ext(&out_path, fmt_hint.as_deref()),
    };
    if fmt_of_ext(&ext).is_some() {
        out_path = util::with_ext(&out_path, &ext);
    }
    util::check_writable(&out_path, ctx)?;
    let quality = if (1..=100).contains(&q) {
        q
    } else {
        rep.get("quality").and_then(|v| v.as_u64()).map(|x| x as u8).unwrap_or(92)
    };
    let mut opts = serde_json::Map::new();
    opts.insert("q".into(), json!(quality));
    opts.insert("fmt".into(), json!(ext));
    if let Some(c) = bg {
        opts.insert("bg".into(), json!(hex(c)));
    }
    let saved = save_ex(&res, &out_path, &Value::Object(opts))?;
    let mut v = rep;
    v["out"] = json!(disp(&out_path));
    v["rel"] = json!(util::short(&out_path, ctx));
    v["in"] = json!(disp(inp));
    v["w"] = json!(res.w);
    v["h"] = json!(res.h);
    v["bytes"] = saved["bytes"].clone();
    v["size"] = saved["size"].clone();
    v["sha"] = saved["sha"].clone();
    v["mime"] = saved["mime"].clone();
    v["fmt"] = saved["fmt"].clone();
    v["quality"] = json!(quality);
    if let Some(n) = saved.get("note") {
        v["note"] = n.clone();
    }
    if let Some(o) = v.as_object_mut() {
        o.remove("steps");
    }
    Ok(v)
}

/// Read `steps` out of a params object: `preset`, `chain|steps|ops`, or `op`+`args`.
fn steps_from(params: &Value) -> R<Vec<Value>> {
    if let Some(p) = asv(params, &["preset"]) {
        return preset_chain(p);
    }
    if let Some(c) = params.get("chain").or_else(|| params.get("steps")).or_else(|| params.get("ops")) {
        return chain_steps(c);
    }
    if let Some(name) = asv(params, &["op"]) {
        let a = cargs(params);
        return Ok(vec![json!({ "op": name, "args": a })]);
    }
    Ok(vec![])
}

/// The `args` object of a single-op call, tolerating a string shorthand.
fn cargs(params: &Value) -> Value {
    params.get("args").or_else(|| params.get("a")).cloned().unwrap_or(json!({}))
}

/// `image.op` / `image.chain` in one call. Params: `in|i|src|path`, `out|o`,
/// `op`+`args` | `chain` | `preset`, `q`, `fmt`, `bg`.
pub fn apply_file(args: &Value, ctx: Option<&Path>) -> R<Value> {
    let src = asv(args, &["in", "i", "src", "path", "input", "file"])
        .ok_or_else(|| bad("missing \"in\" path").hint("in:\"photo.jpg\""))?;
    let inp = resolve(src, ctx)?;
    let dst = asv(args, &["out", "o", "to", "dest", "output"]).map(|d| util::outp(d, ctx));
    let steps = steps_from(args)?;
    let q = au(args, &["q", "quality"], 0) as u8;
    let fmt = asv(args, &["fmt", "f", "format"]).map(|s| s.to_string());
    let bg = col(args, &["bg", "background", "matte"]);
    process_one(&inp, dst.as_deref(), &steps, q, fmt.as_deref(), bg, ctx)
}

/// `image.batch`: run the same chain over many files under rayon. One failure
/// never aborts the rest — each item answers for itself.
pub fn batch(args: &Value, ctx: Option<&Path>) -> R<Vec<Value>> {
    use rayon::prelude::*;
    let mut paths: Vec<PathBuf> = vec![];
    for k in ["paths", "files", "inputs"] {
        let a = util::arr(args, k);
        if !a.is_empty() {
            for v in a {
                let s = value_to_string(&v);
                paths.push(resolve(&s, ctx).map_err(|e| e.details(json!({ "path": s })))?);
            }
            break;
        }
    }
    if paths.is_empty() {
        let dir = asv(args, &["dir", "in_dir", "from"])
            .ok_or_else(|| bad("image.batch needs paths or dir").hint("dir:\"photos\",pattern:\"*.jpg\""))?;
        let dp = resolve(dir, ctx)?;
        let pat = asv(args, &["pattern", "glob", "match"]).unwrap_or("*");
        let mut found: Vec<PathBuf> = std::fs::read_dir(&dp)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .filter(|p| {
                let is_img = fmt_of_ext(&util::ext(p)).is_some();
                is_img && (glob_match(pat, &file_name(p)) || glob_match(pat, &util::ext(p)))
            })
            .collect();
        found.sort();
        paths = found;
    }
    if paths.is_empty() {
        return Err(Fail::new("NOT_FOUND", "image.batch matched no files")
            .hint("check the dir and pattern, e.g. pattern:\"*.jpg\""));
    }
    let steps = steps_from(args)?;
    if steps.is_empty() {
        return Err(bad("image.batch needs chain, preset or op"));
    }
    let out_dir = asv(args, &["out", "o", "out_dir", "dir_out"]).map(|d| util::outp(d, ctx));
    let pattern = asv(args, &["name", "name_pattern", "template"]).unwrap_or("{stem}.{ext}").to_string();
    let fmt = asv(args, &["fmt", "f", "format"]).map(|s| s.to_string());
    let q = au(args, &["q", "quality"], 0) as u8;
    let bg = col(args, &["bg", "background", "matte"]);
    let ctxo = ctx.map(|p| p.to_path_buf());
    let steps2 = std::sync::Arc::new(steps);
    let outd2 = out_dir.clone();
    let res: Vec<Value> = paths
        .par_iter()
        .cloned()
        .map(|p| {
            let c = ctxo.as_deref();
            let steps = steps2.as_ref();
            let target: Option<PathBuf> = outd2.as_ref().map(|od| {
                if pattern.contains('/') || pattern.contains('\\') {
                    od.join(&pattern)
                } else {
                    od.join(render_name(&pattern, &p, fmt.as_deref().unwrap_or("same")))
                }
            });
            match process_one(&p, target.as_deref(), steps, q, fmt.as_deref(), bg, c) {
                Ok(mut v) => {
                    if let Some(o) = v.as_object_mut() {
                        o.insert("ok".into(), json!(true));
                    }
                    v
                }
                Err(e) => {
                    let mut m = serde_json::Map::new();
                    m.insert("ok".into(), json!(false));
                    m.insert("in".into(), json!(disp(&p)));
                    m.insert("e".into(), json!(e.msg));
                    m.insert("code".into(), json!(e.code));
                    if let Some(h) = &e.hint {
                        m.insert("hint".into(), json!(h));
                    }
                    Value::Object(m)
                }
            }
        })
        .collect();
    Ok(res)
}

fn file_name(p: &Path) -> String {
    p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_lowercase()
}

fn render_name(pattern: &str, src: &Path, fmt: &str) -> String {
    let ext = match fmt {
        "jpeg" | "jpg" => "jpg".to_string(),
        "same" | "" => {
            let e = util::ext(src);
            if e.is_empty() {
                "png".to_string()
            } else {
                e
            }
        }
        other => other.to_string(),
    };
    pattern
        .replace("{stem}", &util::stem(src))
        .replace("{ext}", &ext)
        .replace("{name}", &file_name(src))
}

/// Small glob matcher: `*`, `?` and `[abc]`, case-insensitive, no recursion.
pub fn glob_match(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.to_lowercase().chars().collect();
    let t: Vec<char> = s.to_lowercase().chars().collect();
    glob_here(&p, &t)
}

fn glob_here(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    match p[0] {
        '*' => {
            for i in 0..=t.len() {
                if glob_here(&p[1..], &t[i..]) {
                    return true;
                }
            }
            false
        }
        '?' if !t.is_empty() => glob_here(&p[1..], &t[1..]),
        '[' => {
            let close = p.iter().position(|c| *c == ']').unwrap_or(0);
            if close == 0 || t.is_empty() {
                return false;
            }
            let set = &p[1..close];
            let neg = set.first() == Some(&'!') || set.first() == Some(&'^');
            let body = if neg { &set[1..] } else { set };
            let hit = body.contains(&t[0]);
            if hit == neg {
                return false;
            }
            glob_here(&p[close + 1..], &t[1..])
        }
        c if !t.is_empty() && t[0] == c => glob_here(&p[1..], &t[1..]),
        _ => false,
    }
}

/// The op list that feeds `system.caps`: name + one-line description, English.
pub fn ops() -> Vec<(&'static str, &'static str)> {
    OP_TABLE.iter().map(|(n, _, d)| (*n, *d)).collect()
}

/// Ops grouped by kind for the docs: `(group, [(name, description)])`.
pub fn ops_grouped() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    let mut out: Vec<(&'static str, Vec<(&'static str, &'static str)>)> = vec![];
    for (name, group, desc) in OP_TABLE {
        if let Some(e) = out.iter_mut().find(|(g, _)| g == group) {
            e.1.push((name, desc));
        } else {
            out.push((group, vec![(name, desc)]));
        }
    }
    out
}

/// Aliases accepted by [`op`], so the docs can list them honestly.
pub fn aliases() -> Vec<(&'static str, &'static str)> {
    ALIASES.to_vec()
}

/// One-file thumbnail for `image.thumb`: fit into `w` px wide, keep alpha for png.
pub fn thumb_file(inp: &Path, out: &Path, w: u32, q: u8) -> R<Value> {
    let img = load(inp)?;
    let target = if w == 0 { 256 } else { w }.clamp(1, MAX_DIM);
    let (res, _) = op_report(
        "resize",
        img,
        &json!({ "w": target, "h": target, "fit": "contain", "bg": "transparent" }),
        None,
    )?;
    let ext = {
        let e = util::ext(out);
        if fmt_of_ext(&e).is_some() {
            e
        } else if res.has_alpha() {
            "png".to_string()
        } else {
            "jpg".to_string()
        }
    };
    let path = util::with_ext(out, &ext);
    let saved = save_ex(&res, &path, &json!({ "q": q }))?;
    Ok(json!({
        "out": disp(&path),
        "w": res.w,
        "h": res.h,
        "bytes": saved["bytes"],
        "sha": saved["sha"],
        "fmt": ext,
    }))
}

/// `image.hash`: both perceptual hashes plus a size probe, for near-duplicate checks.
pub fn hash_file(p: &Path) -> R<Value> {
    let img = load(p)?;
    Ok(json!({
        "path": disp(p),
        "dhash": dhash(&img),
        "ahash": ahash(&img),
        "w": img.w,
        "h": img.h,
    }))
}

/// `image.hash` entry point: the perceptual hashes of a file, plus the file itself.
/// Same result as [`hash_file`], named the way the RPC surface calls it.
pub fn hash_of(p: &Path) -> R<Value> {
    hash_file(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(s: &str) -> Vec<Value> {
        parse_chain(s).unwrap_or_else(|e| panic!("chain {s:?} failed: {}", e.msg))
    }

    /// The point of the string form: it has to produce the same step objects the
    /// JSON form does, not merely something that runs.
    #[test]
    fn chain_string_form_yields_the_json_form() {
        let got = steps("crop 1:1 > scale 0.5 > sharpen 1.2 > watermark 品牌");
        let want = vec![
            json!({ "op": "crop", "ratio": "1:1" }),
            json!({ "op": "scale", "by": 0.5 }),
            json!({ "op": "sharpen", "amount": 1.2 }),
            json!({ "op": "watermark", "text": "品牌" }),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn every_documented_chain_string_parses() {
        for s in [
            "crop 1:1 > scale 0.5 > sharpen 1.2 > watermark 品牌",
            "crop 400x400 > scale 0.5 > sharpen",
            "crop 9:16 > scale 0.6 > sharpen 1.1 > border 8 #FFFFFF > watermark AgentCanvas",
        ] {
            assert!(!steps(s).is_empty(), "{s}");
        }
    }

    /// Each token has to land on a key the op actually reads, or the step would run
    /// and silently do nothing.
    #[test]
    fn positional_tokens_land_on_the_keys_the_op_reads() {
        let cases = [
            ("crop 400x400", json!({ "op": "crop", "size": "400x400" })),
            ("crop 400", json!({ "op": "crop", "size": "400" })),
            ("crop 400 400", json!({ "op": "crop", "w": 400, "h": 400 })),
            ("resize 800", json!({ "op": "resize", "size": "800" })),
            ("resize 800x600 contain", json!({ "op": "resize", "size": "800x600", "fit": "contain" })),
            ("scale 150%", json!({ "op": "scale", "by": "150%" })),
            ("pad 40 #ffffff", json!({ "op": "pad", "px": 40, "color": "#ffffff" })),
            ("border 8 #FFFFFF", json!({ "op": "border", "width": 8, "color": "#FFFFFF" })),
            ("blur 5", json!({ "op": "blur", "radius": 5 })),
            ("sharpen 1.2", json!({ "op": "sharpen", "amount": 1.2 })),
            ("rotate 90 crop", json!({ "op": "rotate", "angle": 90, "crop": true })),
            ("flip v", json!({ "op": "flip", "dir": "v" })),
            ("round 50%", json!({ "op": "round", "pct": 50 })),
            ("threshold 128 otsu", json!({ "op": "threshold", "value": 128, "otsu": true })),
            ("extend 9:16 60", json!({ "op": "extend", "ratio": "9:16", "blur": 60 })),
            ("remove_bg #00ff00 60 mask", json!({
                "op": "remove_bg", "color": "#00ff00", "tolerance": 60, "mask": true
            })),
            ("watermark logo.png center 0.35", json!({
                "op": "watermark", "src": "logo.png", "anchor": "center", "opacity": 0.35
            })),
            ("grid a.png b.png c.png 2", json!({
                "op": "grid", "paths": ["a.png", "b.png", "c.png"], "cols": 2
            })),
            ("trim 12 white", json!({ "op": "trim", "fuzz": 12, "color": "white" })),
            ("palette 8 kmeans", json!({ "op": "palette", "n": 8, "mode": "kmeans" })),
            ("optimize 200 jpeg", json!({ "op": "optimize", "kb": 200, "fmt": "jpeg" })),
            // key=value still wins and is not disturbed by the positional pass
            ("crop 1:1 anchor=top-left", json!({ "op": "crop", "ratio": "1:1", "anchor": "top-left" })),
        ];
        for (src, want) in cases {
            assert_eq!(steps(src), vec![want], "{src}");
        }
    }

    #[test]
    fn aliases_resolve_to_the_canonical_op_name() {
        assert_eq!(steps("unsharp 0.8"), vec![json!({ "op": "sharpen", "amount": 0.8 })]);
        assert_eq!(steps("trim-border 12"), vec![json!({ "op": "border-trim", "width": 12 })]);
    }

    #[test]
    fn an_argument_the_op_cannot_use_is_an_error() {
        // `adjust` reads eight named knobs and no positional one; guessing which
        // would silently change the picture.
        for src in ["adjust 1.04", "strip 3", "gray 2"] {
            let e = parse_chain(src).unwrap_err();
            assert_eq!(e.code, "INVALID_SCHEMA", "{src}");
            assert!(e.hint.is_some(), "{src} lost its hint");
        }
    }

    fn swatch(w: u32, h: u32) -> Img {
        let mut img = Img::new(w, h).unwrap();
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = (y * w as usize + x) * 4;
                let on = ((x / 4 + y / 4) % 2) == 0;
                img.px[i] = if on { 220 } else { 30 };
                img.px[i + 1] = if on { 90 } else { 200 };
                img.px[i + 2] = u8::try_from(x * 3 % 256).unwrap();
                img.px[i + 3] = 255;
            }
        }
        img
    }

    /// The real promise: the two spellings of a chain end on the same pixels.
    #[test]
    fn string_chain_and_json_chain_agree_pixel_for_pixel() {
        let json_steps = vec![
            json!({ "op": "crop", "ratio": "1:1" }),
            json!({ "op": "resize", "size": "24x24", "fit": "contain" }),
            json!({ "op": "sharpen", "amount": 1.2 }),
            json!({ "op": "blur", "radius": 2 }),
            json!({ "op": "rotate", "angle": 90 }),
            json!({ "op": "border", "width": 3, "color": "#ffffff" }),
        ];
        let string_steps =
            steps("crop 1:1 > resize 24x24 contain > sharpen 1.2 > blur 2 > rotate 90 > border 3 #ffffff");
        assert_eq!(string_steps, json_steps);
        let a = run_chain(swatch(64, 40), &json_steps).unwrap();
        let b = run_chain(swatch(64, 40), &string_steps).unwrap();
        assert_eq!((a.w, a.h), (b.w, b.h));
        assert_eq!(a.px, b.px);
        assert_eq!(dhash(&a), dhash(&b));
    }
}






