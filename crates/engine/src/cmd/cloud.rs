//! Cloud and AI (DLightroom): the settings of the AI image-variation provider and of Google Drive,
//! the credentials (kept in the system keychain, never in a file), and the background jobs that
//! talk to them: AI variations of a board image (`boards.variations`), connecting Google Drive
//! (`cloud.drive.connect`) and uploading exported files to it (`cloud.drive.upload`).
//!
//! Nothing here runs unless the user asks: no request is made, and no file leaves the computer,
//! without one of these commands. DLightroom ships no API key and no OAuth client; without them
//! the commands say what to set up. Jobs run on threads; `cloud.jobs` reports their progress and
//! brings in what finished (variations go on their board as one board undo step), `cloud.cancel`
//! stops one and `cloud.retry` runs a failed one again.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vectorcraft_cloud::ai::{self, OpenAiCompatible, ProviderSettings, VariationProvider, VariationRequest};
use vectorcraft_cloud::drive::{self, Drive, Upload};
use vectorcraft_cloud::http::{Transport, host_of};
use vectorcraft_cloud::job::{CANCELLED, Job, Outcome};
use vectorcraft_cloud::oauth::{Loopback, Pkce};
use vectorcraft_cloud::secrets::SecretStore;

use super::*;

/// Keychain names.
pub const AI_KEY: &str = "ai-api-key";
pub const GOOGLE_SECRET: &str = "google-client-secret";
/// How long the browser has to finish signing in.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);
/// Finished jobs kept for `cloud.jobs`.
const KEEP_FINISHED: usize = 20;
/// Most bytes uploaded in one `cloud.drive.upload`.
const MAX_UPLOAD_TOTAL: u64 = 4 << 30;
/// The Drive folder when none is set.
pub const DEFAULT_FOLDER: &str = "DLightroom Exports";

/// The settings the user entered (secrets aside), saved in the app's settings folder.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CloudSettings {
    pub ai: ProviderSettings,
    pub google_client_id: String,
    pub drive_folder: String,
}

/// What a finished job brings back.
enum Done {
    Variations { board: String, source: u64, images: Vec<Vec<u8>>, origin: String },
    Connected,
    Uploaded { ok: Vec<String>, failed: Vec<String>, folder: String },
}

enum State {
    Running(Job<Done>),
    Finished { ok: bool, message: String, result: Value },
    Cancelled,
}

struct Entry {
    id: u64,
    kind: &'static str,
    label: String,
    state: State,
    /// The command and params that started it, for `cloud.retry`.
    retry: (&'static str, Value),
}

/// The session's cloud state: settings, where they're saved, the transport and secret store, and
/// the jobs. Secrets are kept for the session only unless the app asks for the system keychain
/// ([`Cloud::system`]): tests, agents and other embedders never read or write the user's keychain.
#[derive(Default)]
pub struct Cloud {
    pub settings: CloudSettings,
    settings_path: Option<PathBuf>,
    transport: Option<Arc<dyn Transport>>,
    secrets: Option<Arc<dyn SecretStore>>,
    jobs: Vec<Entry>,
    next_job: u64,
    /// Whether a secret is stored, as last looked up (the keychain isn't asked every frame).
    stored: std::collections::HashMap<&'static str, bool>,
    /// Back-off between Drive retries; tests set it to nothing.
    pub drive_backoff: Option<fn(u32) -> Duration>,
}

impl Cloud {
    /// Use `transport` and `secrets` instead of the network and the keychain (tests).
    pub fn with(transport: Arc<dyn Transport>, secrets: Arc<dyn SecretStore>) -> Self {
        Self { transport: Some(transport), secrets: Some(secrets), ..Self::default() }
    }

    /// Over the network, with secrets in the system keychain: the desktop app with its preferences.
    pub fn system() -> Self {
        Self { secrets: Some(vectorcraft_cloud::secrets::default_store()), ..Self::default() }
    }

    fn transport(&mut self) -> Arc<dyn Transport> {
        self.transport.get_or_insert_with(vectorcraft_cloud::http::default_transport).clone()
    }

    fn secrets(&mut self) -> Arc<dyn SecretStore> {
        self.secrets.get_or_insert_with(|| Arc::new(vectorcraft_cloud::secrets::MemoryStore::default())).clone()
    }

    fn secret(&mut self, name: &str) -> Result<Option<String>> {
        self.secrets().get(name).map_err(EngineError::Other)
    }

    fn is_stored(&mut self, name: &'static str) -> bool {
        if let Some(b) = self.stored.get(name) {
            return *b;
        }
        // An unavailable keychain reads as nothing stored; saving says why.
        let b = matches!(self.secrets().get(name), Ok(Some(v)) if !v.is_empty());
        self.stored.insert(name, b);
        b
    }

    fn set_secret(&mut self, name: &'static str, value: &str) -> Result<()> {
        let store = self.secrets();
        let r = if value.is_empty() { store.delete(name) } else { store.set(name, value) };
        self.stored.remove(name);
        r.map_err(EngineError::Other)
    }

    fn save(&self) -> Result<()> {
        let Some(path) = &self.settings_path else { return Ok(()) };
        let text = serde_json::to_string_pretty(&self.settings).map_err(|e| EngineError::Other(e.to_string()))?;
        write_atomic(path, text.as_bytes()).map_err(EngineError::Other)
    }

    fn start(&mut self, kind: &'static str, label: String, job: Job<Done>, retry: (&'static str, Value)) -> u64 {
        self.next_job += 1;
        let id = self.next_job;
        self.jobs.push(Entry { id, kind, label, state: State::Running(job), retry });
        id
    }

    /// Whether there are jobs to show (running or finished and not dismissed).
    pub fn has_jobs(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// Whether a job is still running (the UI keeps repainting while one is).
    pub fn busy(&self) -> bool {
        self.jobs.iter().any(|j| matches!(j.state, State::Running(_)))
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::result::Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("can't save {}: {e}", path.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("can't save {}: {e}", path.display()))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "cloud.setSettingsFile", "Cloud Settings File", [], None, "{path} where the cloud and AI settings are saved (the app sets it, in its settings folder); loads them", always, set_settings_file),
        cmd!(
            query "cloud.status",
            "Cloud and AI Status",
            [],
            None,
            "{} → {ai: {provider, baseUrl, model, keyStored, ready}, drive: {clientId, clientSecretStored, connected, folder, ready}, keyStore (where secrets are kept), network (false on the web), busy}",
            always,
            status
        ),
        cmd!(
            query "cloud.setAiProvider",
            "AI Provider Settings",
            [],
            None,
            "{baseUrl?, model?, apiKey?} the OpenAI-compatible images endpoint (https://, or http://localhost), the model, and the API key (kept in the system keychain; \"\" removes it) → the status",
            always,
            set_ai
        ),
        cmd!(
            query "cloud.setGoogleClient",
            "Google Drive Settings",
            [],
            None,
            "{clientId?, clientSecret?, folder?} your Google OAuth client (Desktop app type; the secret goes in the system keychain, \"\" removes it) and the Drive folder exports go to → the status",
            always,
            set_google
        ),
        cmd!(
            query "cloud.drive.connect",
            "Connect Google Drive",
            [],
            None,
            "{} start signing in: open the returned `url` in the browser; the job finishes when Google answers → {job, url}",
            always,
            connect
        ),
        cmd!(query "cloud.drive.disconnect", "Disconnect Google Drive", [], None, "{} forget the stored sign-in → the status", always, disconnect),
        cmd!(
            query "cloud.drive.upload",
            "Upload to Google Drive",
            [],
            None,
            "{paths: [file…], folder?} upload these files (exported ones) into the Drive folder (default: the one set) as a background job → {job}",
            always,
            upload
        ),
        cmd!(
            query "boards.variations",
            "AI Variations",
            [],
            None,
            "{id, item, count?: 1–4 (2), size?: 256|512|1024 (1024)} send board image `item` to the AI provider set up and put the variations it returns beside it (one board undo step) → {job, host}",
            always,
            variations
        ),
        cmd!(
            query "cloud.jobs",
            "Background Jobs",
            [],
            None,
            "{} the cloud and AI jobs (finished ones are brought in) → {jobs: [{id, kind, label, state: running|done|failed|cancelled, progress, message, result?}], busy}",
            always,
            jobs
        ),
        cmd!(query "cloud.cancel", "Cancel Job", [], None, "{job} stop a running job", always, cancel),
        cmd!(query "cloud.retry", "Retry Job", [], None, "{job} run a failed or cancelled job again → {job}", always, retry),
        cmd!(query "cloud.dismiss", "Dismiss Job", [], None, "{job} remove a finished job from the list", always, dismiss),
    ]
}

fn set_settings_file(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").filter(|p| !p.trim().is_empty()).ok_or_else(|| bad("cloud.setSettingsFile", "missing `path`"))?;
    let path = PathBuf::from(path);
    // A missing or unreadable file is a fresh start, not an error.
    s.cloud.settings = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    s.cloud.settings_path = Some(path.clone());
    Ok(json!({ "path": path.to_string_lossy() }))
}

fn base_url(settings: &CloudSettings) -> String {
    let b = settings.ai.base_url.trim();
    if b.is_empty() { OpenAiCompatible::DEFAULT_BASE.into() } else { b.into() }
}

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    let c = &mut s.cloud;
    let key = c.is_stored(AI_KEY);
    let secret = c.is_stored(GOOGLE_SECRET);
    let connected = c.is_stored(drive::REFRESH_KEY);
    let base = base_url(&c.settings);
    let local = base.starts_with("http://localhost") || base.starts_with("http://127.0.0.1");
    let model = if c.settings.ai.model.trim().is_empty() { OpenAiCompatible::DEFAULT_MODEL.to_string() } else { c.settings.ai.model.clone() };
    let folder = if c.settings.drive_folder.trim().is_empty() { DEFAULT_FOLDER.to_string() } else { c.settings.drive_folder.clone() };
    let describe = c.secrets().describe();
    Ok(json!({
        "ai": { "provider": OpenAiCompatible::ID, "baseUrl": base, "host": host_of(&base), "model": model, "keyStored": key, "ready": key || local },
        "drive": { "clientId": c.settings.google_client_id, "clientSecretStored": secret, "connected": connected, "folder": folder, "ready": !c.settings.google_client_id.trim().is_empty() },
        "keyStore": describe,
        "network": cfg!(not(target_arch = "wasm32")),
        "busy": c.busy(),
    }))
}

fn set_ai(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "cloud.setAiProvider";
    let mut next = s.cloud.settings.ai.clone();
    next.provider = OpenAiCompatible::ID.into();
    if let Some(b) = str_param(p, "baseUrl") {
        next.base_url = b.trim().to_string();
    }
    if let Some(m) = str_param(p, "model") {
        next.model = m.trim().to_string();
    }
    // The endpoint is checked now, so a mistake shows while the settings are open.
    let check = ProviderSettings {
        base_url: if next.base_url.is_empty() { OpenAiCompatible::DEFAULT_BASE.into() } else { next.base_url.clone() },
        ..next.clone()
    };
    OpenAiCompatible::new(&check, "checked-later").map_err(|e| bad(C, e))?;
    if let Some(k) = str_param(p, "apiKey") {
        let k = k.trim();
        if k.chars().any(|c| c.is_control() || c == ' ') {
            return Err(bad(C, "the API key has spaces or control characters in it"));
        }
        s.cloud.set_secret(AI_KEY, k)?;
    }
    s.cloud.settings.ai = next;
    s.cloud.save()?;
    status(s, &json!({}))
}

fn set_google(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "cloud.setGoogleClient";
    if let Some(id) = str_param(p, "clientId") {
        let id = id.trim();
        if id.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(bad(C, "the client id has spaces in it"));
        }
        if id != s.cloud.settings.google_client_id {
            // A sign-in belongs to the client that made it.
            s.cloud.set_secret(drive::REFRESH_KEY, "")?;
        }
        s.cloud.settings.google_client_id = id.to_string();
    }
    if let Some(secret) = str_param(p, "clientSecret") {
        s.cloud.set_secret(GOOGLE_SECRET, secret.trim())?;
    }
    if let Some(f) = str_param(p, "folder") {
        let f = f.trim();
        if f.chars().any(char::is_control) || f.len() > 200 {
            return Err(bad(C, "the folder name is too long or has control characters"));
        }
        s.cloud.settings.drive_folder = f.to_string();
    }
    s.cloud.save()?;
    status(s, &json!({}))
}

fn google(s: &mut Session) -> Result<vectorcraft_cloud::oauth::OAuthClient> {
    let id = s.cloud.settings.google_client_id.clone();
    let secret = s.cloud.secret(GOOGLE_SECRET)?;
    drive::google_client(&id, secret.as_deref()).map_err(EngineError::Other)
}

fn connect(s: &mut Session, _: &Value) -> Result<Value> {
    let client = google(s)?;
    let lb = Loopback::bind().map_err(EngineError::Other)?;
    let pkce = Pkce::new().map_err(EngineError::Other)?;
    let url = client.authorization_url(&pkce, &lb.redirect_uri);
    let transport = s.cloud.transport();
    let secrets = s.cloud.secrets();
    let job = Job::spawn("drive-connect", move |progress, cancel| {
        progress.set(0.1, "Waiting for you to sign in in the browser");
        let code = lb.wait(&pkce.state, SIGN_IN_TIMEOUT, cancel)?;
        progress.set(0.7, "Signing in");
        let tokens = client.exchange(transport.as_ref(), &code, &pkce, &lb.redirect_uri)?;
        let refresh = tokens
            .refresh_token
            .ok_or("Google didn't grant lasting access; remove DLightroom from your Google Account's third-party access and connect again")?;
        secrets.set(drive::REFRESH_KEY, &refresh)?;
        Ok(Done::Connected)
    });
    s.cloud.stored.remove(drive::REFRESH_KEY);
    let id = s.cloud.start("driveConnect", "Connect Google Drive".into(), job, ("cloud.drive.connect", json!({})));
    Ok(json!({ "job": id, "url": url }))
}

fn disconnect(s: &mut Session, _: &Value) -> Result<Value> {
    s.cloud.set_secret(drive::REFRESH_KEY, "")?;
    status(s, &json!({}))
}

fn upload(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "cloud.drive.upload";
    let paths: Vec<String> =
        p.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    if paths.is_empty() {
        return Err(bad(C, "missing `paths`: the files to upload"));
    }
    let client = google(s)?;
    let secrets = s.cloud.secrets();
    if !drive::connected(secrets.as_ref()) {
        return Err(EngineError::Other("Google Drive isn't connected: connect it in Cloud and AI Settings first".into()));
    }
    let folder = str_param(p, "folder").map(str::trim).filter(|f| !f.is_empty()).map(String::from).unwrap_or_else(|| {
        let f = s.cloud.settings.drive_folder.trim();
        if f.is_empty() { DEFAULT_FOLDER.to_string() } else { f.to_string() }
    });
    let mut total = 0u64;
    let mut files = vec![];
    for path in &paths {
        let path = Path::new(path);
        let meta = std::fs::metadata(path).map_err(|e| bad(C, format!("can't open {}: {e}", path.display())))?;
        if !meta.is_file() {
            return Err(bad(C, format!("{} isn't a file", path.display())));
        }
        total += meta.len();
        if total > MAX_UPLOAD_TOTAL || meta.len() > drive::MAX_FILE as u64 {
            return Err(bad(C, "the files are too large to upload at once"));
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "export".into());
        let data = std::fs::read(path).map_err(|e| bad(C, format!("can't read {}: {e}", path.display())))?;
        files.push(Upload { mime: drive::mime_of(&name).into(), name, data });
    }
    let transport = s.cloud.transport();
    let backoff = s.cloud.drive_backoff;
    let count = files.len();
    let target = folder.clone();
    let job = Job::spawn("drive-upload", move |progress, cancel| {
        let mut d = Drive::new(transport, client, secrets.as_ref())?;
        if let Some(b) = backoff {
            d.backoff = b;
        }
        let (ok, failed) = d.upload_all(&target, &files, cancel, progress)?;
        if ok.is_empty() && !failed.is_empty() {
            return Err(failed.join("; "));
        }
        Ok(Done::Uploaded { ok, failed, folder: target })
    });
    let label = format!("Upload {count} file(s) to Google Drive › {folder}");
    let id = s.cloud.start("driveUpload", label, job, (C, p.clone()));
    Ok(json!({ "job": id }))
}

fn variations(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "boards.variations";
    let board =
        str_param(p, "id").filter(|i| vectorcraft_boards::valid_id(i)).map(String::from).ok_or_else(|| bad(C, "missing or invalid board `id`"))?;
    let item = p.get("item").and_then(Value::as_u64).ok_or_else(|| bad(C, "missing `item`: the image to vary"))?;
    let count = p.get("count").and_then(Value::as_u64).unwrap_or(2);
    if !(1..=u64::from(ai::MAX_COUNT)).contains(&count) {
        return Err(bad(C, format!("`count` must be 1 to {}", ai::MAX_COUNT)));
    }
    let size = p.get("size").and_then(Value::as_u64).unwrap_or(1024);
    if ![256, 512, 1024].contains(&size) {
        return Err(bad(C, "`size` must be 256, 512 or 1024"));
    }
    let key = s.cloud.secret(AI_KEY)?.unwrap_or_default();
    let settings = ProviderSettings { base_url: base_url(&s.cloud.settings), ..s.cloud.settings.ai.clone() };
    let provider = OpenAiCompatible::new(&settings, &key)
        .map_err(|e| EngineError::Other(format!("AI variations need a provider: {e} (Cloud and AI Settings)")))?;
    let bytes = s
        .boards
        .as_mut()
        .ok_or_else(|| EngineError::Other("Inspiration Boards aren't set up here".into()))?
        .image_bytes(&board, item)
        .map_err(EngineError::Other)?;
    let transport = s.cloud.transport();
    let host = host_of(&provider.base_url).to_string();
    let origin = provider.origin();
    let (b, it) = (board.clone(), item);
    let job = Job::spawn("ai-variations", move |progress, cancel| {
        progress.set(0.05, "Preparing the image");
        let image_png = ai::prepare_image(&bytes)?;
        cancel.check()?;
        // Counts and sizes were checked above.
        let req = VariationRequest { image_png, count: count as u32, size: size as u32 };
        let images = provider.variations(transport.as_ref(), &req, progress, cancel)?;
        Ok(Done::Variations { board: b, source: it, images, origin })
    });
    let id = s.cloud.start("aiVariations", format!("AI variations ({host})"), job, (C, p.clone()));
    Ok(json!({ "job": id, "host": host }))
}

/// Bring in what finished: variations go on their board.
fn collect(s: &mut Session) {
    let mut finished = vec![];
    for (i, e) in s.cloud.jobs.iter_mut().enumerate() {
        if let State::Running(job) = &mut e.state
            && let Some(outcome) = job.poll()
        {
            finished.push((i, outcome));
        }
    }
    for (i, outcome) in finished {
        let state = match outcome {
            Outcome::Done(done) => match bring_in(s, done) {
                Ok((message, result)) => State::Finished { ok: true, message, result },
                Err(e) => State::Finished { ok: false, message: e, result: Value::Null },
            },
            Outcome::Failed(e) if e == CANCELLED => State::Cancelled,
            Outcome::Failed(e) => State::Finished { ok: false, message: e, result: Value::Null },
            Outcome::Cancelled => State::Cancelled,
        };
        if let Some(e) = s.cloud.jobs.get_mut(i) {
            e.state = state;
        }
    }
    // Keep the newest finished ones.
    let done: Vec<u64> = s.cloud.jobs.iter().filter(|j| !matches!(j.state, State::Running(_))).map(|j| j.id).collect();
    if done.len() > KEEP_FINISHED {
        let drop = &done[..done.len() - KEEP_FINISHED];
        s.cloud.jobs.retain(|j| !drop.contains(&j.id));
    }
}

fn bring_in(s: &mut Session, done: Done) -> std::result::Result<(String, Value), String> {
    match done {
        Done::Variations { board, source, images, origin } => {
            let bs = s.boards.as_mut().ok_or("Inspiration Boards aren't set up here")?;
            let src = bs.board(&board)?.item(source).cloned();
            // Beside the source image, left to right; where the board is looked at if it's gone.
            let (mut x, y, step) = match &src {
                Some(it) => (it.x + it.w + 40.0, it.y, it.h.max(80.0) + 24.0),
                None => {
                    let v = bs.board(&board)?.view;
                    (v.center[0], v.center[1], 280.0)
                }
            };
            // As tall as the source image (square: the provider's results are).
            let side = src.as_ref().map_or(0.0, |it| it.h);
            let mut list = vec![];
            for (n, bytes) in images.into_iter().enumerate() {
                list.push(vectorcraft_boards::NewImage { bytes, x, y, w: side, h: side, caption: format!("Variation {}", n + 1) });
                x += step;
            }
            let count = list.len();
            let ids = bs.add_images(&board, &list, &origin)?;
            Ok((format!("{count} variation(s) added to the board"), json!({ "board": board, "items": ids })))
        }
        Done::Connected => {
            s.cloud.stored.remove(drive::REFRESH_KEY);
            Ok(("Google Drive is connected".into(), json!({})))
        }
        Done::Uploaded { ok, failed, folder } => {
            let message = if failed.is_empty() {
                format!("Uploaded {} file(s) to Google Drive › {folder}", ok.len())
            } else {
                format!("Uploaded {} file(s) to Google Drive › {folder}; {} failed: {}", ok.len(), failed.len(), failed.join("; "))
            };
            Ok((message, json!({ "uploaded": ok, "failed": failed, "folder": folder })))
        }
    }
}

fn entry_json(e: &Entry) -> Value {
    let (state, progress, message, result) = match &e.state {
        State::Running(job) => {
            let (f, m) = job.progress();
            ("running", f, m, Value::Null)
        }
        State::Finished { ok, message, result } => (if *ok { "done" } else { "failed" }, 1.0, message.clone(), result.clone()),
        State::Cancelled => ("cancelled", 0.0, "Cancelled".to_string(), Value::Null),
    };
    json!({ "id": e.id, "kind": e.kind, "label": e.label, "state": state, "progress": progress, "message": message, "result": result })
}

fn jobs(s: &mut Session, _: &Value) -> Result<Value> {
    collect(s);
    let list: Vec<Value> = s.cloud.jobs.iter().map(entry_json).collect();
    Ok(json!({ "jobs": list, "busy": s.cloud.busy() }))
}

fn job_param(s: &Session, p: &Value, cmd: &str) -> Result<usize> {
    let id = p.get("job").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "missing `job`"))?;
    s.cloud.jobs.iter().position(|j| j.id == id).ok_or_else(|| bad(cmd, format!("no job {id}")))
}

fn cancel(s: &mut Session, p: &Value) -> Result<Value> {
    let i = job_param(s, p, "cloud.cancel")?;
    if let Some(Entry { state: State::Running(job), .. }) = s.cloud.jobs.get(i) {
        job.cancel();
    }
    Ok(json!({}))
}

fn retry(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "cloud.retry";
    let i = job_param(s, p, C)?;
    let Some(e) = s.cloud.jobs.get(i) else { return Err(bad(C, "no such job")) };
    if matches!(e.state, State::Running(_)) || matches!(e.state, State::Finished { ok: true, .. }) {
        return Err(bad(C, "only a failed or cancelled job can be run again"));
    }
    let (cmd, params) = e.retry.clone();
    let r = s.execute(cmd, &params)?;
    s.cloud.jobs.retain(|j| j.id != p["job"].as_u64().unwrap_or(0));
    Ok(r)
}

fn dismiss(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "cloud.dismiss";
    let i = job_param(s, p, C)?;
    if matches!(s.cloud.jobs.get(i).map(|j| &j.state), Some(State::Running(_))) {
        return Err(bad(C, "cancel the job first"));
    }
    s.cloud.jobs.remove(i);
    Ok(json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectorcraft_cloud::http::{MockTransport, Response};
    use vectorcraft_cloud::secrets::MemoryStore;

    fn session(answers: Vec<std::result::Result<Response, String>>) -> (Session, MockTransport, Arc<MemoryStore>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("dlightroom-engine-cloud-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        let t = MockTransport::new(answers);
        let store = Arc::new(MemoryStore::default());
        let mut s = Session::new();
        s.cloud = Cloud::with(Arc::new(t.clone()), store.clone());
        s.cloud.drive_backoff = Some(|_| Duration::ZERO);
        s.execute("boards.setFolder", &json!({"path": dir.join("boards").to_string_lossy()})).unwrap();
        s.execute("cloud.setSettingsFile", &json!({"path": dir.join("cloud.json").to_string_lossy()})).unwrap();
        (s, t, store)
    }

    fn png(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba(c));
        let mut out = std::io::Cursor::new(vec![]);
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// Poll `cloud.jobs` until job `id` isn't running.
    fn wait(s: &mut Session, id: u64) -> Value {
        for _ in 0..500 {
            let r = s.execute("cloud.jobs", &json!({})).unwrap();
            let j = r["jobs"].as_array().unwrap().iter().find(|j| j["id"] == id).cloned().unwrap();
            if j["state"] != "running" {
                return j;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("job {id} didn't finish");
    }

    #[test]
    fn settings_persist_and_secrets_stay_in_the_keychain() {
        let (mut s, _, store) = session(vec![]);
        let st = s.execute("cloud.status", &json!({})).unwrap();
        assert_eq!(
            (st["ai"]["ready"].as_bool(), st["drive"]["ready"].as_bool(), st["drive"]["connected"].as_bool()),
            (Some(false), Some(false), Some(false))
        );
        assert_eq!(st["ai"]["baseUrl"], OpenAiCompatible::DEFAULT_BASE);
        assert!(s.execute("cloud.setAiProvider", &json!({"baseUrl": "http://example.com"})).is_err(), "plain http to another computer");
        let st = s.execute("cloud.setAiProvider", &json!({"baseUrl": "https://ai.example.com/v1/", "model": "m1", "apiKey": "sk-test"})).unwrap();
        assert_eq!((st["ai"]["keyStored"].as_bool(), st["ai"]["host"].as_str()), (Some(true), Some("ai.example.com")));
        s.execute(
            "cloud.setGoogleClient",
            &json!({"clientId": "123.apps.googleusercontent.com", "clientSecret": "GOCSPX-x", "folder": "Client work"}),
        )
        .unwrap();
        let path = s.cloud.settings_path.clone().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("123.apps.googleusercontent.com") && text.contains("Client work"));
        assert!(!text.contains("sk-test") && !text.contains("GOCSPX"), "no secret in the settings file: {text}");
        assert_eq!(store.get(AI_KEY).unwrap().as_deref(), Some("sk-test"));
        let mut again = Session::new();
        again.cloud = Cloud::with(Arc::new(MockTransport::default()), store.clone());
        again.execute("cloud.setSettingsFile", &json!({"path": path.to_string_lossy()})).unwrap();
        let st = again.execute("cloud.status", &json!({})).unwrap();
        assert_eq!(
            (st["ai"]["model"].as_str(), st["drive"]["folder"].as_str(), st["drive"]["clientSecretStored"].as_bool()),
            (Some("m1"), Some("Client work"), Some(true))
        );
        again.execute("cloud.setAiProvider", &json!({"apiKey": ""})).unwrap();
        assert_eq!(store.get(AI_KEY).unwrap(), None);
    }

    #[test]
    fn a_plain_session_never_uses_the_system_keychain() {
        // Tests, agents and embedders get session-only secrets; only the app opts into the keychain.
        let mut s = Session::new();
        let memory = MemoryStore::default().describe();
        assert_eq!(s.execute("cloud.status", &json!({})).unwrap()["keyStore"], memory);
        s.execute("cloud.setAiProvider", &json!({"apiKey": "sk-session"})).unwrap();
        assert_eq!(Session::new().execute("cloud.status", &json!({})).unwrap()["ai"]["keyStored"], false, "another session doesn't see it");
    }

    #[test]
    fn variations_need_a_provider_and_land_beside_their_image() {
        let result = png(8, 8, [0, 200, 0, 255]);
        let body = json!({"data": [{"b64_json": vectorcraft_cloud::util::base64_encode(&result)}, {"b64_json": vectorcraft_cloud::util::base64_encode(&result)}]}).to_string();
        let (mut s, t, _) =
            session(vec![Ok(Response::new(200, body)), Ok(Response::new(401, r#"{"error":{"message":"Incorrect API key provided"}}"#))]);
        let board = s.execute("boards.create", &json!({"name": "Mood"})).unwrap()["id"].as_str().unwrap().to_string();
        let src = s
            .execute("boards.addImage", &json!({"id": board, "dataBase64": vectorcraft_format::base64_encode(&png(40, 20, [200, 0, 0, 255])), "name": "red.png", "x": 0, "y": 0}))
            .unwrap()["item"]
            .as_u64()
            .unwrap();
        let e = s.execute("boards.variations", &json!({"id": board, "item": src})).unwrap_err();
        assert!(e.to_string().contains("API key"), "{e}");
        assert!(t.sent().is_empty(), "nothing is sent without a provider");
        s.execute("cloud.setAiProvider", &json!({"baseUrl": "https://ai.example.com/v1", "apiKey": "sk-1"})).unwrap();
        let job = s.execute("boards.variations", &json!({"id": board, "item": src, "count": 2, "size": 256})).unwrap()["job"].as_u64().unwrap();
        let j = wait(&mut s, job);
        assert_eq!(j["state"], "done", "{j}");
        let b = s.execute("boards.get", &json!({"id": board})).unwrap();
        let items = b["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[1]["origin"], "ai:ai.example.com (dall-e-2)");
        assert_eq!(items[2]["caption"], "Variation 2");
        assert!(items[1]["x"].as_f64().unwrap() > items[0]["x"].as_f64().unwrap() + items[0]["w"].as_f64().unwrap());
        let sent = t.sent();
        assert_eq!(sent[0].url, "https://ai.example.com/v1/images/variations");
        assert_eq!(sent[0].header_value("Authorization"), Some("Bearer sk-1"));
        // A refusal is reported as the provider said it, and can be run again.
        let job = s.execute("boards.variations", &json!({"id": board, "item": src})).unwrap()["job"].as_u64().unwrap();
        let j = wait(&mut s, job);
        assert_eq!(j["state"], "failed");
        assert!(j["message"].as_str().unwrap().contains("Incorrect API key"), "{j}");
        let again = s.execute("cloud.retry", &json!({"job": job})).unwrap()["job"].as_u64().unwrap();
        assert_eq!(wait(&mut s, again)["state"], "failed", "out of scripted answers: still an error, not a made-up image");
        assert_eq!(s.execute("boards.get", &json!({"id": board})).unwrap()["items"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn drive_needs_a_client_and_a_sign_in() {
        let (mut s, _, store) = session(vec![]);
        assert!(s.execute("cloud.drive.connect", &json!({})).unwrap_err().to_string().contains("client id"));
        s.execute("cloud.setGoogleClient", &json!({"clientId": "abc.apps.googleusercontent.com"})).unwrap();
        let r = s.execute("cloud.drive.connect", &json!({})).unwrap();
        let url = r["url"].as_str().unwrap();
        assert!(
            url.starts_with(drive::AUTH_URL)
                && url.contains("client_id=abc.apps.googleusercontent.com")
                && url.contains("code_challenge_method=S256")
        );
        let job = r["job"].as_u64().unwrap();
        s.execute("cloud.cancel", &json!({"job": job})).unwrap();
        assert_eq!(wait(&mut s, job)["state"], "cancelled");
        assert!(!drive::connected(store.as_ref()));
        let dir = std::env::temp_dir().join(format!("dlightroom-upload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("board-1.svg");
        std::fs::write(&f, "<svg/>").unwrap();
        let e = s.execute("cloud.drive.upload", &json!({"paths": [f.to_string_lossy()]})).unwrap_err();
        assert!(e.to_string().contains("isn't connected"), "{e}");
    }

    #[test]
    fn exported_files_upload_into_the_drive_folder() {
        let token = || Ok(Response::new(200, r#"{"access_token":"AT","expires_in":3600}"#));
        let (mut s, t, store) = session(vec![
            token(),
            Ok(Response::new(200, r#"{"files":[]}"#)),
            Ok(Response::new(200, r#"{"id":"FOLDER"}"#)),
            Ok(Response::new(200, "").with_header("Location", "https://www.googleapis.com/upload/drive/v3/files?upload_id=1")),
            Ok(Response::new(200, r#"{"id":"F1"}"#)),
        ]);
        s.execute("cloud.setGoogleClient", &json!({"clientId": "abc.apps.googleusercontent.com", "folder": "Spring"})).unwrap();
        store.set(drive::REFRESH_KEY, "RT").unwrap();
        s.cloud.stored.clear();
        assert_eq!(s.execute("cloud.status", &json!({})).unwrap()["drive"]["connected"], true);
        let dir = std::env::temp_dir().join(format!("dlightroom-upload2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("board-1.svg");
        std::fs::write(&f, "<svg/>").unwrap();
        let job = s.execute("cloud.drive.upload", &json!({"paths": [f.to_string_lossy()]})).unwrap()["job"].as_u64().unwrap();
        let j = wait(&mut s, job);
        assert_eq!(j["state"], "done", "{j}");
        assert_eq!(j["result"]["uploaded"], json!(["board-1.svg"]));
        assert!(j["message"].as_str().unwrap().contains("Spring"));
        let sent = t.sent();
        assert_eq!(sent[3].header_value("X-Upload-Content-Type"), Some("image/svg+xml"));
        assert_eq!(sent[4].body, b"<svg/>");
        s.execute("cloud.dismiss", &json!({"job": job})).unwrap();
        assert!(s.execute("cloud.jobs", &json!({})).unwrap()["jobs"].as_array().unwrap().is_empty());
        s.execute("cloud.drive.disconnect", &json!({})).unwrap();
        assert!(!drive::connected(store.as_ref()));
    }
}
