//! The undo trail is a *project* fact, not a process fact.
//!
//! A desktop window is one long-lived process, so its stacks are simply always
//! there. A CLI or MCP client runs one process per call — `agentcanvas
//! canvas.transform …` then `agentcanvas history.undo …` — so unless the trail is
//! persisted, the second process starts with an empty stack and `history.undo`
//! "succeeds" while changing nothing. These tests drive exactly that sequence by
//! unregistering and re-registering the project (the in-process shape of "a new
//! process opens the same project directory"), and assert on the `Cmd` payloads
//! that come back, not just on a length.
//!
//! The last two sections cover what an undo must *restore* (a hand-placed box keeps
//! the node's layout ownership) and what an import must record (an svg is `vector`),
//! because both are facts the trail carries across processes.

use std::path::{Path, PathBuf};

use serde_json::json;

use agentcanvas_core::assets;
use agentcanvas_core::commands::{self, ApplyCtx, Cmd};
use agentcanvas_core::doc::{Document, LayoutMode};
use agentcanvas_core::project::Project;
use agentcanvas_core::session::{self, HistoryEntry};

/// A scratch project directory, kept alive for the whole process (the stacks under
/// test are global, so a dropped tempdir could vanish while another test reads it).
fn tmp() -> PathBuf {
    let d = tempfile::tempdir().expect("tempdir");
    let p = d.path().to_path_buf();
    std::mem::forget(d);
    p
}

fn entry(at: u64, label: &str, undo: Cmd, redo: Cmd, changed: &[&str]) -> HistoryEntry {
    HistoryEntry {
        undo,
        redo,
        at,
        origin: "cli".into(),
        changed: changed.iter().map(|s| s.to_string()).collect(),
        label: label.into(),
        rev: Some(at),
        warnings: Vec::new(),
        snap: None,
    }
}

/// The drag the CLI scenario performs, with the box it puts back.
fn drag(at: u64) -> HistoryEntry {
    entry(
        at,
        "Transform n1",
        Cmd::Transform { id: "n1".into(), x: 100.0, y: 100.0, w: 200.0, h: 120.0 },
        Cmd::Transform { id: "n1".into(), x: 500.0, y: 400.0, w: 200.0, h: 120.0 },
        &["n1"],
    )
}

fn rename(at: u64) -> HistoryEntry {
    entry(
        at,
        "Text t1",
        Cmd::Text { id: "t1".into(), content: "old".into() },
        Cmd::Text { id: "t1".into(), content: "new".into() },
        &["t1"],
    )
}

/// `project.open` in a process that has never seen this project: an empty document
/// registered against the same directory.
fn reopen(dir: &Path, id: &str) -> String {
    let mut doc = Document::default();
    doc.project_id = id.to_string();
    session::register(Project { dir: dir.to_path_buf(), path: dir.join("project.json"), doc })
}

/// The same, after the previous "process" let go of it.
fn fresh_process(dir: &Path, id: &str) -> String {
    let _ = session::unregister(id);
    assert!(!session::is_open(id), "{id} must be forgotten before the next process opens it");
    reopen(dir, id)
}

fn trail(dir: &Path) -> PathBuf {
    dir.join(session::HISTORY_FILE)
}

fn lines(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(trail(dir))
        .unwrap_or_default()
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn pushed_entries_are_written_to_the_project_directory() {
    let dir = tmp();
    let id = reopen(&dir, "proj_push");
    session::push_history(&id, drag(1)).unwrap();
    session::push_history(&id, rename(2)).unwrap();

    let rows = lines(&dir);
    assert_eq!(rows.len(), 2, "one JSON line per push: {rows:?}");
    for r in &rows {
        let v: serde_json::Value = serde_json::from_str(r).expect("each line is one JSON object");
        assert_eq!(v["k"], "push", "the kind is recorded so replay can tell the events apart");
    }
    assert!(rows[0].contains("\"Transform n1\""), "the label is stored, not invented on read: {}", rows[0]);
}

#[test]
fn a_second_process_undoes_an_edit_it_never_made() {
    let dir = tmp();
    let id = reopen(&dir, "proj_hydrate");
    session::push_history(&id, drag(1)).unwrap();
    session::push_history(&id, rename(2)).unwrap();

    // New process: nothing in memory, and the stacks must come back from the file.
    let p2 = fresh_process(&dir, "proj_hydrate");
    assert_eq!(session::depth(&p2), (2, 0), "the trail was replayed into the undo stack");

    let (undo, redo) = session::history_of(&p2, 10);
    assert_eq!(redo.len(), 0);
    assert_eq!(undo.len(), 2, "oldest first");
    assert_eq!(undo[0].label, "Transform n1");
    assert_eq!(undo[1].label, "Text t1");
    // The payloads survived the JSON round-trip, not just the labels.
    match &undo[0].redo {
        Cmd::Transform { id, x, y, w, h } => {
            assert_eq!(id, "n1");
            assert_eq!((*x, *y, *w, *h), (500.0, 400.0, 200.0, 120.0));
        }
        other => panic!("expected the forward Transform back, got {other:?}"),
    }
    match &undo[0].undo {
        Cmd::Transform { x, y, .. } => assert_eq!((*x, *y), (100.0, 100.0), "the undo puts the box home"),
        other => panic!("expected the inverse Transform, got {other:?}"),
    }
    match &undo[1].undo {
        Cmd::Text { content, .. } => assert_eq!(content, "old"),
        other => panic!("expected the inverse Text, got {other:?}"),
    }

    // `history.list` shape: newest first, commands as JSON.
    let recs = session::history_records(&p2, 10);
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0]["label"], "Text t1");
    assert_eq!(recs[0]["redo"]["t"], "Text");
    assert_eq!(recs[1]["undo"]["t"], "Transform");
    assert_eq!(recs[1]["undo"]["x"], 100.0);

    // And the entry is really applicable, i.e. it is a command and not a summary.
    let e = session::take_undo(&p2).expect("the fresh process can undo");
    assert_eq!(e.label, "Text t1");
}

#[test]
fn an_undo_taken_in_one_process_is_redoable_in_the_next() {
    let dir = tmp();
    let id = reopen(&dir, "proj_redo");
    session::push_history(&id, drag(1)).unwrap();

    // Process 2 performs the undo (this is the CLI's `history.undo`).
    let p2 = fresh_process(&dir, "proj_redo");
    assert_eq!(session::depth(&p2), (1, 0));
    let e = session::take_undo(&p2).expect("the drag is undone");
    assert_eq!(e.label, "Transform n1");
    assert_eq!(session::depth(&p2), (0, 1), "the entry moved to the redo stack");

    // Process 3 must be able to redo it: the marker, not just the entry, is kept.
    let p3 = fresh_process(&dir, "proj_redo");
    assert_eq!(session::depth(&p3), (0, 1), "replay leaves the entry on redo, not back on undo");
    let back = session::take_redo(&p3).expect("redo survives too");
    match &back.redo {
        Cmd::Transform { x, y, .. } => assert_eq!((*x, *y), (500.0, 400.0)),
        other => panic!("the redo command is the original drag, got {other:?}"),
    }

    // Process 4 sees the drag on the undo stack again.
    let p4 = fresh_process(&dir, "proj_redo");
    assert_eq!(session::depth(&p4), (1, 0));
    assert!(session::take_redo(&p4).is_none(), "no stale redo after a redo");
}

#[test]
fn a_fresh_edit_clears_a_redo_branch_another_process_left() {
    let dir = tmp();
    let id = reopen(&dir, "proj_branch");
    session::push_history(&id, drag(1)).unwrap();
    let p2 = fresh_process(&dir, "proj_branch");
    assert!(session::take_undo(&p2).is_some(), "the drag is undone, redo now holds it");

    // A new edit in a *third* process must not keep a redo branch to a future that
    // just stopped existing.
    let p3 = fresh_process(&dir, "proj_branch");
    assert_eq!(session::depth(&p3), (0, 1), "hydrated with the entry on redo");
    session::push_history(&p3, rename(5)).unwrap();
    assert_eq!(session::depth(&p3), (1, 0), "the push invalidated redo as it does in memory");
    let (undo, _) = session::history_of(&p3, 10);
    assert_eq!(undo[0].label, "Text t1");
}

#[test]
fn a_broken_line_is_skipped_and_never_fails_the_call() {
    let dir = tmp();
    let id = reopen(&dir, "proj_corrupt");
    session::push_history(&id, drag(1)).unwrap();
    session::push_history(&id, rename(2)).unwrap();

    // Truncated write, garbage, a line that is valid JSON but not a known event, and
    // an entry that is missing its commands.
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(trail(&dir)).unwrap();
    writeln!(f, "{{\"k\":\"push\",\"undo\":{{\"t\":\"NodeDel\",\"i").unwrap();
    writeln!(f, "not json at all").unwrap();
    writeln!(f, "{{\"k\":\"from-the-future\"}}").unwrap();
    writeln!(f, "{{\"k\":\"push\"}}").unwrap();
    drop(f);

    let p2 = fresh_process(&dir, "proj_corrupt");
    assert_eq!(session::depth(&p2), (2, 0), "the two good entries survive the damage");
    // The stack is still usable: a push appends without touching the bad lines.
    session::push_history(&p2, rename(9)).unwrap();
    assert_eq!(session::depth(&p2), (3, 0));
    let (undo, _) = session::history_of(&p2, 10);
    assert_eq!(undo.last().unwrap().label, "Text t1");
}

#[test]
fn close_in_one_process_does_not_erase_the_trail_for_the_next() {
    let dir = tmp();
    let id = reopen(&dir, "proj_close");
    session::push_history(&id, drag(1)).unwrap();

    // `project.close` drops the stacks in this process...
    let _ = session::unregister(&id);
    // ...and re-opening (the documented way to get a clean slate) reads them back.
    let p2 = reopen(&dir, "proj_close");
    assert_eq!(session::depth(&p2), (1, 0), "closing a project is not deleting its history");
    let e = session::take_undo(&p2).expect("the trail is intact");
    assert_eq!(e.changed, vec!["n1".to_string()]);
}

#[test]
fn a_project_that_was_never_written_still_opens_with_empty_stacks() {
    let dir = tmp();
    let id = reopen(&dir, "proj_empty");
    assert_eq!(session::depth(&id), (0, 0));
    assert!(!trail(&dir).exists(), "reading a trail does not create one");
    // Hydration is once per project per process: the second call is a flag check.
    session::hydrate_history(&id);
    assert_eq!(session::depth(&id), (0, 0));
    assert!(!trail(&dir).exists(), "a flag check does not read, let alone create, a file");
}

// ------------------------------------------------- what an undo has to restore

/// One board, one node, and the layout state the caller asked for.
fn doc_with(mode: &str, pinned: Option<bool>) -> Document {
    serde_json::from_value(json!({
        "artboards": [{ "id": "b1", "width": 1080.0, "height": 1440.0, "layers": ["n1", "t1"] }],
        "nodes": [
            { "id": "n1", "type": "rect",
              "box_": { "x": 100.0, "y": 100.0, "w": 200.0, "h": 120.0 },
              "layout": { "mode": mode, "pinned": pinned, "intent": { "anchor": "top", "margin": 110.0 } } },
            { "id": "t1", "type": "text",
              "box_": { "x": 100.0, "y": 300.0, "w": 400.0, "h": 60.0 },
              "text": { "content": "old", "size": 40.0, "lines": [] },
              "layout": { "mode": mode } },
        ],
    }))
    .expect("valid document json")
}

#[test]
fn a_drag_owns_the_node_and_the_undo_hands_it_back_to_the_solver() {
    let mut d = doc_with("constraint", None);
    let a = commands::apply(
        &mut d,
        &Cmd::Transform { id: "n1".into(), x: 500.0, y: 400.0, w: 200.0, h: 120.0 },
        &ApplyCtx::default(),
    )
    .unwrap();

    // A hand-placed box is not re-solved: `layout::skip` needs both flags.
    let n = d.node("n1").unwrap();
    assert_eq!(n.box_.x, 500.0);
    assert_eq!(n.layout.mode, LayoutMode::Manual, "a transform marks the node manual");
    assert_eq!(n.layout.pinned, Some(true));
    // ...and the solve pass that `rpc::run_cmd` runs right after leaves it alone.
    agentcanvas_core::layout::solve(&mut d, Some("b1")).unwrap();
    assert_eq!(d.node("n1").unwrap().box_.x, 500.0, "the automatic pass must not cover the drag");

    // Undo: the box *and* the layout ownership come back.
    commands::apply(&mut d, &a.undo, &ApplyCtx::default()).unwrap();
    let n = d.node("n1").unwrap();
    assert_eq!((n.box_.x, n.box_.y), (100.0, 100.0));
    assert_eq!(n.layout.mode, LayoutMode::Constraint, "undo puts it back under constraint control");
    assert_eq!(n.layout.pinned, None, "and does not leave it pinned");

    agentcanvas_core::layout::solve(&mut d, Some("b1")).unwrap();
    assert_eq!(d.node("n1").unwrap().box_.y, 110.0, "the intent position is reachable again");
}

#[test]
fn a_drag_of_an_already_manual_node_undoes_to_a_plain_transform() {
    let mut d = doc_with("manual", Some(true));
    let a = commands::apply(
        &mut d,
        &Cmd::Transform { id: "n1".into(), x: 500.0, y: 400.0, w: 200.0, h: 120.0 },
        &ApplyCtx::default(),
    )
    .unwrap();
    // Nothing about layout ownership changed, so the undo says so: one command.
    assert!(matches!(a.undo, Cmd::Transform { .. }), "undo: {:?}", a.undo);
    commands::apply(&mut d, &a.undo, &ApplyCtx::default()).unwrap();
    assert_eq!(d.node("n1").unwrap().layout.mode, LayoutMode::Manual);
}

#[test]
fn only_a_transform_claims_layout_ownership() {
    let mut d = doc_with("constraint", None);
    commands::apply(&mut d, &Cmd::NodeSet { id: "n1".into(), patch: json!({ "opacity": 0.5 }) }, &ApplyCtx::default())
        .unwrap();
    commands::apply(
        &mut d,
        &Cmd::Style { id: "n1".into(), style: json!({ "fill": { "t": "solid", "color": "#ffffff" } }) },
        &ApplyCtx::default(),
    )
    .unwrap();
    commands::apply(&mut d, &Cmd::Text { id: "t1".into(), content: "new".into() }, &ApplyCtx::default()).unwrap();

    for id in ["n1", "t1"] {
        let n = d.node(id).unwrap();
        assert_eq!(n.layout.mode, LayoutMode::Constraint, "{id} was not dragged");
        assert_ne!(n.layout.pinned, Some(true), "{id} was not pinned by a non-geometric edit");
    }
    // The intent is still there, so the node is still the solver's to move.
    agentcanvas_core::layout::solve(&mut d, Some("b1")).unwrap();
    assert_eq!(d.node("n1").unwrap().box_.y, 110.0);
}

// ------------------------------------------------------ what an import records

fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).expect("write fixture");
    p
}

#[test]
fn an_imported_svg_is_a_vector_sized_by_its_viewbox() {
    let dir = tmp();
    let mut doc = Document::default();

    let vb = write(&dir, "logo.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 200 60\"><rect width=\"200\" height=\"60\"/></svg>");
    let logo = assets::import(&mut doc, &dir, &vb, &json!({})).unwrap();
    assert_eq!(logo.kind, "vector", "the import answer carries the kind the record has");
    assert_eq!(logo.to_value()["kind"], "vector");
    assert_eq!(doc.assets[0].kind.as_deref(), Some("vector"));
    assert!(logo.alpha, "a logo has no pixels to be opaque with");
    assert!(logo.palette.is_empty(), "and no palette to sample");
    assert_eq!((logo.w, logo.h), (200.0, 60.0));

    // The viewBox is what the author drew in, so it wins over the export box.
    let both = write(&dir, "both.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"50\" viewBox=\"0 0 200 60\"/>");
    let imp = assets::import(&mut doc, &dir, &both, &json!({ "dedupe": 0 })).unwrap();
    assert_eq!((imp.w, imp.h), (200.0, 60.0), "viewBox first");

    // No viewBox: the root width/height are the fallback.
    let attrs = write(&dir, "attrs.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"300\" height=\"150\"/>");
    let imp = assets::import(&mut doc, &dir, &attrs, &json!({ "dedupe": 0 })).unwrap();
    assert_eq!((imp.w, imp.h), (300.0, 150.0));
    assert_eq!(imp.kind, "vector");

    // A raster is still a raster, kind included, with a palette of its own.
    let png = dir.join("pix.png");
    let img = agentcanvas_core::imageops::Img { w: 8, h: 8, px: vec![255u8; 8 * 8 * 4] };
    agentcanvas_core::imageops::save(&img, &png, 90).unwrap();
    let raster = assets::import(&mut doc, &dir, &png, &json!({})).unwrap();
    assert_eq!(raster.kind, "raster");
    assert_eq!(raster.w, 8.0);
}
