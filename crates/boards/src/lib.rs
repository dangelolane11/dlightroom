//! DLightroom's Inspiration Boards: moodboards that sit beside documents. A board holds images
//! (embedded copies or links to files), text notes, colours and web references, laid out freely
//! on an infinite, zoomable plane. Boards are files of their own, never part of a document, kept in
//! a folder of the app's settings:
//!
//! ```text
//! <folder>/<board id>/board.json      the board (format "dlightroom-board", version 1)
//! <folder>/<board id>/assets/<sha256>.<ext>   embedded images, named by their content
//! ```
//!
//! [`Boards`] is the editing front: it opens boards from a [`Store`], applies edits ([`Edit`]),
//! keeps an undo history per board and saves after every change (writing a temporary file and
//! renaming it over the old one, so a crash never leaves half a board). Everything read from disk
//! is untrusted: ids and asset names are checked before they become paths, sizes and counts are
//! capped, and nothing panics.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The `format` of a board file.
pub const FORMAT: &str = "dlightroom-board";
/// The board file version this build writes (and the newest it reads).
pub const VERSION: u32 = 1;
/// Most items one board holds.
pub const MAX_ITEMS: usize = 5000;
/// Largest image file a board takes in.
pub const MAX_IMAGE_BYTES: usize = 64 << 20;
/// Largest board file read.
const MAX_BOARD_BYTES: u64 = 32 << 20;
/// Largest image (in pixels, either side) a board takes in.
pub const MAX_IMAGE_SIDE: u32 = 20_000;
/// Longest note, caption, title or URL kept (characters).
pub const MAX_TEXT: usize = 20_000;
/// Undo steps kept per board.
const MAX_UNDO: usize = 100;
/// Longest side an image item starts with on the board (points).
const IMAGE_START: f64 = 320.0;
/// Coordinates are kept within ± this on the board plane.
const MAX_COORD: f64 = 1.0e7;

type Result<T> = std::result::Result<T, String>;

/// One board.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub format: String,
    pub version: u32,
    pub id: String,
    pub name: String,
    /// In paint order: later items lie on top.
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default)]
    pub view: View,
    #[serde(default)]
    pub next_id: u64,
    /// Unix seconds.
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub modified: u64,
}

/// Where the board was last looked at: the plane point at the centre of the view, and the zoom.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub center: [f64; 2],
    pub zoom: f64,
}

impl Default for View {
    fn default() -> Self {
        Self { center: [0.0, 0.0], zoom: 1.0 }
    }
}

/// An item on a board: its box on the plane and what it shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: u64,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(flatten)]
    pub content: Content,
}

/// What an item shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Content {
    /// An image: an embedded copy (`asset`, in the board's assets folder) or a link to a file
    /// (`link`, read where it is). `origin` says where it came from: a file path, `pasted`, or
    /// `ai:<provider>` for an AI variation.
    #[serde(rename_all = "camelCase")]
    Image {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        asset: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        link: Option<String>,
        pixel_width: u32,
        pixel_height: u32,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        origin: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        caption: String,
    },
    /// A text note on a coloured card (`#rrggbb`).
    Note { text: String, color: String },
    /// A colour chip (`#rrggbb`) with an optional name.
    Color {
        hex: String,
        #[serde(default)]
        name: String,
    },
    /// A reference to a web page or other resource.
    Reference {
        url: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        note: String,
    },
}

impl Content {
    /// The kind's name, as boards and commands write it.
    pub fn kind(&self) -> &'static str {
        match self {
            Content::Image { .. } => "image",
            Content::Note { .. } => "note",
            Content::Color { .. } => "color",
            Content::Reference { .. } => "reference",
        }
    }
}

/// A board as `list` shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub items: usize,
    pub modified: u64,
}

/// (id, x, y, w, h) for [`Edit::Place`]; `None` keeps that value.
pub type Placement = (u64, Option<f64>, Option<f64>, Option<f64>, Option<f64>);

/// A change to one board: what [`Boards::edit`] applies (one undo step each).
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    Rename(String),
    /// Add an item; its id is assigned. `w`/`h` of 0 pick a size for the content.
    Add(Item),
    /// Move and/or resize items: (id, x, y, w, h), any of them `None` to keep.
    Place(Vec<Placement>),
    /// Move items by (dx, dy).
    Nudge(Vec<u64>, f64, f64),
    /// Change what an item shows (same kind).
    SetContent(u64, Content),
    Remove(Vec<u64>),
    /// Bring items to the front (true) or send them to the back.
    Arrange(Vec<u64>, bool),
    /// Copy items, offset by (dx, dy).
    Duplicate(Vec<u64>, f64, f64),
    /// Several edits as one undo step (all or none).
    Batch(Vec<Edit>),
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A board id: letters, digits, `-` and `_`, 1–64 of them (it names a folder).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// An asset name: 64 hex digits, a dot and an image extension.
pub fn valid_asset(name: &str) -> bool {
    let Some((hash, ext)) = name.split_once('.') else { return false };
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) && IMAGE_EXTS.contains(&ext)
}

/// The image file extensions boards store.
const IMAGE_EXTS: &[&str] = &["png", "jpg", "gif", "webp", "tif", "bmp"];

/// `#rrggbb` (lowercase) for a colour written as `#rgb`, `#rrggbb` or without the `#`.
pub fn normalize_hex(s: &str) -> Option<String> {
    let h = s.trim().trim_start_matches('#');
    if !h.is_ascii() {
        return None;
    }
    let full = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 => h.to_string(),
        _ => return None,
    };
    full.chars().all(|c| c.is_ascii_hexdigit()).then(|| format!("#{}", full.to_ascii_lowercase()))
}

fn clip(s: &str) -> String {
    s.chars().take(MAX_TEXT).collect()
}

fn finite(v: f64, what: &str) -> Result<f64> {
    if v.is_finite() && v.abs() <= MAX_COORD { Ok(v) } else { Err(format!("{what} must be a finite number within ±{MAX_COORD}")) }
}

impl Board {
    /// A new, empty board.
    pub fn new(id: String, name: &str) -> Self {
        let t = now();
        Self {
            format: FORMAT.into(),
            version: VERSION,
            id,
            name: clip(name.trim()),
            items: vec![],
            view: View::default(),
            next_id: 1,
            created: t,
            modified: t,
        }
    }

    /// Read a board file, checking what it holds.
    pub fn from_json(text: &str) -> Result<Self> {
        let mut b: Board = serde_json::from_str(text).map_err(|e| format!("not a board file: {e}"))?;
        if b.format != FORMAT {
            return Err(format!("not a board file (format {:?})", b.format));
        }
        if b.version > VERSION {
            return Err(format!("board version {} is newer than this DLightroom reads ({VERSION})", b.version));
        }
        if !valid_id(&b.id) {
            return Err("the board's id is not valid".into());
        }
        if b.items.len() > MAX_ITEMS {
            return Err(format!("the board holds more than {MAX_ITEMS} items"));
        }
        b.sanitize();
        Ok(b)
    }

    /// Make what was read safe to use: finite boxes, valid colours and asset names, unique ids.
    fn sanitize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        let mut max_id = 0;
        self.items.retain_mut(|it| {
            let ok = [it.x, it.y, it.w, it.h].iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD) && it.w > 0.0 && it.h > 0.0 && seen.insert(it.id);
            if let Content::Image { asset: Some(a), .. } = &it.content
                && !valid_asset(a)
            {
                return false;
            }
            match &mut it.content {
                Content::Note { text, color } => {
                    *text = clip(text);
                    *color = normalize_hex(color).unwrap_or_else(|| "#fff3a6".into());
                }
                Content::Color { hex, name } => {
                    *hex = normalize_hex(hex).unwrap_or_else(|| "#000000".into());
                    *name = clip(name);
                }
                Content::Reference { url, title, note } => {
                    *url = clip(url);
                    *title = clip(title);
                    *note = clip(note);
                }
                Content::Image { caption, origin, .. } => {
                    *caption = clip(caption);
                    *origin = clip(origin);
                }
            }
            if ok {
                max_id = max_id.max(it.id);
            }
            ok
        });
        self.next_id = self.next_id.max(max_id + 1);
        if !(self.view.zoom.is_finite() && self.view.zoom > 0.0) {
            self.view.zoom = 1.0;
        }
        self.view.zoom = self.view.zoom.clamp(0.02, 50.0);
        if !self.view.center.iter().all(|v| v.is_finite()) {
            self.view.center = [0.0, 0.0];
        }
        self.name = clip(&self.name);
    }

    pub fn item(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    fn item_mut(&mut self, id: u64) -> Result<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id).ok_or_else(|| format!("no item {id} on the board"))
    }

    /// The colours on the board (colour chips and notes), in paint order, without repeats.
    pub fn palette(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = vec![];
        for it in &self.items {
            if let Content::Color { hex, name } = &it.content
                && !out.iter().any(|(h, _)| h == hex)
            {
                out.push((hex.clone(), name.clone()));
            }
        }
        out
    }

    /// The box around `ids` (all items when empty).
    pub fn bounds(&self, ids: &[u64]) -> Option<[f64; 4]> {
        let mut b: Option<[f64; 4]> = None;
        for it in self.items.iter().filter(|i| ids.is_empty() || ids.contains(&i.id)) {
            let r = [it.x, it.y, it.x + it.w, it.y + it.h];
            b = Some(match b {
                None => r,
                Some(o) => [o[0].min(r[0]), o[1].min(r[1]), o[2].max(r[2]), o[3].max(r[3])],
            });
        }
        b
    }

    /// Apply `e`; the id of an added item, else `None`.
    pub fn apply(&mut self, e: Edit) -> Result<Option<u64>> {
        match e {
            Edit::Rename(name) => {
                let name = name.trim();
                if name.is_empty() {
                    return Err("a board needs a name".into());
                }
                self.name = clip(name);
            }
            Edit::Add(mut item) => {
                if self.items.len() >= MAX_ITEMS {
                    return Err(format!("a board holds at most {MAX_ITEMS} items"));
                }
                item.content = checked_content(item.content)?;
                let (w, h) = default_size(&item.content);
                // NaN counts as unset too.
                if item.w.is_nan() || item.w <= 0.0 {
                    item.w = w;
                }
                if item.h.is_nan() || item.h <= 0.0 {
                    item.h = h;
                }
                for (v, what) in [(item.x, "x"), (item.y, "y"), (item.w, "width"), (item.h, "height")] {
                    finite(v, what)?;
                }
                item.id = self.next_id;
                self.next_id += 1;
                let id = item.id;
                self.items.push(item);
                self.modified = now();
                return Ok(Some(id));
            }
            Edit::Place(list) => {
                for (id, x, y, w, h) in list {
                    let it = self.item_mut(id)?;
                    if let Some(v) = x {
                        it.x = finite(v, "x")?;
                    }
                    if let Some(v) = y {
                        it.y = finite(v, "y")?;
                    }
                    if let Some(v) = w {
                        it.w = finite(v, "width")?.max(4.0);
                    }
                    if let Some(v) = h {
                        it.h = finite(v, "height")?.max(4.0);
                    }
                }
            }
            Edit::Nudge(ids, dx, dy) => {
                let (dx, dy) = (finite(dx, "dx")?, finite(dy, "dy")?);
                for id in ids {
                    let it = self.item_mut(id)?;
                    it.x = finite(it.x + dx, "x")?;
                    it.y = finite(it.y + dy, "y")?;
                }
            }
            Edit::SetContent(id, content) => {
                let content = checked_content(content)?;
                let it = self.item_mut(id)?;
                if it.content.kind() != content.kind() {
                    return Err(format!("item {id} is a {}, not a {}", it.content.kind(), content.kind()));
                }
                it.content = content;
            }
            Edit::Remove(ids) => {
                for id in &ids {
                    self.item_mut(*id)?;
                }
                self.items.retain(|i| !ids.contains(&i.id));
            }
            Edit::Arrange(ids, front) => {
                for id in &ids {
                    self.item_mut(*id)?;
                }
                let (mut moved, rest): (Vec<Item>, Vec<Item>) = std::mem::take(&mut self.items).into_iter().partition(|i| ids.contains(&i.id));
                self.items = if front {
                    let mut v = rest;
                    v.append(&mut moved);
                    v
                } else {
                    moved.extend(rest);
                    moved
                };
            }
            Edit::Batch(edits) => {
                let mut added = None;
                for e in edits {
                    added = self.apply(e)?.or(added);
                }
                return Ok(added);
            }
            Edit::Duplicate(ids, dx, dy) => {
                let copies: Vec<Item> =
                    ids.iter().map(|id| self.item(*id).cloned().ok_or_else(|| format!("no item {id} on the board"))).collect::<Result<_>>()?;
                if self.items.len() + copies.len() > MAX_ITEMS {
                    return Err(format!("a board holds at most {MAX_ITEMS} items"));
                }
                for mut c in copies {
                    c.id = self.next_id;
                    self.next_id += 1;
                    c.x = finite(c.x + dx, "x")?;
                    c.y = finite(c.y + dy, "y")?;
                    self.items.push(c);
                }
            }
        }
        self.modified = now();
        Ok(None)
    }
}

/// `c` checked and tidied: colours normalized, text clipped, an image with exactly one source.
fn checked_content(c: Content) -> Result<Content> {
    Ok(match c {
        Content::Note { text, color } => {
            Content::Note { text: clip(&text), color: normalize_hex(&color).ok_or_else(|| format!("{color:?} is not a colour (#rrggbb)"))? }
        }
        Content::Color { hex, name } => {
            Content::Color { hex: normalize_hex(&hex).ok_or_else(|| format!("{hex:?} is not a colour (#rrggbb)"))?, name: clip(&name) }
        }
        Content::Reference { url, title, note } => {
            let url = url.trim().to_string();
            if url.is_empty() {
                return Err("a reference needs a URL or path".into());
            }
            Content::Reference { url: clip(&url), title: clip(&title), note: clip(&note) }
        }
        Content::Image { asset, link, pixel_width, pixel_height, origin, caption } => {
            match (&asset, &link) {
                (Some(a), None) if valid_asset(a) => {}
                (None, Some(l)) if !l.trim().is_empty() => {}
                _ => return Err("an image is either embedded (asset) or linked (link)".into()),
            }
            if pixel_width == 0 || pixel_height == 0 || pixel_width > MAX_IMAGE_SIDE || pixel_height > MAX_IMAGE_SIDE {
                return Err("the image has no usable size".into());
            }
            Content::Image { asset, link, pixel_width, pixel_height, origin: clip(&origin), caption: clip(&caption) }
        }
    })
}

/// The size an item of `c` starts with on the board.
fn default_size(c: &Content) -> (f64, f64) {
    match c {
        Content::Image { pixel_width, pixel_height, .. } => {
            let (w, h) = (f64::from(*pixel_width).max(1.0), f64::from(*pixel_height).max(1.0));
            let k = (IMAGE_START / w.max(h)).min(1.0);
            ((w * k).max(8.0), (h * k).max(8.0))
        }
        Content::Note { .. } => (200.0, 140.0),
        Content::Color { .. } => (96.0, 120.0),
        Content::Reference { .. } => (240.0, 96.0),
    }
}

/// What an image file is: its extension (as boards store it) and size in pixels. The size comes
/// from the file's header; nothing is decoded beyond it.
pub fn probe_image(bytes: &[u8]) -> Result<(&'static str, u32, u32)> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!("images larger than {} MB can't go on a board", MAX_IMAGE_BYTES >> 20));
    }
    let format = image::guess_format(bytes).map_err(|_| "not an image a board can show (PNG, JPEG, GIF, WebP, TIFF or BMP)".to_string())?;
    let ext = match format {
        image::ImageFormat::Png => "png",
        image::ImageFormat::Jpeg => "jpg",
        image::ImageFormat::Gif => "gif",
        image::ImageFormat::WebP => "webp",
        image::ImageFormat::Tiff => "tif",
        image::ImageFormat::Bmp => "bmp",
        _ => return Err("not an image a board can show (PNG, JPEG, GIF, WebP, TIFF or BMP)".into()),
    };
    let reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let (w, h) = reader.into_dimensions().map_err(|e| format!("the image can't be read: {e}"))?;
    if w == 0 || h == 0 || w > MAX_IMAGE_SIDE || h > MAX_IMAGE_SIDE {
        return Err(format!("images up to {MAX_IMAGE_SIDE} pixels a side can go on a board"));
    }
    Ok((ext, w, h))
}

/// Board files on disk: one folder per board under `dir`.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn board_dir(&self, id: &str) -> Result<PathBuf> {
        if valid_id(id) { Ok(self.dir.join(id)) } else { Err(format!("{id:?} is not a board id")) }
    }

    /// The file of an embedded image on board `id`.
    pub fn asset_path(&self, id: &str, asset: &str) -> Result<PathBuf> {
        if !valid_asset(asset) {
            return Err(format!("{asset:?} is not an asset name"));
        }
        Ok(self.board_dir(id)?.join("assets").join(asset))
    }

    /// Every board in the folder, most recently changed first. Folders that don't hold a readable
    /// board are skipped.
    pub fn list(&self) -> Result<Vec<Summary>> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(format!("can't read the boards folder {}: {e}", self.dir.display())),
        };
        let mut out = vec![];
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !valid_id(&name) {
                continue;
            }
            if let Ok(b) = self.load(&name) {
                out.push(Summary { id: b.id, name: b.name, items: b.items.len(), modified: b.modified });
            }
        }
        out.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.name.cmp(&b.name)));
        Ok(out)
    }

    pub fn load(&self, id: &str) -> Result<Board> {
        let path = self.board_dir(id)?.join("board.json");
        let meta = std::fs::metadata(&path).map_err(|e| format!("can't open board {id}: {e}"))?;
        if meta.len() > MAX_BOARD_BYTES {
            return Err(format!("board {id} is larger than {} MB", MAX_BOARD_BYTES >> 20));
        }
        let text = std::fs::read_to_string(&path).map_err(|e| format!("can't read board {id}: {e}"))?;
        let b = Board::from_json(&text)?;
        if b.id != id {
            return Err(format!("the board in folder {id} says it is {}", b.id));
        }
        Ok(b)
    }

    /// Write `b` (to a temporary file, then renamed over the old one).
    pub fn save(&self, b: &Board) -> Result<()> {
        let dir = self.board_dir(&b.id)?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
        let text = serde_json::to_string_pretty(b).map_err(|e| format!("can't write board {}: {e}", b.id))?;
        let tmp = dir.join("board.json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("can't write board {}: {e}", b.id))?;
        std::fs::rename(&tmp, dir.join("board.json")).map_err(|e| format!("can't save board {}: {e}", b.id))
    }

    /// A new board's id: not used by any folder yet.
    fn new_id(&self) -> String {
        let base = format!("b{:x}", SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
        let mut id = base.clone();
        let mut n = 1u32;
        while self.dir.join(&id).exists() {
            n += 1;
            id = format!("{base}-{n}");
        }
        id
    }

    pub fn create(&self, name: &str) -> Result<Board> {
        let name = name.trim();
        let b = Board::new(self.new_id(), if name.is_empty() { "Untitled Board" } else { name });
        self.save(&b)?;
        Ok(b)
    }

    /// Delete board `id` with its assets.
    pub fn delete(&self, id: &str) -> Result<()> {
        let dir = self.board_dir(id)?;
        // Only a folder that holds a board is removed.
        self.load(id)?;
        std::fs::remove_dir_all(&dir).map_err(|e| format!("can't delete board {id}: {e}"))
    }

    /// A copy of board `id` (its embedded images copied too) named `name`.
    pub fn duplicate(&self, id: &str, name: &str) -> Result<Board> {
        let mut b = self.load(id)?;
        b.id = self.new_id();
        b.name = clip(name.trim());
        let t = now();
        (b.created, b.modified) = (t, t);
        let (from, to) = (self.board_dir(id)?.join("assets"), self.board_dir(&b.id)?.join("assets"));
        for it in &b.items {
            if let Content::Image { asset: Some(a), .. } = &it.content {
                std::fs::create_dir_all(&to).map_err(|e| format!("can't create {}: {e}", to.display()))?;
                std::fs::copy(from.join(a), to.join(a)).map_err(|e| format!("can't copy image {a}: {e}"))?;
            }
        }
        self.save(&b)?;
        Ok(b)
    }

    /// Take image `bytes` into board `id`'s assets (stored once per content): its asset name and
    /// size in pixels.
    pub fn import_image(&self, id: &str, bytes: &[u8]) -> Result<(String, u32, u32)> {
        let (ext, w, h) = probe_image(bytes)?;
        let hash = Sha256::digest(bytes);
        let name = format!("{}.{ext}", hash.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let path = self.asset_path(id, &name)?;
        if !path.exists() {
            let dir = path.parent().ok_or("no assets folder")?;
            std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, bytes).map_err(|e| format!("can't store the image: {e}"))?;
            std::fs::rename(&tmp, &path).map_err(|e| format!("can't store the image: {e}"))?;
        }
        Ok((name, w, h))
    }

    /// The bytes of image item content `c` on board `id`: its embedded copy or the file it links.
    pub fn image_bytes(&self, id: &str, c: &Content) -> Result<Vec<u8>> {
        let path = match c {
            Content::Image { asset: Some(a), .. } => self.asset_path(id, a)?,
            Content::Image { link: Some(l), .. } => PathBuf::from(l),
            _ => return Err("not an image".into()),
        };
        let meta = std::fs::metadata(&path).map_err(|e| format!("the image {} is missing: {e}", path.display()))?;
        if meta.len() > MAX_IMAGE_BYTES as u64 {
            return Err(format!("the image {} is larger than {} MB", path.display(), MAX_IMAGE_BYTES >> 20));
        }
        std::fs::read(&path).map_err(|e| format!("can't read the image {}: {e}", path.display()))
    }
}

/// An open board with its undo history.
#[derive(Clone, Debug)]
struct Open {
    board: Board,
    undo: Vec<Board>,
    redo: Vec<Board>,
}

/// The boards the app is editing: a [`Store`] and the boards opened from it, each with its own
/// undo history. Every change is saved at once.
#[derive(Clone, Debug)]
pub struct Boards {
    store: Store,
    open: HashMap<String, Open>,
}

impl Boards {
    pub fn new(store: Store) -> Self {
        Self { store, open: HashMap::new() }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn list(&self) -> Result<Vec<Summary>> {
        self.store.list()
    }

    /// Board `id` (opened from disk the first time).
    pub fn board(&mut self, id: &str) -> Result<&Board> {
        Ok(&self.open_board(id)?.board)
    }

    fn open_board(&mut self, id: &str) -> Result<&mut Open> {
        if !self.open.contains_key(id) {
            let board = self.store.load(id)?;
            self.open.insert(id.to_string(), Open { board, undo: vec![], redo: vec![] });
        }
        self.open.get_mut(id).ok_or_else(|| format!("no board {id}"))
    }

    pub fn create(&mut self, name: &str) -> Result<Board> {
        let b = self.store.create(name)?;
        self.open.insert(b.id.clone(), Open { board: b.clone(), undo: vec![], redo: vec![] });
        Ok(b)
    }

    pub fn delete(&mut self, id: &str) -> Result<()> {
        self.store.delete(id)?;
        self.open.remove(id);
        Ok(())
    }

    pub fn duplicate(&mut self, id: &str, name: &str) -> Result<Board> {
        // Save pending state first: the copy is made from disk.
        if let Some(o) = self.open.get(id) {
            self.store.save(&o.board)?;
        }
        let b = self.store.duplicate(id, name)?;
        self.open.insert(b.id.clone(), Open { board: b.clone(), undo: vec![], redo: vec![] });
        Ok(b)
    }

    /// Apply `e` to board `id` as one undo step and save it: the id of an added item.
    pub fn edit(&mut self, id: &str, e: Edit) -> Result<Option<u64>> {
        let store = self.store.clone();
        let o = self.open_board(id)?;
        let before = o.board.clone();
        let r = o.board.apply(e);
        match r {
            Ok(added) => {
                if let Err(err) = store.save(&o.board) {
                    o.board = before;
                    return Err(err);
                }
                o.undo.push(before);
                if o.undo.len() > MAX_UNDO {
                    o.undo.remove(0);
                }
                o.redo.clear();
                Ok(added)
            }
            Err(err) => {
                o.board = before;
                Err(err)
            }
        }
    }

    /// Remember where board `id` is looked at (not an undo step).
    pub fn set_view(&mut self, id: &str, view: View) -> Result<()> {
        let store = self.store.clone();
        let o = self.open_board(id)?;
        if !(view.zoom.is_finite() && view.zoom > 0.0 && view.center.iter().all(|v| v.is_finite())) {
            return Err("the view must have a finite centre and a positive zoom".into());
        }
        o.board.view = View { center: view.center, zoom: view.zoom.clamp(0.02, 50.0) };
        store.save(&o.board)
    }

    pub fn undo(&mut self, id: &str) -> Result<bool> {
        self.step(id, true)
    }

    pub fn redo(&mut self, id: &str) -> Result<bool> {
        self.step(id, false)
    }

    fn step(&mut self, id: &str, back: bool) -> Result<bool> {
        let store = self.store.clone();
        let o = self.open_board(id)?;
        let Some(mut target) = (if back { o.undo.pop() } else { o.redo.pop() }) else { return Ok(false) };
        // Where the board is looked at isn't part of its history.
        target.view = o.board.view;
        let current = std::mem::replace(&mut o.board, target);
        if back {
            o.redo.push(current)
        } else {
            o.undo.push(current)
        }
        store.save(&o.board)?;
        Ok(true)
    }

    pub fn can_undo(&self, id: &str) -> bool {
        self.open.get(id).is_some_and(|o| !o.undo.is_empty())
    }

    pub fn can_redo(&self, id: &str) -> bool {
        self.open.get(id).is_some_and(|o| !o.redo.is_empty())
    }

    /// Put image `bytes` on board `id` at (x, y), embedded: the new item's id.
    pub fn add_image(&mut self, id: &str, bytes: &[u8], x: f64, y: f64, origin: &str) -> Result<u64> {
        let (asset, w, h) = self.store.import_image(id, bytes)?;
        let content =
            Content::Image { asset: Some(asset), link: None, pixel_width: w, pixel_height: h, origin: origin.into(), caption: String::new() };
        self.edit(id, Edit::Add(Item { id: 0, x, y, w: 0.0, h: 0.0, content }))?.ok_or_else(|| "the image wasn't added".into())
    }

    /// Put a link to the image file `path` on board `id` at (x, y): the new item's id. The file is
    /// read to check it is an image and to learn its size; the board keeps only its path.
    pub fn add_linked_image(&mut self, id: &str, path: &Path, x: f64, y: f64) -> Result<u64> {
        let meta = std::fs::metadata(path).map_err(|e| format!("can't open {}: {e}", path.display()))?;
        if meta.len() > MAX_IMAGE_BYTES as u64 {
            return Err(format!("images larger than {} MB can't go on a board", MAX_IMAGE_BYTES >> 20));
        }
        let bytes = std::fs::read(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
        let (_, w, h) = probe_image(&bytes)?;
        let link = path.to_string_lossy().to_string();
        let content = Content::Image { asset: None, link: Some(link.clone()), pixel_width: w, pixel_height: h, origin: link, caption: String::new() };
        self.edit(id, Edit::Add(Item { id: 0, x, y, w: 0.0, h: 0.0, content }))?.ok_or_else(|| "the image wasn't added".into())
    }

    /// The bytes of image item `item` on board `id`.
    pub fn image_bytes(&mut self, id: &str, item: u64) -> Result<Vec<u8>> {
        let store = self.store.clone();
        let o = self.open_board(id)?;
        let it = o.board.item(item).ok_or_else(|| format!("no item {item} on the board"))?;
        store.image_bytes(id, &it.content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("dlightroom-boards-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
        let mut out = std::io::Cursor::new(vec![]);
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn note(text: &str) -> Item {
        Item { id: 0, x: 10.0, y: 20.0, w: 0.0, h: 0.0, content: Content::Note { text: text.into(), color: "#ffe066".into() } }
    }

    #[test]
    fn boards_are_created_edited_saved_and_listed() {
        let mut boards = Boards::new(Store::new(scratch()));
        let b = boards.create("Spring campaign").unwrap();
        let n = boards.edit(&b.id, Edit::Add(note("Warm, hand-drawn"))).unwrap().unwrap();
        let c = boards
            .edit(
                &b.id,
                Edit::Add(Item { id: 0, x: 300.0, y: 20.0, w: 0.0, h: 0.0, content: Content::Color { hex: "F60".into(), name: "Tangerine".into() } }),
            )
            .unwrap()
            .unwrap();
        boards.edit(&b.id, Edit::Place(vec![(n, Some(50.0), None, Some(260.0), None)])).unwrap();
        let fresh = Boards::new(Store::new(boards.store().dir())).store().load(&b.id).unwrap();
        assert_eq!(fresh.items.len(), 2);
        assert_eq!((fresh.items[0].x, fresh.items[0].w), (50.0, 260.0));
        assert_eq!(fresh.palette(), vec![("#ff6600".to_string(), "Tangerine".to_string())]);
        assert_eq!(fresh.item(c).map(|i| i.content.kind()), Some("color"));
        let list = boards.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].name.as_str(), list[0].items), ("Spring campaign", 2));
    }

    #[test]
    fn undo_and_redo_walk_the_history_and_keep_the_view() {
        let mut boards = Boards::new(Store::new(scratch()));
        let id = boards.create("B").unwrap().id;
        let a = boards.edit(&id, Edit::Add(note("one"))).unwrap().unwrap();
        boards.edit(&id, Edit::Add(note("two"))).unwrap();
        boards.edit(&id, Edit::Remove(vec![a])).unwrap();
        boards.set_view(&id, View { center: [5.0, 6.0], zoom: 2.0 }).unwrap();
        assert_eq!(boards.board(&id).unwrap().items.len(), 1);
        assert!(boards.undo(&id).unwrap());
        assert_eq!(boards.board(&id).unwrap().items.len(), 2);
        assert_eq!(boards.board(&id).unwrap().view.zoom, 2.0, "the view isn't undone");
        assert!(boards.undo(&id).unwrap() && boards.undo(&id).unwrap());
        assert!(boards.board(&id).unwrap().items.is_empty());
        assert!(!boards.undo(&id).unwrap(), "nothing more to undo");
        assert!(boards.redo(&id).unwrap());
        assert_eq!(boards.board(&id).unwrap().items.len(), 1);
        // A new edit drops the redo steps; undo is saved to disk too.
        boards.edit(&id, Edit::Add(note("three"))).unwrap();
        assert!(!boards.can_redo(&id));
        assert_eq!(boards.store().load(&id).unwrap().items.len(), 2);
    }

    #[test]
    fn images_are_embedded_once_or_linked_and_read_back() {
        let dir = scratch();
        let mut boards = Boards::new(Store::new(dir.join("boards")));
        let id = boards.create("Moods").unwrap().id;
        let bytes = png(640, 320, [200, 10, 10]);
        let a = boards.add_image(&id, &bytes, 0.0, 0.0, "pasted").unwrap();
        let b = boards.add_image(&id, &bytes, 400.0, 0.0, "pasted").unwrap();
        let board = boards.board(&id).unwrap().clone();
        let (Content::Image { asset: Some(x), .. }, Content::Image { asset: Some(y), .. }) =
            (&board.item(a).unwrap().content, &board.item(b).unwrap().content)
        else {
            panic!("embedded")
        };
        assert_eq!(x, y, "the same content is stored once");
        assert_eq!(std::fs::read_dir(dir.join("boards").join(&id).join("assets")).unwrap().count(), 1);
        let it = board.item(a).unwrap();
        assert_eq!((it.w, it.h), (320.0, 160.0), "fitted to 320 points, aspect kept");
        assert_eq!(boards.image_bytes(&id, a).unwrap(), bytes);
        // A linked image keeps only its path; a missing file is reported, not a panic.
        let file = dir.join("ref.png");
        std::fs::write(&file, png(10, 20, [0, 0, 255])).unwrap();
        let l = boards.add_linked_image(&id, &file, 0.0, 300.0).unwrap();
        assert_eq!(std::fs::read_dir(dir.join("boards").join(&id).join("assets")).unwrap().count(), 1, "nothing copied");
        assert_eq!(boards.image_bytes(&id, l).unwrap().len(), std::fs::read(&file).unwrap().len());
        std::fs::remove_file(&file).unwrap();
        assert!(boards.image_bytes(&id, l).unwrap_err().contains("missing"));
        assert!(boards.add_image(&id, b"not an image", 0.0, 0.0, "").is_err());
    }

    #[test]
    fn arrange_duplicate_and_bad_edits() {
        let mut boards = Boards::new(Store::new(scratch()));
        let id = boards.create("B").unwrap().id;
        let a = boards.edit(&id, Edit::Add(note("a"))).unwrap().unwrap();
        let b = boards.edit(&id, Edit::Add(note("b"))).unwrap().unwrap();
        boards.edit(&id, Edit::Arrange(vec![a], true)).unwrap();
        assert_eq!(boards.board(&id).unwrap().items.iter().map(|i| i.id).collect::<Vec<_>>(), vec![b, a]);
        boards.edit(&id, Edit::Duplicate(vec![a], 20.0, 20.0)).unwrap();
        assert_eq!(boards.board(&id).unwrap().items.len(), 3);
        let before = boards.board(&id).unwrap().clone();
        for bad in [
            Edit::Add(Item { id: 0, x: f64::NAN, y: 0.0, w: 0.0, h: 0.0, content: Content::Note { text: "x".into(), color: "#fff".into() } }),
            Edit::Add(Item { id: 0, x: 0.0, y: 0.0, w: 0.0, h: 0.0, content: Content::Color { hex: "red".into(), name: String::new() } }),
            Edit::Remove(vec![999]),
            Edit::SetContent(a, Content::Color { hex: "#000".into(), name: String::new() }),
            Edit::Rename("   ".into()),
            Edit::Nudge(vec![a], f64::INFINITY, 0.0),
        ] {
            assert!(boards.edit(&id, bad.clone()).is_err(), "{bad:?}");
        }
        assert_eq!(boards.board(&id).unwrap(), &before, "a refused edit changes nothing");
        // A batch is one undo step, and a failing part refuses all of it.
        boards.edit(&id, Edit::Batch(vec![Edit::Nudge(vec![a], 5.0, 0.0), Edit::Nudge(vec![b], 0.0, 5.0)])).unwrap();
        assert!(boards.edit(&id, Edit::Batch(vec![Edit::Nudge(vec![a], 5.0, 0.0), Edit::Remove(vec![999])])).is_err());
        assert!(boards.undo(&id).unwrap());
        assert_eq!(boards.board(&id).unwrap(), &before);
    }

    #[test]
    fn hostile_board_files_are_refused_or_tidied() {
        assert!(Board::from_json("{}").is_err());
        assert!(Board::from_json(r#"{"format":"other","version":1,"id":"x","name":"n"}"#).is_err());
        assert!(Board::from_json(r#"{"format":"dlightroom-board","version":99,"id":"x","name":"n"}"#).is_err());
        assert!(Board::from_json(r#"{"format":"dlightroom-board","version":1,"id":"../etc","name":"n"}"#).is_err());
        assert!(Board::from_json(r#"{"format":"dlightroom-board","version":1,"id":"x","name":"n","view":{"center":[1e400,0],"zoom":1}}"#).is_err());
        let b = Board::from_json(
            r##"{"format":"dlightroom-board","version":1,"id":"ok","name":"n","view":{"center":[3,4],"zoom":-3},
            "items":[
              {"id":1,"x":0,"y":0,"w":10,"h":10,"kind":"note","text":"hi","color":"nope"},
              {"id":1,"x":0,"y":0,"w":10,"h":10,"kind":"note","text":"dup","color":"#000"},
              {"id":2,"x":0,"y":0,"w":-5,"h":10,"kind":"color","hex":"#000"},
              {"id":3,"x":0,"y":0,"w":10,"h":10,"kind":"image","asset":"../../secret.png","pixelWidth":1,"pixelHeight":1}
            ]}"##,
        )
        .unwrap();
        assert_eq!(b.items.len(), 1, "duplicates, empty boxes and unsafe asset names are dropped");
        assert!(matches!(&b.items[0].content, Content::Note { color, .. } if color == "#fff3a6"));
        assert_eq!((b.view.zoom, b.next_id), (1.0, 2));
        let store = Store::new(scratch());
        assert!(store.load("../x").is_err());
        assert!(store.asset_path("ok", "../../etc/passwd").is_err());
    }

    #[test]
    fn duplicate_and_delete_take_the_assets_along() {
        let dir = scratch();
        let mut boards = Boards::new(Store::new(&dir));
        let id = boards.create("Original").unwrap().id;
        boards.add_image(&id, &png(4, 4, [1, 2, 3]), 0.0, 0.0, "").unwrap();
        let copy = boards.duplicate(&id, "Copy").unwrap();
        assert_ne!(copy.id, id);
        assert_eq!(std::fs::read_dir(dir.join(&copy.id).join("assets")).unwrap().count(), 1);
        boards.delete(&id).unwrap();
        assert!(!dir.join(&id).exists());
        assert_eq!(boards.list().unwrap().len(), 1);
        assert!(boards.delete("not-a-board").is_err());
    }

    #[test]
    fn colours_normalize() {
        assert_eq!(normalize_hex("#ABC").as_deref(), Some("#aabbcc"));
        assert_eq!(normalize_hex("ff8800").as_deref(), Some("#ff8800"));
        for bad in ["", "#12", "#gggggg", "#ﬀ8800", "red"] {
            assert_eq!(normalize_hex(bad), None, "{bad}");
        }
    }
}
