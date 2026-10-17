# Batch export by artboard and Google Drive

Status: shipped in the advanced-tools milestone (2026-10). Revision history at the end.

## Local batch export (always available)

**File › Export › Export for Screens** exports artboards, assets, layers or the selection, many files at once, with no account needed:

- **Artboards:** All, a range (`1-3, 5`) or Full Document, chosen with checkboxes and thumbnails, with Include Bleed.
- **Formats:** one row per output: PNG, PNG 8, JPG, WebP, GIF, TIFF, SVG, PDF and more. Each row has a scale (`1x`, `2x`, `100w`, `100h`, `72ppi`) and a suffix.
- **Naming templates:** `<name>`, `<filename>`, `####` counters, `YYYYMMDD` and more, with a live example of the result. Add a prefix, and choose folders: all in one, or a folder per row, per format, per item, or per item and format.
- **Presets:** the built-in presets (iOS, Android, …), plus your own saved presets.

The `document.exportForScreens` command does the same from the control channel, MCP and Actions. The end-to-end test `advanced_tools::artboards_manage_and_batch_export_svg_png_pdf` covers two artboards × SVG, PNG and PDF with a naming template.

## Upload to Google Drive (optional)

Export for Screens can also upload what it exports to a folder in your Google Drive. The local export always happens first: Drive gets copies of the files written to your folder.

### What you need

DLightroom ships no Google credentials. You use your own OAuth client, which takes about five minutes to set up once:

1. Go to <https://console.cloud.google.com/>. Create (or pick) a project.
2. Turn on the API: **APIs & Services › Library › Google Drive API › Enable**.
3. Configure the consent screen: **APIs & Services › OAuth consent screen**. Choose *External*, fill in the app name (for example "My DLightroom") and your email, and add yourself under **Test users**. While the app is in testing, only test users can sign in, which is what you want for personal use.
4. Create the client: **APIs & Services › Credentials › Create credentials › OAuth client ID**, with application type **Desktop app**. Google shows a *Client ID* and a *Client secret*.
5. In DLightroom, open **Edit › Cloud and AI Settings…**. Paste the Client ID and the Client secret, choose a Drive folder name (default "DLightroom Exports"), and click **Save**.

The client ID is saved in `cloud.json` in your settings folder. The client secret goes into the system keychain. For a Desktop client Google treats this secret as non-confidential, but it is still kept out of files.

### Connecting

In **Cloud and AI Settings**, the Google Drive status reads **● Disconnected** until you connect.

1. Click **Connect…**. Your browser opens Google's sign-in page.
2. Allow access. DLightroom asks only for the `drive.file` scope: it can see and change only the files it creates, not the rest of your Drive.
3. The browser shows "DLightroom is connected". The status turns to **● Connected**.

How it works: OAuth 2.0 for installed apps (RFC 8252) with PKCE (S256). The answer comes back to a one-time listener on `127.0.0.1` with a random port. The sign-in times out after 5 minutes and can be cancelled from the Background Tasks list. The refresh token Google returns is stored in the keychain. **Disconnect** removes it. To revoke access completely, also remove the app at <https://myaccount.google.com/permissions>.

### Uploading

In Export for Screens, under the folder, tick **Also Upload to Google Drive** (it's available only while Drive is connected) and export. Then:

- The files are written to the local folder as usual.
- A background task uploads them into the Drive folder, creating the folder if it doesn't exist. It uses resumable uploads in 2 MB chunks.
- The Background Tasks window (bottom right) shows the progress and lets you cancel. When the upload finishes it shows "Uploaded N file(s) to Google Drive › <folder>", or what failed.
- Network errors, rate limits (429) and server errors (5xx) are retried up to 5 times with increasing waits. An expired access token is refreshed once. An interrupted upload resumes from what Google confirmed.
- If one file fails (for example, your Drive is full), the others still upload, and the message lists the failure. **Retry** runs the whole upload again.

`cloud.drive.upload {paths, folder?}` does the same from the control channel and MCP.

### What isn't sent

Nothing goes to Google without one of these actions: **Connect…**, an export with **Also Upload to Google Drive** ticked, or the `cloud.drive.upload` command. DLightroom never uploads your documents on its own.

## Status without an account

- No client ID: Connect is greyed out, with "Enter your OAuth client ID first". Export for Screens shows "Google Drive: Disconnected" and a **Set Up…** button.
- Drive not connected: the upload checkbox is off and can't be ticked. `cloud.drive.upload` answers "Google Drive isn't connected".
- The web build: cloud features say they need the desktop app.

## For developers

- `crates/cloud` (`vectorcraft-cloud`) holds the network clients:
  - `http`: a `Transport` (ureq over rustls) and a scripted `MockTransport`;
  - `oauth`: PKCE and the loopback listener;
  - `drive`: folders, resumable uploads and retries;
  - `ai`: the variation provider interface;
  - `secrets`: keychain storage;
  - `job`: background jobs with progress and cancellation.
- Every client is tested offline against the mock and a local TCP listener. CI makes no network requests and needs no credentials.
- `crates/engine/src/cmd/cloud.rs` holds the commands: `cloud.status`, `cloud.setAiProvider`, `cloud.setGoogleClient`, `cloud.drive.connect`, `cloud.drive.disconnect`, `cloud.drive.upload`, `cloud.jobs`, `cloud.cancel`, `cloud.retry`, `cloud.dismiss`.
- **Not verified in CI:** a real sign-in and upload against Google. That needs your own OAuth client and a Google account; run through the manual checklist in `docs/advanced-tools-audit.md` once you have them.

## Revision history

- 2026-10-17: first version.
