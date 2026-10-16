# DLightroom app icon

**Motif:** the letter D drawn as one editable vector path (a stem and a Bézier bowl), with the anchor
points and the tangent handle a pen tool shows, and a spark of light in the bowl: a vector editor
called DLightroom.

**Palette:**

| Colour | Hex | Used for |
|---|---|---|
| Amber | `#ffc24b` | gradient, top left |
| Coral | `#ff5d5d` | gradient, middle |
| Violet | `#7b3df0` | gradient, bottom right |
| Paper | `#fffaf2` | the letter, anchors and spark |
| Ink | `#2b1746` | anchor and handle outlines |

**Tile:** `viewBox="0 0 512 512"`, a rounded square with `rx=112`, clipped. The macOS files add Apple's
transparent margin (tile = 824/1024 of the canvas); Windows and Linux use the tile edge to edge.

**Provenance:** hand-written SVG made for DLightroom (`dlightroom.svg` is the canonical artwork). No
tracing, no third-party artwork, nothing from another product's icon. Licence: see `LICENSE.txt`.

## Files

| File | What | Used by |
|---|---|---|
| `dlightroom.svg` | master vector | source for everything below |
| `dlightroom-1024.png` | full tile, 1024 px | docs, README |
| `dlightroom-macos-512.png` | tile with Apple margin, 512 px | runtime Dock icon (`apps/vectorcraft/src/main.rs`) |
| `dlightroom.icns` | macOS icon set, 16 to 1024 px | `DLightroom.app` (`CFBundleIconFile`): `packaging/macos/build-app.sh`, `package.sh`, `cargo xtask bundle` |
| `dlightroom.ico` | Windows icon, 16 to 256 px | `apps/vectorcraft/build.rs` (embedded in the `.exe`) |
| `hicolor/<size>/apps/ai.storyteller.vectorcraft.png` | Linux theme icons, 16 to 512 px; the 256 one is also the runtime icon on Windows and Linux, the 128 one the brand mark in the app bar and About box (`crates/ui-egui/src/brand.rs`) | `packaging/linux/` |
| `hicolor/scalable/apps/ai.storyteller.vectorcraft.svg` | Linux scalable icon (a copy of `dlightroom.svg`) | as above |

The hicolor files keep the upstream Linux desktop-entry id (`ai.storyteller.vectorcraft`) in their
names, because the Linux packages and the app's Wayland app id still use it.

## Regenerate

Edit `dlightroom.svg`, then run `packaging/icons.sh` (needs `resvg`; the `.icns` comes from
`iconutil` on macOS or Pillow elsewhere). It writes every PNG, the `.ico` (via `cargo xtask ico`)
and the `.icns`.
