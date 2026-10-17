# Inspiration Boards

Status: shipped in the advanced-tools milestone (2026-10). Revision history at the end.

Inspiration Boards are DLightroom's own moodboards: a workspace next to your documents where you collect images, notes, colours and links, arrange them freely, and send what you picked into a vector document. They are an original DLightroom feature, not a copy of any other product's boards, and they work fully offline without any account.

## Opening the workspace

**Window › Inspiration Boards** shows the workspace in place of the canvas (run it again, or click **‹ Back to Document**, to return). It has three parts:

- **Left: the boards.** New Board, Rename…, Duplicate and Delete Board… (asks first, can't be undone). The number after each name is how many items the board holds.
- **Middle: the board.** The toolbar has Add Images…, Add the Document's Selection, Undo and Redo (for board changes), Zoom Out, Zoom In, Fit Board in Window, Send to Document, and Remove the Chosen Items.
- **Right: the inspector.** It adds notes, colours and links, edits the chosen item, offers AI Variations for an image, and lists background tasks.

## Working on a board

| Action | How |
|---|---|
| Add images | Drag image files from the Finder or Explorer onto the window, or use **Add Images…**. PNG, JPEG, GIF, WebP, TIFF and BMP up to 64 MB each. |
| Add the document's art | Select art in the active document, then **Add the Document's Selection**. It lands on the board as a PNG at the art's size. |
| Add a note, colour or link | Use the inspector's **Add to Board** section. |
| Choose items | Click an item. Shift-click adds or removes one. Click empty board to choose nothing. |
| Move | Drag the chosen items. |
| Resize | Drag the square handle at the chosen item's bottom-right corner. |
| Pan | Drag empty board, or scroll. |
| Zoom | Ctrl/Cmd + scroll, or pinch, zooms about the pointer. You can also use the toolbar buttons or Fit Board in Window. The board remembers where you left it. |
| Edit an item | Change it in the inspector: a note's text and colour, a colour's value and name, a link's address, title and note (plus Open in Browser), or an image's caption. |
| Stack order | With several items chosen: Bring to Front / Send to Back. |
| Duplicate / remove | **Duplicate** in the inspector. **Remove the Chosen Items** in the toolbar, or Delete / Backspace. |
| Undo / redo | Cmd/Ctrl+Z and Shift+Cmd/Ctrl+Z (or Edit › Undo / Redo), or the toolbar's buttons: while the workspace is open they undo board changes. Each board has its own history (100 steps); the document's history is untouched. |
| Keys | Delete / Backspace removes the chosen items and Escape chooses none. While the workspace is open, the document's tool and editing shortcuts are off. |

## Into a vector document

**Send to Document** places the chosen items (with none chosen, the whole board) in the active document, laid out as on the board and centred on the first artboard:

- images become embedded images;
- notes and links become point type;
- colours become swatches, plus a row of filled squares.

This is one document undo step.

## Where boards are kept

Each board is a folder in your settings folder, in `Inspiration Boards/<board id>/`:

- `board.json` holds the board, with the format header `"format": "dlightroom-board", "version": 1`.
- `assets/` holds the embedded images, named by their SHA-256 hash, so the same picture is stored once.

Boards save as you change them, using atomic replace, so a crash can't leave half a file behind. Images you add by path are copied in (embedded). Through the commands you can keep a link to the file instead (`boards.addImage` with `link: true`); a linked image that has moved shows as "Missing image".

Runs without preferences (`VECTORCRAFT_NO_PREFS`, used by tests and agents) keep their boards in a temporary folder.

## AI Variations (optional)

An image on a board can be sent to an image-variation service of your choice. The results land beside it as new images. Each result's "From" line reads `ai:<host> (<model>)`, and it is tagged **AI** on the board.

- **Off until you set it up.** DLightroom includes no AI service and no key. In **Edit › Cloud and AI Settings…**, enter an OpenAI-compatible images endpoint (`https://…/v1`, or `http://localhost…` for a server on your own computer), a model (default `dall-e-2`), and your API key. The key goes into the system keychain (macOS Keychain, Windows Credential Manager, the Linux kernel keyring), never into a file.
- **Nothing is sent until you click Generate Variations.** The inspector says which host will receive the image. The image is centre-cropped to a square, scaled down to at most 1024 px and sent as a PNG under 4 MB.
- **Progress, cancel, retry.** The request runs as a background task. Its progress shows in the inspector's Background Tasks list, which offers Cancel while it runs, and Retry and Dismiss once it fails.
- **Real results only.** Every image added is what the service returned. Errors are reported as the service gave them (for example "HTTP 401: Incorrect API key provided"), and nothing is ever invented.
- All the variations from one request are one board undo step.

## Commands

The workspace drives these commands, which the control channel and MCP reach too (`vectorcraft-cli` lists their parameters):

- `boards.setFolder`, `boards.list`, `boards.create`, `boards.get`, `boards.rename`, `boards.delete`, `boards.duplicate`
- `boards.addImage`, `boards.addNote`, `boards.addColor`, `boards.addReference`, `boards.updateItem`, `boards.nudge`, `boards.removeItems`, `boards.arrange`, `boards.duplicateItems`
- `boards.setView`, `boards.undo`, `boards.redo`, `boards.toDocument`, `boards.captureSelection`
- `boards.variations`, `cloud.jobs`, `cloud.cancel`, `cloud.retry`, `cloud.dismiss`

## Limits

- Up to 5000 items per board and 64 MB per image.
- Image items are drawn from a copy at most 1024 px on a side, to keep the workspace responsive.
- The web build has no settings folder, so it shows the workspace's explanation instead of boards. AI Variations need the desktop app.

## Revision history

- 2026-10-17: first version.
