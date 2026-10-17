//! Google Drive as an export destination: OAuth with the `drive.file` scope (DLightroom sees only
//! the files it creates), a folder for the export, and resumable uploads that survive dropped
//! connections, with retries, cancelling and progress.
//!
//! The user brings their own OAuth client ("Desktop app" type, from the Google Cloud console):
//! DLightroom ships no client id or secret. See `docs/cloud-export.md`.

use std::sync::Arc;
use std::time::Duration;

use crate::http::{Request, Response, Transport, encode};
use crate::job::{Cancel, Progress};
use crate::oauth::{OAuthClient, Tokens};
use crate::secrets::SecretStore;

pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const FILES_URL: &str = "https://www.googleapis.com/drive/v3/files";
const UPLOAD_URL: &str = "https://www.googleapis.com/upload/drive/v3/files";
/// Where the refresh token is kept in the secret store.
pub const REFRESH_KEY: &str = "google-drive-refresh-token";
/// Upload chunk: a multiple of 256 KiB, as Drive asks.
pub const CHUNK: usize = 8 * 256 * 1024;
/// Largest file uploaded.
pub const MAX_FILE: usize = 2 << 30;
/// Attempts per request before giving up.
pub const ATTEMPTS: u32 = 5;

/// The Google OAuth client for `client_id` (and the secret Google issues with Desktop clients).
pub fn google_client(client_id: &str, client_secret: Option<&str>) -> Result<OAuthClient, String> {
    let id = client_id.trim();
    if id.is_empty() {
        return Err("no Google OAuth client id is set up (see Preferences › Cloud)".into());
    }
    Ok(OAuthClient {
        client_id: id.to_string(),
        client_secret: client_secret.map(str::trim).filter(|s| !s.is_empty()).map(String::from),
        auth_url: AUTH_URL.into(),
        token_url: TOKEN_URL.into(),
        scope: SCOPE.into(),
    })
}

/// Whether Drive is connected: a refresh token is stored.
pub fn connected(store: &dyn SecretStore) -> bool {
    matches!(store.get(REFRESH_KEY), Ok(Some(t)) if !t.is_empty())
}

/// Forget the stored sign-in.
pub fn disconnect(store: &dyn SecretStore) -> Result<(), String> {
    store.delete(REFRESH_KEY)
}

/// A file to upload.
#[derive(Clone, Debug, PartialEq)]
pub struct Upload {
    pub name: String,
    pub mime: String,
    pub data: Vec<u8>,
}

/// The MIME type for an export file name.
pub fn mime_of(name: &str) -> &'static str {
    match name.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("pdf") => "application/pdf",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

/// A signed-in Drive session.
pub struct Drive {
    transport: Arc<dyn Transport>,
    client: OAuthClient,
    tokens: Option<Tokens>,
    refresh_token: String,
    /// Wait before retry `n` (1-based); tests make it zero.
    pub backoff: fn(u32) -> Duration,
}

fn default_backoff(n: u32) -> Duration {
    Duration::from_millis(500u64.saturating_mul(1 << n.min(6)))
}

/// Whether an answer is worth retrying: rate limits and server errors.
fn transient(r: &Response) -> bool {
    r.status == 429 || r.status == 408 || r.status >= 500
}

impl Drive {
    /// With the refresh token stored after connecting.
    pub fn new(transport: Arc<dyn Transport>, client: OAuthClient, store: &dyn SecretStore) -> Result<Self, String> {
        let refresh_token = store.get(REFRESH_KEY)?.filter(|t| !t.is_empty()).ok_or("Google Drive isn't connected")?;
        Ok(Self { transport, client, tokens: None, refresh_token, backoff: default_backoff })
    }

    fn access_token(&mut self) -> Result<String, String> {
        if let Some(t) = self.tokens.as_ref().filter(|t| t.fresh()) {
            return Ok(t.access_token.clone());
        }
        let t =
            self.client.refresh(self.transport.as_ref(), &self.refresh_token).map_err(|e| format!("Google Drive sign-in: {e} (connect again)"))?;
        let at = t.access_token.clone();
        self.tokens = Some(t);
        Ok(at)
    }

    /// Send `make(token)` with retries on network errors and transient answers; a 401 refreshes
    /// the access token once.
    fn send(&mut self, cancel: &Cancel, make: &dyn Fn(&str) -> Request) -> Result<Response, String> {
        let mut refreshed = false;
        let mut last = String::new();
        for attempt in 1..=ATTEMPTS {
            cancel.check()?;
            let token = self.access_token()?;
            match self.transport.send(&make(&token)) {
                Ok(r) if r.status == 401 && !refreshed => {
                    refreshed = true;
                    self.tokens = None;
                    last = r.error_message();
                    continue;
                }
                Ok(r) if transient(&r) => last = r.error_message(),
                Ok(r) => return Ok(r),
                Err(e) => last = e,
            }
            if attempt < ATTEMPTS {
                std::thread::sleep((self.backoff)(attempt));
            }
        }
        Err(format!("Google Drive didn't answer after {ATTEMPTS} attempts ({last})"))
    }

    /// The id of the folder `name` (in My Drive's root) that DLightroom created, creating it if
    /// there is none.
    pub fn ensure_folder(&mut self, name: &str, cancel: &Cancel) -> Result<String, String> {
        let escaped = name.replace('\\', "\\\\").replace('\'', "\\'");
        let q = format!("name = '{escaped}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false and 'root' in parents");
        let url = format!("{FILES_URL}?q={}&fields=files(id)&spaces=drive", encode(&q));
        let r = self.send(cancel, &|t| Request::new("GET", url.clone()).header("Authorization", format!("Bearer {t}")))?;
        if !r.ok() {
            return Err(format!("Google Drive: {}", r.error_message()));
        }
        if let Some(id) = r.json()?.pointer("/files/0/id").and_then(|i| i.as_str()) {
            return Ok(id.to_string());
        }
        let meta = serde_json::json!({"name": name, "mimeType": "application/vnd.google-apps.folder"}).to_string().into_bytes();
        let r = self.send(cancel, &|t| {
            Request::new("POST", format!("{FILES_URL}?fields=id"))
                .header("Authorization", format!("Bearer {t}"))
                .header("Content-Type", "application/json; charset=UTF-8")
                .body(meta.clone())
        })?;
        if !r.ok() {
            return Err(format!("Google Drive couldn't make the folder: {}", r.error_message()));
        }
        r.json()?.get("id").and_then(|i| i.as_str()).map(String::from).ok_or_else(|| "Google Drive's answer has no folder id".to_string())
    }

    /// Upload `file` into `folder` with a resumable session: its Drive file id.
    pub fn upload(&mut self, folder: &str, file: &Upload, cancel: &Cancel, progress: &dyn Fn(u64, u64)) -> Result<String, String> {
        if file.data.len() > MAX_FILE {
            return Err(format!("{} is too large to upload", file.name));
        }
        let total = file.data.len() as u64;
        let meta = serde_json::json!({"name": file.name, "parents": [folder]}).to_string().into_bytes();
        let r = self.send(cancel, &|t| {
            Request::new("POST", format!("{UPLOAD_URL}?uploadType=resumable&fields=id"))
                .header("Authorization", format!("Bearer {t}"))
                .header("Content-Type", "application/json; charset=UTF-8")
                .header("X-Upload-Content-Type", file.mime.clone())
                .header("X-Upload-Content-Length", total.to_string())
                .body(meta.clone())
        })?;
        if !r.ok() {
            return Err(format!("Google Drive refused {}: {}", file.name, r.error_message()));
        }
        let session = r.header("Location").filter(|l| l.starts_with("https://")).ok_or("Google Drive didn't open an upload session")?.to_string();
        let mut sent: u64 = 0;
        let mut stalls = 0;
        progress(0, total);
        loop {
            cancel.check()?;
            let start = usize::try_from(sent).map_err(|_| "upload offset out of range")?;
            let end = (start + CHUNK).min(file.data.len());
            let chunk = file.data.get(start..end).ok_or("upload offset out of range")?.to_vec();
            let range = if total == 0 { "bytes */0".to_string() } else { format!("bytes {start}-{}/{total}", end.saturating_sub(1)) };
            let r = self.send(cancel, &|t| {
                Request::new("PUT", session.clone())
                    .header("Authorization", format!("Bearer {t}"))
                    .header("Content-Range", range.clone())
                    .body(chunk.clone())
            })?;
            match r.status {
                200 | 201 => {
                    progress(total, total);
                    return r
                        .json()?
                        .get("id")
                        .and_then(|i| i.as_str())
                        .map(String::from)
                        .ok_or_else(|| "Google Drive's answer has no file id".to_string());
                }
                308 => {
                    // "Range: bytes=0-N" says what arrived; none means nothing yet.
                    let next = match r.header("Range").and_then(|h| h.rsplit('-').next()).and_then(|n| n.trim().parse::<u64>().ok()) {
                        Some(last) => last + 1,
                        None => 0,
                    };
                    if next > total {
                        return Err("Google Drive lost the upload; export again".into());
                    }
                    stalls = if next > sent { 0 } else { stalls + 1 };
                    if stalls >= ATTEMPTS {
                        return Err(format!("Google Drive isn't taking {}; export again", file.name));
                    }
                    sent = next;
                    progress(sent, total);
                }
                404 | 410 => return Err("the upload session expired; export again".into()),
                _ => return Err(format!("Google Drive refused {}: {}", file.name, r.error_message())),
            }
        }
    }

    /// Upload every file into the folder `folder_name`: (uploaded names, failures). A failure of
    /// one file doesn't stop the others; cancelling does.
    pub fn upload_all(
        &mut self,
        folder_name: &str,
        files: &[Upload],
        cancel: &Cancel,
        progress: &Progress,
    ) -> Result<(Vec<String>, Vec<String>), String> {
        let folder = self.ensure_folder(folder_name, cancel)?;
        let total: u64 = files.iter().map(|f| f.data.len() as u64).sum::<u64>().max(1);
        let mut done_bytes = 0u64;
        let (mut ok, mut failed) = (vec![], vec![]);
        for f in files {
            cancel.check()?;
            progress.set(done_bytes as f32 / total as f32, format!("Uploading {}", f.name));
            let base = done_bytes;
            match self.upload(&folder, f, cancel, &|s, _| progress.set((base + s) as f32 / total as f32, format!("Uploading {}", f.name))) {
                Ok(_) => ok.push(f.name.clone()),
                Err(e) if e == crate::job::CANCELLED => return Err(e),
                Err(e) => failed.push(format!("{}: {e}", f.name)),
            }
            done_bytes += f.data.len() as u64;
        }
        progress.set(1.0, "Uploaded");
        Ok((ok, failed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::MockTransport;
    use crate::secrets::MemoryStore;

    fn token() -> Result<Response, String> {
        Ok(Response::new(200, r#"{"access_token":"AT","expires_in":3600}"#))
    }

    fn drive(answers: Vec<Result<Response, String>>) -> (Drive, MockTransport) {
        let store = MemoryStore::default();
        store.set(REFRESH_KEY, "RT").unwrap();
        let t = MockTransport::new(answers);
        let mut d = Drive::new(Arc::new(t.clone()), google_client("id.apps", Some("sec")).unwrap(), &store).unwrap();
        d.backoff = |_| Duration::ZERO;
        (d, t)
    }

    #[test]
    fn not_connected_without_a_stored_token_and_no_client_without_an_id() {
        let store = MemoryStore::default();
        assert!(!connected(&store));
        assert!(Drive::new(Arc::new(MockTransport::default()), google_client("x", None).unwrap(), &store).is_err());
        assert!(google_client("  ", None).unwrap_err().contains("client id"));
        store.set(REFRESH_KEY, "RT").unwrap();
        assert!(connected(&store));
        disconnect(&store).unwrap();
        assert!(!connected(&store));
    }

    #[test]
    fn folders_are_found_or_made() {
        let (mut d, t) = drive(vec![
            token(),
            Ok(Response::new(200, r#"{"files":[{"id":"F1"}]}"#)),
            Ok(Response::new(200, r#"{"files":[]}"#)),
            Ok(Response::new(200, r#"{"id":"F2"}"#)),
        ]);
        assert_eq!(d.ensure_folder("It's art", &Cancel::default()).unwrap(), "F1");
        let q = &t.sent()[1].url;
        assert!(q.contains("It%5C%27s%20art"), "{q}");
        assert_eq!(t.sent()[1].header_value("Authorization"), Some("Bearer AT"));
        assert_eq!(d.ensure_folder("New", &Cancel::default()).unwrap(), "F2");
        assert_eq!(t.sent().len(), 4, "the access token is reused while fresh");
    }

    #[test]
    fn uploads_resume_in_chunks_and_retry_transient_failures() {
        let data = vec![7u8; CHUNK + 10];
        let (mut d, t) = drive(vec![
            token(),
            Ok(Response::new(200, "").with_header("Location", "https://upload.example/session1")),
            Err("connection reset".into()),
            Ok(Response::new(308, "").with_header("Range", &format!("bytes=0-{}", CHUNK - 1))),
            Ok(Response::new(503, "busy")),
            Ok(Response::new(201, r#"{"id":"FILE"}"#)),
        ]);
        let seen = std::sync::Mutex::new(vec![]);
        let id = d
            .upload("F", &Upload { name: "a.png".into(), mime: "image/png".into(), data: data.clone() }, &Cancel::default(), &|s, t| {
                seen.lock().unwrap().push((s, t))
            })
            .unwrap();
        assert_eq!(id, "FILE");
        let sent = t.sent();
        assert_eq!(sent[1].header_value("X-Upload-Content-Length"), Some(&*data.len().to_string()));
        assert_eq!(sent[3].header_value("Content-Range"), Some(&*format!("bytes 0-{}/{}", CHUNK - 1, data.len())));
        assert_eq!(sent[5].header_value("Content-Range"), Some(&*format!("bytes {CHUNK}-{}/{}", data.len() - 1, data.len())));
        assert_eq!(sent[5].body.len(), 10);
        assert_eq!(seen.lock().unwrap().last(), Some(&(data.len() as u64, data.len() as u64)));
    }

    #[test]
    fn expired_access_tokens_are_refreshed_once_and_failures_reported() {
        let (mut d, t) = drive(vec![
            token(),
            Ok(Response::new(401, r#"{"error":{"message":"Invalid Credentials"}}"#)),
            token(),
            Ok(Response::new(200, r#"{"files":[{"id":"F"}]}"#)),
        ]);
        assert_eq!(d.ensure_folder("x", &Cancel::default()).unwrap(), "F");
        assert_eq!(t.sent().len(), 4);
        let (mut d, _) = drive(vec![
            token(),
            Ok(Response::new(500, "")),
            Ok(Response::new(500, "")),
            Ok(Response::new(500, "")),
            Ok(Response::new(500, "")),
            Ok(Response::new(500, "")),
        ]);
        assert!(d.ensure_folder("x", &Cancel::default()).unwrap_err().contains("after 5 attempts"));
        let (mut d, _) = drive(vec![Ok(Response::new(400, r#"{"error":"invalid_grant"}"#))]);
        assert!(d.ensure_folder("x", &Cancel::default()).unwrap_err().contains("connect again"));
    }

    #[test]
    fn upload_all_keeps_going_past_a_failed_file_and_stops_when_cancelled() {
        let (mut d, _) = drive(vec![
            token(),
            Ok(Response::new(200, r#"{"files":[{"id":"F"}]}"#)),
            Ok(Response::new(403, r#"{"error":{"message":"The user's Drive storage quota has been exceeded."}}"#)),
            Ok(Response::new(200, "").with_header("Location", "https://upload.example/s")),
            Ok(Response::new(200, r#"{"id":"B"}"#)),
        ]);
        let files = [
            Upload { name: "a.svg".into(), mime: mime_of("a.svg").into(), data: b"<svg/>".to_vec() },
            Upload { name: "b.pdf".into(), mime: mime_of("b.pdf").into(), data: b"%PDF".to_vec() },
        ];
        let p = Progress::default();
        let (ok, failed) = d.upload_all("Exports", &files, &Cancel::default(), &p).unwrap();
        assert_eq!(ok, vec!["b.pdf"]);
        assert!(failed[0].starts_with("a.svg: ") && failed[0].contains("quota"));
        assert_eq!(p.get().0, 1.0);
        let cancel = Cancel::default();
        cancel.cancel();
        let (mut d, _) = drive(vec![]);
        assert_eq!(d.upload_all("Exports", &files, &cancel, &p).unwrap_err(), crate::job::CANCELLED);
    }

    #[test]
    fn mime_types() {
        assert_eq!(mime_of("x.SVG"), "image/svg+xml");
        assert_eq!(mime_of("board-1@2x.png"), "image/png");
        assert_eq!(mime_of("noext"), "application/octet-stream");
    }
}
