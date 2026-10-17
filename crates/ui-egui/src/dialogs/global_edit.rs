//! Select › Start Global Edit: match objects to the selected one on chosen attributes, preview the
//! matches (they are selected and counted), then change them all in one undo step
//! (`select.globalEdit.find`, `select.globalEdit.apply`).
//!
//! Fields: `reference` (object id) and `refLabel`; `m.<attribute>` for each attribute of
//! [`vectorcraft_engine::cmd::globaledit::ATTRIBUTES`]; `scope` (`all` | `artboard`) and
//! `artboard` (the active one when the dialog opened); `includeHidden`, `includeLocked`;
//! `found`, `hidden`, `locked` and `foundKey` (the search the counts belong to); `s.<property>`
//! (change it) with the values `fill`, `stroke` (`#rrggbb`), `strokeWeight` (pt), `opacity`
//! (0–100), `blendMode` (a mode's label) and `graphicStyle` (a style name).

use serde_json::{Value, json};
use vectorcraft_color::BlendMode;

use super::{DialogSpec, form};
use crate::state::Dialog;
use crate::{VectorcraftApp, widgets};

/// The dialog kind of Global Edit.
pub const KIND: &str = "globalEdit";

const FIND: &str = "select.globalEdit.find";
const APPLY: &str = "select.globalEdit.apply";

/// The attributes to match, with their labels, in the order the dialog lists them.
const MATCH: [(&str, &str); 10] = [
    ("shapeType", "Shape"),
    ("size", "Size"),
    ("fill", "Fill Color"),
    ("stroke", "Stroke Color"),
    ("strokeWeight", "Stroke Weight"),
    ("opacity", "Opacity"),
    ("blendMode", "Blending Mode"),
    ("appearance", "Appearance"),
    ("graphicStyle", "Graphic Style"),
    ("symbol", "Symbol Instance"),
];

/// The properties the dialog can change, with their labels.
const SET: [(&str, &str); 6] = [
    ("fill", "Fill Color"),
    ("stroke", "Stroke Color"),
    ("strokeWeight", "Stroke Weight"),
    ("opacity", "Opacity"),
    ("blendMode", "Blending Mode"),
    ("graphicStyle", "Graphic Style"),
];

/// Width of the label column.
const LABEL_W: f32 = 120.0;

pub(super) const SPEC: DialogSpec = DialogSpec {
    heading: |_| tl!("Global Edit").into(),
    body,
    confirm,
    ok: Some("Apply"),
    min_width: 440.0,
    ok_enabled: Some(|_, d| can_apply(d)),
    ..DialogSpec::FORM
};

/// Open Global Edit on the first selected object.
pub fn open(app: &mut VectorcraftApp) -> Result<Value, String> {
    let st = app.session.active().ok_or("No document open")?;
    let reference = *st.selection.objects.first().ok_or("Select the object whose matches you want to edit")?;
    let node = st.doc.node(reference).ok_or("Select the object whose matches you want to edit")?;
    let label = node.name.clone().unwrap_or_else(|| node.kind_label().to_string());
    let fill = vectorcraft_engine::cmd::globaledit::fill_hex(node).unwrap_or_else(|| "#000000".into());
    let stroke = vectorcraft_engine::cmd::globaledit::stroke_hex(node).unwrap_or_else(|| "#000000".into());
    let style = st.doc.graphic_styles.first().map(|g| g.name.clone()).unwrap_or_default();
    let artboard = app.view().map_or(0, |v| v.artboard);
    let mut fields = json!({
        "reference": reference.0,
        "refLabel": label,
        "scope": "all",
        "artboard": artboard,
        "includeHidden": false,
        "includeLocked": false,
        "fill": fill,
        "stroke": stroke,
        "strokeWeight": 1.0,
        "opacity": 100.0,
        "blendMode": "Normal",
        "graphicStyle": style,
    });
    // Shape and fill colour to begin with, as Select › Same › Fill Color and Shape would.
    for key in ["shapeType", "fill"] {
        fields[format!("m.{key}")] = json!(true);
    }
    app.ui.dialog = Some(Dialog::new(KIND, fields));
    Ok(Value::Null)
}

/// `select.globalEdit.find` parameters from the fields (the search part, shared with apply).
fn query(d: &Dialog) -> Value {
    let attrs: Vec<&str> = MATCH.iter().map(|(k, _)| *k).filter(|k| d.bool(&format!("m.{k}"))).collect();
    let mut p = json!({
        "reference": d.fields.get("reference").cloned().unwrap_or(Value::Null),
        "match": attrs,
        "scope": d.str("scope"),
        "includeHidden": d.bool("includeHidden"),
        "includeLocked": d.bool("includeLocked"),
    });
    if d.str("scope") == "artboard" {
        p["artboard"] = d.fields.get("artboard").cloned().unwrap_or(json!(0));
    }
    p
}

/// What `query` asks, as a key: counts found for another search no longer apply.
fn query_key(d: &Dialog) -> String {
    query(d).to_string()
}

/// The changes picked: `select.globalEdit.apply`'s `set`.
fn set(d: &Dialog) -> Value {
    let mut set = serde_json::Map::new();
    for (key, _) in SET {
        if !d.bool(&format!("s.{key}")) {
            continue;
        }
        let v = match key {
            "strokeWeight" => json!(d.f64(key, 1.0).clamp(0.0, 1000.0)),
            "opacity" => json!(d.f64(key, 100.0).clamp(0.0, 100.0)),
            _ => json!(d.str(key)),
        };
        set.insert(key.into(), v);
    }
    Value::Object(set)
}

fn found(d: &Dialog) -> Option<u64> {
    (d.str("foundKey") == query_key(d)).then(|| d.fields.get("found").and_then(Value::as_u64)).flatten()
}

fn can_apply(d: &Dialog) -> bool {
    found(d).is_some_and(|n| n > 0) && set(d).as_object().is_some_and(|o| !o.is_empty())
}

fn body(app: &mut VectorcraftApp, ui: &mut egui::Ui, d: &mut Dialog) -> bool {
    widgets::label_row(ui, tl!("Reference:"), LABEL_W, |ui| {
        ui.label(d.str("refLabel"));
    });
    ui.add_space(8.0);
    widgets::dim_label(ui, tl!("Match objects with the same"));
    egui::Grid::new("global-edit-match").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
        for (i, (key, label)) in MATCH.iter().enumerate() {
            form::check(ui, d, &format!("m.{key}"), tl!(label));
            if i % 2 == 1 {
                ui.end_row();
            }
        }
    });
    ui.add_space(8.0);
    widgets::label_row(ui, tl!("Search:"), LABEL_W, |ui| {
        let all = d.str("scope") != "artboard";
        if widgets::radio(ui, tl!("All Artboards"), all, true) {
            d.fields.insert("scope".into(), json!("all"));
        }
        if widgets::radio(ui, tl!("Active Artboard"), !all, true) {
            d.fields.insert("scope".into(), json!("artboard"));
        }
    });
    form::check(ui, d, "includeHidden", tl!("Include Hidden Objects"));
    form::check(ui, d, "includeLocked", tl!("Include Locked Objects"));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if widgets::flat_button(ui, tl!("Find Matches"), 120.0).clicked() {
            match app.run(FIND, query(d)) {
                Ok(r) => {
                    for key in ["found", "hidden", "locked"] {
                        let v = r.get(if key == "found" { "count" } else { key }).cloned().unwrap_or(json!(0));
                        d.fields.insert(key.into(), v);
                    }
                    d.fields.insert("foundKey".into(), json!(query_key(d)));
                    d.fields.remove("error");
                }
                Err(e) => {
                    d.fields.insert("error".into(), json!(e));
                    d.fields.remove("foundKey");
                }
            }
        }
        match found(d) {
            Some(n) => {
                let text = crate::i18n::fmt(
                    tl!("{count} matching objects ({hidden} hidden, {locked} locked)"),
                    &[
                        ("count", &n.to_string()),
                        ("hidden", &d.fields.get("hidden").and_then(Value::as_u64).unwrap_or(0).to_string()),
                        ("locked", &d.fields.get("locked").and_then(Value::as_u64).unwrap_or(0).to_string()),
                    ],
                );
                ui.label(text);
            }
            None if d.fields.contains_key("error") => {
                widgets::dim_label(ui, &d.str("error"));
            }
            None => {
                widgets::dim_label(ui, tl!("Find the matches to preview them"));
            }
        }
    });
    ui.add_space(10.0);
    widgets::dim_label(ui, tl!("Change on every match"));
    let styles: Vec<String> = app.session.active().map(|st| st.doc.graphic_styles.iter().map(|g| g.name.clone()).collect()).unwrap_or_default();
    for (key, label) in SET {
        ui.horizontal(|ui| {
            ui.set_min_width(LABEL_W + 160.0);
            let on = d.bool(&format!("s.{key}"));
            if widgets::check(ui, tl!(label), on, true) {
                d.fields.insert(format!("s.{key}"), json!(!on));
            }
            ui.add_space((LABEL_W - 110.0).max(0.0));
            ui.add_enabled_ui(on, |ui| match key {
                "fill" | "stroke" => color(ui, d, key),
                "strokeWeight" => {
                    form::length(ui, d, key, app.session.general_unit(), 72.0);
                }
                "opacity" => {
                    if let Some(v) = widgets::plain_field(ui, "global-edit-opacity", d.f64(key, 100.0), "%", 0, 72.0) {
                        d.fields.insert(key.into(), json!(v.clamp(0.0, 100.0)));
                    }
                }
                "blendMode" => {
                    let labels: Vec<&str> = BlendMode::ALL.iter().map(|m| m.label()).collect();
                    let shown = d.str(key);
                    if let Some(i) = widgets::dropdown(ui, "global-edit-blend", &shown, &labels, 150.0)
                        && let Some(l) = labels.get(i)
                    {
                        d.fields.insert(key.into(), json!(l));
                    }
                }
                _ => {
                    let names: Vec<&str> = styles.iter().map(String::as_str).collect();
                    let shown = d.str(key);
                    if let Some(i) = widgets::dropdown_names(ui, "global-edit-style", &shown, &names, 150.0)
                        && let Some(n) = names.get(i)
                    {
                        d.fields.insert(key.into(), json!(n));
                    }
                }
            });
        });
    }
    false
}

/// A colour button and its hex value for `d.fields[key]` (`#rrggbb`).
fn color(ui: &mut egui::Ui, d: &mut Dialog, key: &str) {
    let hex = d.str(key);
    let mut rgb = parse_hex(&hex).unwrap_or([0, 0, 0]);
    if egui::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
        d.fields.insert(key.into(), json!(format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])));
    }
    if form::text(ui, d, key, 80.0) {
        let typed = d.str(key);
        let fixed = parse_hex(&typed).map_or(hex, |c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]));
        d.fields.insert(key.into(), json!(fixed));
    }
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    let byte = |i: usize| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
    Some([byte(0)?, byte(2)?, byte(4)?])
}

fn confirm(app: &mut VectorcraftApp, d: &Dialog) -> Result<Value, String> {
    if !can_apply(d) {
        return Err("Find the matches and choose what to change first".into());
    }
    let mut p = query(d);
    p["set"] = set(d);
    let r = app.run(APPLY, p)?;
    app.ui.dialog = None;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectorcraft_engine::Session;

    fn app_with_rects() -> (VectorcraftApp, Vec<u64>) {
        let mut app = VectorcraftApp::new(Session::new(), Default::default());
        app.session.execute("file.new", &json!({"width": 400, "height": 300})).unwrap();
        let mut ids = vec![];
        for (x, c) in [(10, "#ff0000"), (100, "#ff0000"), (200, "#00ff00")] {
            let id = app.session.execute("shape.rectangle", &json!({"x": x, "y": 10, "width": 30, "height": 30})).unwrap()["id"].as_u64().unwrap();
            app.session.execute("paint.setFill", &json!({"ids": [id], "color": c})).unwrap();
            ids.push(id);
        }
        app.session.execute("select.set", &json!({"ids": [ids[0]]})).unwrap();
        (app, ids)
    }

    fn frame(app: &mut VectorcraftApp) {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.ui(ui));
        out.textures_delta.clear();
    }

    #[test]
    fn the_menu_opens_it_on_the_selection_and_apply_changes_every_match_in_one_step() {
        let (mut app, ids) = app_with_rects();
        crate::menus::invoke(&mut app, "select.globalEdit.find", json!({}));
        assert_eq!(app.ui.dialog.as_ref().map(|d| d.kind.as_str()), Some(KIND));
        frame(&mut app);
        let mut d = app.ui.dialog.clone().unwrap();
        assert_eq!(d.str("fill"), "#ff0000", "the change starts from the reference's fill");
        assert!(!can_apply(&d), "nothing found yet");
        // Find (as the button does), then pick the change.
        let r = app.run(FIND, query(&d)).unwrap();
        d.fields.insert("found".into(), r["count"].clone());
        d.fields.insert("foundKey".into(), json!(query_key(&d)));
        assert_eq!(r["count"], 2);
        d.fields.insert("s.fill".into(), json!(true));
        d.fields.insert("fill".into(), json!("#0000ff"));
        assert!(can_apply(&d));
        // Changing the search makes the counts stale again.
        let mut stale = d.clone();
        stale.fields.insert("m.size".into(), json!(true));
        assert!(!can_apply(&stale));
        let steps = app.session.active().unwrap().history.undo.len();
        confirm(&mut app, &d).unwrap();
        assert!(app.ui.dialog.is_none());
        assert_eq!(app.session.active().unwrap().history.undo.len(), steps + 1, "one undo step");
        let fill = |app: &VectorcraftApp, id: u64| {
            let n = app.session.active().unwrap().doc.node(vectorcraft_engine::doc::NodeId(id)).unwrap().clone();
            vectorcraft_engine::cmd::globaledit::fill_hex(&n)
        };
        assert_eq!(fill(&app, ids[0]).as_deref(), Some("#0000ff"));
        assert_eq!(fill(&app, ids[1]).as_deref(), Some("#0000ff"));
        assert_eq!(fill(&app, ids[2]).as_deref(), Some("#00ff00"));
    }

    #[test]
    fn without_a_selection_it_says_what_to_do() {
        let mut app = VectorcraftApp::new(Session::new(), Default::default());
        app.session.execute("file.new", &json!({})).unwrap();
        assert!(open(&mut app).is_err());
        assert!(app.ui.dialog.is_none());
    }

    #[test]
    fn hex_values_parse_strictly() {
        assert_eq!(parse_hex("#ff8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex("FF8000"), Some([255, 128, 0]));
        for bad in ["", "#fff", "#gg0000", "#ff80001", "#ﬀ8000"] {
            assert_eq!(parse_hex(bad), None, "{bad}");
        }
    }
}
