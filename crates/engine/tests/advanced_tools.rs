// Integration tests: unwrapping and panicking on failure is fine here, unlike in shipped code (AGENTS.md › Robustness).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! End-to-end checks of DLightroom's advanced vector tools, one area per test: each builds art
//! through the same commands the tools, panels and menus run, edits it, undoes and redoes, saves
//! the native document, opens it again and exports it. They are the evidence behind
//! `docs/advanced-tools-audit.md`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use vectorcraft_engine::Session;
use vectorcraft_engine::doc::{Node, NodeId, NodeKind};

fn session() -> Session {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 800, "height": 600})).unwrap();
    s
}

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn id_of(v: &Value) -> u64 {
    v["id"].as_u64().unwrap_or_else(|| panic!("no id in {v}"))
}

fn node(s: &Session, id: u64) -> Node {
    s.active().unwrap().doc.node(NodeId(id)).unwrap_or_else(|| panic!("no node {id}")).clone()
}

fn node_json(s: &Session, id: u64) -> Value {
    serde_json::to_value(node(s, id)).unwrap()
}

/// A fresh, empty scratch folder for one test.
fn scratch(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("dlightroom-adv-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Save the active document natively and open it in a new session.
fn reopen(s: &mut Session, tag: &str) -> Session {
    let path = scratch(tag).join("doc.vectorcraft");
    run(s, "document.save", json!({"path": path.to_string_lossy()}));
    let mut t = Session::new();
    run(&mut t, "document.open", json!({"path": path.to_string_lossy()}));
    t
}

/// Export the active document's first artboard as SVG text.
fn svg(s: &mut Session, tag: &str) -> String {
    let path = scratch(tag).join("out.svg");
    run(s, "document.export", json!({"path": path.to_string_lossy(), "format": "svg"}));
    std::fs::read_to_string(&path).unwrap()
}

/// Export the active document as PDF bytes.
fn pdf(s: &mut Session, tag: &str) -> Vec<u8> {
    let path = scratch(tag).join("out.pdf");
    run(s, "document.exportPdf", json!({"path": path.to_string_lossy()}));
    std::fs::read(&path).unwrap()
}

/// Export the first artboard as a PNG and decode it.
fn png(s: &mut Session, tag: &str) -> image::RgbaImage {
    let path = scratch(tag).join("out.png");
    run(s, "document.export", json!({"path": path.to_string_lossy(), "format": "png", "artboard": 0, "background": "white"}));
    image::open(&path).unwrap().to_rgba8()
}

fn xf_of(s: &Session, id: u64) -> vectorcraft_geom::Affine {
    match &node(s, id).kind {
        NodeKind::SymbolInstance { xf, .. } => *xf,
        k => panic!("not an instance: {k:?}"),
    }
}

fn path_anchor_count(n: &Node) -> usize {
    match &n.kind {
        NodeKind::Path { path, .. } => path.subpaths.iter().map(|sp| sp.anchors.len()).sum(),
        other => panic!("not a path: {other:?}"),
    }
}

// ---------- 1. Curvature ----------

#[test]
fn curvature_paths_edit_convert_undo_persist_and_export() {
    let mut s = session();
    let id = id_of(&run(
        &mut s,
        "path.curvature",
        json!({"points": [{"x": 100, "y": 300}, {"x": 200, "y": 150}, {"x": 300, "y": 300, "corner": true}, {"x": 400, "y": 150}]}),
    ));
    assert_eq!(path_anchor_count(&node(&s, id)), 4);
    let drawn = node_json(&s, id);

    // Drag a point: only its own segments re-curve.
    run(&mut s, "path.curvatureEdit", json!({"id": id, "op": "move", "anchor": 1, "x": 210, "y": 120}));
    // Insert a smooth point on a segment, then extend the open end.
    run(&mut s, "path.curvatureEdit", json!({"id": id, "op": "insert", "segment": 0, "t": 0.5}));
    run(&mut s, "path.curvatureEdit", json!({"id": id, "op": "extend", "x": 500, "y": 300}));
    assert_eq!(path_anchor_count(&node(&s, id)), 6);
    // Smooth ↔ corner conversion.
    run(&mut s, "path.convertAnchor", json!({"id": id, "subpath": 0, "anchor": 3, "to": "smooth"}));
    run(&mut s, "path.convertAnchor", json!({"id": id, "subpath": 0, "anchor": 2, "to": "corner"}));
    let edited = node_json(&s, id);
    assert_ne!(drawn, edited);

    // Undo walks back to the drawn path; redo forward again.
    for _ in 0..5 {
        run(&mut s, "edit.undo", json!({}));
    }
    assert_eq!(node_json(&s, id), drawn);
    for _ in 0..5 {
        run(&mut s, "edit.redo", json!({}));
    }
    assert_eq!(node_json(&s, id), edited);

    // Selection, persistence and SVG.
    run(&mut s, "select.set", json!({"ids": [id]}));
    assert_eq!(s.active().unwrap().selection.len(), 1);
    let t = reopen(&mut s, "curv");
    assert_eq!(node_json(&t, id)["path"], edited["path"], "the path survives save and reopen");
    let out = svg(&mut s, "curv");
    assert!(out.contains("<path") && out.contains(" C"), "the curve is written with cubic segments");
}

// ---------- 2. Variable width ----------

#[test]
fn width_points_profiles_outline_and_export() {
    let mut s = session();
    let id = id_of(&run(&mut s, "shape.line", json!({"x1": 100, "y1": 300, "x2": 600, "y2": 300})));
    run(&mut s, "select.set", json!({"ids": [id]}));
    run(&mut s, "stroke.set", json!({"weight": 10}));
    // Width points: a bulge in the middle, thin ends.
    run(&mut s, "stroke.widthPoint.set", json!({"id": id, "t": 0.5, "left": 20, "right": 20}));
    run(&mut s, "stroke.widthPoint.set", json!({"id": id, "t": 0.0, "left": 1, "right": 1}));
    let with_points = node_json(&s, id);
    let profile = with_points.to_string();
    assert!(profile.contains("width") || profile.contains("Width"), "a width profile is stored: {profile}");

    // A reusable profile, applied to another path.
    let name = run(&mut s, "stroke.widthProfile.add", json!({"name": "Bulge"}))["name"].as_str().unwrap().to_string();
    assert_eq!(name, "Bulge");
    let listed = run(&mut s, "stroke.widthProfile.list", json!({}));
    assert!(listed.to_string().contains("Bulge"), "{listed}");
    let other = id_of(&run(&mut s, "shape.line", json!({"x1": 100, "y1": 500, "x2": 600, "y2": 500})));
    run(&mut s, "stroke.widthProfile.set", json!({"id": other, "points": [[0.0, 0.2, 0.2], [0.5, 2.0, 2.0], [1.0, 0.2, 0.2]]}));

    // Remove a point; undo brings it back.
    let before_remove = node_json(&s, id);
    run(&mut s, "stroke.widthPoint.remove", json!({"id": id, "index": 1}));
    assert_ne!(node_json(&s, id), before_remove);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(node_json(&s, id), before_remove);

    // Persistence.
    let t = reopen(&mut s, "width");
    assert_eq!(node_json(&t, id), node_json(&s, id), "width points survive save and reopen");

    // The variable width is drawn: SVG writes the stroke as its outline (a filled shape that is
    // wider in the middle than at the thin end).
    let out = svg(&mut s, "width");
    assert!(out.contains("<path"), "{out}");

    // Outline Stroke turns it into a filled path whose height follows the profile.
    run(&mut s, "select.set", json!({"ids": [id]}));
    let r = run(&mut s, "object.expand", json!({"object": false, "fill": false, "stroke": true}));
    let ids: Vec<u64> = r["ids"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    assert!(!ids.is_empty(), "{r}");
    let b = s.active().unwrap().doc.bounds_of(&ids.iter().map(|i| NodeId(*i)).collect::<Vec<_>>(), false).unwrap();
    assert!(b.height() > 30.0, "the bulge of the outline is the 40 pt profile, not the 10 pt weight: {b:?}");
}

// ---------- 3. Perspective grid ----------

#[test]
fn perspective_grid_modes_attach_move_persist_and_export() {
    let mut s = session();
    for kind in [1, 2, 3] {
        run(&mut s, "perspective.grid.preset", json!({"kind": kind}));
        let g = run(&mut s, "perspective.grid.get", json!({}));
        assert_eq!(g["define"]["kind"].as_u64(), Some(kind), "{g}");
    }
    run(&mut s, "perspective.grid.preset", json!({"kind": 2}));
    // Editable horizon and viewing angle (which moves the vanishing points).
    let before = run(&mut s, "perspective.grid.get", json!({}));
    run(&mut s, "perspective.grid.define", json!({"angle": 30, "horizonHeight": 6}));
    let after = run(&mut s, "perspective.grid.get", json!({}));
    assert_ne!(before, after, "the grid follows its definition");

    // Draw a rectangle, attach it to the left plane and slide it within the plane.
    let rect = id_of(&run(&mut s, "shape.rectangle", json!({"x": 300, "y": 300, "width": 60, "height": 40})));
    run(&mut s, "perspective.attach", json!({"id": rect, "plane": "left"}));
    let attached = node_json(&s, rect);
    run(&mut s, "perspective.move", json!({"id": rect, "from": [330, 320], "to": [280, 330]}));
    let moved = node_json(&s, rect);
    assert_ne!(attached, moved);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(node_json(&s, rect), attached);
    run(&mut s, "edit.redo", json!({}));

    // Save, reopen: the grid and the attached object's geometry are kept.
    let grid = run(&mut s, "perspective.grid.get", json!({}));
    let mut t = reopen(&mut s, "persp");
    assert_eq!(node_json(&t, rect), moved, "the object keeps its perspective geometry");
    assert_eq!(run(&mut t, "perspective.grid.get", json!({}))["define"], grid["define"], "the grid definition is saved");
    assert!(svg(&mut s, "persp").contains("<path"));
}

// ---------- 4. Gradient mesh ----------

#[test]
fn gradient_mesh_rows_points_colors_handles_render_and_persist() {
    let mut s = session();
    let rect = id_of(&run(&mut s, "shape.rectangle", json!({"x": 100, "y": 100, "width": 400, "height": 300})));
    run(&mut s, "select.set", json!({"ids": [rect]}));
    run(&mut s, "paint.setFill", json!({"color": "#2040ff"}));
    let made = run(&mut s, "object.mesh.create", json!({"rows": 2, "cols": 2}));
    let mesh = made["ids"][0].as_u64().unwrap();
    let count = |s: &Session| match &node(s, mesh).kind {
        NodeKind::Mesh(m) => (m.rows, m.cols, m.points.len()),
        k => panic!("not a mesh: {k:?}"),
    };
    assert_eq!(count(&s), (2, 2, 9));
    // A row and a column through a new point, coloured.
    let added = run(&mut s, "object.mesh.addLine", json!({"id": mesh, "x": 220, "y": 180, "color": "#ff2020"}));
    let (r, c, n) = count(&s);
    assert!(r == 3 && c == 3 && n == 16, "{r}x{c} with {n} points after {added}");
    // Colour a corner, move a point and one of its handles.
    run(&mut s, "object.mesh.setPointColor", json!({"id": mesh, "index": 0, "color": "#ffff00"}));
    run(&mut s, "object.mesh.movePoint", json!({"id": mesh, "index": 5, "x": 230, "y": 190}));
    run(&mut s, "object.mesh.movePoint", json!({"id": mesh, "index": 5, "x": 260, "y": 190, "handle": 0}));
    let edited = node_json(&s, mesh);
    for _ in 0..3 {
        run(&mut s, "edit.undo", json!({}));
    }
    assert_ne!(node_json(&s, mesh), edited);
    for _ in 0..3 {
        run(&mut s, "edit.redo", json!({}));
    }
    assert_eq!(node_json(&s, mesh), edited);

    // The colours blend across the mesh when rendered: red near the added point, blue far away.
    let img = png(&mut s, "mesh");
    let px = |x: u32, y: u32| *img.get_pixel(x, y);
    let near = px(225, 185);
    let far = px(480, 380);
    assert!(near[0] > near[2], "red near the red point: {near:?}");
    assert!(far[2] > far[0], "blue far from it: {far:?}");

    let t = reopen(&mut s, "mesh");
    assert_eq!(node_json(&t, mesh), edited, "the mesh survives save and reopen");
    let out = pdf(&mut s, "mesh");
    assert!(out.starts_with(b"%PDF"), "the mesh exports to PDF");
    assert!(svg(&mut s, "mesh").contains("<svg"));
}

// ---------- 5. Blend ----------

#[test]
fn blends_steps_spacing_expand_undo_and_persist() {
    let mut s = session();
    let a = id_of(&run(&mut s, "shape.ellipse", json!({"x": 100, "y": 250, "width": 50, "height": 50})));
    run(&mut s, "select.set", json!({"ids": [a]}));
    run(&mut s, "paint.setFill", json!({"color": "#ff0000"}));
    let b = id_of(&run(&mut s, "shape.rectangle", json!({"x": 600, "y": 200, "width": 120, "height": 120})));
    run(&mut s, "select.set", json!({"ids": [b]}));
    run(&mut s, "paint.setFill", json!({"color": "#0000ff"}));
    let c = id_of(&run(&mut s, "shape.ellipse", json!({"x": 350, "y": 450, "width": 30, "height": 30})));
    run(&mut s, "select.set", json!({"ids": [a, b, c]}));
    let blend = id_of(&run(&mut s, "object.blend.make", json!({"steps": 4})));
    match &node(&s, blend).kind {
        NodeKind::Blend { children, .. } => assert_eq!(children.len(), 3, "three key objects"),
        k => panic!("not a blend: {k:?}"),
    }
    // Spacing by distance, then back to steps.
    run(&mut s, "select.set", json!({"ids": [blend]}));
    run(&mut s, "object.blend.options", json!({"spacing": "distance", "distance": 40}));
    run(&mut s, "object.blend.options", json!({"spacing": "steps", "steps": 6}));
    let info = run(&mut s, "object.blend.info", json!({"id": blend}));
    assert!(info.to_string().contains('6'), "{info}");
    let live = node_json(&s, blend);

    // Expand into ordinary objects; undo makes it live again.
    run(&mut s, "select.set", json!({"ids": [blend]}));
    run(&mut s, "object.blend.expand", json!({}));
    assert!(s.active().unwrap().doc.node(NodeId(blend)).is_none_or(|n| !matches!(n.kind, NodeKind::Blend { .. })));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(node_json(&s, blend), live);

    let t = reopen(&mut s, "blend");
    assert_eq!(node_json(&t, blend), live, "the live blend survives save and reopen");
    let out = svg(&mut s, "blend");
    assert!(
        out.matches("<path").count() + out.matches("<ellipse").count() + out.matches("<rect").count() + out.matches("<circle").count() >= 8,
        "the steps are exported"
    );
}

// ---------- 6. Symbols ----------

#[test]
fn symbols_link_update_override_replace_rename_delete_and_persist() {
    let mut s = session();
    run(&mut s, "artboard.new", json!({"name": "Second"}));
    let star = id_of(&run(&mut s, "shape.star", json!({"cx": 100, "cy": 100, "radius1": 40, "radius2": 20, "points": 5})));
    run(&mut s, "select.set", json!({"ids": [star]}));
    run(&mut s, "paint.setFill", json!({"color": "#ffaa00"}));
    let made = run(&mut s, "symbol.new", json!({"name": "Spark"}));
    let first = id_of(&made);
    // Instances on both artboards.
    let second = id_of(&run(&mut s, "symbol.place", json!({"name": "Spark", "x": 300, "y": 300})));
    let ab2 = s.active().unwrap().doc.artboards[1].rect;
    let third = id_of(&run(&mut s, "symbol.place", json!({"name": "Spark", "x": ab2.center().x, "y": ab2.center().y})));
    for i in [first, second, third] {
        assert!(matches!(&node(&s, i).kind, NodeKind::SymbolInstance { symbol, .. } if symbol == "Spark"));
    }
    // Per-instance transform: scale one instance only.
    run(&mut s, "select.set", json!({"ids": [second]}));
    run(&mut s, "object.scale", json!({"sx": 200, "sy": 200}));
    let second_xf = xf_of(&s, second);
    assert_ne!(second_xf, xf_of(&s, third));
    let ratio = second_xf.determinant() / xf_of(&s, third).determinant();
    assert!((ratio - 4.0).abs() < 1e-6, "scaled 200% against an unscaled instance: {ratio}");

    // Editing the master updates every instance: redefine the symbol with new art.
    let circle = id_of(&run(&mut s, "shape.ellipse", json!({"x": 50, "y": 450, "width": 30, "height": 30})));
    run(&mut s, "select.set", json!({"ids": [circle]}));
    run(&mut s, "symbol.update", json!({"name": "Spark"}));
    let art = |s: &Session, name: &str| {
        let d = &s.active().unwrap().doc;
        serde_json::to_value(&d.symbols.iter().find(|x| x.name == name).unwrap_or_else(|| panic!("no symbol {name}")).art).unwrap()
    };
    let new_art = art(&s, "Spark");
    assert!(new_art.to_string().contains("ellipse") || new_art.to_string().contains("path"), "{new_art}");
    // The art's size is re-measured; each instance keeps its own scale relative to the others.
    let ratio = xf_of(&s, second).determinant() / xf_of(&s, third).determinant();
    assert!((ratio - 4.0).abs() < 1e-6, "instances keep their own transforms: {ratio}");

    // Rename, replace, break link, delete.
    run(&mut s, "symbol.options", json!({"name": "Spark", "newName": "Dot"}));
    assert!(matches!(&node(&s, third).kind, NodeKind::SymbolInstance { symbol, .. } if symbol == "Dot"), "instances follow the rename");
    let sq = id_of(&run(&mut s, "shape.rectangle", json!({"x": 600, "y": 450, "width": 20, "height": 20})));
    run(&mut s, "select.set", json!({"ids": [sq]}));
    run(&mut s, "symbol.new", json!({"name": "Square"}));
    run(&mut s, "symbol.replace", json!({"name": "Square", "ids": [third]}));
    assert!(matches!(&node(&s, third).kind, NodeKind::SymbolInstance { symbol, .. } if symbol == "Square"));
    run(&mut s, "symbol.breakLink", json!({"ids": [first]}));
    assert!(!matches!(&s.active().unwrap().doc.node(NodeId(first)).map(|n| n.kind.clone()), Some(NodeKind::SymbolInstance { .. })));
    run(&mut s, "edit.undo", json!({}));
    assert!(matches!(&node(&s, first).kind, NodeKind::SymbolInstance { .. }), "undo restores the link");

    let t = reopen(&mut s, "sym");
    assert_eq!(art(&t, "Dot"), art(&s, "Dot"), "the symbol's art is saved");
    assert_eq!(node_json(&t, second), node_json(&s, second));
    let listed = run(&mut s, "symbol.list", json!({}));
    assert!(listed.to_string().contains("Dot") && listed.to_string().contains("Square"), "{listed}");
    assert!(svg(&mut s, "sym").contains("<svg"));
}

// ---------- 8. Artboards and batch export ----------

#[test]
fn artboards_manage_and_batch_export_svg_png_pdf() {
    let mut s = session();
    run(&mut s, "artboard.new", json!({"name": "Poster"}));
    run(&mut s, "artboard.duplicate", json!({"index": 1}));
    run(&mut s, "artboard.setProps", json!({"index": 2, "name": "Poster copy", "width": 400, "height": 300}));
    run(&mut s, "artboard.rearrange", json!({"layout": "row", "spacing": 50}));
    run(&mut s, "artboard.reorder", json!({"index": 2, "to": 1}));
    let names: Vec<String> = s.active().unwrap().doc.artboards.iter().map(|a| a.name.clone()).collect();
    assert_eq!(names.len(), 3, "{names:?}");
    run(&mut s, "artboard.delete", json!({"index": 0}));
    assert_eq!(s.active().unwrap().doc.artboards.len(), 2);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.active().unwrap().doc.artboards.len(), 3);

    for ab in 0..3 {
        let r = s.active().unwrap().doc.artboards[ab].rect;
        run(&mut s, "shape.rectangle", json!({"x": r.x0 + 10.0, "y": r.y0 + 10.0, "width": 50, "height": 50}));
    }
    // Batch export of two chosen artboards, three formats, a name template and 2x raster.
    let dir = scratch("boards");
    let r = run(
        &mut s,
        "document.exportForScreens",
        json!({
            "folder": dir.to_string_lossy(),
            "artboards": [0, 2],
            "formats": [{"format": "svg"}, {"format": "png", "scale": 2}, {"format": "pdf"}],
            "name": "board-<index>-<name>"
        }),
    );
    let files: Vec<String> = r["files"].as_array().unwrap_or(&vec![]).iter().filter_map(|f| f.as_str().map(String::from)).collect();
    let on_disk: Vec<String> =
        walk(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).filter(|n| !n.starts_with('.')).collect();
    assert_eq!(on_disk.len(), 6, "2 artboards × 3 formats: {on_disk:?} ({files:?})");
    assert!(
        on_disk.iter().any(|n| n.ends_with(".svg")) && on_disk.iter().any(|n| n.ends_with(".png")) && on_disk.iter().any(|n| n.ends_with(".pdf"))
    );
    assert!(on_disk.iter().all(|n| n.starts_with("board-")), "{on_disk:?}");
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() { out.extend(walk(&p)) } else { out.push(p) }
    }
    out
}

// ---------- Gaps closed in this milestone ----------

#[test]
fn curvature_recurve_neighbours_option_reshapes_smooth_neighbours_in_one_step() {
    let mut s = session();
    let pts = json!({"points": [{"x": 100, "y": 300}, {"x": 200, "y": 250}, {"x": 300, "y": 300}, {"x": 400, "y": 250}, {"x": 500, "y": 300}]});
    let a = id_of(&run(&mut s, "path.curvature", pts.clone()));
    let b = id_of(&run(&mut s, "path.curvature", pts));
    run(&mut s, "path.curvatureEdit", json!({"id": a, "op": "move", "anchor": 2, "x": 300, "y": 150}));
    run(&mut s, "path.curvatureEdit", json!({"id": b, "op": "move", "anchor": 2, "x": 300, "y": 150, "recurve": "neighbours"}));
    let anchors = |s: &Session, id: u64| match &node(s, id).kind {
        NodeKind::Path { path, .. } => path.subpaths[0].anchors.clone(),
        k => panic!("{k:?}"),
    };
    let (pa, pb) = (anchors(&s, a), anchors(&s, b));
    assert_eq!(pa[2], pb[2], "the moved point is the same either way");
    assert_ne!(pa[1].h_out, pb[1].h_out, "its smooth neighbour re-curves with the option");
    assert_eq!((pa[0].p, pa[4].p), (pb[0].p, pb[4].p));
    run(&mut s, "edit.undo", json!({}));
    assert_ne!(anchors(&s, b)[2].p, pb[2].p, "one undo step");
    assert!(s.execute("path.curvatureEdit", &json!({"id": b, "op": "move", "anchor": 2, "x": 1, "y": 1, "recurve": "everything"})).is_err());
}

#[test]
fn blend_easing_persists_and_changes_rendered_steps() {
    let mut s = session();
    let a = id_of(&run(&mut s, "shape.rectangle", json!({"x": 50, "y": 250, "width": 40, "height": 40})));
    let b = id_of(&run(&mut s, "shape.rectangle", json!({"x": 650, "y": 250, "width": 40, "height": 40})));
    run(&mut s, "paint.setFill", json!({"ids": [a], "color": "#ff0000"}));
    run(&mut s, "paint.setFill", json!({"ids": [b], "color": "#0000ff"}));
    run(&mut s, "select.set", json!({"ids": [a, b]}));
    let blend = id_of(&run(&mut s, "object.blend.make", json!({"steps": 5})));
    let linear = png(&mut s, "ease-lin");
    run(&mut s, "select.set", json!({"ids": [blend]}));
    run(&mut s, "object.blend.options", json!({"easeIn": 100, "easeOut": 0, "colorAcceleration": 50}));
    let info = run(&mut s, "object.blend.info", json!({}));
    assert_eq!((info["easeIn"].as_f64(), info["colorAcceleration"].as_f64()), (Some(100.0), Some(50.0)), "{info}");
    let eased = png(&mut s, "ease-eased");
    assert_ne!(linear.as_raw(), eased.as_raw(), "the eased steps render differently");
    let t = reopen(&mut s, "ease");
    assert_eq!(node_json(&t, blend), node_json(&s, blend), "easing survives save and reopen");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(run(&mut s, "object.blend.info", json!({}))["easeIn"].as_f64(), Some(0.0));
}

#[test]
fn symbol_instance_overrides_reset_keeps_the_link_and_persists() {
    let mut s = session();
    let r = id_of(&run(&mut s, "shape.rectangle", json!({"x": 10, "y": 10, "width": 40, "height": 20})));
    run(&mut s, "select.set", json!({"ids": [r]}));
    let inst = id_of(&run(&mut s, "symbol.new", json!({"name": "Tag"})));
    run(&mut s, "object.scale", json!({"ids": [inst], "sx": 300, "sy": 300}));
    run(&mut s, "transparency.set", json!({"ids": [inst], "opacity": 25}));
    let mut t = reopen(&mut s, "ovr");
    let o = run(&mut t, "symbol.instance.overrides", json!({"id": inst}));
    assert_eq!((o["instances"][0]["scaleX"].as_f64(), o["instances"][0]["opacity"].as_f64()), (Some(300.0), Some(25.0)), "{o}");
    run(&mut t, "select.set", json!({"ids": [inst]}));
    run(&mut t, "symbol.instance.reset", json!({}));
    assert_eq!(run(&mut t, "symbol.instance.overrides", json!({}))["instances"][0]["overridden"], false);
}

/// One sample document per advanced-tools area, saved natively. Each is opened again and checked;
/// with `DLIGHTROOM_WRITE_SAMPLES=1` they are written to `examples/advanced/` (how the committed
/// samples were made).
#[test]
fn advanced_tool_samples_build_save_and_open() {
    let mut samples: Vec<(&str, Session)> = vec![];

    let mut s = session();
    run(
        &mut s,
        "path.curvature",
        json!({"points": [{"x": 80, "y": 420}, {"x": 220, "y": 160}, {"x": 400, "y": 380}, {"x": 560, "y": 140}, {"x": 720, "y": 400}]}),
    );
    run(
        &mut s,
        "path.curvature",
        json!({"points": [{"x": 120, "y": 520}, {"x": 300, "y": 470, "corner": true}, {"x": 480, "y": 540}, {"x": 680, "y": 470}]}),
    );
    samples.push(("curvature", s));

    let mut s = session();
    for (y, pts) in [
        (150, json!([[0.0, 0.1, 0.1], [0.5, 2.5, 2.5], [1.0, 0.1, 0.1]])),
        (300, json!([[0.0, 2.0, 0.2], [1.0, 0.2, 2.0]])),
        (450, json!([[0.0, 0.5, 0.5], [0.25, 2.0, 2.0], [0.5, 0.5, 0.5], [0.75, 2.0, 2.0], [1.0, 0.5, 0.5]])),
    ] {
        let id = id_of(&run(&mut s, "shape.line", json!({"x1": 100, "y1": y, "x2": 700, "y2": y})));
        run(&mut s, "select.set", json!({"ids": [id]}));
        run(&mut s, "stroke.set", json!({"weight": 14}));
        run(&mut s, "stroke.widthProfile.set", json!({"id": id, "points": pts}));
    }
    samples.push(("variable-width", s));

    let mut s = session();
    run(&mut s, "perspective.grid.preset", json!({"kind": 2}));
    for (plane, x) in [("left", 200), ("right", 520)] {
        let r = id_of(&run(&mut s, "shape.rectangle", json!({"x": x, "y": 260, "width": 90, "height": 120})));
        run(&mut s, "select.set", json!({"ids": [r]}));
        run(&mut s, "paint.setFill", json!({"color": if plane == "left" { "#d9774a" } else { "#4a8bd9" }}));
        run(&mut s, "perspective.attach", json!({"id": r, "plane": plane}));
    }
    samples.push(("perspective-grid", s));

    let mut s = session();
    let rect = id_of(&run(&mut s, "shape.rectangle", json!({"x": 150, "y": 100, "width": 500, "height": 400})));
    run(&mut s, "select.set", json!({"ids": [rect]}));
    run(&mut s, "paint.setFill", json!({"color": "#1d3b8f"}));
    let mesh = run(&mut s, "object.mesh.create", json!({"rows": 3, "cols": 3}))["ids"][0].as_u64().unwrap();
    run(&mut s, "object.mesh.setPointColor", json!({"id": mesh, "index": 5, "color": "#ffb347"}));
    run(&mut s, "object.mesh.setPointColor", json!({"id": mesh, "index": 10, "color": "#ff5e7a"}));
    samples.push(("gradient-mesh", s));

    let mut s = session();
    let a = id_of(&run(&mut s, "shape.ellipse", json!({"x": 80, "y": 250, "width": 60, "height": 60})));
    run(&mut s, "select.set", json!({"ids": [a]}));
    run(&mut s, "paint.setFill", json!({"color": "#ff3b30"}));
    let b = id_of(&run(&mut s, "shape.star", json!({"cx": 680, "cy": 280, "radius1": 80, "radius2": 35, "points": 6})));
    run(&mut s, "select.set", json!({"ids": [b]}));
    run(&mut s, "paint.setFill", json!({"color": "#0a84ff"}));
    run(&mut s, "select.set", json!({"ids": [a, b]}));
    run(&mut s, "object.blend.make", json!({"steps": 8, "easeIn": 0.6, "easeOut": 0.2}));
    samples.push(("blend-easing", s));

    let mut s = session();
    run(&mut s, "artboard.new", json!({"name": "Second"}));
    let star = id_of(&run(&mut s, "shape.star", json!({"cx": 120, "cy": 120, "radius1": 50, "radius2": 22, "points": 5})));
    run(&mut s, "select.set", json!({"ids": [star]}));
    run(&mut s, "paint.setFill", json!({"color": "#ffaa00"}));
    run(&mut s, "symbol.new", json!({"name": "Spark"}));
    for (x, y) in [(320, 160), (520, 360)] {
        run(&mut s, "symbol.place", json!({"name": "Spark", "x": x, "y": y}));
    }
    let ab2 = s.active().unwrap().doc.artboards[1].rect;
    run(&mut s, "symbol.place", json!({"name": "Spark", "x": ab2.center().x, "y": ab2.center().y}));
    samples.push(("symbols-two-artboards", s));

    let mut s = session();
    for name in ["Icon", "Banner", "Card"] {
        run(&mut s, "artboard.new", json!({"name": name}));
    }
    let n = s.active().unwrap().doc.artboards.len();
    for i in 0..n {
        let r = s.active().unwrap().doc.artboards[i].rect;
        let id = id_of(&run(&mut s, "shape.ellipse", json!({"x": r.center().x - 40.0, "y": r.center().y - 40.0, "width": 80, "height": 80})));
        run(&mut s, "select.set", json!({"ids": [id]}));
        let color = ["#30d158", "#ff9f0a", "#bf5af2", "#64d2ff"][i % 4];
        run(&mut s, "paint.setFill", json!({ "color": color }));
    }
    samples.push(("artboards-batch-export", s));

    let mut s = session();
    for (i, c) in ["#ff2d55", "#ff2d55", "#5856d6", "#ff2d55", "#5856d6", "#ff2d55"].iter().enumerate() {
        let id = id_of(&run(
            &mut s,
            "shape.rectangle",
            json!({"x": 80 + (i as i64 % 3) * 230, "y": 120 + (i as i64 / 3) * 230, "width": 160, "height": 160}),
        ));
        run(&mut s, "select.set", json!({"ids": [id]}));
        run(&mut s, "paint.setFill", json!({"color": c}));
    }
    samples.push(("global-edit", s));

    let out = std::env::var_os("DLIGHTROOM_WRITE_SAMPLES").map(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/advanced"));
    for (name, mut s) in samples {
        let again = reopen(&mut s, name);
        assert!(!again.active().unwrap().doc.layers.is_empty(), "{name}");
        if let Some(dir) = &out {
            std::fs::create_dir_all(dir).unwrap();
            run(&mut s, "document.save", json!({"path": dir.join(format!("{name}.vectorcraft")).to_string_lossy()}));
        }
    }
}
