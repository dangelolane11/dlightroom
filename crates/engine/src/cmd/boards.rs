//! Inspiration Boards (DLightroom): moodboards beside the documents, kept as files of their own in
//! the folder the app gives (`boards.setFolder`, under its settings folder). These commands are
//! how the Inspiration Boards window, the control channel and MCP read and change them; the model,
//! storage and per-board undo history are `vectorcraft_boards`'.
//!
//! Board edits are undone with `boards.undo` / `boards.redo` (each board has its own history), not
//! with Edit › Undo, which belongs to the active document. `boards.toDocument` is the bridge: it
//! places the chosen items in the active document (images as embedded images, notes and
//! references as type, colours as swatches) in one document undo step.

use std::path::PathBuf;

use serde_json::{Value, json};
use vectorcraft_boards::{Board, Boards, Content, Edit, Item, Store, View};

use super::*;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "boards.setFolder", "Inspiration Boards Folder", [], None, "{path} the folder boards are kept in (the app sets it, in its settings folder) → {path}", always, set_folder),
        cmd!(query "boards.list", "Inspiration Boards", [], None, "{} → {boards: [{id, name, items, modified (Unix seconds)}], folder}", always, list),
        cmd!("boards.create", "New Board", [], None, "{name?} a new, empty board → the board", always, create),
        cmd!(query "boards.get", "Board", [], None, "{id} → the board: {id, name, items: [{id, kind, x, y, w, h, …}], view: {center, zoom}, canUndo, canRedo, folder} (image items also give `path`, the file shown)", always, get),
        cmd!("boards.rename", "Rename Board", [], None, "{id, name} (an undo step of the board)", always, rename),
        cmd!("boards.delete", "Delete Board", [], None, "{id} delete the board and its embedded images (can't be undone)", always, delete),
        cmd!("boards.duplicate", "Duplicate Board", [], None, "{id, name?} a copy of the board with its images → the new board", always, duplicate),
        cmd!(
            "boards.addImage",
            "Add Image to Board",
            [],
            None,
            "{id, path? | dataBase64?, name?, link?: false (path only: keep a link to the file instead of a copy), x?, y?: where on the board (default: the view's centre)} put an image (PNG, JPEG, GIF, WebP, TIFF or BMP, up to 64 MB) on the board → {item}",
            always,
            add_image
        ),
        cmd!("boards.addNote", "Add Note to Board", [], None, "{id, text, color?: #rrggbb (default #fff3a6), x?, y?} → {item}", always, add_note),
        cmd!("boards.addColor", "Add Color to Board", [], None, "{id, color: #rrggbb, name?, x?, y?} → {item}", always, add_color),
        cmd!(
            "boards.addReference",
            "Add Link to Board",
            [],
            None,
            "{id, url, title?, note?, x?, y?} a web page or other reference → {item}",
            always,
            add_reference
        ),
        cmd!(
            "boards.updateItem",
            "Edit Board Item",
            [],
            None,
            "{id, item, x?, y?, w?, h?, text?, color?, name?, url?, title?, note?, caption?} move, resize or change an item (what fits its kind: a note's text and color, a colour chip's color and name, a reference's url, title and note, an image's caption) → {item}",
            always,
            update_item
        ),
        cmd!("boards.nudge", "Move Board Items", [], None, "{id, items: [item…], dx, dy}", always, nudge),
        cmd!("boards.removeItems", "Remove Board Items", [], None, "{id, items: [item…]}", always, remove_items),
        cmd!("boards.arrange", "Arrange Board Items", [], None, "{id, items: [item…], front?: true (false: to the back)}", always, arrange),
        cmd!(
            "boards.duplicateItems",
            "Duplicate Board Items",
            [],
            None,
            "{id, items: [item…], dx?: 24, dy?: 24} → {items: the copies}",
            always,
            duplicate_items
        ),
        cmd!(query "boards.setView", "Board View", [], None, "{id, center: [x, y], zoom} where the board is looked at (not an undo step)", always, set_view),
        cmd!("boards.undo", "Undo Board Change", [], None, "{id} → {undone}", always, |s, p| step(s, p, true)),
        cmd!("boards.redo", "Redo Board Change", [], None, "{id} → {redone}", always, |s, p| step(s, p, false)),
        cmd!(
            "boards.toDocument",
            "Send to Document",
            [],
            None,
            "{id, items?: [item…] (default: all), artboard?: index (default 0)} put the items in the active document, laid out as on the board and centred on the artboard: images embedded, notes and references as point type, colours as swatches (and a row of filled squares) → {ids, swatches}. One undo step of the document",
            has_doc,
            to_document
        ),
        cmd!(
            "boards.captureSelection",
            "Add Selection to Board",
            [],
            None,
            "{id, x?, y?, scale?: 2} put a picture (PNG) of the document's selected art on the board → {item}",
            has_selection,
            capture_selection
        ),
    ]
}

const NOT_SET: &str = "Inspiration Boards keep their files in the app's settings folder, which isn't set here";

fn boards(s: &mut Session) -> Result<&mut Boards> {
    s.boards.as_mut().ok_or_else(|| EngineError::Other(NOT_SET.into()))
}

/// A board error as a command error.
fn err(e: String) -> EngineError {
    EngineError::Other(e)
}

fn board_id(p: &Value, cmd: &str) -> Result<String> {
    str_param(p, "id").filter(|i| vectorcraft_boards::valid_id(i)).map(String::from).ok_or_else(|| bad(cmd, "missing or invalid board `id`"))
}

fn item_param(p: &Value, key: &str, cmd: &str) -> Result<u64> {
    p.get(key).and_then(Value::as_u64).ok_or_else(|| bad(cmd, format!("missing item id `{key}`")))
}

fn items_param(p: &Value, cmd: &str) -> Result<Vec<u64>> {
    let list = p.get("items").and_then(Value::as_array).ok_or_else(|| bad(cmd, "missing `items`: a list of item ids"))?;
    list.iter().map(|v| v.as_u64().ok_or_else(|| bad(cmd, "`items` lists item ids"))).collect()
}

/// A finite number param, or `default`.
fn num(p: &Value, key: &str, default: f64, cmd: &str) -> Result<f64> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| bad(cmd, format!("`{key}` must be a finite number"))),
    }
}

/// Where a new item goes: (x, y) when given, else around the view's centre.
fn at(s: &mut Session, id: &str, p: &Value, cmd: &str) -> Result<(f64, f64)> {
    let view = boards(s)?.board(id).map_err(err)?.view;
    Ok((num(p, "x", view.center[0] - 60.0, cmd)?, num(p, "y", view.center[1] - 40.0, cmd)?))
}

/// A board as the commands return it: its items with their kinds, and image files' paths.
fn board_json(s: &mut Session, b: &Board) -> Result<Value> {
    let bs = boards(s)?;
    let store = bs.store().clone();
    let mut v = serde_json::to_value(b).map_err(|e| err(e.to_string()))?;
    if let Some(items) = v.get_mut("items").and_then(Value::as_array_mut) {
        for (it, item) in items.iter_mut().zip(&b.items) {
            let path = match &item.content {
                Content::Image { asset: Some(a), .. } => store.asset_path(&b.id, a).ok(),
                Content::Image { link: Some(l), .. } => Some(PathBuf::from(l)),
                _ => None,
            };
            if let Some(p) = path {
                it["path"] = json!(p.to_string_lossy());
            }
        }
    }
    v["canUndo"] = json!(bs.can_undo(&b.id));
    v["canRedo"] = json!(bs.can_redo(&b.id));
    v["folder"] = json!(store.dir().to_string_lossy());
    Ok(v)
}

fn set_folder(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").filter(|p| !p.trim().is_empty()).ok_or_else(|| bad("boards.setFolder", "missing `path`"))?;
    s.boards = Some(Boards::new(Store::new(path)));
    Ok(json!({ "path": path }))
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    let bs = boards(s)?;
    let list = bs.list().map_err(err)?;
    Ok(json!({ "boards": list, "folder": bs.store().dir().to_string_lossy() }))
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    let b = boards(s)?.create(str_param(p, "name").unwrap_or("")).map_err(err)?;
    board_json(s, &b)
}

fn get(s: &mut Session, p: &Value) -> Result<Value> {
    let id = board_id(p, "boards.get")?;
    let b = boards(s)?.board(&id).map_err(err)?.clone();
    board_json(s, &b)
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "boards.rename";
    let id = board_id(p, C)?;
    let name = str_param(p, "name").ok_or_else(|| bad(C, "missing `name`"))?;
    boards(s)?.edit(&id, Edit::Rename(name.into())).map_err(err)?;
    Ok(json!({ "id": id, "name": name.trim() }))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let id = board_id(p, "boards.delete")?;
    boards(s)?.delete(&id).map_err(err)?;
    Ok(json!({ "deleted": id }))
}

fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "boards.duplicate";
    let id = board_id(p, C)?;
    let bs = boards(s)?;
    let name = match str_param(p, "name") {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => format!("{} copy", bs.board(&id).map_err(err)?.name),
    };
    let b = bs.duplicate(&id, &name).map_err(err)?;
    board_json(s, &b)
}

fn add_image(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "boards.addImage";
    let id = board_id(p, C)?;
    let (x, y) = at(s, &id, p, C)?;
    let item = match (str_param(p, "path"), str_param(p, "dataBase64")) {
        (Some(path), None) if bool_or(p, "link", false) => boards(s)?.add_linked_image(&id, std::path::Path::new(path), x, y).map_err(err)?,
        (Some(path), None) => {
            let path = std::path::Path::new(path);
            let len = std::fs::metadata(path).map_err(|e| err(format!("can't open {}: {e}", path.display())))?.len();
            if len > vectorcraft_boards::MAX_IMAGE_BYTES as u64 {
                return Err(err(format!("images larger than {} MB can't go on a board", vectorcraft_boards::MAX_IMAGE_BYTES >> 20)));
            }
            let bytes = std::fs::read(path).map_err(|e| err(format!("can't read {}: {e}", path.display())))?;
            boards(s)?.add_image(&id, &bytes, x, y, &path.to_string_lossy()).map_err(err)?
        }
        (None, Some(data)) => {
            if data.len() > vectorcraft_boards::MAX_IMAGE_BYTES / 3 * 4 + 4 {
                return Err(err(format!("images larger than {} MB can't go on a board", vectorcraft_boards::MAX_IMAGE_BYTES >> 20)));
            }
            let bytes = vectorcraft_format::base64_decode(data).ok_or_else(|| bad(C, "`dataBase64` isn't base64"))?;
            let origin = str_param(p, "name").unwrap_or("pasted");
            boards(s)?.add_image(&id, &bytes, x, y, origin).map_err(err)?
        }
        _ => return Err(bad(C, "give `path` or `dataBase64`")),
    };
    Ok(json!({ "item": item }))
}

fn add(s: &mut Session, id: &str, p: &Value, cmd: &str, content: Content) -> Result<Value> {
    let (x, y) = at(s, id, p, cmd)?;
    let item = boards(s)?.edit(id, Edit::Add(Item { id: 0, x, y, w: 0.0, h: 0.0, content })).map_err(err)?;
    Ok(json!({ "item": item }))
}

fn add_note(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "boards.addNote";
    let id = board_id(p, C)?;
    let text = str_param(p, "text").ok_or_else(|| bad(C, "missing `text`"))?.to_string();
    let color = str_param(p, "color").unwrap_or("#fff3a6").to_string();
    add(s, &id, p, C, Content::Note { text, color })
}

fn add_color(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "boards.addColor";
    let id = board_id(p, C)?;
    let hex = str_param(p, "color").ok_or_else(|| bad(C, "missing `color`"))?.to_string();
    add(s, &id, p, C, Content::Color { hex, name: str_param(p, "name").unwrap_or("").into() })
}

fn add_reference(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "boards.addReference";
    let id = board_id(p, C)?;
    let url = str_param(p, "url").ok_or_else(|| bad(C, "missing `url`"))?.to_string();
    let title = str_param(p, "title").unwrap_or("").to_string();
    add(s, &id, p, C, Content::Reference { url, title, note: str_param(p, "note").unwrap_or("").into() })
}

fn update_item(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "boards.updateItem";
    let id = board_id(p, C)?;
    let item = item_param(p, "item", C)?;
    let bs = boards(s)?;
    let current = bs.board(&id).map_err(err)?.item(item).cloned().ok_or_else(|| bad(C, format!("no item {item} on the board")))?;
    let opt = |k: &str| -> Result<Option<f64>> { if p.get(k).is_some() { num(p, k, 0.0, C).map(Some) } else { Ok(None) } };
    let place = (opt("x")?, opt("y")?, opt("w")?, opt("h")?);
    let text = |k: &str, cur: &str| str_param(p, k).unwrap_or(cur).to_string();
    let content = match &current.content {
        Content::Note { text: t, color } => Content::Note { text: text("text", t), color: text("color", color) },
        Content::Color { hex, name } => Content::Color { hex: text("color", hex), name: text("name", name) },
        Content::Reference { url, title, note } => {
            Content::Reference { url: text("url", url), title: text("title", title), note: text("note", note) }
        }
        Content::Image { asset, link, pixel_width, pixel_height, origin, caption } => Content::Image {
            asset: asset.clone(),
            link: link.clone(),
            pixel_width: *pixel_width,
            pixel_height: *pixel_height,
            origin: origin.clone(),
            caption: text("caption", caption),
        },
    };
    let mut edits = vec![];
    if content != current.content {
        edits.push(Edit::SetContent(item, content));
    }
    if place != (None, None, None, None) {
        edits.push(Edit::Place(vec![(item, place.0, place.1, place.2, place.3)]));
    }
    // Content and box together are one undo step.
    if !edits.is_empty() {
        bs.edit(&id, Edit::Batch(edits)).map_err(err)?;
    }
    Ok(json!({ "item": item }))
}

fn nudge(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "boards.nudge";
    let id = board_id(p, C)?;
    let items = items_param(p, C)?;
    let (dx, dy) = (num(p, "dx", 0.0, C)?, num(p, "dy", 0.0, C)?);
    boards(s)?.edit(&id, Edit::Nudge(items, dx, dy)).map_err(err)?;
    Ok(Value::Null)
}

fn remove_items(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "boards.removeItems";
    let id = board_id(p, C)?;
    let items = items_param(p, C)?;
    boards(s)?.edit(&id, Edit::Remove(items.clone())).map_err(err)?;
    Ok(json!({ "removed": items }))
}

fn arrange(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "boards.arrange";
    let id = board_id(p, C)?;
    let items = items_param(p, C)?;
    boards(s)?.edit(&id, Edit::Arrange(items, bool_or(p, "front", true))).map_err(err)?;
    Ok(Value::Null)
}

fn duplicate_items(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "boards.duplicateItems";
    let id = board_id(p, C)?;
    let items = items_param(p, C)?;
    let (dx, dy) = (num(p, "dx", 24.0, C)?, num(p, "dy", 24.0, C)?);
    let bs = boards(s)?;
    let before = bs.board(&id).map_err(err)?.next_id;
    bs.edit(&id, Edit::Duplicate(items.clone(), dx, dy)).map_err(err)?;
    Ok(json!({ "items": (before..before + items.len() as u64).collect::<Vec<_>>() }))
}

fn set_view(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "boards.setView";
    let id = board_id(p, C)?;
    let c = p.get("center").and_then(Value::as_array).ok_or_else(|| bad(C, "missing `center`: [x, y]"))?;
    let (Some(x), Some(y)) = (c.first().and_then(Value::as_f64), c.get(1).and_then(Value::as_f64)) else { return Err(bad(C, "`center` is [x, y]")) };
    let zoom = num(p, "zoom", 1.0, C)?;
    boards(s)?.set_view(&id, View { center: [x, y], zoom }).map_err(err)?;
    Ok(Value::Null)
}

fn step(s: &mut Session, p: &Value, back: bool) -> Result<Value> {
    let id = board_id(p, if back { "boards.undo" } else { "boards.redo" })?;
    let bs = boards(s)?;
    let done = if back { bs.undo(&id) } else { bs.redo(&id) }.map_err(err)?;
    Ok(json!({ if back { "undone" } else { "redone" }: done }))
}

/// The point type size for notes and references sent to a document.
const NOTE_TYPE_SIZE: f64 = 14.0;

fn to_document(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "boards.toDocument";
    let id = board_id(p, C)?;
    let board = boards(s)?.board(&id).map_err(err)?.clone();
    let chosen: Vec<u64> = match p.get("items") {
        Some(_) => items_param(p, C)?,
        None => board.items.iter().map(|i| i.id).collect(),
    };
    let items: Vec<&Item> = board.items.iter().filter(|i| chosen.contains(&i.id)).collect();
    if items.is_empty() {
        return Err(bad(C, "the board has none of those items"));
    }
    let artboard = p.get("artboard").and_then(Value::as_u64).unwrap_or(0) as usize;
    let ab = s.doc()?.doc.artboards.get(artboard).map(|a| a.rect).ok_or_else(|| bad(C, format!("no artboard {artboard}")))?;
    let Some([x0, y0, x1, y1]) = board.bounds(&chosen) else { return Err(bad(C, "nothing to send")) };
    // Board points become document points, the group centred on the artboard.
    let (dx, dy) = (ab.center().x - (x0 + x1) / 2.0, ab.center().y - (y0 + y1) / 2.0);
    let mut images = vec![];
    for it in &items {
        if matches!(it.content, Content::Image { .. }) {
            images.push((it.id, boards(s)?.image_bytes(&id, it.id).map_err(err)?));
        }
    }
    let grouped = s.active().is_some_and(|d| d.undo_group.is_none());
    if grouped {
        s.begin_named_undo_group(Some("Send to Document".into()));
    }
    let result = (|| -> Result<(Vec<u64>, Vec<String>)> {
        let mut ids = vec![];
        let mut swatches = vec![];
        for it in &items {
            let (x, y, w, h) = (it.x + dx, it.y + dy, it.w, it.h);
            match &it.content {
                Content::Image { origin, .. } => {
                    let Some((_, bytes)) = images.iter().find(|(i, _)| *i == it.id) else { continue };
                    let name = std::path::Path::new(origin)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .filter(|n| n.contains('.'))
                        .unwrap_or_else(|| "board-image.png".into());
                    let r = s.execute(
                        "file.place",
                        &json!({ "name": name, "dataBase64": vectorcraft_format::base64_encode(bytes), "rect": [x, y, w, h] }),
                    )?;
                    ids.extend(r["ids"].as_array().into_iter().flatten().filter_map(Value::as_u64));
                }
                Content::Note { text, .. } if !text.trim().is_empty() => {
                    let r = s.execute("text.create", &json!({ "x": x, "y": y + NOTE_TYPE_SIZE, "text": text, "size": NOTE_TYPE_SIZE }))?;
                    ids.extend(r["id"].as_u64());
                }
                Content::Reference { url, title, .. } => {
                    let text = if title.trim().is_empty() { url.clone() } else { format!("{title}\n{url}") };
                    let r = s.execute("text.create", &json!({ "x": x, "y": y + NOTE_TYPE_SIZE, "text": text, "size": NOTE_TYPE_SIZE }))?;
                    ids.extend(r["id"].as_u64());
                }
                Content::Color { hex, name } => {
                    let square = s.execute("shape.rectangle", &json!({ "x": x, "y": y, "width": w.min(h), "height": w.min(h) }))?;
                    if let Some(sq) = square["id"].as_u64() {
                        s.execute("paint.setFill", &json!({ "ids": [sq], "color": hex }))?;
                        s.execute("paint.setStroke", &json!({ "ids": [sq], "none": true }))?;
                        ids.push(sq);
                    }
                    let swatch_name = if name.trim().is_empty() { hex.clone() } else { name.clone() };
                    let exists = s.doc()?.doc.swatches.iter().any(|sw| sw.name == swatch_name);
                    if !exists {
                        s.execute("swatch.new", &json!({ "name": swatch_name, "color": hex }))?;
                        swatches.push(swatch_name);
                    }
                }
                Content::Note { .. } => {}
            }
        }
        Ok((ids, swatches))
    })();
    match result {
        Ok((ids, swatches)) => {
            if grouped {
                s.end_undo_group(false);
            }
            let sel: Vec<NodeId> = ids.iter().map(|i| NodeId(*i)).collect();
            s.select(|_, x| x.set(sel.iter().copied()))?;
            Ok(json!({ "ids": ids, "swatches": swatches }))
        }
        Err(e) => {
            if grouped {
                s.end_undo_group(true);
            }
            Err(e)
        }
    }
}

fn capture_selection(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "boards.captureSelection";
    let id = board_id(p, C)?;
    let scale = num(p, "scale", 2.0, C)?.clamp(0.1, 8.0);
    let r = s.execute("document.exportSelection", &json!({ "format": "png", "scale": scale }))?;
    let data = r["dataBase64"].as_str().ok_or_else(|| err("the selection couldn't be drawn".into()))?;
    let bytes = vectorcraft_format::base64_decode(data).ok_or_else(|| err("the selection couldn't be drawn".into()))?;
    let (x, y) = at(s, &id, p, C)?;
    let item = boards(s)?.add_image(&id, &bytes, x, y, "selection").map_err(err)?;
    // At the art's size on the board, not its pixels' (which are `scale` times as many).
    let b = boards(s)?.board(&id).map_err(err)?.item(item).cloned();
    if let (Some(it), Some(bounds)) = (b, r.get("bounds")) {
        let w = bounds.get("width").and_then(Value::as_f64).or_else(|| bounds.get(2).and_then(Value::as_f64));
        let h = bounds.get("height").and_then(Value::as_f64).or_else(|| bounds.get(3).and_then(Value::as_f64));
        if let (Some(w), Some(h)) = (w, h)
            && w > 0.0
            && h > 0.0
        {
            boards(s)?.edit(&id, Edit::Place(vec![(it.id, None, None, Some(w), Some(h))])).map_err(err)?;
        }
    }
    Ok(json!({ "item": item }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> (Session, PathBuf) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("dlightroom-engine-boards-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::new();
        s.execute("boards.setFolder", &json!({"path": dir.to_string_lossy()})).unwrap();
        (s, dir)
    }

    fn png_b64(w: u32, h: u32) -> String {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 120, 200, 255]));
        let mut out = std::io::Cursor::new(vec![]);
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        vectorcraft_format::base64_encode(&out.into_inner())
    }

    #[test]
    fn boards_need_a_folder() {
        let mut s = Session::new();
        let e = s.execute("boards.list", &json!({})).unwrap_err();
        assert!(e.to_string().contains("settings folder"), "{e}");
    }

    #[test]
    fn a_board_is_built_edited_undone_and_listed() {
        let (mut s, _) = session();
        let id = s.execute("boards.create", &json!({"name": "Brand mood"})).unwrap()["id"].as_str().unwrap().to_string();
        let note = s.execute("boards.addNote", &json!({"id": id, "text": "bold and warm", "x": 0, "y": 0})).unwrap()["item"].as_u64().unwrap();
        let color = s.execute("boards.addColor", &json!({"id": id, "color": "#ff5d5d", "name": "Coral", "x": 240, "y": 0})).unwrap()["item"]
            .as_u64()
            .unwrap();
        s.execute("boards.addReference", &json!({"id": id, "url": "https://example.com/type", "title": "Type study", "x": 0, "y": 200})).unwrap();
        let img =
            s.execute("boards.addImage", &json!({"id": id, "dataBase64": png_b64(400, 200), "name": "sky.png", "x": 400, "y": 0})).unwrap()["item"]
                .as_u64()
                .unwrap();
        s.execute("boards.updateItem", &json!({"id": id, "item": note, "text": "bold, warm, playful", "w": 260})).unwrap();
        s.execute("boards.nudge", &json!({"id": id, "items": [color], "dx": 10, "dy": 5})).unwrap();
        s.execute("boards.arrange", &json!({"id": id, "items": [note], "front": true})).unwrap();
        let b = s.execute("boards.get", &json!({"id": id})).unwrap();
        assert_eq!(b["items"].as_array().unwrap().len(), 4);
        let last = &b["items"][3];
        assert_eq!((last["id"].as_u64(), last["text"].as_str()), (Some(note), Some("bold, warm, playful")));
        let image = b["items"].as_array().unwrap().iter().find(|i| i["id"].as_u64() == Some(img)).unwrap();
        assert!(std::path::Path::new(image["path"].as_str().unwrap()).is_file(), "the embedded copy is on disk");
        assert_eq!(b["canUndo"], true);
        s.execute("boards.undo", &json!({"id": id})).unwrap();
        s.execute("boards.undo", &json!({"id": id})).unwrap();
        let b = s.execute("boards.get", &json!({"id": id})).unwrap();
        let c = b["items"].as_array().unwrap().iter().find(|i| i["id"].as_u64() == Some(color)).unwrap();
        assert_eq!(c["x"].as_f64(), Some(240.0), "the nudge is undone");
        let list = s.execute("boards.list", &json!({})).unwrap();
        assert_eq!(list["boards"][0]["name"], "Brand mood");
        // A fresh session sees the same board on disk.
        let mut t = Session::new();
        t.execute("boards.setFolder", &json!({"path": list["folder"]})).unwrap();
        assert_eq!(t.execute("boards.get", &json!({"id": id})).unwrap()["items"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn items_go_to_the_document_in_one_undo_step() {
        let (mut s, _) = session();
        s.execute("file.new", &json!({"width": 800, "height": 600})).unwrap();
        let id = s.execute("boards.create", &json!({"name": "To doc"})).unwrap()["id"].as_str().unwrap().to_string();
        s.execute("boards.addImage", &json!({"id": id, "dataBase64": png_b64(100, 50), "x": 0, "y": 0})).unwrap();
        s.execute("boards.addNote", &json!({"id": id, "text": "headline idea", "x": 0, "y": 120})).unwrap();
        s.execute("boards.addColor", &json!({"id": id, "color": "#123456", "name": "Ink Blue", "x": 220, "y": 0})).unwrap();
        let steps = s.doc().unwrap().history.undo.len();
        let r = s.execute("boards.toDocument", &json!({"id": id})).unwrap();
        assert_eq!(s.doc().unwrap().history.undo.len(), steps + 1, "one undo step");
        let ids: Vec<u64> = r["ids"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
        assert!(ids.len() >= 3, "{r}");
        let d = &s.doc().unwrap().doc;
        assert!(d.swatches.iter().any(|sw| sw.name == "Ink Blue"));
        assert!(ids.iter().any(|i| matches!(d.node(NodeId(*i)).map(|n| &n.kind), Some(vectorcraft_doc::NodeKind::Image(_)))));
        assert!(ids.iter().any(|i| matches!(d.node(NodeId(*i)).map(|n| &n.kind), Some(vectorcraft_doc::NodeKind::Text(_)))));
        // Centred on the artboard.
        let b = d.bounds_of(&ids.iter().map(|i| NodeId(*i)).collect::<Vec<_>>(), false).unwrap();
        assert!((b.center().x - 400.0).abs() < 40.0, "{b:?}");
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(s.doc().unwrap().doc.node(NodeId(ids[0])).is_none(), "undone as one step");
    }

    #[test]
    fn the_selection_becomes_a_picture_on_the_board() {
        let (mut s, _) = session();
        s.execute("file.new", &json!({"width": 400, "height": 300})).unwrap();
        s.execute("shape.rectangle", &json!({"x": 10, "y": 10, "width": 80, "height": 40})).unwrap();
        s.execute("select.all", &json!({})).unwrap();
        let id = s.execute("boards.create", &json!({})).unwrap()["id"].as_str().unwrap().to_string();
        let item = s.execute("boards.captureSelection", &json!({"id": id, "x": 0, "y": 0})).unwrap()["item"].as_u64().unwrap();
        let b = s.execute("boards.get", &json!({"id": id})).unwrap();
        let it = b["items"].as_array().unwrap().iter().find(|i| i["id"].as_u64() == Some(item)).unwrap();
        assert_eq!(it["kind"], "image");
        assert!(it["w"].as_f64().unwrap() > 70.0, "{it}");
    }

    #[test]
    fn bad_params_are_errors_not_panics() {
        let (mut s, _) = session();
        let id = s.execute("boards.create", &json!({})).unwrap()["id"].as_str().unwrap().to_string();
        for (cmd, p) in [
            ("boards.get", json!({"id": "../../etc"})),
            ("boards.addNote", json!({"id": id})),
            ("boards.addColor", json!({"id": id, "color": "blue-ish"})),
            ("boards.addImage", json!({"id": id, "dataBase64": "%%%"})),
            ("boards.addImage", json!({"id": id, "path": "/no/such/file.png"})),
            ("boards.removeItems", json!({"id": id, "items": [42]})),
            ("boards.setView", json!({"id": id, "center": [0, 0], "zoom": -1})),
            ("boards.nudge", json!({"id": id, "items": [], "dx": "far"})),
        ] {
            assert!(s.execute(cmd, &p).is_err(), "{cmd} {p}");
        }
    }
}
