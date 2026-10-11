//! Asset import, dedupe, thumbnails and inspection.
//!
//! An asset is a *file the document points at*, not a blob inside the document:
//! `Asset.uri` is relative to the project directory (`assets/<sha8>.png`) or an
//! absolute path when the project only references it. Everything here keeps that
//! promise — the copy, the name, the hash, the thumbnail and the "is it still
//! there?" answer the UI shows as a real state.
//!
//! Rules that matter to callers:
//! - Dedupe is by sha256 of the *file bytes*. A duplicate import returns the id
//!   that already exists with `dup` set and does **not** add a second record.
//! - Names are content-addressed: `<sha256[0..8]>.<ext>`, with a counter suffix
//!   when a different file already owns that name.
//! - Every write goes through [`crate::util::check_writable`] against the project
//!   directory, so a project cannot be made to write outside itself.
//! - Vectors (`svg`) record `kind:"vector"`, `alpha:true` and an empty palette: a
//!   logo has no pixels. Their size is the root `viewBox` (the coordinate system the
//!   author drew in), falling back to the root `width`/`height` attributes, then `0`
//!   when the file declares nothing at all.

use crate::doc::{Asset, Document};
use crate::error::{Fail, R};
use crate::imageops;
use crate::util;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const ASSET_DIR: &str = "assets";
pub const THUMB_DIR: &str = "thumbs";
/// Palette length stored per raster asset.
pub const PALETTE_N: usize = 6;

/// What [`import`] and [`replace`] report. Field names are the contract's;
/// [`Imported::to_value`] is the compact shape the RPC layer returns.
#[derive(Serialize, Clone, Debug)]
pub struct Imported {
    pub id: String,
    pub uri: String,
    pub name: String,
    pub mime: String,
    /// `vector` | `raster`. Part of the record the caller just created, so an agent
    /// that imports a logo and reads it back in the same call sees the same `kind`
    /// `asset.list` / `asset.inspect` report.
    pub kind: String,
    pub w: f64,
    pub h: f64,
    pub bytes: u64,
    pub sha256: String,
    pub alpha: bool,
    pub dup: Option<String>,
    pub palette: Vec<String>,
}

impl Imported {
    /// Compact result JSON with the short keys the API promises.
    pub fn to_value(&self) -> Value {
        json!({
            "id": self.id,
            "uri": self.uri,
            "name": self.name,
            "mime": self.mime,
            "kind": self.kind,
            "w": util::round(self.w, 2),
            "h": util::round(self.h, 2),
            "bytes": self.bytes,
            "size": util::human_bytes(self.bytes),
            "sha": util::short_hash(&self.sha256),
            "sha256": self.sha256,
            "alpha": self.alpha,
            "dup": self.dup,
            "palette": self.palette,
        })
    }
}

// ------------------------------------------------------------------ helpers

fn guard(dir: &Path, p: &Path) -> R<()> {
    util::check_writable(p, Some(dir)).map_err(|e| {
        e.details(json!({ "path": util::short(p, Some(dir)), "project": util::short(dir, None) }))
    })
}

/// Absolute path of an asset's file: `uri` is project-relative unless absolute.
pub fn resolve_uri(dir: &Path, uri: &str) -> PathBuf {
    let p = Path::new(uri);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        dir.join(p)
    }
}

/// Forward-slash, root-relative display form for `Asset.uri`.
fn rel_uri(dir: &Path, p: &Path) -> String {
    util::short(p, Some(dir))
}

fn asset_of<'d>(doc: &'d Document, id: &str) -> Option<&'d Asset> {
    doc.assets.iter().find(|a| a.id == id || a.name == id || a.uri == id)
}

fn svg_root(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes).to_string();
    let start = text.find("<svg")?;
    let rest = &text[start..];
    let end = rest.find('>')?;
    Some(rest[..=end].to_string())
}

fn attr(root: &str, name: &str) -> Option<String> {
    let mut from = 0usize;
    while let Some(i) = root[from..].find(name) {
        let after = i + name.len() + from;
        // make sure it is a whole attribute name, not a suffix of another
        let before_ok = root[..i]
            .chars()
            .last()
            .map(|c| c == ' ' || c == '\t' || c == '\n' || c == '<')
            .unwrap_or(true);
        let tail = root[after..].trim_start();
        if before_ok && tail.starts_with('=') {
            let v = tail[1..].trim_start();
            let quote = v.chars().next()?;
            if quote == '"' || quote == '\'' {
                let end = v[1..].find(quote)?;
                return Some(v[1..1 + end].to_string());
            }
            let end = v.find(|c: char| c == ' ' || c == '>' ).unwrap_or(v.len());
            return Some(v[..end].to_string());
        }
        from = after;
    }
    None
}

/// Length attribute in px units (`300`, `300px`, `10em`, `12pt`, `4in`).
/// A percentage has no intrinsic size, so it reports `None` and [`svg_size`] falls
/// back through the `viewBox` (which is checked first anyway) to `None` itself.
fn length_px(s: &str) -> Option<f64> {
    let t = s.trim().to_lowercase();
    if t.is_empty() || t.ends_with('%') {
        return None;
    }
    let table: &[(&str, f64)] = &[
        ("mm", 96.0 / 25.4),
        ("cm", 96.0 / 2.54),
        ("in", 96.0),
        ("pt", 96.0 / 72.0),
        ("pc", 16.0),
        ("rem", 16.0),
        ("em", 16.0),
        ("px", 1.0),
    ];
    let mut num: String = t.clone();
    let mut factor = 1.0;
    for (unit, f) in table {
        if let Some(stripped) = t.strip_suffix(unit) {
            if stripped.chars().last().map(|c| c.is_ascii_digit() || c == '.').unwrap_or(false) {
                num = stripped.to_string();
                factor = *f;
                break;
            }
        }
    }
    if !num
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E')
    {
        return None;
    }
    let v = num.parse::<f64>().ok()?;
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    Some(v * factor)
}

/// The `viewBox` of the root element, as the two size numbers it declares (the
/// pair after the `min-x`/`min-y` origin).
fn viewbox_size(root: &str) -> Option<(f64, f64)> {
    let vb = attr(root, "viewBox")?;
    let nums: Vec<f64> = vb
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.trim().is_empty())
        .filter_map(|p| p.parse::<f64>().ok())
        .collect();
    if nums.len() == 4 && nums[2].is_finite() && nums[3].is_finite() && nums[2] > 0.0 && nums[3] > 0.0 {
        Some((nums[2], nums[3]))
    } else {
        None
    }
}

/// Intrinsic size of an SVG document without rasterising it.
///
/// The root `viewBox` is the coordinate system the author drew in, so it wins; the
/// root `width`/`height` attributes are the fallback for a file that declares no
/// viewBox. `None` when neither is usable — the caller then either asks the
/// rasteriser what it would draw or records `0`, the honest "no intrinsic size".
fn svg_size(bytes: &[u8]) -> Option<(f64, f64)> {
    let root = svg_root(bytes)?;
    if let Some(vb) = viewbox_size(&root) {
        return Some(vb);
    }
    let w = attr(&root, "width").as_deref().and_then(length_px)?;
    let h = attr(&root, "height").as_deref().and_then(length_px)?;
    if w > 0.0 && h > 0.0 {
        Some((w, h))
    } else {
        None
    }
}

fn is_svg(p: &Path, mime_hint: Option<&str>) -> bool {
    util::ext(p) == "svg" || matches!(mime_hint, Some("image/svg+xml"))
}

/// Everything measured from the file, produced in one pass.
struct Read {
    w: f64,
    h: f64,
    alpha: bool,
    palette: Vec<String>,
    kind: &'static str,
}

fn measure(src: &Path) -> R<Read> {
    let bytes = util::read(src)?;
    if is_svg(src, None) {
        let (w, h) = match svg_size(&bytes) {
            Some(v) => v,
            None => match imageops::raster_svg(&bytes, None) {
                // no intrinsic size declared: ask the rasteriser what it would draw
                Ok(img) => (f64::from(img.w), f64::from(img.h)),
                // and when even that has nothing to say, `0` is the honest answer:
                // the record keeps the bytes and the hash, and the first caller that
                // needs real dimensions (a `keep_aspect_ratio` intent, an export)
                // reports it there instead of the import inventing a size.
                Err(_) => (0.0, 0.0),
            },
        };
        return Ok(Read {
            w,
            h,
            alpha: true,
            palette: vec![],
            kind: "vector",
        });
    }
    let img = imageops::decode_bytes(&bytes)?;
    let palette: Vec<String> = imageops::palette_of(&img, PALETTE_N)
        .into_iter()
        .map(|(hex, _)| hex)
        .collect();
    Ok(Read {
        w: f64::from(img.w),
        h: f64::from(img.h),
        alpha: img.has_alpha(),
        palette,
        kind: "raster",
    })
}

fn find_by_sha<'d>(doc: &'d Document, sha: &str) -> Option<&'d Asset> {
    doc.assets.iter().find(|a| a.sha256.as_deref() == Some(sha))
}

/// `kind` for a record the caller handed back: what it says, and what the file
/// name says when an older document has no `kind` stored.
fn kind_of(a: &Asset) -> String {
    match a.kind.as_deref() {
        Some(k) => k.to_string(),
        None => {
            let svg = util::ext(Path::new(&a.uri)) == "svg" || a.mime.as_deref() == Some("image/svg+xml");
            if svg { "vector" } else { "raster" }.to_string()
        }
    }
}

fn imported_of(a: &Asset, dup: Option<String>) -> Imported {
    Imported {
        id: a.id.clone(),
        uri: a.uri.clone(),
        name: a.name.clone(),
        mime: a.mime.clone().unwrap_or_else(|| "application/octet-stream".into()),
        kind: kind_of(a),
        w: a.width,
        h: a.height,
        bytes: a.bytes,
        sha256: a.sha256.clone().unwrap_or_default(),
        alpha: a.alpha.unwrap_or(false),
        dup,
        palette: a.palette.clone().unwrap_or_default(),
    }
}

/// Content-addressed, collision-free file name inside the project.
fn target_name(dir: &Path, sha: &str, ext: &str) -> R<PathBuf> {
    let assets = dir.join(ASSET_DIR);
    std::fs::create_dir_all(&assets)?;
    let base = &sha[..8.min(sha.len())];
    let mut cand = assets.join(if ext.is_empty() {
        format!("{base}")
    } else {
        format!("{base}.{ext}")
    });
    let mut n = 2u32;
    loop {
        match std::fs::metadata(&cand) {
            Ok(_) => {
                // same content already on disk under that name: reuse it
                if util::sha256_file(&cand).map(|s| s == sha).unwrap_or(false) {
                    return Ok(cand);
                }
                let name = if ext.is_empty() {
                    format!("{base}-{n}")
                } else {
                    format!("{base}-{n}.{ext}")
                };
                cand = assets.join(name);
                n += 1;
            }
            Err(_) => return Ok(cand),
        }
    }
}

// ------------------------------------------------------------------ import

/// Copy (or reference) `src` into `<dir>/assets/`, deduped by sha256.
///
/// `opts`: `mode:"copy"|"ref"`, `dedupe:0/1`, `name`, `tags:[..]`, `id`,
/// `mime`, `force:1` (skip the readable-file checks and store as-is).
pub fn import(doc: &mut Document, dir: &Path, src: &Path, opts: &Value) -> R<Imported> {
    let src = util::clean(src);
    if !src.is_file() {
        return Err(Fail::new("NOT_FOUND", format!("no such file: {}", src.display()))
            .hint("asset.import wants a file path, not a directory; check `in`/`path`"));
    }
    let sha = util::sha256_file(&src)?;
    let dedupe = !matches!(util::f(opts, "dedupe"), Some(v) if v == 0.0)
        && !util::opt_b(opts, "no_dedupe", false)
        && opts.get("dedupe").map(|v| !matches!(v, Value::Bool(false))).unwrap_or(true);
    if dedupe {
        if let Some(existing) = find_by_sha(doc, &sha) {
            // A second copy of the same bytes: report the record that already
            // exists and add nothing. Callers see `dup` and can skip the node.
            let mut out = imported_of(existing, Some(existing.id.clone()));
            if out.uri.is_empty() {
                out.uri = existing.uri.clone();
            }
            return Ok(out);
        }
    }
    let mode = util::ps(opts, "mode").unwrap_or("copy").to_lowercase();
    let referenced = matches!(mode.as_str(), "ref" | "reference" | "link" | "keep");
    let ext = util::ext(&src);
    let mime = util::ps(opts, "mime").map(|s| s.to_string()).unwrap_or_else(|| {
        let m = imageops::mime_of(&ext);
        if m == "application/octet-stream" {
            guess_mime(&src, &ext)
        } else {
            m.to_string()
        }
    });
    let (uri, bytes_on_disk) = if referenced {
        (disp_abs(&src), std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0))
    } else {
        let target = target_name(dir, &sha, &ext)?;
        guard(dir, &target)?;
        let n = util::copy(&src, &target)?;
        (rel_uri(dir, &target), n)
    };
    let r = measure(&src)?;
    let name = util::ps(opts, "name")
        .map(|s| s.to_string())
        .unwrap_or_else(|| src.file_stem().and_then(|s| s.to_str()).unwrap_or("asset").to_string());
    let id = util::ps(opts, "id")
        .map(|s| s.to_string())
        .unwrap_or_else(|| util::id("as"));
    if doc.assets.iter().any(|a| a.id == id) {
        return Err(inval_id_conflict(&id));
    }
    let tags: Vec<String> = util::arr(opts, "tags")
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    let asset = Asset {
        id: id.clone(),
        name,
        uri: uri.clone(),
        mime: Some(mime.clone()),
        width: r.w,
        height: r.h,
        bytes: bytes_on_disk,
        sha256: Some(sha.clone()),
        alpha: Some(r.alpha),
        kind: Some(r.kind.to_string()),
        source: Some(if referenced { "ref".into() } else { "copy".into() }),
        tags,
        thumb: None,
        palette: if r.palette.is_empty() { None } else { Some(r.palette.clone()) },
        orientation: Some(imageops::orientation_label(r.w.max(0.0) as u32, r.h.max(0.0) as u32).to_string()),
    };
    doc.assets.push(asset.clone());
    Ok(imported_of(&asset, None))
}

fn inval_id_conflict(id: &str) -> Fail {
    Fail::new("INVALID_VALUE", format!("asset id \"{id}\" already exists")).hint("omit `id` to let the engine assign one")
}

fn disp_abs(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn guess_mime(p: &Path, ext: &str) -> String {
    if ext == "svg" {
        return "image/svg+xml".to_string();
    }
    let Ok(b) = std::fs::read(p) else {
        return "application/octet-stream".to_string()
    };
    let head = &b[..b.len().min(16)];
    let sig = if head.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if head.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        "image/gif"
    } else if head.starts_with(&[b'B', b'M']) {
        "image/bmp"
    } else if head.starts_with(b"RIFF") && b.len() > 12 && &b[8..12] == b"WEBP" {
        "image/webp"
    } else if head.starts_with(&[0x49, 0x49, 0x2a, 0x00]) || head.starts_with(&[0x4d, 0x4d, 0x00, 0x2a]) {
        "image/tiff"
    } else {
        "text/plain"
    };
    sig.to_string()
}

/// Import a list in one call; `paths` may contain directories to skip.
pub fn import_many(doc: &mut Document, dir: &Path, srcs: &[PathBuf], opts: &Value) -> R<Vec<Value>> {
    let mut out = Vec::with_capacity(srcs.len());
    for s in srcs {
        let one = import(doc, dir, s, opts)?;
        out.push(one.to_value());
    }
    Ok(out)
}

// ------------------------------------------------------------------ thumbnails

/// Write `<dir>/thumbs/<id>_<w>.png` (or `.jpg` when the asset has no alpha) and
/// return the path. The file is decoded once and reused for both the size and
/// the pixels, so a thumbnail never costs two reads.
pub fn thumb(doc: &Document, dir: &Path, asset_id: &str, w: u32) -> R<PathBuf> {
    Ok(thumb_json(doc, dir, asset_id, w)?.1)
}

/// [`thumb`] plus the record-shaped summary callers usually want.
pub fn thumb_json(doc: &Document, dir: &Path, asset_id: &str, w: u32) -> R<(Value, PathBuf)> {
    let a = asset_of(doc, asset_id).ok_or_else(|| asset_not_found(asset_id))?;
    let path = resolve_uri(dir, &a.uri);
    if !path.is_file() {
        return Err(Fail::new("ASSET_MISSING", format!("{} is gone", a.uri))
            .hint("asset.missing lists every broken reference"));
    }
    let want = if w == 0 { 256 } else { w.clamp(1, imageops::MAX_DIM) };
    let is_vec = matches!(a.kind.as_deref(), Some("vector")) || util::ext(&path) == "svg";
    // one decode for both the natural size and the pixels
    let img = if is_vec {
        let bytes = util::read(&path)?;
        let nat = svg_size(&bytes).unwrap_or_else(|| {
            let i = imageops::raster_svg(&bytes, None).unwrap_or(imageops::Img {
                w: 1,
                h: 1,
                px: vec![0; 4],
            });
            (f64::from(i.w), f64::from(i.h))
        });
        let scale = (f64::from(want) / nat.0.max(1.0)).clamp(0.01, 32.0);
        imageops::raster_svg(&bytes, Some(scale))?
    } else {
        let img = imageops::load(&path)?;
        if img.w == 0 {
            return Err(Fail::new("DECODE_FAILED", format!("{} has zero width", a.uri)));
        }
        let factor = f64::from(want) / f64::from(img.w);
        if (factor - 1.0).abs() > 1e-6 {
            let h = (f64::from(img.h) * factor).round().max(1.0) as u32;
            imageops::resample(&img, want, h, "lanczos")?
        } else {
            img
        }
    };
    let alpha = img.has_alpha();
    let ext = if alpha || is_vec { "png" } else { "jpg" };
    let thumbs = dir.join(THUMB_DIR);
    std::fs::create_dir_all(&thumbs)?;
    let out = thumbs.join(format!("{}_{}.{}", a.id, want, ext));
    guard(dir, &out)?;
    let q = if ext == "jpg" { 82 } else { 92 };
    imageops::save(&img, &out, q)?;
    let v = json!({
        "path": rel_uri(dir, &out),
        "abs": disp_abs(&out),
        "w": img.w,
        "h": img.h,
        "bytes": std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0),
        "fmt": ext,
        "asset": a.id,
    });
    Ok((v, out))
}

fn asset_not_found(id: &str) -> Fail {
    Fail::new("ASSET_NOT_FOUND", format!("no asset with id or name \"{id}\""))
        .hint("asset.list shows the ids in this project")
}

// ------------------------------------------------------------------ inspect / missing

/// Which nodes use this asset, so a delete can warn about what breaks.
pub fn usage(doc: &Document, id: &str) -> Vec<Value> {
    let mut out = vec![];
    for n in &doc.nodes {
        let used = n
            .image
            .as_ref()
            .map(|i| i.asset == id)
            .unwrap_or(false);
        let painted = n
            .style
            .as_ref()
            .and_then(|s| s.fill.as_ref())
            .map(|p| match p {
                crate::doc::Paint::Image { asset, .. } => asset == id,
                _ => false,
            })
            .unwrap_or(false);
        if used || painted {
            out.push(json!({
                "node": n.id,
                "name": n.name,
                "board": n.artboard,
                "where": if painted { "fill" } else { "image" },
            }));
        }
    }
    for b in &doc.artboards {
        if let Some(crate::doc::Paint::Image { asset, .. }) = b.background.as_ref() {
            if asset == id {
                out.push(json!({ "node": b.id, "name": b.name, "board": b.id, "where": "artboard-background" }));
            }
        }
    }
    out
}

/// Full record plus usage — the shape `asset.inspect` returns.
pub fn inspect(doc: &Document, dir: &Path, id: &str) -> R<Value> {
    let a = asset_of(doc, id).ok_or_else(|| asset_not_found(id))?;
    let path = resolve_uri(dir, &a.uri);
    let on_disk = path.is_file();
    let live_sha = if on_disk { util::sha256_file(&path).ok() } else { None };
    let stale = match (&a.sha256, &live_sha) {
        (Some(want), Some(have)) => want != have,
        _ => false,
    };
    let mut v = json!({
        "id": a.id,
        "name": a.name,
        "uri": a.uri,
        "path": disp_abs(&path),
        "mime": a.mime,
        "kind": a.kind,
        "source": a.source,
        "w": util::round(a.width, 2),
        "h": util::round(a.height, 2),
        "bytes": a.bytes,
        "size": util::human_bytes(a.bytes),
        "sha": a.sha256.as_deref().map(util::short_hash),
        "sha256": a.sha256,
        "alpha": a.alpha,
        "orientation": a.orientation,
        "palette": a.palette,
        "tags": a.tags,
        "thumb": a.thumb,
        "on_disk": on_disk,
        "stale": stale,
        "used_by": usage(doc, &a.id),
        "uses": usage_count(doc, &a.id),
    });
    if on_disk {
        if let Some(meta) = std::fs::metadata(&path).ok() {
            v["actual_bytes"] = json!(meta.len());
        }
    }
    Ok(v)
}

pub fn usage_count(doc: &Document, id: &str) -> usize {
    usage(doc, id).len()
}

/// `asset.list` with an optional query over name / mime / kind.
pub fn list(doc: &Document, q: Option<&str>) -> Vec<Value> {
    let needle = q.map(|s| s.to_lowercase());
    doc.assets
        .iter()
        .filter(|a| match &needle {
            None => true,
            Some(n) => {
                a.name.to_lowercase().contains(n)
                    || a.id.to_lowercase().contains(n)
                    || a.mime.as_deref().unwrap_or("").contains(n)
                    || a.kind.as_deref().unwrap_or("").contains(n)
                    || a.tags.iter().any(|t| t.to_lowercase().contains(n))
            }
        })
        .map(|a| {
            json!({
                "id": a.id,
                "name": a.name,
                "uri": a.uri,
                "w": util::round(a.width, 2),
                "h": util::round(a.height, 2),
                "kind": a.kind,
                "mime": a.mime,
                "bytes": a.bytes,
                "alpha": a.alpha,
                "uses": usage_count(doc, &a.id),
            })
        })
        .collect()
}

/// Assets whose file is gone. The UI shows this as a real state, so it checks
/// existence on disk rather than trusting the record.
pub fn missing(doc: &Document, dir: &Path) -> R<Vec<Value>> {
    let mut out = vec![];
    for a in &doc.assets {
        let p = resolve_uri(dir, &a.uri);
        if p.is_file() {
            continue;
        }
        out.push(json!({
            "id": a.id,
            "uri": a.uri,
            "path": disp_abs(&p),
            "name": a.name,
            "kind": a.kind,
            "bytes": a.bytes,
            "uses": usage_count(doc, &a.id),
        }));
    }
    Ok(out)
}

/// Swap the file behind an id. Node references and the id survive; dims, sha,
/// mime, palette and byte count are re-read. The old sha is returned for the
/// audit log; the stale file is kept unless `purge:1`.
pub fn replace(doc: &mut Document, dir: &Path, id: &str, src: &Path) -> R<Imported> {
    let (v, imported) = replace_json(doc, dir, id, src, &json!({}))?;
    let _ = v;
    Ok(imported)
}

/// [`replace`] plus the audit summary `{id, sha, old_sha, ...}`.
pub fn replace_json(
    doc: &mut Document,
    dir: &Path,
    id: &str,
    src: &Path,
    opts: &Value,
) -> R<(Value, Imported)> {
    let idx = doc
        .assets
        .iter()
        .position(|a| a.id == id || a.name == id)
        .ok_or_else(|| asset_not_found(id))?;
    let old = doc.assets[idx].clone();
    let old_sha = old.sha256.clone().unwrap_or_default();
    let src = util::clean(src);
    if !src.is_file() {
        return Err(Fail::new("NOT_FOUND", format!("no such file: {}", src.display())));
    }
    let sha = util::sha256_file(&src)?;
    let ext = util::ext(&src);
    let referenced = old.source.as_deref() == Some("ref");
    let (uri, bytes) = if referenced {
        (disp_abs(&src), std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0))
    } else {
        let keep_name = util::opt_b(opts, "keep_name", false) && util::ext(Path::new(&old.uri)) == ext;
        let target = if keep_name {
            let p = resolve_uri(dir, &old.uri);
            guard(dir, &p)?;
            p
        } else {
            let p = target_name(dir, &sha, &ext)?;
            guard(dir, &p)?;
            p
        };
        util::copy(&src, &target)?;
        (rel_uri(dir, &target), std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0))
    };
    let r = measure(&src)?;
    let mut a = old.clone();
    a.uri = uri.clone();
    a.sha256 = Some(sha.clone());
    a.bytes = bytes;
    a.width = r.w;
    a.height = r.h;
    a.alpha = Some(r.alpha);
    a.kind = Some(r.kind.to_string());
    a.palette = if r.palette.is_empty() { None } else { Some(r.palette.clone()) };
    a.mime = Some(match old.mime.as_deref() {
        Some(m) if util::ext(Path::new(&uri)) == ext => m.to_string(),
        _ => guess_mime(&src, &ext),
    });
    a.orientation = Some(
        imageops::orientation_label(r.w.max(0.0) as u32, r.h.max(0.0) as u32).to_string(),
    );
    a.thumb = None;
    doc.assets[idx] = a.clone();
    if util::opt_b(opts, "purge", false) && old_sha != sha {
        let stale = resolve_uri(dir, &old.uri);
        if stale.is_file() && usage(doc, &a.id).is_empty() {
            guard(dir, &stale)?;
            let _ = std::fs::remove_file(&stale);
        }
    }
    let imported = imported_of(&a, None);
    let v = json!({
        "id": a.id,
        "ok": true,
        "sha": util::short_hash(&sha),
        "sha256": sha,
        "old_sha": util::short_hash(&old_sha),
        "old_sha256": old_sha,
        "uri": a.uri,
        "w": util::round(a.width, 2),
        "h": util::round(a.height, 2),
        "bytes": a.bytes,
        "alpha": a.alpha.unwrap_or(false),
        "palette": a.palette,
        "kept_old_file": !util::opt_b(opts, "purge", false),
    });
    Ok((v, imported))
}

/// Drop an asset record; refuses to act on something still referenced unless
/// `force` is set, and only deletes the file when nothing else points at it.
pub fn remove(doc: &mut Document, dir: &Path, id: &str, force: bool, purge_files: bool) -> R<Value> {
    let idx = doc
        .assets
        .iter()
        .position(|a| a.id == id || a.name == id)
        .ok_or_else(|| asset_not_found(id))?;
    let a = doc.assets[idx].clone();
    let used = usage(doc, &a.id);
    if !used.is_empty() && !force {
        return Err(Fail::new("INVALID_VALUE", format!("asset \"{}\" is used by {} node(s)", a.id, used.len()))
            .details(json!({ "used_by": used }))
            .hint("pass force:1 to delete anyway"));
    }
    doc.assets.remove(idx);
    let mut removed_file = false;
    if purge_files {
        let p = resolve_uri(dir, &a.uri);
        let still_used = doc.assets.iter().any(|x| x.uri == a.uri);
        if p.is_file() && !still_used && a.source.as_deref() != Some("ref") {
            guard(dir, &p)?;
            std::fs::remove_file(&p)?;
            removed_file = true;
        }
    }
    Ok(json!({ "ok": true, "id": a.id, "removed_file": removed_file, "unlinked_nodes": used.len() }))
}

/// Re-check every record against the disk and fix dims/hash after an external
/// edit. Used by `project.open` recovery.
pub fn verify(doc: &mut Document, dir: &Path, opts: &Value) -> R<Vec<Value>> {
    let fix = util::opt_b(opts, "fix", false);
    let mut changed = vec![];
    for i in 0..doc.assets.len() {
        let path = resolve_uri(dir, &doc.assets[i].uri);
        if !path.is_file() {
            changed.push(json!({ "id": doc.assets[i].id, "state": "missing" }));
            continue;
        }
        let sha = util::sha256_file(&path).ok();
        let stale = sha.as_deref() != doc.assets[i].sha256.as_deref();
        if !stale {
            continue;
        }
        changed.push(json!({
            "id": doc.assets[i].id,
            "state": "changed",
            "old": doc.assets[i].sha256,
            "new": sha,
        }));
        if fix {
            let r = measure(&path)?;
            let meta = std::fs::metadata(&path)?;
            let a = &mut doc.assets[i];
            a.sha256 = sha;
            a.bytes = meta.len();
            a.width = r.w;
            a.height = r.h;
            a.alpha = Some(r.alpha);
            a.kind = Some(r.kind.to_string());
            a.palette = if r.palette.is_empty() { None } else { Some(r.palette) };
            a.orientation = Some(imageops::orientation_label(r.w.max(0.0) as u32, r.h.max(0.0) as u32).to_string());
        }
    }
    Ok(changed)
}
