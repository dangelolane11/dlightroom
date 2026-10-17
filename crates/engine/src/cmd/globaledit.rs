//! Select › Start Global Edit: find every object that matches a reference object on the
//! attributes the user picks (shape, fill, stroke, weight, opacity, blend mode, appearance,
//! graphic style, symbol, size), on the active artboard or all of them, preview the matches, and
//! change them all in one undo step.
//!
//! Hidden and locked objects (or objects in hidden or locked layers and groups) are left out
//! unless `includeHidden` / `includeLocked` ask for them. The edit itself runs the same commands
//! the panels run (`paint.setFill`, `stroke.set`, `transparency.set`, `graphicStyle.apply`) on
//! the matched ids, inside one undo group, so type, swatches and colour modes behave as they do
//! everywhere else, and a failure part-way leaves the document as it was.

use serde_json::{Value, json};
use vectorcraft_doc::{Document, Node, NodeId, NodeKind};
use vectorcraft_geom::Rect;

use super::wand::{fill_of, stroke_of};
use super::*;

const FIND: &str = "select.globalEdit.find";
const APPLY: &str = "select.globalEdit.apply";

/// The attributes a match can be required to share with the reference object.
pub const ATTRIBUTES: &[&str] =
    &["shapeType", "fill", "stroke", "strokeWeight", "opacity", "blendMode", "appearance", "graphicStyle", "symbol", "size"];

/// At most this many matches are reported and edited in one go.
const MAX_MATCHES: usize = 100_000;

/// Sizes within this many points of the reference's count as the same size.
const SIZE_TOLERANCE: f64 = 0.5;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "select.globalEdit.find",
            "Start Global Edit",
            ["Select"],
            None,
            "{reference?: id (default: the first selected object), match: [shapeType|fill|stroke|strokeWeight|opacity|blendMode|appearance|graphicStyle|symbol|size…] (at least one; size: the same width and height within 0.5 pt), scope?: all (default)|artboard, artboard?: index (scope artboard: 0-based, default 0; the app passes the active one), includeHidden?: false, includeLocked?: false, select?: true (select the matches that can be selected, as a preview)} the objects that share the chosen attributes with the reference, itself included → {count, ids, reference, hidden, locked}",
            has_doc,
            find
        ),
        cmd!(
            "select.globalEdit.apply",
            "Apply Global Edit",
            [],
            None,
            "{ids?: [id…] (the objects to change, as select.globalEdit.find returned them; default: find again with the params below), …select.globalEdit.find's params, set: {fill?: \"#rrggbb\"|\"none\"|{…paint.setFill params}, stroke?: \"#rrggbb\"|\"none\"|{…paint.setStroke params}, strokeWeight?: pt, opacity?: 0..100, blendMode?: name, graphicStyle?: name}} change every one of them, in one undo step (hidden and locked ones only with includeHidden / includeLocked) → {count, ids, changed: [the properties set]}",
            has_doc,
            apply
        ),
    ]
}

/// What the matches must share with the reference object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Attr {
    ShapeType,
    Fill,
    Stroke,
    StrokeWeight,
    Opacity,
    BlendMode,
    Appearance,
    GraphicStyle,
    Symbol,
    Size,
}

impl Attr {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "shapeType" | "shape" => Self::ShapeType,
            "fill" | "fillColor" => Self::Fill,
            "stroke" | "strokeColor" => Self::Stroke,
            "strokeWeight" => Self::StrokeWeight,
            "opacity" => Self::Opacity,
            "blendMode" | "blendingMode" => Self::BlendMode,
            "appearance" => Self::Appearance,
            "graphicStyle" | "style" => Self::GraphicStyle,
            "symbol" | "symbolInstance" => Self::Symbol,
            "size" => Self::Size,
            _ => return None,
        })
    }
}

/// The search: reference object, attributes, scope and what to include.
struct Query {
    reference: NodeId,
    attrs: Vec<Attr>,
    area: Option<Rect>,
    include_hidden: bool,
    include_locked: bool,
}

impl Query {
    fn parse(s: &Session, p: &Value, cmd: &str) -> Result<Self> {
        let st = s.doc()?;
        let reference = match p.get("reference") {
            Some(v) => NodeId(v.as_u64().ok_or_else(|| bad(cmd, "`reference` must be an object id"))?),
            None => *st.selection.objects.first().ok_or_else(|| bad(cmd, "select the object to match, or give `reference`"))?,
        };
        let node = st.doc.node(reference).ok_or_else(|| bad(cmd, format!("no object {reference}")))?;
        if node.is_container() && !matches!(node.kind, NodeKind::Compound { .. } | NodeKind::Blend { .. }) {
            return Err(bad(cmd, "the reference must be an object, not a layer or group"));
        }
        let list = p.get("match").and_then(Value::as_array).ok_or_else(|| bad(cmd, "missing `match`: the attributes to match"))?;
        let mut attrs = Vec::with_capacity(list.len());
        for v in list {
            let name = v.as_str().ok_or_else(|| bad(cmd, "`match` lists attribute names"))?;
            let a = Attr::parse(name).ok_or_else(|| bad(cmd, format!("unknown attribute `{name}` (one of {})", ATTRIBUTES.join(", "))))?;
            if !attrs.contains(&a) {
                attrs.push(a);
            }
        }
        if attrs.is_empty() {
            return Err(bad(cmd, "choose at least one attribute to match"));
        }
        if attrs.contains(&Attr::Symbol) && !matches!(node.kind, NodeKind::SymbolInstance { .. }) {
            return Err(bad(cmd, "matching by symbol needs a symbol instance as the reference"));
        }
        if attrs.contains(&Attr::GraphicStyle) && super::style::linked_style(&st.doc, node).is_none() {
            return Err(bad(cmd, "the reference has no graphic style"));
        }
        let area = match str_param(p, "scope").unwrap_or("all") {
            "all" => None,
            "artboard" => {
                let i = match p.get("artboard") {
                    None => 0,
                    Some(v) => v.as_u64().ok_or_else(|| bad(cmd, "`artboard` must be an index"))? as usize,
                };
                Some(st.doc.artboards.get(i).map(|a| a.rect).ok_or_else(|| bad(cmd, format!("no artboard {i}")))?)
            }
            other => return Err(bad(cmd, format!("unknown scope `{other}` (all or artboard)"))),
        };
        Ok(Self { reference, attrs, area, include_hidden: bool_or(p, "includeHidden", false), include_locked: bool_or(p, "includeLocked", false) })
    }
}

/// The objects matching `q` in paint order, and how many of them are hidden and locked.
struct Found {
    ids: Vec<NodeId>,
    hidden: usize,
    locked: usize,
}

fn search(doc: &Document, q: &Query) -> Result<Found> {
    let reference = doc.node(q.reference).ok_or(EngineError::NoNode(q.reference))?;
    let ref_size = size_of(doc, q.reference);
    let mut found = Found { ids: vec![], hidden: 0, locked: 0 };
    // (node, hidden by itself or an ancestor, locked likewise)
    let mut stack: Vec<(&Node, bool, bool)> = doc.layers.iter().rev().map(|l| (l.as_ref(), !l.visible, l.locked)).collect();
    while let Some((n, hidden, locked)) = stack.pop() {
        if (hidden && !q.include_hidden) || (locked && !q.include_locked) {
            continue;
        }
        let descend = matches!(n.kind, NodeKind::Layer { .. } | NodeKind::Group { .. });
        if descend {
            for c in n.children().into_iter().flatten().rev() {
                stack.push((c.as_ref(), hidden || !c.visible, locked || c.locked));
            }
            continue;
        }
        if matches!(n.kind, NodeKind::Path { clipping: true, .. } | NodeKind::Path { guide: true, .. }) {
            continue;
        }
        if let Some(area) = q.area
            && !doc.bounds_of(&[n.id], false).is_some_and(|b| b.intersect(area).area() > 0.0 || area.contains(b.center()))
        {
            continue;
        }
        if q.attrs.iter().all(|a| same(doc, *a, reference, n, ref_size)) {
            if found.ids.len() >= MAX_MATCHES {
                return Err(bad(FIND, format!("more than {MAX_MATCHES} objects match: narrow the search")));
            }
            found.ids.push(n.id);
            found.hidden += usize::from(hidden);
            found.locked += usize::from(locked);
        }
    }
    Ok(found)
}

fn size_of(doc: &Document, id: NodeId) -> Option<(f64, f64)> {
    doc.bounds_of(&[id], false).map(|b| (b.width(), b.height()))
}

fn same(doc: &Document, a: Attr, r: &Node, n: &Node, ref_size: Option<(f64, f64)>) -> bool {
    match a {
        Attr::ShapeType => r.kind_label() == n.kind_label(),
        Attr::Fill => fill_of(r) == fill_of(n),
        Attr::Stroke => stroke_of(r).0 == stroke_of(n).0,
        Attr::StrokeWeight => {
            let w = |x: &Node| {
                let (paint, width) = stroke_of(x);
                if paint.is_none() { 0.0 } else { width }
            };
            (w(r) - w(n)).abs() < 1e-6
        }
        Attr::Opacity => (r.opacity - n.opacity).abs() < 1e-6,
        Attr::BlendMode => r.blend == n.blend,
        Attr::Appearance => r.appearance == n.appearance && r.opacity == n.opacity && r.blend == n.blend,
        Attr::GraphicStyle => {
            let id = |x: &Node| super::style::linked_style(doc, x).map(|g| g.id);
            id(r).is_some() && id(r) == id(n)
        }
        Attr::Symbol => match (&r.kind, &n.kind) {
            (NodeKind::SymbolInstance { symbol: a, .. }, NodeKind::SymbolInstance { symbol: b, .. }) => a == b,
            _ => false,
        },
        Attr::Size => match (ref_size, size_of(doc, n.id)) {
            (Some((rw, rh)), Some((w, h))) => (rw - w).abs() <= SIZE_TOLERANCE && (rh - h).abs() <= SIZE_TOLERANCE,
            _ => false,
        },
    }
}

/// The solid fill colour `n` shows, as `#rrggbb` (what Global Edit starts a fill change from).
pub fn fill_hex(n: &Node) -> Option<String> {
    fill_of(n).color().map(|c| c.to_hex())
}

/// The solid stroke colour `n` shows, as `#rrggbb`.
pub fn stroke_hex(n: &Node) -> Option<String> {
    stroke_of(n).0.color().map(|c| c.to_hex())
}

fn ids_json(ids: &[NodeId]) -> Value {
    Value::Array(ids.iter().map(|i| json!(i.0)).collect())
}

fn find(s: &mut Session, p: &Value) -> Result<Value> {
    let q = Query::parse(s, p, FIND)?;
    let found = search(&s.doc()?.doc, &q)?;
    if bool_or(p, "select", true) {
        // Hidden and locked matches can't be selected; the result still lists them.
        let ids = found.ids.clone();
        s.select(|_, sel| sel.set(ids.iter().copied()))?;
    }
    Ok(json!({
        "count": found.ids.len(),
        "ids": ids_json(&found.ids),
        "reference": q.reference.0,
        "hidden": found.hidden,
        "locked": found.locked,
    }))
}

/// One property change: the command that makes it and its params, less the ids.
fn changes(set: &Value) -> Result<Vec<(&'static str, &'static str, Value)>> {
    let obj = set.as_object().ok_or_else(|| bad(APPLY, "`set` must be an object of the properties to change"))?;
    let paint = |v: &Value, what: &str| -> Result<Value> {
        match v {
            Value::String(s) if s == "none" => Ok(json!({ "none": true })),
            Value::String(s) => Ok(json!({ "color": s })),
            Value::Object(_) => Ok(v.clone()),
            _ => Err(bad(APPLY, format!("`{what}` must be a colour, \"none\" or paint params"))),
        }
    };
    let mut out = vec![];
    for (key, v) in obj {
        match key.as_str() {
            "fill" => out.push(("fill", "paint.setFill", paint(v, "fill")?)),
            "stroke" => out.push(("stroke", "paint.setStroke", paint(v, "stroke")?)),
            "strokeWeight" => {
                let w = v
                    .as_f64()
                    .filter(|w| w.is_finite() && (0.0..=1000.0).contains(w))
                    .ok_or_else(|| bad(APPLY, "`strokeWeight` must be 0–1000 pt"))?;
                out.push(("strokeWeight", "stroke.set", json!({ "weight": w })));
            }
            "opacity" => {
                let o = v.as_f64().filter(|o| o.is_finite() && (0.0..=100.0).contains(o)).ok_or_else(|| bad(APPLY, "`opacity` must be 0–100"))?;
                out.push(("opacity", "transparency.set", json!({ "opacity": o })));
            }
            "blendMode" => {
                let b = v.as_str().ok_or_else(|| bad(APPLY, "`blendMode` must be a blend mode name"))?;
                out.push(("blendMode", "transparency.set", json!({ "blend": b })));
            }
            "graphicStyle" => {
                let n = v.as_str().ok_or_else(|| bad(APPLY, "`graphicStyle` must be a style name"))?;
                out.push(("graphicStyle", "graphicStyle.apply", json!({ "name": n })));
            }
            other => return Err(bad(APPLY, format!("can't set `{other}` (fill, stroke, strokeWeight, opacity, blendMode, graphicStyle)"))),
        }
    }
    if out.is_empty() {
        return Err(bad(APPLY, "`set` names no property to change"));
    }
    Ok(out)
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let set = p.get("set").ok_or_else(|| bad(APPLY, "missing `set`: the properties to change"))?;
    let changes = changes(set)?;
    let include_hidden = bool_or(p, "includeHidden", false);
    let include_locked = bool_or(p, "includeLocked", false);
    let ids: Vec<NodeId> = match p.get("ids") {
        Some(v) => {
            let list = v.as_array().ok_or_else(|| bad(APPLY, "`ids` must be a list of object ids"))?;
            let mut ids = Vec::with_capacity(list.len());
            for i in list {
                ids.push(NodeId(i.as_u64().ok_or_else(|| bad(APPLY, "`ids` must be a list of object ids"))?));
            }
            ids
        }
        None => {
            let q = Query::parse(s, p, APPLY)?;
            search(&s.doc()?.doc, &q)?.ids
        }
    };
    if ids.is_empty() {
        return Err(bad(APPLY, "no objects match"));
    }
    if ids.len() > MAX_MATCHES {
        return Err(bad(APPLY, format!("at most {MAX_MATCHES} objects at a time")));
    }
    // Every id must name an object the edit may touch.
    let (locked, _hidden) = {
        let doc = &s.doc()?.doc;
        let mut locked = vec![];
        let mut hidden = vec![];
        for id in &ids {
            doc.node(*id).ok_or_else(|| bad(APPLY, format!("no object {id}")))?;
            let chain = doc.ancestry(*id).unwrap_or_default();
            let nodes: Vec<&Node> = chain.iter().chain(std::iter::once(id)).filter_map(|a| doc.node(*a)).collect();
            if nodes.iter().any(|n| n.locked) {
                locked.push(*id);
            }
            if nodes.iter().any(|n| !n.visible) {
                hidden.push(*id);
            }
        }
        if !include_locked && !locked.is_empty() {
            return Err(bad(APPLY, format!("{} of the objects are locked: pass includeLocked to change them", locked.len())));
        }
        if !include_hidden && !hidden.is_empty() {
            return Err(bad(APPLY, format!("{} of the objects are hidden: pass includeHidden to change them", hidden.len())));
        }
        (locked, hidden)
    };
    // Locked objects (and their locked containers) are unlocked for the edit and locked again
    // after it, all inside the one undo step.
    let relock: Vec<NodeId> = {
        let doc = &s.doc()?.doc;
        let mut v: Vec<NodeId> = vec![];
        for id in &locked {
            for a in doc.ancestry(*id).unwrap_or_default().into_iter().chain(std::iter::once(*id)) {
                if doc.node(a).is_some_and(|n| n.locked) && !v.contains(&a) {
                    v.push(a);
                }
            }
        }
        v
    };
    let selection = s.doc()?.selection.objects.clone();
    let grouped = s.active().is_some_and(|d| d.undo_group.is_none());
    if grouped {
        s.begin_named_undo_group(Some("Global Edit".into()));
    }
    let result = (|| -> Result<Vec<&'static str>> {
        if !relock.is_empty() {
            let r = relock.clone();
            s.edit("Global Edit", |d, _| {
                for id in &r {
                    if let Some(n) = d.node_mut(*id) {
                        n.locked = false;
                    }
                }
                Ok(())
            })?;
        }
        let mut done = vec![];
        let list = ids_json(&ids);
        for (what, cmd, params) in &changes {
            let mut params = params.clone();
            if let Some(o) = params.as_object_mut() {
                o.insert("ids".into(), list.clone());
            }
            s.execute(cmd, &params)?;
            done.push(*what);
        }
        if !relock.is_empty() {
            let r = relock.clone();
            s.edit("Global Edit", |d, _| {
                for id in &r {
                    if let Some(n) = d.node_mut(*id) {
                        n.locked = true;
                    }
                }
                Ok(())
            })?;
        }
        Ok(done)
    })();
    match result {
        Ok(done) => {
            if grouped {
                s.end_undo_group(false);
            }
            // The matches stay selected (those that can be), as after the preview.
            let keep: Vec<NodeId> = if selection.is_empty() { ids.clone() } else { selection };
            s.select(|_, sel| sel.set(keep.iter().copied()))?;
            Ok(json!({ "count": ids.len(), "ids": ids_json(&ids), "changed": done }))
        }
        Err(e) => {
            // Undo what was done before the failure: the document is left as it was.
            if grouped {
                s.end_undo_group(true);
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("file.new", &json!({"width": 800, "height": 600})).unwrap();
        s
    }

    fn rect(s: &mut Session, x: f64, color: &str) -> u64 {
        let id = s.execute("shape.rectangle", &json!({"x": x, "y": 100, "width": 40, "height": 40})).unwrap()["id"].as_u64().unwrap();
        s.execute("paint.setFill", &json!({"ids": [id], "color": color})).unwrap();
        id
    }

    fn fill_hex(s: &Session, id: u64) -> Option<String> {
        fill_of(s.active().unwrap().doc.node(NodeId(id)).unwrap()).color().map(|c| c.to_hex())
    }

    #[test]
    fn finds_by_fill_and_shape_and_previews_the_matches() {
        let mut s = session();
        let a = rect(&mut s, 10.0, "#ff0000");
        let b = rect(&mut s, 100.0, "#ff0000");
        let c = rect(&mut s, 200.0, "#00ff00");
        let e = s.execute("shape.ellipse", &json!({"x": 300, "y": 100, "width": 40, "height": 40})).unwrap()["id"].as_u64().unwrap();
        s.execute("paint.setFill", &json!({"ids": [e], "color": "#ff0000"})).unwrap();
        s.execute("select.set", &json!({"ids": [a]})).unwrap();
        let r = s.execute("select.globalEdit.find", &json!({"match": ["fill"]})).unwrap();
        assert_eq!(r["ids"], json!([a, b, e]));
        let r = s.execute("select.globalEdit.find", &json!({"match": ["fill", "shapeType"]})).unwrap();
        assert_eq!(r["ids"], json!([a, b]));
        assert_eq!(s.active().unwrap().selection.objects, vec![NodeId(a), NodeId(b)], "the matches are selected as the preview");
        let none = s.execute("select.globalEdit.find", &json!({"reference": c, "match": ["fill"], "select": false})).unwrap();
        assert_eq!(none["ids"], json!([c]));
    }

    #[test]
    fn applies_to_every_match_in_one_undo_step() {
        let mut s = session();
        let a = rect(&mut s, 10.0, "#ff0000");
        let b = rect(&mut s, 100.0, "#ff0000");
        let c = rect(&mut s, 200.0, "#0000ff");
        s.execute("select.set", &json!({"ids": [a]})).unwrap();
        let undo_before = s.active().unwrap().history.undo.len();
        let r =
            s.execute("select.globalEdit.apply", &json!({"match": ["fill"], "set": {"fill": "#00ff00", "strokeWeight": 3, "opacity": 50}})).unwrap();
        assert_eq!(r["count"], 2);
        assert_eq!(s.active().unwrap().history.undo.len(), undo_before + 1, "one undo step");
        for id in [a, b] {
            assert_eq!(fill_hex(&s, id).as_deref(), Some("#00ff00"));
            assert!((s.active().unwrap().doc.node(NodeId(id)).unwrap().opacity - 0.5).abs() < 1e-6);
        }
        assert_eq!(fill_hex(&s, c).as_deref(), Some("#0000ff"), "unmatched objects are untouched");
        s.execute("edit.undo", &json!({})).unwrap();
        for id in [a, b] {
            assert_eq!(fill_hex(&s, id).as_deref(), Some("#ff0000"));
        }
    }

    #[test]
    fn hidden_and_locked_objects_are_skipped_unless_asked_for() {
        let mut s = session();
        let a = rect(&mut s, 10.0, "#ff0000");
        let hidden = rect(&mut s, 100.0, "#ff0000");
        let locked = rect(&mut s, 200.0, "#ff0000");
        s.execute("select.set", &json!({"ids": [hidden]})).unwrap();
        s.execute("object.hide", &json!({})).unwrap();
        s.execute("select.set", &json!({"ids": [locked]})).unwrap();
        s.execute("object.lock", &json!({})).unwrap();
        let r = s.execute("select.globalEdit.find", &json!({"reference": a, "match": ["fill"]})).unwrap();
        assert_eq!(r["ids"], json!([a]));
        let r =
            s.execute("select.globalEdit.find", &json!({"reference": a, "match": ["fill"], "includeHidden": true, "includeLocked": true})).unwrap();
        assert_eq!(r["ids"], json!([a, hidden, locked]));
        assert_eq!((r["hidden"].as_u64(), r["locked"].as_u64()), (Some(1), Some(1)));
        // Applying to them needs the same consent, and keeps them hidden and locked.
        let err = s.execute("select.globalEdit.apply", &json!({"ids": [a, locked], "set": {"fill": "#0000ff"}})).unwrap_err();
        assert!(err.to_string().contains("locked"), "{err}");
        s.execute(
            "select.globalEdit.apply",
            &json!({"reference": a, "match": ["fill"], "includeHidden": true, "includeLocked": true, "set": {"fill": "#0000ff"}}),
        )
        .unwrap();
        for id in [a, hidden, locked] {
            assert_eq!(fill_hex(&s, id).as_deref(), Some("#0000ff"), "{id}");
        }
        let d = &s.active().unwrap().doc;
        assert!(d.node(NodeId(locked)).unwrap().locked && !d.node(NodeId(hidden)).unwrap().visible);
    }

    #[test]
    fn scope_artboard_limits_the_search() {
        let mut s = session();
        s.execute("artboard.new", &json!({"x": 1000, "y": 0, "width": 400, "height": 400})).unwrap();
        let a = rect(&mut s, 10.0, "#ff0000");
        let far = s.execute("shape.rectangle", &json!({"x": 1100, "y": 100, "width": 40, "height": 40})).unwrap()["id"].as_u64().unwrap();
        s.execute("paint.setFill", &json!({"ids": [far], "color": "#ff0000"})).unwrap();
        let all = s.execute("select.globalEdit.find", &json!({"reference": a, "match": ["fill", "size"]})).unwrap();
        assert_eq!(all["count"], 2);
        let first = s.execute("select.globalEdit.find", &json!({"reference": a, "match": ["fill"], "scope": "artboard", "artboard": 0})).unwrap();
        assert_eq!(first["ids"], json!([a]));
        let second = s.execute("select.globalEdit.find", &json!({"reference": a, "match": ["fill"], "scope": "artboard", "artboard": 1})).unwrap();
        assert_eq!(second["ids"], json!([far]));
    }

    #[test]
    fn symbol_instances_match_by_symbol_and_bad_params_are_errors() {
        let mut s = session();
        let r = rect(&mut s, 10.0, "#ff0000");
        s.execute("select.set", &json!({"ids": [r]})).unwrap();
        let inst = s.execute("symbol.new", &json!({"name": "Tile"})).unwrap()["id"].as_u64().unwrap();
        let other = s.execute("symbol.place", &json!({"name": "Tile", "x": 400, "y": 400})).unwrap()["id"].as_u64().unwrap();
        let found = s.execute("select.globalEdit.find", &json!({"reference": inst, "match": ["symbol"]})).unwrap();
        assert_eq!(found["ids"], json!([inst, other]));
        for bad_params in [
            json!({"reference": inst, "match": []}),
            json!({"reference": inst, "match": ["colour"]}),
            json!({"reference": inst, "match": ["fill"], "scope": "page"}),
            json!({"reference": 999_999, "match": ["fill"]}),
        ] {
            assert!(s.execute("select.globalEdit.find", &bad_params).is_err(), "{bad_params}");
        }
        for bad_set in [json!({}), json!({"opacity": 500}), json!({"width": 3}), json!({"fill": 7})] {
            assert!(s.execute("select.globalEdit.apply", &json!({"ids": [inst], "set": bad_set})).is_err(), "{bad_set}");
        }
    }
}
