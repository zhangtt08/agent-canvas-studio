//! Acceptance tests for templating: instantiation, variables, series variants and
//! the one-call `poster` product path.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use agentcanvas_core::doc::Document;
use agentcanvas_core::imageops;
use agentcanvas_core::templates;

fn tmp() -> PathBuf {
    let d = tempfile::tempdir().expect("tempdir");
    // Keep the tempdir alive by leaking it: these tests only write under it and
    // the process exits after, so a stray directory is acceptable in a sandboxed
    // build tree.
    let p = d.path().to_path_buf();
    std::mem::forget(d);
    p
}

fn strip_stamp(id: &str) -> String {
    id.rsplit_once('-').map(|(_, s)| s.to_string()).unwrap_or_else(|| id.to_string())
}

fn template_record(doc: &Document, board: &str) -> Vec<Value> {
    doc.artboard(board)
        .and_then(|a| a.vars.get("template"))
        .and_then(|t| t.get("vars"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

// ---------------------------------------------------------------- instantiate

#[test]
fn instantiate_twice_has_disjoint_ids_and_rewritten_intent_refs() {
    let dir = tmp();
    let mut doc = Document::default();
    let a = templates::instantiate(&dir, "ecom_main", &mut doc, &json!({})).unwrap();
    let b = templates::instantiate(&dir, "ecom_main", &mut doc, &json!({})).unwrap();

    // Disjoint node ids between the two instantiations.
    let overlap: Vec<&String> = a.iter().filter(|x| b.contains(x)).collect();
    assert!(overlap.is_empty(), "ids must not collide: {overlap:?}");
    assert!(!a.is_empty() && !b.is_empty());

    // Every internal reference (center_in / below.of) resolves to a node inside
    // the SAME instantiation — this is what silently breaks the layout if missed.
    let set_a: std::collections::HashSet<&String> = a.iter().collect();
    let set_b: std::collections::HashSet<&String> = b.iter().collect();
    for id in a.iter().chain(b.iter()) {
        let n = doc.node(id).expect("node exists");
        if let Some(intent) = &n.layout.intent {
            if let Some(ci) = &intent.center_in {
                let in_a = set_a.contains(ci);
                let in_b = set_b.contains(ci);
                assert!(in_a || in_b, "center_in {ci} points outside any instantiation");
                // Same instantiation as the referring node.
                assert_eq!(in_a, set_a.contains(id), "center_in must stay within its own board");
            }
            for rel in [&intent.below, &intent.above, &intent.left_of, &intent.right_of] {
                if let Some(r) = rel {
                    let in_a = set_a.contains(&r.of);
                    let in_b = set_b.contains(&r.of);
                    assert!(in_a || in_b, "rel of {} points outside any instantiation", r.of);
                    assert_eq!(in_a, set_a.contains(id));
                }
            }
        }
    }
}

#[test]
fn instantiate_into_existing_board() {
    let dir = tmp();
    let mut doc = Document::default();
    doc.artboards.push(serde_json::from_value(json!({
        "id": "host", "width": 800.0, "height": 800.0, "layers": []
    }))
    .unwrap());
    let ids = templates::instantiate(&dir, "ecom_main", &mut doc, &json!({ "board": "host" })).unwrap();
    assert!(!ids.is_empty());
    assert_eq!(doc.artboards.len(), 1, "no new board when placing into an existing one");
    let host = doc.artboard("host").unwrap();
    assert!(!host.layers.is_empty());
    // Every top-level layer belongs to the host board.
    for lid in &host.layers {
        assert_eq!(doc.node(lid).unwrap().artboard.as_deref(), Some("host"));
    }
}

// ---------------------------------------------------------------- variables

#[test]
fn apply_variables_node_scoped_and_type_checks() {
    let mut doc = Document::default();
    doc.nodes.push(serde_json::from_value(json!({
        "id": "n1", "type": "rect", "box_": { "x": 0.0, "y": 0.0, "w": 100.0, "h": 50.0 }
    }))
    .unwrap());

    templates::apply_variables(&mut doc, &json!({ "n1": { "style.fill.color": "#ff0000", "box.w": 200.0 } }))
        .unwrap();
    let n = doc.node("n1").unwrap();
    assert_eq!(n.box_.w, 200.0);
    assert!(matches!(
        n.style.as_ref().and_then(|s| s.fill.as_ref()),
        Some(agentcanvas_core::doc::Paint::Solid { color }) if color == "#ff0000"
    ));

    // A number landing in a colour field is a typed error, not a silent coercion.
    let err = templates::apply_variables(&mut doc, &json!({ "n1": { "style.fill.color": 42 } }));
    assert_eq!(err.err().unwrap().code, "INVALID_VALUE");

    // A string landing in a numeric field likewise.
    let err = templates::apply_variables(&mut doc, &json!({ "n1": { "box.w": "wide" } }));
    assert_eq!(err.err().unwrap().code, "INVALID_VALUE");
}

#[test]
fn apply_variables_by_var_key_after_instantiate() {
    let dir = tmp();
    let mut doc = Document::default();
    templates::instantiate(&dir, "ecom_main", &mut doc, &json!({})).unwrap();
    let defs = template_record(&doc, &doc.artboards[0].id);
    let title_node = defs.iter().find(|d| d["key"] == "title").unwrap()["node"].as_str().unwrap().to_string();

    templates::apply_variables(&mut doc, &json!({ "title": "New Title" })).unwrap();
    assert_eq!(doc.node(&title_node).unwrap().text.as_ref().unwrap().content, "New Title");

    // Unknown key that is neither a node id nor a declared var errors out.
    assert_eq!(templates::apply_variables(&mut doc, &json!({ "nope": 1 })).err().unwrap().code, "INVALID_SCHEMA");
}

// ---------------------------------------------------------------- variants

#[test]
fn create_variants_three_boards_identical_except_accent() {
    let dir = tmp();
    let mut doc = Document::default();
    let base_ids = templates::apply_builtin(&mut doc, &dir, "ecom_main").unwrap();
    assert!(!base_ids.is_empty());
    let src = doc.artboards[0].id.clone();

    let colors = vec!["#ff0000", "#00ff00", "#0000ff"];
    let new_ids = templates::create_variants(&mut doc, &json!({ "from": src, "colors": colors, "count": 3 })).unwrap();
    let created: Vec<&str> = new_ids["boards"].as_array().unwrap().iter().map(|b| b.as_str().unwrap()).collect();
    assert_eq!(created.len(), 3, "the caller gets the board ids it can render");
    assert_eq!(new_ids["ids"].as_array().unwrap().len(), 3 * base_ids.len(), "every node was cloned per variant");
    assert_eq!(doc.artboards.len(), 4, "one source + three variants");

    let variant_boards: Vec<String> = doc.artboards[1..].iter().map(|a| a.id.clone()).collect();

    // Each variant's accent node carries the matching colour.
    let mut seen = vec![];
    for (i, b) in variant_boards.iter().enumerate() {
        let defs = template_record(&doc, b);
        let accent = defs.iter().find(|d| d["key"] == "accent").unwrap()["node"].as_str().unwrap();
        let col = doc
            .node(accent)
            .unwrap()
            .style
            .as_ref()
            .and_then(|s| s.fill.as_ref())
            .and_then(|f| match f {
                agentcanvas_core::doc::Paint::Solid { color } => Some(color.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(col, colors[i], "variant {i} accent");
        seen.push(col);
    }
    assert_eq!(seen, vec!["#ff0000", "#00ff00", "#0000ff"]);

    // Canonical node sets match across variants once the accent is neutralised.
    let canon = variant_boards.iter().map(|b| canonical_nodes(&doc, b)).collect::<Vec<_>>();
    assert_eq!(canon[0], canon[1], "variant node sets must be identical modulo accent");
    assert_eq!(canon[1], canon[2]);

    // Same id suffixes across variants (only the per-instance stamp differs).
    let suffixes = variant_boards.iter().map(|b| {
        let mut s: Vec<String> = doc.paint_order(b).iter().map(|id| strip_stamp(id)).collect();
        s.sort();
        s
    }).collect::<Vec<_>>();
    assert_eq!(suffixes[0], suffixes[1]);
    assert_eq!(suffixes[1], suffixes[2]);
}

/// Serialize a board's nodes with the per-instance stamp removed and the accent
/// colour neutralised, so two structurally identical variants compare equal.
fn canonical_nodes(doc: &Document, board: &str) -> Vec<Value> {
    let order = doc.paint_order(board);
    let stamp = order
        .first()
        .and_then(|id| id.rsplit_once('-').map(|(s, _)| s.to_string()))
        .unwrap_or_default();
    let defs = template_record(doc, board);
    let accent = defs.iter().find(|d| d["key"] == "accent").and_then(|d| d["node"].as_str()).unwrap_or("").to_string();

    let mut out = Vec::new();
    for id in &order {
        let mut v = serde_json::to_value(doc.node(id).unwrap()).unwrap();
        if v["id"].as_str() == Some(&accent) {
            if let Some(f) = v.get_mut("style").and_then(|s| s.get_mut("fill")) {
                f["color"] = json!("ACCENT");
            }
        }
        if let Some(o) = v.as_object_mut() {
            o.remove("artboard");
        }
        let s = v.to_string().replace(&stamp, "S");
        out.push(serde_json::from_str::<Value>(&s).unwrap());
    }
    out.sort_by(|a, b| a["id"].to_string().cmp(&b["id"].to_string()));
    out
}

// ---------------------------------------------------------------- builtin + save

#[test]
fn builtin_lists_eight_and_apply_produces_nodes() {
    let names: Vec<&str> = templates::builtin().iter().map(|b| b.0).collect();
    assert_eq!(names.len(), 8);
    let dir = tmp();
    for name in names {
        let mut doc = Document::default();
        let ids = templates::apply_builtin(&mut doc, &dir, name).unwrap();
        assert!(!ids.is_empty(), "{name} produced no nodes");
        assert_eq!(doc.artboards.len(), 1, "{name} created a board");
        assert_eq!(doc.paint_order(&doc.artboards[0].id).len(), ids.len());
    }
}

#[test]
fn save_from_board_then_instantiate_round_trip() {
    let dir = tmp();
    let mut doc = Document::default();
    doc.tokens.colors.push(agentcanvas_core::doc::ColorToken { name: "accent".into(), value: "#ff0000".into(), note: None });
    doc.assets.push(agentcanvas_core::doc::Asset {
        id: "a1".into(),
        name: "pic".into(),
        uri: "assets/a1.png".into(),
        mime: None,
        width: 200.0,
        height: 200.0,
        bytes: 0,
        sha256: None,
        alpha: None,
        kind: None,
        source: None,
        tags: vec![],
        thumb: None,
        palette: None,
        orientation: None,
    });
    doc.artboards.push(serde_json::from_value(json!({
        "id": "b1", "width": 1000.0, "height": 1000.0, "layers": ["t", "r", "i"]
    }))
    .unwrap());
    for n in [
        json!({ "id": "t", "type": "text", "z_role": "content", "box_": { "x": 0.0, "y": 0.0, "w": 400.0, "h": 80.0 },
                "text": { "content": "Title", "lines": [] } }),
        json!({ "id": "r", "type": "rect", "box_": { "x": 0.0, "y": 100.0, "w": 50.0, "h": 50.0 },
                "style": { "fill": { "t": "solid", "color": "#ff0000" } } }),
        json!({ "id": "i", "type": "image", "box_": { "x": 0.0, "y": 200.0, "w": 200.0, "h": 200.0 },
                "image": { "asset": "a1", "fit": "cover" } }),
    ] {
        doc.nodes.push(serde_json::from_value(n).unwrap());
    }

    let res = templates::save_from_board(&doc, &dir, "b1", "series").unwrap();
    assert!(Path::new(res["path"].as_str().unwrap()).is_file());
    let keypaths: Vec<&str> = res["vars"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["keypath"].as_str())
        .collect();
    assert!(keypaths.contains(&"text.content"), "{keypaths:?}");
    assert!(keypaths.contains(&"style.fill.color"), "{keypaths:?}");
    assert!(keypaths.contains(&"image.asset"), "{keypaths:?}");

    let mut doc2 = Document::default();
    let ids = templates::instantiate(&dir, "series", &mut doc2, &json!({})).unwrap();
    assert_eq!(ids.len(), 3);
    let r = &doc2.nodes.iter().find(|n| n.kind == agentcanvas_core::doc::NodeType::Rect).unwrap();
    // The accent colour round-tripped into the new node.
    assert!(matches!(&r.style.as_ref().unwrap().fill, Some(agentcanvas_core::doc::Paint::Solid { color }) if color == "#ff0000"));
}

// ---------------------------------------------------------------- poster

#[test]
fn poster_actually_paints_the_imported_image() {
    // A poster whose photo silently vanished still exports a valid, non-empty PNG —
    // so "the file exists" is not the assertion that catches it. This one counts the
    // pixels the asset is supposed to have painted.
    let dir = tmp();
    let png = dir.join("magenta.png");
    let mut px = Vec::with_capacity(400 * 500 * 4);
    for _ in 0..400 * 500 {
        px.extend_from_slice(&[255u8, 0, 255, 255]);
    }
    imageops::save(&imageops::Img { w: 400, h: 500, px }, &png, 90).unwrap();

    let spec = json!({
        "name": "painted",
        "preset": "poster",
        "layout": "poster_kv",
        "title": "PHOTO TEST",
        "image": png.to_string_lossy().replace('\\', "/"),
        "out": dir.join("exports").to_string_lossy().replace('\\', "/"),
    });
    let res = templates::poster(&dir, &spec).expect("poster succeeds");

    let main = std::fs::read_dir(dir.join("exports"))
        .expect("exports dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().map(|x| x == "png").unwrap_or(false) && !p.to_string_lossy().contains("preview"))
        .expect("an exported png");
    let out = imageops::decode_bytes(&std::fs::read(&main).unwrap()).expect("export decodes");

    let hits = out.px.chunks_exact(4).filter(|p| p[0] > 200 && p[2] > 200 && p[1] < 90).count();
    let share = hits as f64 / (out.w as f64 * out.h as f64);
    assert!(share > 0.02, "the hero photo painted {share:.4} of the canvas — it is missing");
    // Font-fallback notes are normal; an *asset* the renderer could not read is not.
    let lost = res["files"][0]["warns"]
        .as_array()
        .map(|w| w.iter().any(|x| x.as_str().unwrap_or("").contains("asset")))
        .unwrap_or(false);
    assert!(!lost, "the renderer admitted it lost an asset: {res}");
}


#[test]
fn poster_end_to_end_produces_files_and_no_errors() {
    let dir = tmp();
    // A real 600x800 PNG produced with the engine's own image writer.
    let png = dir.join("input.png");
    let img = imageops::Img {
        w: 600,
        h: 800,
        px: vec![240u8; 600 * 800 * 4],
    };
    imageops::save(&img, &png, 90).unwrap();
    assert!(png.is_file());

    let spec = json!({
        "name": "poster_test",
        "preset": "poster",
        "layout": "poster_kv",
        "title": "MEGA RELEASE",
        "subtitle": "the thing you have been waiting for",
        "accent": "#ff5a5f",
        "image": png.to_string_lossy().replace('\\', "/"),
        "out": dir.join("exports").to_string_lossy().replace('\\', "/"),
    });

    let res = templates::poster(&dir, &spec).expect("poster succeeds");

    // At least one existing, non-empty PNG artifact.
    let files = res["files"].as_array().unwrap();
    let png_file = files
        .iter()
        .filter(|f| f["f"].as_str() == Some("png"))
        .find(|f| {
            let p = f["path"].as_str().unwrap();
            Path::new(p).is_file() && f["bytes"].as_u64().unwrap_or(0) > 0
        });
    assert!(png_file.is_some(), "no exported PNG file on disk: {files:?}");
    let pf = png_file.unwrap();
    assert!(pf["w"].as_u64().unwrap() >= 1 && pf["sha"].as_str().unwrap().len() >= 8);

    // After the auto-fix pass, no error-severity issue remains.
    let errs = res["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["s"].as_str() == Some("error"))
        .cloned()
        .collect::<Vec<_>>();
    assert!(errs.is_empty(), "errors survived the auto-fix: {errs:?}");
    assert_eq!(res["report"]["errors"].as_u64().unwrap(), 0);
    assert_eq!(res["report"]["ok"], true);
}
