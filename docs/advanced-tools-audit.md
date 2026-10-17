# Advanced tools audit

> **Status:** milestone "advanced tools" (one PR) · **Audited:** 2026-10-17 · **Evidence:**
> `crates/engine/tests/advanced_tools.rs` (end-to-end, engine level), the focused tests named
> below, and the manual acceptance checklist at the end.

DLightroom inherits a mature vector engine from VectorCraft. Before writing new code, each of the
nine requested areas was mapped against the code and exercised end to end through the same
commands the tools, panels and menus run: create, edit, undo/redo, native save and reopen, and
SVG/PDF/PNG export. This page records what already worked, what was missing, and what this
milestone changed.

Legend: **Works** = verified by an automated test in this milestone · **Gap** = missing or
incomplete when audited · **Added** = implemented in this milestone.

## 1. Curvature tool (Shift+~)

| Requirement | Before | Where |
|---|---|---|
| Smooth curves and straight segments, live rubber-band preview | Works | `crates/tools/src/draw2/curvature.rs`, `path.curvature` |
| Edit anchors on existing paths (move, insert, extend, close) | Works | `path.curvatureEdit` |
| Smooth ↔ corner conversion (double-click / Alt-click) | Works | `path.convertAnchor` |
| Undo/redo, selection, native persistence, SVG export | Works | `advanced_tools::curvature_*` |
| Dragging a point re-curves its neighbours too (#1236, open upstream) | **Gap** | the dragged point's two segments re-curved; its smooth neighbours kept stale handles |

**This milestone:** the Curvature tool has a **Re-curve Neighbours** option in the Control bar
(tool option `recurveNeighbours`, kept with the preferences). With it on, dragging a point of a
selected path also re-curves its smooth neighbours along the circle through them and their own
neighbours, as drawing them did (`path.curvatureEdit` `move` with `recurve: "neighbours"`). Corner
points and the rest of the path are untouched, and it stays off by default so paths drawn with the
Pen keep the shape the user gave them.

## 2. Variable width tool (Shift+W)

| Requirement | Before | Where |
|---|---|---|
| Draggable width points, add (click/double-click), delete, discontinuous points | Works | `crates/tools/src/distort/width.rs`, `stroke.widthPoint.*` |
| Width Point Edit dialog, adjoining adjustment | Works | `dialogs/width_point.rs` |
| Smooth interpolation, rendering | Works | renderer variable-width outline |
| Reusable profiles (save, apply, delete, reset) | Works | `stroke.widthProfile.*`, Stroke panel |
| Outline Stroke and SVG/PDF export follow the profile | Works | `advanced_tools::width_*` (outline height follows the 40 pt bulge, not the 10 pt weight) |
| Persistence, undo/redo | Works | same test |

No gap found; covered by the new end-to-end test.

## 3. Perspective grid (Shift+P)

| Requirement | Before | Where |
|---|---|---|
| One-, two- and three-point grids, presets (built-in, save, import/export) | Works | `perspective.grid.preset`, `perspective.presets.*`, `dialogs/perspective_*.rs` |
| Editable vanishing points, horizon, viewing angle and distance (widget and Define dialog) | Works | `crates/tools/src/distort/perspective/`, `perspective.grid.define` |
| Attach, draw, move, transform and nudge on planes; Perspective Selection (Shift+V) | Works | `perspective.attach/move/draw/transform/nudge` |
| Snap to grid, show/hide, lock grid and station point | Works | `perspective.grid.snap/show/lock*` |
| Grid and attached geometry survive save/reopen; export | Works | `advanced_tools::perspective_*` |

No gap found; covered by the new end-to-end test.

## 4. Gradient mesh (U)

| Requirement | Before | Where |
|---|---|---|
| Create Gradient Mesh (rows, columns, flat/center/edge) on filled paths | Works | `object.mesh.create` |
| Mesh tool: add rows/columns through a click, delete points, move points and handles | Works | `crates/tools/src/meshedit.rs`, `object.mesh.*` |
| Point colours and opacity | Works | `object.mesh.setPointColor` |
| Smooth bicubic colour interpolation, rendering | Works | PNG check in `advanced_tools::gradient_mesh_*` (red near the red point, blue away from it) |
| PDF/SVG export, save/reopen, undo/redo | Works | same test |

No gap found in the requested scope. (Upstream's open item, a mesh made from an embedded image
(#1676), is outside this milestone.)

## 5. Blend tool (W)

| Requirement | Before | Where |
|---|---|---|
| Blend two or more objects (geometry, fill colours, transforms) | Works | `object.blend.make`, Blend tool |
| Smooth colour / specified steps / specified distance, align to page or path | Works | `object.blend.options`, `dialogs/blend_options.rs` |
| Spine: replace, reverse, edit points | Works | `object.blend.replaceSpine/reverseSpine/spine.*` |
| Expand into ordinary objects; undo restores the live blend | Works | `advanced_tools::blends_*` |
| Save/reopen, export | Works | same test |
| Ease in/out of the steps' spacing, separate colour acceleration (30.7 Blend panel) | **Gap** | listed missing in `docs/gaps.md` (G13) |

**This milestone:** blends have **Ease In**, **Ease Out** and **Color Acceleration**
(`BlendSpec.easing`, written only when set, so older files and older versions still read them).
Blend Options shows them with a live preview; rendering, expansion, SVG/PDF export and the Blend
tool all go through the same eased step positions.

## 6. Symbols panel

| Requirement | Before | Where |
|---|---|---|
| New symbol from selection, place instances (panel, drag, Symbol Sprayer) | Works | `symbol.new/place`, `panels/symbols.rs` |
| Editing the master updates every linked instance, on every artboard | Works | `symbol.edit` + `symbol.update` (Redefine) |
| Per-instance transforms | Works | instance `xf`; kept relative across Redefine |
| Safe per-instance overrides | Partly | opacity, blend mode and a stain (tint) existed, but no way to see or clear them per instance |
| Break link, replace, rename, duplicate, delete, select instances | Works | Symbols panel menu, Symbol Options |
| Persistence and undo/redo | Works | `advanced_tools::symbols_*` |

**This milestone:** instance overrides are explicit: `symbol.instance.overrides` reports an
instance's own transform, tint, opacity and blend mode, and `symbol.instance.reset` clears the
chosen ones (one undo step), from the Symbols panel menu and the instance's Properties.

## 7. Inspiration Boards and AI variations

| Requirement | Before |
|---|---|
| Visual moodboard workspace, persistence, transfer to documents | **Gap** (nothing existed) |
| Provider-based AI image variations | **Gap** (`docs/gaps.md` G18 waits on a provider decision) |

**This milestone:** see [Inspiration Boards](inspiration-boards.md). Boards are an original
feature, fully functional with no account: images (embedded or linked), notes, colours and links
on a zoomable, pannable board, saved locally with their assets, and sent to the active document as
placed images, text and swatches. AI variations are optional, behind a provider interface with an
OpenAI-compatible image-variations provider configured by the user; requests run in the
background with progress, cancellation and error reporting. No credentials ship with the app and
no result is ever fabricated.

## 8. Multiple artboards and bulk (cloud) export

| Requirement | Before | Where |
|---|---|---|
| Add, duplicate, rename, resize, rearrange, reorder, delete, navigate artboards | Works | `artboard.*`, Artboards panel, Artboard tool, status-bar navigator |
| Batch export per artboard to SVG, PNG, PDF (and more), chosen artboards, name pattern, scales | Works | `document.exportForScreens`, Export for Screens dialog |
| Cloud destination (authenticated, resumable, cancellable) | **Gap** | nothing existed |

**This milestone:** a cloud-export adapter interface and a Google Drive destination: OAuth 2.0 for
installed apps (system browser, PKCE, loopback redirect), tokens in the macOS Keychain (or the
platform's secret store), uploads with retries, backoff and cancellation, and the connection
status shown in Export for Screens. Uploads happen only when the user picks the Google Drive
destination and presses Export. It needs a Google Cloud OAuth client of the user's own (see
[Cloud export](cloud-export.md)); without one the destination says so and stays disabled.

## 9. Global editing

| Requirement | Before |
|---|---|
| Start Global Edit | **Gap**: a disabled placeholder in Select |

**This milestone:** Select › Start Global Edit opens the Global Edit panel: choose the attributes to
match (shape type, fill, stroke colour, stroke weight, opacity, blend mode, appearance, graphic
style, symbol), search the active artboard or all artboards, preview the matches (count and
selection), then change fill, stroke, stroke weight, opacity, blend mode or graphic style on all of
them in one undo step. Hidden and locked objects are skipped unless asked for. Commands:
`select.globalEdit.find`, `select.globalEdit.apply`.

## Results

The test results and the manual acceptance checklist are recorded at the end of the milestone
below.
