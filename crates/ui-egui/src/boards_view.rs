//! Inspiration Boards (DLightroom): a moodboard workspace beside the documents (Window ›
//! Inspiration Boards). It takes the place of the canvas while it is open: the boards on the left,
//! the board in the middle (images, notes, colours and references you drag, resize, zoom and pan;
//! files dropped on the window land on it), and the inspector on the right with the chosen item's
//! settings, Send to Document, and AI Variations of an image.
//!
//! Everything goes through the `boards.*` and `cloud.*` commands; boards are files of their own in
//! the settings folder and work without any account. AI Variations needs a provider the user set
//! up in Cloud and AI Settings, and sends the image only when Generate is clicked.

use std::collections::HashMap;

use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use serde_json::{Value, json};

use crate::theme::Tokens;
use crate::{VectorcraftApp, widgets};

/// The workspace's state (not saved: boards save themselves).
#[derive(Clone, Debug, Default)]
pub struct BoardsUi {
    pub open: bool,
    /// The board shown.
    pub board: Option<String>,
    pub selected: Vec<u64>,
    /// Board list, refreshed when it may have changed.
    list: Vec<Value>,
    list_stale: bool,
    /// A drag under way: what it moves, and the board point it started at.
    drag: Option<Drag>,
    /// The view while it is being changed (saved when the change ends).
    view: Option<(Pos2, f32)>,
    view_dirty: bool,
    /// Fields being typed.
    note: String,
    link: String,
    color: [u8; 3],
    rename: Option<String>,
    confirm_delete: bool,
    ai_count: u32,
    ai_size: u32,
    textures: Textures,
}

/// Image textures by file path (`None`: couldn't be read).
#[derive(Clone, Default)]
struct Textures(HashMap<String, Option<egui::TextureHandle>>);

impl std::fmt::Debug for Textures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Textures({})", self.0.len())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    Move { start: Pos2, delta: Vec2 },
    Resize { item: u64, start: Pos2, delta: Vec2 },
    Pan,
}

/// Board items larger than this many pixels a side are shown downscaled.
const MAX_TEXTURE_SIDE: u32 = 1024;

/// Window › Inspiration Boards: show or hide the workspace.
pub fn toggle(app: &mut VectorcraftApp) -> Result<Value, String> {
    let open = !app.ui.boards.open;
    if open {
        app.run("boards.list", json!({}))?;
    }
    app.ui.boards.open = open;
    app.ui.boards.list_stale = true;
    Ok(json!({ "open": open }))
}

fn run(app: &mut VectorcraftApp, id: &str, p: Value) -> Option<Value> {
    match app.run(id, p) {
        Ok(v) => Some(v),
        Err(e) => {
            app.status(e);
            None
        }
    }
}

/// Files dropped while the workspace is open: images go on the board (embedded), at the drop.
pub fn drop_files(app: &mut VectorcraftApp, files: Vec<(String, Option<String>, Vec<u8>)>) {
    let Some(board) = app.ui.boards.board.clone() else {
        app.status(tl!("Choose or create a board first"));
        return;
    };
    let mut added = 0;
    for (name, path, bytes) in files {
        let p = match path {
            Some(path) => json!({ "id": board, "path": path, "name": name }),
            None => json!({ "id": board, "dataBase64": vectorcraft_format::base64_encode(&bytes), "name": name }),
        };
        if run(app, "boards.addImage", p).is_some() {
            added += 1;
        }
    }
    if added > 0 {
        app.status(format!("Added {added} image(s) to the board"));
    }
}

/// The workspace, in place of the canvas.
pub fn show(app: &mut VectorcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, t.pasteboard);
    if app.session.boards.is_none() {
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(full.shrink(24.0)));
        widgets::dim_label(&mut c, tl!("Inspiration Boards keep their files in the app's settings folder, which this build doesn't have."));
        if c.button(tl!("Back to Document")).clicked() {
            app.ui.boards.open = false;
        }
        return;
    }
    if app.ui.boards.ai_count == 0 {
        app.ui.boards.ai_count = 2;
        app.ui.boards.ai_size = 1024;
        app.ui.boards.color = [0xe0, 0x7a, 0x5f];
    }
    if app.ui.boards.list_stale || app.ui.boards.list.is_empty() {
        app.ui.boards.list = run(app, "boards.list", json!({})).and_then(|v| v["boards"].as_array().cloned()).unwrap_or_default();
        app.ui.boards.list_stale = false;
    }
    // The board shown must still exist; else the first one.
    let ids: Vec<String> = app.ui.boards.list.iter().filter_map(|b| b["id"].as_str().map(String::from)).collect();
    if app.ui.boards.board.as_ref().is_none_or(|b| !ids.contains(b)) {
        app.ui.boards.board = ids.first().cloned();
        app.ui.boards.selected.clear();
        app.ui.boards.view = None;
    }
    let left = Rect::from_min_size(full.min, vec2(210.0, full.height()));
    let right = Rect::from_min_max(pos2(full.right() - 280.0, full.top()), full.max);
    let middle = Rect::from_min_max(pos2(left.right(), full.top()), pos2(right.left(), full.bottom()));
    ui.painter().rect_filled(left, 0.0, t.panel);
    ui.painter().rect_filled(right, 0.0, t.panel);
    let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(left.shrink(10.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    board_list(app, &mut lui);
    let board = app.ui.boards.board.clone().and_then(|id| run(app, "boards.get", json!({ "id": id })));
    let mut rui = ui.new_child(egui::UiBuilder::new().max_rect(right.shrink(10.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    egui::ScrollArea::vertical().id_salt("boards-inspector").show(&mut rui, |ui| inspector(app, ui, board.as_ref()));
    let bar = Rect::from_min_size(middle.min, vec2(middle.width(), 34.0));
    ui.painter().rect_filled(bar, 0.0, t.panel_darker);
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(8.0, 5.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    toolbar(app, &mut bui, board.as_ref());
    let area = Rect::from_min_max(pos2(middle.left(), bar.bottom()), middle.max);
    match &board {
        Some(b) => canvas(app, ui, area, b),
        None => {
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(area.shrink(40.0)));
            widgets::dim_label(&mut c, tl!("No boards yet. Create one to collect images, notes, colours and links."));
        }
    }
}

fn board_list(app: &mut VectorcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    if ui.button(tl!("‹ Back to Document")).clicked() {
        app.ui.boards.open = false;
    }
    ui.add_space(8.0);
    ui.label(egui::RichText::new(tl!("Inspiration Boards")).font(crate::theme::semibold(13.0)).color(t.text_strong));
    ui.add_space(6.0);
    if ui.button(tl!("New Board")).clicked()
        && let Some(b) = run(app, "boards.create", json!({ "name": tl!("Untitled Board") }))
    {
        app.ui.boards.board = b["id"].as_str().map(String::from);
        app.ui.boards.list_stale = true;
        app.ui.boards.view = None;
    }
    ui.add_space(6.0);
    let list = app.ui.boards.list.clone();
    egui::ScrollArea::vertical().id_salt("boards-list").max_height(ui.available_height() - 110.0).show(ui, |ui| {
        for b in &list {
            let id = b["id"].as_str().unwrap_or_default().to_string();
            let name = b["name"].as_str().unwrap_or_default();
            let on = app.ui.boards.board.as_deref() == Some(id.as_str());
            let label = format!("{name}  ({})", b["items"].as_u64().unwrap_or(0));
            if ui.selectable_label(on, label).clicked() && !on {
                app.ui.boards.board = Some(id);
                app.ui.boards.selected.clear();
                app.ui.boards.view = None;
                app.ui.boards.rename = None;
                app.ui.boards.confirm_delete = false;
            }
        }
    });
    let Some(id) = app.ui.boards.board.clone() else { return };
    ui.separator();
    match app.ui.boards.rename.clone() {
        Some(mut name) => {
            let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(185.0));
            app.ui.boards.rename = Some(name.clone());
            if r.lost_focus() || ui.button(tl!("Rename")).clicked() {
                if !name.trim().is_empty() {
                    run(app, "boards.rename", json!({ "id": id, "name": name.trim() }));
                }
                app.ui.boards.rename = None;
                app.ui.boards.list_stale = true;
            }
        }
        None => {
            ui.horizontal(|ui| {
                if ui.button(tl!("Rename…")).clicked() {
                    let name =
                        app.ui.boards.list.iter().find(|b| b["id"] == id.as_str()).and_then(|b| b["name"].as_str()).unwrap_or_default().to_string();
                    app.ui.boards.rename = Some(name);
                }
                if ui.button(tl!("Duplicate")).clicked()
                    && let Some(b) = run(app, "boards.duplicate", json!({ "id": id }))
                {
                    app.ui.boards.board = b["id"].as_str().map(String::from);
                    app.ui.boards.list_stale = true;
                }
            });
        }
    }
    if app.ui.boards.confirm_delete {
        widgets::dim_label(ui, tl!("Delete this board and its images? This can't be undone."));
        ui.horizontal(|ui| {
            if ui.button(tl!("Delete")).clicked() {
                run(app, "boards.delete", json!({ "id": id }));
                app.ui.boards.board = None;
                app.ui.boards.list_stale = true;
                app.ui.boards.confirm_delete = false;
            }
            if ui.button(tl!("Cancel")).clicked() {
                app.ui.boards.confirm_delete = false;
            }
        });
    } else if ui.button(tl!("Delete Board…")).clicked() {
        app.ui.boards.confirm_delete = true;
    }
}

/// Edit › Undo / Redo (from the menus, the native menu bar or the control channel) while the
/// workspace is open: the board's change, not the hidden document's. `None` when it isn't open.
pub fn edit_step(app: &mut VectorcraftApp, id: &str) -> Option<Result<Value, String>> {
    if !app.ui.boards.open || !matches!(id, "edit.undo" | "edit.redo") {
        return None;
    }
    let board = app.ui.boards.board.clone()?;
    let cmd = if id == "edit.undo" { "boards.undo" } else { "boards.redo" };
    Some(app.run(cmd, json!({ "id": board })))
}

/// The workspace's keys (while no field has the keyboard): Cmd/Ctrl+Z undoes and Shift+Cmd/Ctrl+Z
/// (or Cmd/Ctrl+Y) redoes the board change, Delete / Backspace removes the chosen items, and Escape
/// chooses none.
pub fn keys(app: &mut VectorcraftApp, ctx: &egui::Context) {
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let Some(id) = app.ui.boards.board.clone() else { return };
    let (undo, redo, delete, escape) = ctx.input_mut(|i| {
        let redo =
            i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y);
        let undo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
        let delete = i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace);
        (undo, redo, delete, i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    });
    if undo {
        run(app, "boards.undo", json!({ "id": id }));
    }
    if redo {
        run(app, "boards.redo", json!({ "id": id }));
    }
    if delete && !app.ui.boards.selected.is_empty() {
        run(app, "boards.removeItems", json!({ "id": id, "items": app.ui.boards.selected.clone() }));
        app.ui.boards.selected.clear();
    }
    if escape {
        app.ui.boards.selected.clear();
    }
}

/// Where a new item goes: the middle of what is shown.
fn view_center(b: &Value) -> (f64, f64) {
    (b["view"]["center"][0].as_f64().unwrap_or(0.0), b["view"]["center"][1].as_f64().unwrap_or(0.0))
}

fn toolbar(app: &mut VectorcraftApp, ui: &mut egui::Ui, board: Option<&Value>) {
    let Some(b) = board else { return };
    let id = b["id"].as_str().unwrap_or_default().to_string();
    let (cx, cy) = view_center(b);
    let pick = app.services.pick_open_multi.is_some();
    if widgets::icon_button_enabled(ui, "image", tl!("Add Images…"), false, pick, 26.0).clicked() {
        let board = id.clone();
        crate::picks::button(app, move |app| {
            let paths = crate::picks::open_many(app);
            let files = paths.into_iter().map(|p| (vectorcraft_engine::cmd::fileio::file_name(&p), Some(p), vec![])).collect();
            let _ = &board;
            drop_files(app, files);
        });
    }
    let has_selection = app.session.active().is_some_and(|st| !st.selection.objects.is_empty());
    if widgets::icon_button_enabled(ui, "copy", tl!("Add the Document's Selection"), false, has_selection, 26.0).clicked() {
        run(app, "boards.captureSelection", json!({ "id": id, "x": cx, "y": cy }));
    }
    ui.separator();
    let can_undo = b["canUndo"].as_bool().unwrap_or(false);
    let can_redo = b["canRedo"].as_bool().unwrap_or(false);
    if widgets::icon_button_enabled(ui, "undo-2", tl!("Undo Board Change"), false, can_undo, 26.0).clicked() {
        run(app, "boards.undo", json!({ "id": id }));
    }
    if widgets::icon_button_enabled(ui, "redo-2", tl!("Redo Board Change"), false, can_redo, 26.0).clicked() {
        run(app, "boards.redo", json!({ "id": id }));
    }
    ui.separator();
    let zoom = app.ui.boards.view.map_or(b["view"]["zoom"].as_f64().unwrap_or(1.0) as f32, |v| v.1);
    if widgets::icon_button(ui, "minus", tl!("Zoom Out"), false, 26.0).clicked() {
        set_zoom(app, b, zoom / 1.25);
    }
    ui.label(format!("{:.0}%", zoom * 100.0));
    if widgets::icon_button(ui, "plus", tl!("Zoom In"), false, 26.0).clicked() {
        set_zoom(app, b, zoom * 1.25);
    }
    if widgets::icon_button(ui, "maximize-2", tl!("Fit Board in Window"), false, 26.0).clicked() {
        app.ui.boards.view = None;
        fit(app, b);
    }
    ui.separator();
    let sel = app.ui.boards.selected.clone();
    let has_doc = app.session.active().is_some();
    let send_tip = if sel.is_empty() { tl!("Send the Board to the Document") } else { tl!("Send the Chosen Items to the Document") };
    if widgets::icon_button_enabled(ui, "share-2", send_tip, false, has_doc && b["items"].as_array().is_some_and(|i| !i.is_empty()), 26.0).clicked() {
        let p = if sel.is_empty() { json!({ "id": id }) } else { json!({ "id": id, "items": sel }) };
        if let Some(r) = run(app, "boards.toDocument", p) {
            let n = r["ids"].as_array().map_or(0, Vec::len);
            app.status(format!("Sent {n} item(s) to the document"));
        }
    }
    if widgets::icon_button_enabled(ui, "trash-2", tl!("Remove the Chosen Items"), false, !sel.is_empty(), 26.0).clicked() {
        run(app, "boards.removeItems", json!({ "id": id, "items": sel }));
        app.ui.boards.selected.clear();
    }
}

fn set_zoom(app: &mut VectorcraftApp, b: &Value, zoom: f32) {
    let (cx, cy) = view_center(b);
    let center = app.ui.boards.view.map_or(pos2(cx as f32, cy as f32), |v| v.0);
    app.ui.boards.view = Some((center, zoom.clamp(0.02, 50.0)));
    app.ui.boards.view_dirty = true;
}

/// Show every item.
fn fit(app: &mut VectorcraftApp, b: &Value) {
    let items = b["items"].as_array().cloned().unwrap_or_default();
    let Some(r) = items.iter().map(item_rect).reduce(|a, b| a.union(b)) else { return };
    let zoom = (700.0 / r.width().max(1.0)).min(500.0 / r.height().max(1.0)).clamp(0.05, 4.0);
    app.ui.boards.view = Some((r.center(), zoom));
    app.ui.boards.view_dirty = true;
}

fn item_rect(it: &Value) -> Rect {
    let f = |k: &str| it[k].as_f64().unwrap_or(0.0) as f32;
    Rect::from_min_size(pos2(f("x"), f("y")), vec2(f("w").max(1.0), f("h").max(1.0)))
}

fn hex_color(h: &str) -> Color32 {
    let h = h.trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0xcccccc);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// The texture of image file `path`, read once.
fn texture(app: &mut VectorcraftApp, ctx: &egui::Context, path: &str) -> Option<egui::TextureHandle> {
    if let Some(t) = app.ui.boards.textures.0.get(path) {
        return t.clone();
    }
    let tex = std::fs::read(path).ok().and_then(|bytes| image::load_from_memory(&bytes).ok()).map(|img| {
        let img = if img.width().max(img.height()) > MAX_TEXTURE_SIDE { img.thumbnail(MAX_TEXTURE_SIDE, MAX_TEXTURE_SIDE) } else { img };
        let rgba = img.to_rgba8();
        let ci = egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
        ctx.load_texture(format!("board-image:{path}"), ci, egui::TextureOptions::LINEAR)
    });
    app.ui.boards.textures.0.insert(path.to_string(), tex.clone());
    tex
}

fn canvas(app: &mut VectorcraftApp, ui: &mut egui::Ui, area: Rect, b: &Value) {
    let t = Tokens::get(ui.ctx());
    let id = b["id"].as_str().unwrap_or_default().to_string();
    let resp = ui.interact(area, ui.id().with("board-canvas"), Sense::click_and_drag());
    let (center, zoom) = app.ui.boards.view.unwrap_or_else(|| {
        let (cx, cy) = view_center(b);
        (pos2(cx as f32, cy as f32), b["view"]["zoom"].as_f64().unwrap_or(1.0) as f32)
    });
    let to_screen = |p: Pos2| area.center() + (p - center) * zoom;
    let to_board = |p: Pos2| center + (p - area.center()) / zoom;
    let items = b["items"].as_array().cloned().unwrap_or_default();
    // Zoom with the wheel and Ctrl/Cmd (or a pinch) about the pointer; scroll pans.
    if resp.hovered() {
        let (scroll, z, hover) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
        if z != 1.0 || scroll != Vec2::ZERO {
            let mut c = center - scroll / zoom;
            let mut nz = zoom;
            if z != 1.0 {
                nz = (zoom * z).clamp(0.02, 50.0);
                if let Some(h) = hover {
                    let under = to_board(h);
                    c = under - (h - area.center()) / nz;
                }
            }
            app.ui.boards.view = Some((c, nz));
            app.ui.boards.view_dirty = true;
        }
    }
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 0.0, if t.dark { Color32::from_gray(38) } else { Color32::from_gray(232) });
    // Items, with what a drag under way does to them.
    let moving = match app.ui.boards.drag {
        Some(Drag::Move { delta, .. }) => delta,
        _ => Vec2::ZERO,
    };
    let selected = app.ui.boards.selected.clone();
    let mut hits: Vec<(u64, Rect)> = vec![];
    for it in &items {
        let iid = it["id"].as_u64().unwrap_or(0);
        let mut r = item_rect(it);
        if selected.contains(&iid) {
            r = r.translate(moving);
        }
        if let Some(Drag::Resize { item, delta, .. }) = app.ui.boards.drag
            && item == iid
        {
            r.max += delta;
            r.max = r.max.max(r.min + vec2(16.0, 16.0));
        }
        let sr = Rect::from_min_max(to_screen(r.min), to_screen(r.max));
        hits.push((iid, sr));
        if !sr.intersects(area) {
            continue;
        }
        paint_item(app, ui.ctx(), &painter, it, sr, &t);
        if selected.contains(&iid) {
            painter.rect_stroke(sr.expand(2.0), 2.0, Stroke::new(2.0, t.accent), StrokeKind::Outside);
            painter.rect_filled(Rect::from_center_size(sr.max, vec2(9.0, 9.0)), 1.0, t.accent);
        }
    }
    // Pointer: press picks (Shift adds), drag moves the chosen items, resizes from the corner
    // handle, or pans on empty board.
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let shift = ui.input(|i| i.modifiers.shift);
    if resp.drag_started()
        && let Some(p) = pointer
    {
        let handle = hits.iter().rev().find(|(i, r)| selected.contains(i) && Rect::from_center_size(r.max, vec2(14.0, 14.0)).contains(p));
        let hit = hits.iter().rev().find(|(_, r)| r.contains(p)).map(|(i, _)| *i);
        app.ui.boards.drag = match (handle, hit) {
            (Some((i, _)), _) => Some(Drag::Resize { item: *i, start: to_board(p), delta: Vec2::ZERO }),
            (None, Some(i)) => {
                if !app.ui.boards.selected.contains(&i) {
                    if !shift {
                        app.ui.boards.selected.clear();
                    }
                    app.ui.boards.selected.push(i);
                }
                Some(Drag::Move { start: to_board(p), delta: Vec2::ZERO })
            }
            (None, None) => Some(Drag::Pan),
        };
    }
    if resp.dragged()
        && let Some(p) = pointer
    {
        match &mut app.ui.boards.drag {
            Some(Drag::Move { start, delta }) | Some(Drag::Resize { start, delta, .. }) => *delta = to_board(p) - *start,
            Some(Drag::Pan) => {
                app.ui.boards.view = Some((center - resp.drag_delta() / zoom, zoom));
                app.ui.boards.view_dirty = true;
            }
            None => {}
        }
    }
    if resp.drag_stopped() {
        match app.ui.boards.drag.take() {
            Some(Drag::Move { delta, .. }) if delta.length() > 0.5 => {
                run(app, "boards.nudge", json!({ "id": id, "items": app.ui.boards.selected.clone(), "dx": delta.x, "dy": delta.y }));
            }
            Some(Drag::Resize { item, delta, .. }) if delta.length() > 0.5 => {
                if let Some(it) = items.iter().find(|i| i["id"] == item) {
                    let r = item_rect(it);
                    let (w, h) = ((r.width() + delta.x).max(16.0), (r.height() + delta.y).max(16.0));
                    run(app, "boards.updateItem", json!({ "id": id, "item": item, "w": w, "h": h }));
                }
            }
            _ => {}
        }
    }
    if resp.clicked()
        && let Some(p) = pointer
    {
        let hit = hits.iter().rev().find(|(_, r)| r.contains(p)).map(|(i, _)| *i);
        match hit {
            Some(i) if shift => {
                if let Some(at) = app.ui.boards.selected.iter().position(|s| *s == i) {
                    app.ui.boards.selected.remove(at);
                } else {
                    app.ui.boards.selected.push(i);
                }
            }
            Some(i) => app.ui.boards.selected = vec![i],
            None => app.ui.boards.selected.clear(),
        }
    }
    // The view is saved with the board once it stops changing.
    let busy = app.ui.boards.drag.is_some() || ui.input(|i| i.smooth_scroll_delta != Vec2::ZERO || i.zoom_delta() != 1.0);
    if app.ui.boards.view_dirty && !busy {
        app.ui.boards.view_dirty = false;
        if let Some((c, z)) = app.ui.boards.view {
            run(app, "boards.setView", json!({ "id": id, "center": [c.x, c.y], "zoom": z }));
        }
    }
    if items.is_empty() {
        painter.text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            tl!("Drop images here, or add notes, colours and links on the right."),
            egui::FontId::proportional(14.0),
            t.text_dim,
        );
    }
}

fn paint_item(app: &mut VectorcraftApp, ctx: &egui::Context, painter: &egui::Painter, it: &Value, r: Rect, t: &Tokens) {
    let text_color =
        |bg: Color32| if (bg.r() as u32 * 3 + bg.g() as u32 * 6 + bg.b() as u32) / 10 > 140 { Color32::from_gray(30) } else { Color32::WHITE };
    let font = egui::FontId::proportional((13.0 * (r.width() / 220.0).clamp(0.4, 3.0)).max(6.0));
    match it["kind"].as_str() {
        Some("image") => {
            let path = it["path"].as_str().unwrap_or_default().to_string();
            match texture(app, ctx, &path) {
                Some(tex) => {
                    painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                None => {
                    painter.rect_filled(r, 2.0, Color32::from_gray(90));
                    painter.text(r.center(), egui::Align2::CENTER_CENTER, tl!("Missing image"), font.clone(), Color32::WHITE);
                }
            }
            let caption = it["caption"].as_str().unwrap_or_default();
            let ai = it["origin"].as_str().is_some_and(|o| o.starts_with("ai:"));
            if !caption.is_empty() || ai {
                let label = if ai && caption.is_empty() {
                    "AI".to_string()
                } else if ai {
                    format!("AI · {caption}")
                } else {
                    caption.to_string()
                };
                let g = painter.layout_no_wrap(label, egui::FontId::proportional(11.0), Color32::WHITE);
                let tag = Rect::from_min_size(r.left_bottom() + vec2(4.0, -g.size().y - 8.0), g.size() + vec2(8.0, 4.0));
                painter.rect_filled(tag, 3.0, Color32::from_black_alpha(160));
                painter.galley(tag.min + vec2(4.0, 2.0), g, Color32::WHITE);
            }
        }
        Some("note") => {
            let bg = hex_color(it["color"].as_str().unwrap_or("#fff3a6"));
            painter.rect_filled(r, 3.0, bg);
            let g = painter.layout(it["text"].as_str().unwrap_or_default().to_string(), font, text_color(bg), (r.width() - 16.0).max(4.0));
            painter.with_clip_rect(r.shrink(2.0)).galley(r.min + vec2(8.0, 8.0), g, text_color(bg));
        }
        Some("color") => {
            let c = hex_color(it["hex"].as_str().unwrap_or("#cccccc"));
            let chip = Rect::from_min_max(r.min, pos2(r.max.x, r.max.y - (r.height() * 0.28).max(10.0)));
            painter.rect_filled(r, 3.0, Color32::WHITE);
            painter.rect_filled(chip, 3.0, c);
            let name = it["name"]
                .as_str()
                .filter(|n| !n.is_empty())
                .map_or_else(|| it["hex"].as_str().unwrap_or_default().to_string(), |n| format!("{n} {}", it["hex"].as_str().unwrap_or_default()));
            painter.with_clip_rect(r).text(pos2(r.left() + 6.0, chip.bottom() + 3.0), egui::Align2::LEFT_TOP, name, font, Color32::from_gray(40));
        }
        Some("reference") => {
            painter.rect_filled(r, 3.0, if t.dark { Color32::from_gray(60) } else { Color32::WHITE });
            painter.rect_stroke(r, 3.0, Stroke::new(1.0, t.border), StrokeKind::Inside);
            let title = it["title"].as_str().filter(|s| !s.is_empty()).or(it["url"].as_str()).unwrap_or_default().to_string();
            let body = format!("🔗 {title}\n{}\n{}", it["url"].as_str().unwrap_or_default(), it["note"].as_str().unwrap_or_default());
            let g = painter.layout(body, font, t.text, (r.width() - 16.0).max(4.0));
            painter.with_clip_rect(r.shrink(2.0)).galley(r.min + vec2(8.0, 8.0), g, t.text);
        }
        _ => {}
    }
}

fn inspector(app: &mut VectorcraftApp, ui: &mut egui::Ui, board: Option<&Value>) {
    let t = Tokens::get(ui.ctx());
    let Some(b) = board else { return };
    let id = b["id"].as_str().unwrap_or_default().to_string();
    let (cx, cy) = view_center(b);
    let heading = |ui: &mut egui::Ui, s: &str| {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(s).font(crate::theme::semibold(12.5)).color(t.text_strong));
    };
    heading(ui, tl!("Add to Board"));
    ui.add(egui::TextEdit::multiline(&mut app.ui.boards.note).hint_text(tl!("Note text")).desired_rows(2).desired_width(255.0));
    if ui.add_enabled(!app.ui.boards.note.trim().is_empty(), egui::Button::new(tl!("Add Note"))).clicked() {
        let text = std::mem::take(&mut app.ui.boards.note);
        run(app, "boards.addNote", json!({ "id": id, "text": text, "x": cx - 110.0, "y": cy - 60.0 }));
    }
    ui.horizontal(|ui| {
        ui.color_edit_button_srgb(&mut app.ui.boards.color);
        let [r, g, bl] = app.ui.boards.color;
        let hex = format!("#{r:02x}{g:02x}{bl:02x}");
        ui.label(&hex);
        if ui.button(tl!("Add Color")).clicked() {
            run(app, "boards.addColor", json!({ "id": id, "color": hex, "x": cx - 60.0, "y": cy - 60.0 }));
        }
    });
    ui.add(egui::TextEdit::singleline(&mut app.ui.boards.link).hint_text("https://…").desired_width(255.0));
    if ui.add_enabled(!app.ui.boards.link.trim().is_empty(), egui::Button::new(tl!("Add Link"))).clicked() {
        let url = std::mem::take(&mut app.ui.boards.link);
        run(app, "boards.addReference", json!({ "id": id, "url": url.trim(), "x": cx - 120.0, "y": cy - 50.0 }));
    }
    let items = b["items"].as_array().cloned().unwrap_or_default();
    let sel = app.ui.boards.selected.clone();
    let chosen: Vec<&Value> = items.iter().filter(|i| i["id"].as_u64().is_some_and(|x| sel.contains(&x))).collect();
    ui.add_space(4.0);
    ui.separator();
    match chosen.as_slice() {
        [] => {
            widgets::dim_label(ui, tl!("Click an item to change it. Shift-click picks several; drag the corner handle to resize."));
        }
        [it] => item_settings(app, ui, &id, it),
        many => {
            heading(ui, &format!("{} items", many.len()));
            ui.horizontal(|ui| {
                if ui.button(tl!("Bring to Front")).clicked() {
                    run(app, "boards.arrange", json!({ "id": id, "items": sel, "front": true }));
                }
                if ui.button(tl!("Send to Back")).clicked() {
                    run(app, "boards.arrange", json!({ "id": id, "items": sel, "front": false }));
                }
            });
        }
    }
    if !sel.is_empty()
        && ui.button(tl!("Duplicate")).clicked()
        && let Some(r) = run(app, "boards.duplicateItems", json!({ "id": id, "items": sel }))
    {
        app.ui.boards.selected = r["items"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    }
    jobs(app, ui);
}

fn item_settings(app: &mut VectorcraftApp, ui: &mut egui::Ui, board: &str, it: &Value) {
    let t = Tokens::get(ui.ctx());
    let item = it["id"].as_u64().unwrap_or(0);
    let field = |ui: &mut egui::Ui, key: &str, value: &str, multiline: bool| -> Option<String> {
        let mut v = value.to_string();
        let r = if multiline {
            ui.add(egui::TextEdit::multiline(&mut v).desired_rows(3).desired_width(255.0))
        } else {
            ui.add(egui::TextEdit::singleline(&mut v).desired_width(255.0))
        };
        let _ = key;
        (r.changed() && v != value).then_some(v)
    };
    let update = |app: &mut VectorcraftApp, key: &str, v: Value| {
        run(app, "boards.updateItem", json!({ "id": board, "item": item, key: v }));
    };
    match it["kind"].as_str() {
        Some("note") => {
            ui.label(egui::RichText::new(tl!("Note")).font(crate::theme::semibold(12.5)).color(t.text_strong));
            if let Some(v) = field(ui, "text", it["text"].as_str().unwrap_or_default(), true) {
                update(app, "text", json!(v));
            }
            let mut c = hex_color(it["color"].as_str().unwrap_or("#fff3a6"));
            ui.horizontal(|ui| {
                widgets::dim_label(ui, tl!("Color:"));
                if ui.color_edit_button_srgba(&mut c).changed() {
                    update(app, "color", json!(format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())));
                }
            });
        }
        Some("color") => {
            ui.label(egui::RichText::new(tl!("Color")).font(crate::theme::semibold(12.5)).color(t.text_strong));
            let mut c = hex_color(it["hex"].as_str().unwrap_or("#cccccc"));
            if ui.color_edit_button_srgba(&mut c).changed() {
                update(app, "color", json!(format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())));
            }
            widgets::dim_label(ui, tl!("Name:"));
            if let Some(v) = field(ui, "name", it["name"].as_str().unwrap_or_default(), false) {
                update(app, "name", json!(v));
            }
        }
        Some("reference") => {
            ui.label(egui::RichText::new(tl!("Link")).font(crate::theme::semibold(12.5)).color(t.text_strong));
            for (key, label) in [("url", tl!("Address:")), ("title", tl!("Title:")), ("note", tl!("Note:"))] {
                widgets::dim_label(ui, label);
                if let Some(v) = field(ui, key, it[key].as_str().unwrap_or_default(), key == "note") {
                    update(app, key, json!(v));
                }
            }
            if let Some(url) = it["url"].as_str().filter(|u| u.starts_with("https://") || u.starts_with("http://"))
                && ui.button(tl!("Open in Browser")).clicked()
            {
                match app.services.open_url.as_mut() {
                    Some(open) => open(url),
                    None => ui.ctx().open_url(egui::OpenUrl::new_tab(url)),
                }
            }
        }
        Some("image") => {
            ui.label(egui::RichText::new(tl!("Image")).font(crate::theme::semibold(12.5)).color(t.text_strong));
            widgets::dim_label(ui, &format!("{} × {} px", it["pixelWidth"].as_u64().unwrap_or(0), it["pixelHeight"].as_u64().unwrap_or(0)));
            let origin = it["origin"].as_str().unwrap_or_default();
            if !origin.is_empty() {
                widgets::dim_label(ui, &format!("{} {origin}", tl!("From:")));
            }
            widgets::dim_label(ui, tl!("Caption:"));
            if let Some(v) = field(ui, "caption", it["caption"].as_str().unwrap_or_default(), false) {
                update(app, "caption", json!(v));
            }
            ui.add_space(6.0);
            ai_variations(app, ui, board, item);
        }
        _ => {}
    }
}

/// AI Variations of image `item`: what it needs, where the image goes, and Generate.
fn ai_variations(app: &mut VectorcraftApp, ui: &mut egui::Ui, board: &str, item: u64) {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(tl!("AI Variations")).font(crate::theme::semibold(12.5)).color(t.text_strong));
    let status = app.run("cloud.status", json!({})).unwrap_or_default();
    if status["network"] == false {
        widgets::dim_label(ui, tl!("AI Variations need the desktop app."));
        return;
    }
    let ready = status["ai"]["ready"].as_bool().unwrap_or(false);
    let host = status["ai"]["host"].as_str().unwrap_or_default().to_string();
    if !ready {
        widgets::dim_label(
            ui,
            tl!("Not set up: AI Variations use an image service you choose, with your own API key. Boards work fully without one."),
        );
        if ui.button(tl!("Cloud and AI Settings…")).clicked() {
            crate::dialogs::cloud_settings::open(app);
        }
        return;
    }
    ui.horizontal(|ui| {
        widgets::dim_label(ui, tl!("Count:"));
        for n in 1..=4u32 {
            if ui.selectable_label(app.ui.boards.ai_count == n, n.to_string()).clicked() {
                app.ui.boards.ai_count = n;
            }
        }
    });
    ui.horizontal(|ui| {
        widgets::dim_label(ui, tl!("Size:"));
        for s in [256u32, 512, 1024] {
            if ui.selectable_label(app.ui.boards.ai_size == s, s.to_string()).clicked() {
                app.ui.boards.ai_size = s;
            }
        }
    });
    widgets::dim_label(ui, &format!("{} {host} ({}).", tl!("Generate sends this image to"), status["ai"]["model"].as_str().unwrap_or_default()));
    if ui.button(tl!("Generate Variations")).clicked()
        && let Some(r) =
            run(app, "boards.variations", json!({ "id": board, "item": item, "count": app.ui.boards.ai_count, "size": app.ui.boards.ai_size }))
    {
        app.status(format!("Asking {} for variations…", r["host"].as_str().unwrap_or_default()));
    }
}

/// The background jobs in a corner window, while neither the workspace nor Cloud and AI Settings
/// (which list them too) is open.
pub fn jobs_window(app: &mut VectorcraftApp, ctx: &egui::Context) {
    let settings_open = app.ui.dialog.as_ref().is_some_and(|d| d.kind == crate::dialogs::cloud_settings::KIND);
    if !app.session.cloud.has_jobs() || app.ui.boards.open || settings_open {
        return;
    }
    egui::Window::new(tl!("Background Tasks"))
        .id(egui::Id::new("cloud-jobs"))
        .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -40.0))
        .resizable(false)
        .collapsible(true)
        .default_width(280.0)
        .show(ctx, |ui| jobs(app, ui));
}

/// The background jobs (AI variations, Drive), with progress, Cancel, Retry and Dismiss.
pub fn jobs(app: &mut VectorcraftApp, ui: &mut egui::Ui) {
    let r = app.run("cloud.jobs", json!({})).unwrap_or_default();
    let list = r["jobs"].as_array().cloned().unwrap_or_default();
    if r["busy"] == true {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
    if list.is_empty() {
        return;
    }
    // Images that just arrived are read again.
    if list.iter().any(|j| j["state"] == "done" && j["kind"] == "aiVariations") {
        app.ui.boards.list_stale = true;
    }
    ui.add_space(8.0);
    ui.separator();
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(tl!("Background Tasks")).font(crate::theme::semibold(12.5)).color(t.text_strong));
    for j in list.iter().rev() {
        let job = j["id"].as_u64().unwrap_or(0);
        ui.label(j["label"].as_str().unwrap_or_default());
        match j["state"].as_str() {
            Some("running") => {
                ui.add(
                    egui::ProgressBar::new(j["progress"].as_f64().unwrap_or(0.0) as f32)
                        .text(j["message"].as_str().unwrap_or_default())
                        .desired_width(255.0),
                );
                if ui.small_button(tl!("Cancel")).clicked() {
                    run(app, "cloud.cancel", json!({ "job": job }));
                }
            }
            Some(state) => {
                let color = if state == "done" { t.text_dim } else { Color32::from_rgb(0xd9, 0x48, 0x3b) };
                ui.label(egui::RichText::new(j["message"].as_str().unwrap_or_default()).color(color));
                ui.horizontal(|ui| {
                    if state != "done" && ui.small_button(tl!("Retry")).clicked() {
                        run(app, "cloud.retry", json!({ "job": job }));
                    }
                    if ui.small_button(tl!("Dismiss")).clicked() {
                        run(app, "cloud.dismiss", json!({ "job": job }));
                    }
                });
            }
            None => {}
        }
        ui.add_space(4.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectorcraft_engine::Session;

    fn app() -> VectorcraftApp {
        let dir = std::env::temp_dir().join(format!("dlightroom-ui-boards-{}-{}", std::process::id(), rand_suffix()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = VectorcraftApp::new(Session::new(), crate::Services::default());
        app.session.execute("boards.setFolder", &json!({"path": dir.to_string_lossy()})).unwrap();
        app
    }

    fn rand_suffix() -> usize {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }

    fn frame(app: &mut VectorcraftApp, ctx: &egui::Context) {
        let mut out =
            ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), ..Default::default() }, |ui| {
                egui::CentralPanel::default().show(ui, |ui| show(app, ui));
            });
        out.textures_delta.clear();
    }

    #[test]
    fn the_workspace_toggles_and_shows_a_board_with_every_kind_of_item() {
        let mut app = app();
        assert_eq!(app.run("window.inspirationBoards", json!({})).unwrap()["open"], true);
        let id = app.run("boards.create", json!({"name": "Mood"})).unwrap()["id"].as_str().unwrap().to_string();
        app.run("boards.addNote", json!({"id": id, "text": "Warm light"})).unwrap();
        app.run("boards.addColor", json!({"id": id, "color": "#e07a5f", "name": "Terracotta"})).unwrap();
        app.run("boards.addReference", json!({"id": id, "url": "https://example.com/inspo", "title": "Example"})).unwrap();
        let img = image::RgbaImage::from_pixel(30, 20, image::Rgba([10, 120, 200, 255]));
        let mut png = std::io::Cursor::new(vec![]);
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        // A drop needs a board shown: the first frame shows the only one.
        drop_files(&mut app, vec![("early.png".into(), None, png.get_ref().clone())]);
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        app.ui.boards.list_stale = true;
        frame(&mut app, &ctx);
        drop_files(&mut app, vec![("blue.png".into(), None, png.into_inner())]);
        frame(&mut app, &ctx);
        assert_eq!(app.ui.boards.board.as_deref(), Some(id.as_str()));
        let b = app.run("boards.get", json!({"id": id})).unwrap();
        assert_eq!(b["items"].as_array().unwrap().len(), 4);
        assert_eq!(app.ui.boards.textures.0.len(), 1, "the image was drawn");
        assert!(app.ui.boards.textures.0.values().all(Option::is_some));
        // Without a provider, the image's AI Variations say what to set up; Generate isn't offered.
        app.ui.boards.selected = vec![b["items"][3]["id"].as_u64().unwrap()];
        frame(&mut app, &ctx);
        assert!(app.run("boards.variations", json!({"id": id, "item": b["items"][3]["id"]})).is_err());
        assert_eq!(app.run("window.inspirationBoards", json!({})).unwrap()["open"], false);
    }
}
