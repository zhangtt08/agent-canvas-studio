//! Acceptance tests for the machine-readable quality inspector.
//!
//! Documents are built straight from JSON so the tests read like the wire format
//! an agent would see. Numeric evidence is asserted to the pixel/ratio.

use std::path::Path;

use serde_json::{json, Value};

use agentcanvas_core::commands::Cmd;
use agentcanvas_core::doc::Document;
use agentcanvas_core::inspect::{self, Issue};

fn doc(v: Value) -> Document {
    serde_json::from_value(v).expect("valid document json")
}

/// Find the first issue (compact form) with a given code, if any.
fn find<'a>(report: &'a Value, code: &str) -> Option<&'a Value> {
    report["issues"]
        .as_array()?
        .iter()
        .find(|i| i["c"].as_str() == Some(code))
}

fn codes(report: &Value) -> Vec<String> {
    report["issues"]
        .as_array()
        .map(|a| a.iter().filter_map(|i| i["c"].as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default()
}

#[test]
fn offboard_overflow_r_is_exactly_20() {
    // A 40px node whose right edge reaches x=1100 on a 1080-wide board overflows
    // the right edge by exactly 20px.
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 1080.0, "height": 1440.0, "layers": ["n1"] }],
        "nodes": [{ "id": "n1", "type": "rect", "z_role": "decor", "box_": { "x": 1060.0, "y": 100.0, "w": 40.0, "h": 40.0 } }],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({})).unwrap();
    let iss = find(&r, "offboard").expect("offboard reported");
    assert_eq!(iss["s"], "warn", "partial overhang is a warn");
    assert_eq!(iss["n"], "n1");
    assert_eq!(iss["e"]["overflow"]["r"].as_f64().unwrap(), 20.0);
    assert_eq!(iss["e"]["board"]["w"].as_f64().unwrap(), 1080.0);
    // A mechanical fix is attached: translate the box back inside.
    assert_eq!(iss["f"]["t"], "Transform");
    assert_eq!(iss["f"]["id"], "n1");
}

#[test]
fn fully_outside_offboard_is_error() {
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 100.0, "height": 100.0, "layers": ["n1"] }],
        "nodes": [{ "id": "n1", "type": "rect", "box_": { "x": 500.0, "y": 500.0, "w": 40.0, "h": 40.0 } }],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({})).unwrap();
    let iss = find(&r, "offboard").unwrap();
    assert_eq!(iss["s"], "error");
    assert_eq!(iss["e"]["fully_out"], true);
}

#[test]
fn white_on_paper_is_a_contrast_error_near_1_09() {
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 1080.0, "height": 1440.0,
            "background": { "t": "solid", "color": "#f7f5f1" }, "layers": ["t1"] }],
        "nodes": [{ "id": "t1", "type": "text", "z_role": "content",
            "box_": { "x": 100.0, "y": 100.0, "w": 500.0, "h": 200.0 },
            "text": { "content": "Hello", "size": 48.0, "color": "#ffffff", "overflow": "clip", "lines": [] } }],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({ "contrast": 1 })).unwrap();
    let iss = find(&r, "contrast").expect("contrast reported");
    assert_eq!(iss["s"], "error", "ratio ~1.09 is far below 3.0");
    let ratio = iss["e"]["ratio"].as_f64().unwrap();
    assert!(ratio > 1.0 && ratio < 1.15, "ratio was {ratio}, expected ~1.09");
    assert_eq!(iss["e"]["need"].as_f64().unwrap(), 4.5);
    assert_eq!(iss["e"]["fg"], "#ffffff");
    assert_eq!(iss["e"]["bg"], "#f7f5f1");
    assert_eq!(r["ok"], false);
    assert_eq!(r["errors"].as_u64().unwrap(), 1);
}

#[test]
fn good_contrast_is_not_reported() {
    // rgb(255,255,255) on rgb(0,0,0) is ~21:1 — comfortably above the 4.5 bar.
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 1080.0, "height": 1440.0,
            "background": { "t": "solid", "color": "rgb(0,0,0)" }, "layers": ["t1"] }],
        "nodes": [{ "id": "t1", "type": "text", "z_role": "content",
            "box_": { "x": 100.0, "y": 100.0, "w": 500.0, "h": 200.0 },
            "text": { "content": "Hello", "size": 48.0, "color": "rgb(255,255,255)", "overflow": "shrink", "lines": [] } }],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({})).unwrap();
    assert!(find(&r, "contrast").is_none());
    assert_eq!(r["ok"], true);
}

#[test]
fn text_overflow_reports_need_have_lines() {
    // A long clip-mode paragraph in a squat box must overflow.
    let long = "The quick brown fox jumps over the lazy dog and keeps running far past the box it was given.";
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 1080.0, "height": 1440.0, "layers": ["t1"] }],
        "nodes": [{ "id": "t1", "type": "text", "z_role": "content",
            "box_": { "x": 100.0, "y": 100.0, "w": 300.0, "h": 40.0 },
            "text": { "content": long, "size": 40.0, "color": "#ffffff", "overflow": "clip", "lines": [] } }],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({})).unwrap();
    let iss = find(&r, "text-overflow").expect("text overflow reported");
    let need = iss["e"]["need"].as_f64().unwrap();
    let have = iss["e"]["have"].as_f64().unwrap();
    assert!(need >= have, "need {need} should not be less than have {have}");
    assert!(iss["e"]["lines"].is_number(), "lines evidence present");
    assert_eq!(iss["e"]["font_size"].as_f64().unwrap(), 40.0);
    // Suggested fix shrinks the font.
    assert_eq!(iss["f"]["t"], "NodeSet");
}

#[test]
fn shrink_mode_fits_without_overflow() {
    // A short string in an ample box, shrink mode, must not be flagged as
    // overflowing — the detector reports no false positives on fitting text.
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 1080.0, "height": 1440.0, "layers": ["t1"] }],
        "nodes": [{ "id": "t1", "type": "text", "z_role": "content",
            "box_": { "x": 100.0, "y": 100.0, "w": 600.0, "h": 120.0 },
            "text": { "content": "Aa", "size": 40.0, "color": "rgb(255,255,255)", "overflow": "shrink", "lines": [] } }],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({})).unwrap();
    assert!(find(&r, "text-overflow").is_none(), "shrink-to-fit must not overflow");
}

#[test]
fn zero_size_and_empty_text_and_bad_color() {
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 800.0, "height": 600.0, "layers": ["z", "e", "c"] }],
        "nodes": [
            { "id": "z", "type": "rect", "box_": { "x": 10.0, "y": 10.0, "w": 0.5, "h": 20.0 } },
            { "id": "e", "type": "text", "box_": { "x": 10.0, "y": 40.0, "w": 100.0, "h": 20.0 }, "text": { "content": "   ", "lines": [] } },
            { "id": "c", "type": "rect", "box_": { "x": 10.0, "y": 70.0, "w": 20.0, "h": 20.0 }, "style": { "fill": { "t": "solid", "color": "not-a-color" } } },
        ],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({})).unwrap();
    let c = codes(&r);
    assert!(c.contains(&"zero-size".to_string()), "{c:?}");
    assert!(c.contains(&"empty-text".to_string()), "{c:?}");
    assert!(c.contains(&"bad-color".to_string()), "{c:?}");
    assert_eq!(find(&r, "bad-color").unwrap()["s"], "error");
    assert_eq!(find(&r, "bad-color").unwrap()["e"]["value"], "not-a-color");
}

#[test]
fn missing_asset_is_warn() {
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 800.0, "height": 600.0, "layers": ["i1"] }],
        "nodes": [{ "id": "i1", "type": "image", "z_role": "content", "box_": { "x": 10.0, "y": 10.0, "w": 100.0, "h": 100.0 }, "image": { "asset": "ghost", "fit": "cover" } }],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({})).unwrap();
    assert_eq!(find(&r, "missing-asset").unwrap()["s"], "warn");
    assert_eq!(find(&r, "missing-asset").unwrap()["e"]["asset"], "ghost");
}

#[test]
fn low_res_when_upscaled() {
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 1080.0, "height": 1080.0, "layers": ["i1"] }],
        "assets": [{ "id": "a1", "name": "small", "uri": "assets/a1.png", "width": 100.0, "height": 100.0 }],
        "nodes": [{ "id": "i1", "type": "image", "z_role": "content", "box_": { "x": 10.0, "y": 10.0, "w": 300.0, "h": 300.0 }, "image": { "asset": "a1", "fit": "fill" } }],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({ "lowres": 1 })).unwrap();
    let iss = find(&r, "low-res").expect("low-res reported");
    assert!((iss["e"]["scale"].as_f64().unwrap() - 3.0).abs() < 0.01);
    // Toggling lowres off hides it.
    let r2 = inspect::validate(&d, Path::new("."), &json!({ "lowres": 0 })).unwrap();
    assert!(find(&r2, "low-res").is_none());
}

#[test]
fn content_fully_covered_by_opaque_is_invisible() {
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 1000.0, "height": 1000.0,
            "background": { "t": "solid", "color": "#101318" }, "layers": ["t1", "cover"] }],
        "nodes": [
            { "id": "t1", "type": "text", "z_role": "content", "box_": { "x": 100.0, "y": 100.0, "w": 300.0, "h": 120.0 },
              "text": { "content": "Hi", "size": 40.0, "color": "#ffffff", "overflow": "shrink", "lines": [] } },
            { "id": "cover", "type": "rect", "z_role": "decor", "box_": { "x": 0.0, "y": 0.0, "w": 1000.0, "h": 1000.0 },
              "style": { "fill": { "t": "solid", "color": "#202020" } } },
        ],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({})).unwrap();
    let iss = find(&r, "invisible").expect("covered content is invisible");
    assert_eq!(iss["n"], "t1");
    assert_eq!(iss["e"]["by"], "cover");
    assert_eq!(iss["e"]["reason"], "covered");
}

#[test]
fn decor_overlap_is_not_an_invisible_error() {
    // A background swatch covering a *decor* node must stay silent: overlap is
    // judged by z_role, and decor may be covered.
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 1000.0, "height": 1000.0, "layers": ["a", "b"] }],
        "nodes": [
            { "id": "a", "type": "rect", "z_role": "decor", "box_": { "x": 100.0, "y": 100.0, "w": 200.0, "h": 200.0 } },
            { "id": "b", "type": "rect", "z_role": "background", "box_": { "x": 0.0, "y": 0.0, "w": 1000.0, "h": 1000.0 },
              "style": { "fill": { "t": "solid", "color": "#ffffff" } } },
        ],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({})).unwrap();
    assert!(find(&r, "invisible").is_none());
}

#[test]
fn safe_area_crossing_is_warn() {
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 1080.0, "height": 1440.0, "safe_margin": 72.0, "layers": ["n1"] }],
        "nodes": [{ "id": "n1", "type": "rect", "z_role": "content", "box_": { "x": 20.0, "y": 200.0, "w": 100.0, "h": 100.0 } }],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({})).unwrap();
    let iss = find(&r, "safe-area").expect("safe area reported");
    assert_eq!(iss["e"]["margin"].as_f64().unwrap(), 72.0);
    assert_eq!(iss["e"]["overflow"]["l"].as_f64().unwrap(), 52.0);
}

#[test]
fn tiny_target_under_48() {
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 1080.0, "height": 1440.0, "layers": ["btn"] }],
        "nodes": [{ "id": "btn", "type": "rect", "role": "button", "box_": { "x": 100.0, "y": 100.0, "w": 40.0, "h": 30.0 } }],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({})).unwrap();
    let iss = find(&r, "tiny-target").expect("tiny target reported");
    assert_eq!(iss["e"]["need"].as_f64().unwrap(), 48.0);
    assert_eq!(iss["e"]["min"].as_f64().unwrap(), 30.0);
}

#[test]
fn dup_id_is_error() {
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 100.0, "height": 100.0, "layers": ["x"] }],
        "nodes": [
            { "id": "x", "type": "rect", "box_": { "x": 0.0, "y": 0.0, "w": 10.0, "h": 10.0 } },
            { "id": "x", "type": "rect", "box_": { "x": 0.0, "y": 0.0, "w": 10.0, "h": 10.0 } },
        ],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({})).unwrap();
    assert_eq!(find(&r, "dup-id").unwrap()["s"], "error");
}

#[test]
fn unlinked_node_is_info() {
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 100.0, "height": 100.0, "layers": [] }],
        "nodes": [{ "id": "orphan", "type": "rect", "box_": { "x": 0.0, "y": 0.0, "w": 10.0, "h": 10.0 } }],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({})).unwrap();
    let iss = find(&r, "unlinked").expect("orphan is unlinked");
    assert_eq!(iss["s"], "info");
    assert_eq!(iss["n"], "orphan");
}

#[test]
fn sev_min_filters_down() {
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 800.0, "height": 600.0, "layers": ["c", "z"] }],
        "nodes": [
            { "id": "c", "type": "rect", "box_": { "x": 10.0, "y": 10.0, "w": 20.0, "h": 20.0 }, "style": { "fill": { "t": "solid", "color": "oops" } } },
            { "id": "z", "type": "rect", "box_": { "x": 10.0, "y": 40.0, "w": 0.0, "h": 5.0 } },
        ],
    }));
    let all = inspect::validate(&d, Path::new("."), &json!({ "sev_min": "info" })).unwrap();
    assert!(all["warns"].as_u64().unwrap() >= 1);
    let errs = inspect::validate(&d, Path::new("."), &json!({ "sev_min": "error" })).unwrap();
    assert_eq!(errs["warns"], 0);
    assert_eq!(errs["infos"], 0);
    assert!(errs["errors"].as_u64().unwrap() >= 1);
    assert_eq!(codes(&errs), vec!["bad-color".to_string()]);
}

#[test]
fn board_filter_restricts_scope() {
    let d = doc(json!({
        "artboards": [
            { "id": "b1", "width": 100.0, "height": 100.0, "layers": ["n1"] },
            { "id": "b2", "width": 100.0, "height": 100.0, "layers": ["n2"] },
        ],
        "nodes": [
            { "id": "n1", "type": "rect", "box_": { "x": 0.0, "y": 0.0, "w": 10.0, "h": 10.0 } },
            { "id": "n2", "type": "rect", "box_": { "x": 90.0, "y": 0.0, "w": 20.0, "h": 10.0 } },
        ],
    }));
    let only = inspect::validate(&d, Path::new("."), &json!({ "board": "b2" })).unwrap();
    assert_eq!(only["boards"].as_array().unwrap().len(), 1);
    assert_eq!(only["boards"][0]["b"], "b2");
    assert!(codes(&only).contains(&"offboard".to_string()));
    // dup-id / unlinked are skipped when scoped to a single board.
    assert!(!codes(&only).contains(&"unlinked".to_string()));
}

#[test]
fn boards_report_ink_coverage() {
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 200.0, "height": 200.0, "layers": ["n1"] }],
        // a content node covering exactly a quarter of the board -> ink 0.25
        "nodes": [{ "id": "n1", "type": "rect", "z_role": "content", "box_": { "x": 0.0, "y": 0.0, "w": 100.0, "h": 100.0 } }],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({})).unwrap();
    let ink = r["boards"][0]["ink"].as_f64().unwrap();
    assert!((ink - 0.25).abs() < 0.01, "ink was {ink}");
    assert_eq!(r["boards"][0]["nodes"].as_u64().unwrap(), 1);
}

#[test]
fn geometry_text_metrics_asset_usage_and_diff() {
    let a = doc(json!({
        "artboards": [{ "id": "b1", "width": 100.0, "height": 100.0, "layers": ["n1"] }],
        "assets": [{ "id": "a1", "name": "pic", "uri": "assets/a1.png", "width": 10.0, "height": 10.0 }],
        "nodes": [{ "id": "n1", "type": "image", "box_": { "x": 0.0, "y": 0.0, "w": 50.0, "h": 40.0 },
            "rotation": 0.0, "image": { "asset": "a1", "fit": "cover" } }],
    }));
    let g = inspect::geometry(&a, &["n1".to_string()]);
    assert_eq!(g.len(), 1);
    assert_eq!(g[0]["rb"].as_array().unwrap().len(), 4);

    let usage = inspect::asset_usage(&a);
    assert_eq!(usage.iter().find(|u| u["id"] == "a1").unwrap()["uses"].as_u64().unwrap(), 1);

    let b = doc(json!({
        "artboards": [{ "id": "b1", "width": 100.0, "height": 100.0, "layers": ["n1"] }],
        "nodes": [{ "id": "n1", "type": "image", "box_": { "x": 0.0, "y": 0.0, "w": 60.0, "h": 40.0 } }],
    }));
    let diff = inspect::diff_docs(&a, &b);
    // a's node carried an image sub-object b dropped; box_.w changed 50 -> 60.
    let changed = diff["changed"].as_array().unwrap();
    let n1 = changed.iter().find(|c| c["id"] == "n1").expect("n1 changed");
    let keys: Vec<&str> = n1["keys"].as_array().unwrap().iter().filter_map(|k| k["k"].as_str()).collect();
    assert!(keys.contains(&"box.w"), "{keys:?}");

    let m = inspect::text_metrics(&doc(json!({
        "artboards": [{ "id": "b1", "width": 100.0, "height": 100.0, "layers": ["t1"] }],
        "nodes": [{ "id": "t1", "type": "text", "box_": { "x": 0.0, "y": 0.0, "w": 80.0, "h": 40.0 },
            "text": { "content": "Hi", "size": 20.0, "lines": [] } }],
    })), "t1");
    assert!(m.is_ok());
    assert!(m.unwrap()["need"].is_number());
}

#[test]
fn suggest_fixes_round_trips_cmds() {
    let fix = serde_json::to_value(Cmd::Transform { id: "n1".into(), x: 5.0, y: 6.0, w: 7.0, h: 8.0 }).unwrap();
    let issues = vec![Issue {
        sev: "warn",
        code: "offboard",
        node: Some("n1".into()),
        board: Some("b1".into()),
        msg: "off".into(),
        evidence: json!({}),
        fix: Some(fix),
    }];
    let cmds = inspect::suggest_fixes(&issues);
    assert_eq!(cmds.len(), 1);
    assert!(matches!(cmds[0], Cmd::Transform { .. }));
}

#[test]
fn tofu_and_missing_font_on_latin_only_face() {
    // An explicit family that is not installed should read as missing-font, and
    // CJK content in a Latin-only face should raise tofu-risk.
    let d = doc(json!({
        "artboards": [{ "id": "b1", "width": 800.0, "height": 600.0, "layers": ["t1"] }],
        "nodes": [{ "id": "t1", "type": "text", "z_role": "content",
            "box_": { "x": 100.0, "y": 100.0, "w": 500.0, "h": 200.0 },
            "text": { "content": "你好世界设计", "family": "Definitely Not Installed XYZ", "size": 40.0, "color": "#ffffff", "overflow": "shrink", "lines": [] } }],
    }));
    let r = inspect::validate(&d, Path::new("."), &json!({})).unwrap();
    let c = codes(&r);
    assert!(c.contains(&"missing-font".to_string()), "expected missing-font in {c:?}");
    let mf = find(&r, "missing-font").unwrap();
    assert!(mf["e"]["want"].is_string());
    assert!(mf["e"]["got"].is_string());
}
