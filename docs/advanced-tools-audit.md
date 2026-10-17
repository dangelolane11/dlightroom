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

**This milestone:** a Google Drive destination for Export for Screens, built on a small cloud
layer (`crates/cloud`: an HTTP `Transport` that tests replace with a scripted mock, background jobs
with progress and cancelling, keychain secret storage). It signs in with OAuth 2.0 for installed
apps (system browser, PKCE S256, loopback redirect on 127.0.0.1, `drive.file` scope), keeps the
refresh token in the system keychain, and uploads with resumable sessions, retries with backoff
on network errors, 429 and 5xx, a token refresh on 401, cancellation and per-file failures
reported. The local export always happens first; the upload happens only when **Also Upload to
Google Drive** is ticked (or `cloud.drive.upload` is run). It needs a Google OAuth client of the
user's own (see [Cloud export](cloud-export.md)); without one Cloud and AI Settings shows
"Disconnected" and Connect stays disabled.

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

### Automated tests

The tests that back this audit; the results from the milestone's run are in the PR description.

- `crates/engine/tests/advanced_tools.rs`: one end-to-end test per area. Each builds art, edits it,
  undoes and redoes, saves, reopens and exports (SVG, PDF, PNG pixels). The file also builds the
  sample documents in `examples/advanced/`.
- `crates/engine/src/cmd/globaledit.rs`, `boards.rs`, `cloud.rs` and `tests_brushsym.rs`: command
  tests for Global Edit, boards, cloud and AI jobs (against a scripted transport and an in-memory
  keychain), and symbol overrides.
- `crates/boards`: storage, undo, limits and refusals of bad input. `crates/cloud`: HTTP, PKCE, the
  loopback sign-in listener, token exchange and refresh, Drive folders and resumable uploads with
  retries, AI variations, jobs and cancelling. All offline.
- `crates/ui-egui`: headless frame tests of the Global Edit, Blend Options, Cloud and AI Settings
  and Inspiration Boards UI.

### Manual acceptance checklist

Run these on the macOS Intel build (the `DLightroom.app` artifact of the `macos-intel` workflow).
Items marked **(account)** need your own provider key or Google OAuth client.

1. **Curvature (Shift+~):** click four points, one with Alt for a corner, then press Esc. Drag a
   point: only its neighbours re-curve. Turn on Re-curve Neighbours in the Control bar and drag
   again. Double-click a point to toggle smooth/corner. Undo, redo, save, reopen.
2. **Width (Shift+W):** drag a stroke's edge to add a width point, Alt-drag one side,
   double-click to edit the point numerically. Save the profile from the Control bar and apply it
   to another path. Object › Expand Appearance.
3. **Perspective Grid (Shift+P):** switch the 1-, 2- and 3-point grids, move the vanishing points,
   draw a rectangle on a plane, move it with Perspective Selection (Shift+V), release it from
   perspective.
4. **Gradient Mesh (U):** click a filled shape to add mesh lines, colour a point, move a point
   and a handle. Object › Create Gradient Mesh with 3 × 3, then export to PNG.
5. **Blend (W):** click two shapes and get a blend. Object › Blend › Blend Options: Specified Steps
   and Distance, Ease In / Ease Out / Color Acceleration with Preview. Replace Spine, Reverse,
   Expand, Release.
6. **Symbols:** make a symbol from art and place instances on two artboards. Double-click an
   instance to edit the master: every instance updates. Scale and tint one instance, then Reset
   Instance Overrides. Break Link, Replace, Rename and Delete Symbol.
   Undo each step.
7. **Inspiration Boards:** open Window › Inspiration Boards and create a board. Drop images from
   the Finder, add a note, a colour and a link. Move, resize, zoom (Cmd+scroll), pan, undo and
   redo (Cmd+Z, Shift+Cmd+Z). Quit and relaunch: the board and its images are back. Send to
   Document puts images, type and swatches in the document. Add the Document's Selection works.
   **(account)** AI Variations:
   - With no key, the inspector says it isn't set up, and boards still work fully.
   - With your key in Edit › Cloud and AI Settings, Generate shows progress and the results land
     beside the image, marked AI. Cancel stops it. A wrong key shows the provider's error.
   - The key is in Keychain Access (service "DLightroom"), not in `cloud.json`.
8. **Artboards and batch export:** add, duplicate, rename, reorder and delete artboards. In File ›
   Export › Export for Screens, pick two artboards, add SVG, PNG 2x and PDF rows, set a naming
   template, and export: the files appear in the folder.
   **(account)** Google Drive:
   - Edit › Cloud and AI Settings shows Disconnected.
   - Enter your Desktop OAuth client and click Connect…. Allow access in the browser: the status
     reads Connected.
   - Export with Also Upload to Google Drive ticked: Background Tasks shows progress, and the files
     appear in the Drive folder.
   - Cancel works mid-upload. Disconnect works.
9. **Global Edit:** select a shape that has look-alikes, then Select › Start Global Edit. Choose
   the attributes, then Find Matches (count and selection preview). Set fill and stroke weight,
   then Apply. Edit › Undo undoes it all in one step. Hidden and locked matches are left alone
   unless their checkboxes are on.

### Blocked without an account

- **AI Variations against a real service:** needs a provider API key. Verified only against a
  scripted transport in tests.
- **Google Drive sign-in and upload:** needs a Google OAuth client and a Google account. Verified
  only against a scripted transport and a local loopback listener in tests.
