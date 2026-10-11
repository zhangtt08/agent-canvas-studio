//! The constraint solver's two failure shapes: a cycle in the ordering, and a
//! relation that points at something which does not exist. Both must be *named* —
//! a silent no-op is how a layout ends up wrong without anyone being told.

use agentcanvas_core::doc::{Artboard, Document, Node};
use agentcanvas_core::layout;
use serde_json::json;

fn board(doc: &mut Document, layers: &[&str]) {
    let ab: Artboard = serde_json::from_value(json!({
        "id": "b1", "name": "board", "width": 1000.0, "height": 1000.0,
        "layers": layers,
    }))
    .unwrap();
    doc.artboards.push(ab);
}

fn rect(doc: &mut Document, id: &str, intent: serde_json::Value) {
    rect_named(doc, id, id, intent);
}

fn rect_named(doc: &mut Document, id: &str, name: &str, intent: serde_json::Value) {
    let n: Node = serde_json::from_value(json!({
        "id": id, "type": "rect", "name": name, "artboard": "b1",
        "box": {"x": 0.0, "y": 0.0, "w": 100.0, "h": 100.0},
        "layout": {"mode": "constraint", "intent": intent},
    }))
    .unwrap();
    doc.nodes.push(n);
}

#[test]
fn a_relation_may_name_its_target_when_the_name_is_unique() {
    let mut d = Document::default();
    board(&mut d, &["re_1", "re_2"]);
    rect_named(&mut d, "re_1", "标题", json!({ "anchor": "top", "margin": 40.0 }));
    rect_named(&mut d, "re_2", "主图", json!({ "below": "标题", "gap": 24.0 }));
    let rep = layout::solve(&mut d, Some("b1")).unwrap();
    assert!(rep.ok(), "naming a target is legal: {:?}", rep.conflicts);
    let a = d.node("re_1").unwrap().box_;
    assert_eq!(d.node("re_2").unwrap().box_.y, a.y + a.h + 24.0, "the name resolved to the node");
}

#[test]
fn an_ambiguous_name_is_not_a_reference() {
    let mut d = Document::default();
    board(&mut d, &["re_1", "re_2", "re_3"]);
    rect_named(&mut d, "re_1", "dup", json!({ "anchor": "top", "margin": 40.0 }));
    rect_named(&mut d, "re_2", "dup", json!({ "anchor": "bottom", "margin": 40.0 }));
    rect_named(&mut d, "re_3", "x", json!({ "below": "dup" }));
    let rep = layout::solve(&mut d, Some("b1")).unwrap();
    assert_eq!(rep.conflicts.len(), 1, "{:?}", rep.conflicts);
    assert_eq!(rep.conflicts[0].node, "re_3");
    assert_eq!(rep.conflicts[0].with.as_deref(), Some("dup"), "the caller is told which name was ambiguous");
}

#[test]
fn an_ordered_pair_places_the_second_below_the_first() {
    let mut d = Document::default();
    board(&mut d, &["a", "b"]);
    rect(&mut d, "a", json!({ "anchor": "top", "margin": 40.0 }));
    rect(&mut d, "b", json!({ "below": "a", "gap": 20.0 }));
    let rep = layout::solve(&mut d, Some("b1")).unwrap();
    assert!(rep.ok(), "{:?}", rep.conflicts);
    let (a, b) = (d.node("a").unwrap().box_, d.node("b").unwrap().box_);
    assert_eq!(a.y, 40.0);
    assert_eq!(b.y, a.y + a.h + 20.0, "below means below");
}

#[test]
fn a_cycle_is_quarantined_and_names_its_members() {
    let mut d = Document::default();
    board(&mut d, &["a", "b", "c"]);
    rect(&mut d, "a", json!({ "below": "b" }));
    rect(&mut d, "b", json!({ "above": "a" }));
    rect(&mut d, "c", json!({ "anchor": "bottom", "margin": 50.0 }));
    let rep = layout::solve(&mut d, Some("b1")).unwrap();
    let cyclic: Vec<&str> = rep.conflicts.iter().map(|c| c.node.as_str()).collect();
    assert!(cyclic.contains(&"a") && cyclic.contains(&"b"), "{:?}", rep.conflicts);
    assert_eq!(rep.conflicts.len(), 2, "the healthy node is not blamed: {:?}", rep.conflicts);
    assert!(rep.conflicts.iter().all(|c| c.why.contains("cycle")), "{:?}", rep.conflicts);
    assert_eq!(cyclic.iter().filter(|n| **n == "c").count(), 0);
    // Quarantine means it is still placed by what it can reach, not left at NaN.
    for id in ["a", "b", "c"] {
        let b = d.node(id).unwrap().box_;
        assert!(b.x.is_finite() && b.y.is_finite() && b.w.is_finite(), "{id} went non-finite");
    }
}

#[test]
fn a_relation_pointing_at_nothing_is_reported_not_ignored() {
    let mut d = Document::default();
    board(&mut d, &["a"]);
    rect(&mut d, "a", json!({ "below": "te_ghost", "center_in": "b_nope" }));
    let rep = layout::solve(&mut d, Some("b1")).unwrap();
    assert_eq!(rep.conflicts.len(), 2, "{:?}", rep.conflicts);
    let named: Vec<String> = rep.conflicts.iter().map(|c| format!("{}:{}", c.intent, c.with.as_deref().unwrap_or(""))).collect();
    assert!(named.contains(&"below:te_ghost".to_string()), "{named:?}");
    assert!(named.contains(&"center_in:b_nope".to_string()), "{named:?}");
    assert!(rep.conflicts.iter().all(|c| c.why.contains("no node")), "{:?}", rep.conflicts);
}

#[test]
fn a_ref_to_a_node_in_another_board_is_not_a_dangling_ref() {
    let mut d = Document::default();
    let ab: Artboard = serde_json::from_value(json!({"id":"b2","name":"other","width":500.0,"height":500.0,"layers":["z"]})).unwrap();
    board(&mut d, &["a"]);
    d.artboards.push(ab);
    rect(&mut d, "a", json!({ "below": "z" }));
    let z: Node = serde_json::from_value(json!({
        "id":"z","type":"rect","name":"z","artboard":"b2",
        "box":{"x":10.0,"y":10.0,"w":50.0,"h":50.0},"layout":{"mode":"manual"},
    }))
    .unwrap();
    d.nodes.push(z);
    let rep = layout::solve(&mut d, Some("b1")).unwrap();
    assert!(rep.ok(), "a cross-board id is legal: {:?}", rep.conflicts);
}
