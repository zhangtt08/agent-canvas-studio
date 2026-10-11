//! The picture has to contain the picture. These call the real `rpc::call` entry the
//! CLI/MCP/HTTP doors share, then read back the exported bytes: a render that drops an
//! asset still writes a valid PNG, so only the pixels can prove it painted.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use agentcanvas_core::{imageops, rpc};

fn tmp() -> PathBuf {
    let d = tempfile::tempdir().expect("tempdir");
    let p = d.path().to_path_buf();
    std::mem::forget(d);
    p
}

fn call(ctx: &rpc::Ctx, m: &str, p: Value) -> Value {
    rpc::call(m, &p, ctx).unwrap_or_else(|e| panic!("{m} failed: {} | {}", e.code, e.msg))
}

/// A solid magenta 400x500 PNG written with the engine's own encoder.
fn magenta(dir: &Path) -> PathBuf {
    let png = dir.join("magenta.png");
    let px: Vec<u8> = (0..400 * 500).flat_map(|_| [255u8, 0, 255, 255]).collect();
    imageops::save(&imageops::Img { w: 400, h: 500, px }, &png, 90).unwrap();
    png
}

/// Share of the raster that reads as the asset's magenta.
fn magenta_share(path: &Path) -> f64 {
    let img = imageops::decode_bytes(&std::fs::read(path).expect("raster reads back")).expect("png decodes");
    let hits = img.px.chunks_exact(4).filter(|p| p[0] > 200 && p[2] > 200 && p[1] < 90).count();
    hits as f64 / (img.w as f64 * img.h as f64)
}

/// The session registry is process-global and keyed by project id while cargo runs these
/// tests on threads of one process, so each test needs its own id — otherwise the second
/// `asset.import` lands in the first test's project directory.
fn project_with_photo(dir: &Path, id: &str) -> (rpc::Ctx, PathBuf) {
    let ctx = rpc::Ctx { root: Some(dir.to_path_buf()), origin: "test".into() };
    call(&ctx, "project.create", json!({"id": id, "name": id, "w": 800, "h": 800}));
    let png = magenta(dir);
    let imp = call(&ctx, "asset.import", json!({"p": id, "path": png.to_string_lossy().replace('\\', "/")}));
    let aid = imp["assets"][0]["id"].as_str().expect("asset id").to_string();
    call(&ctx, "canvas.add", json!({"p": id, "b": "b1", "t": "image", "asset": aid, "x": 80, "y": 80, "w": 640, "h": 640}));
    (ctx, dir.join("projects").join(id))
}

#[test]
fn preview_png_contains_the_photo_not_just_the_text() {
    let dir = tmp();
    let (ctx, prj) = project_with_photo(&dir, "pv-preview");
    call(&ctx, "render.preview", json!({"p": "pv-preview", "sc": 1.0}));
    let share = magenta_share(&prj.join("preview.png"));
    assert!(share > 0.5, "preview.png is {share:.3} magenta — the photo did not survive the compile");
}

#[test]
fn export_png_contains_the_photo() {
    let dir = tmp();
    let (ctx, _prj) = project_with_photo(&dir, "pv-export");
    let out = call(&ctx, "render.export", json!({"p": "pv-export", "f": "png", "o": "shots"}));
    let rel = out["files"][0]["path"].as_str().expect("path");
    let path = if Path::new(rel).is_absolute() { PathBuf::from(rel) } else { dir.join(rel) };
    let share = magenta_share(&path);
    assert!(share > 0.5, "export is {share:.3} magenta — the photo did not survive the compile");
}

#[test]
fn the_saved_card_preview_contains_the_photo_too() {
    // `project.save` refreshes the thumbnail the home page cards show. It used to
    // rasterise without an asset base, so every card was a poster without its photo.
    let dir = tmp();
    let (ctx, prj) = project_with_photo(&dir, "pv-card");
    call(&ctx, "project.save", json!({"p": "pv-card"}));
    let share = magenta_share(&prj.join("preview.png"));
    assert!(share > 0.5, "card preview is {share:.3} magenta — the photo did not survive the compile");
}
