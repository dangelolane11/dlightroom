//! Edit › Cloud and AI Settings (DLightroom): the AI image-variation provider (an
//! OpenAI-compatible images endpoint, its model and your API key) and Google Drive (your own OAuth
//! client, the folder exports go to, and Connect / Disconnect). Keys and secrets go into the
//! system keychain; the fields for them stay empty and only replace what is stored when typed in.
//! The status lines say what is set up, and Google Drive reads Disconnected until you connect.
//!
//! Fields: `baseUrl`, `model`, `apiKey` (typed: replaces the stored key), `clientId`,
//! `clientSecret` (typed: replaces the stored secret), `folder`.

use serde_json::{Value, json};

use super::DialogSpec;
use crate::state::Dialog;
use crate::{VectorcraftApp, widgets};

pub const KIND: &str = "cloudSettings";

pub(super) const SPEC: DialogSpec =
    DialogSpec { heading: |_| tl!("Cloud and AI Settings").into(), body, confirm, ok: Some("Save"), min_width: 500.0, ..DialogSpec::FORM };

/// Open the dialog on the settings saved.
pub fn open(app: &mut VectorcraftApp) {
    let st = app.run("cloud.status", json!({})).unwrap_or_default();
    let fields = json!({
        "baseUrl": st["ai"]["baseUrl"],
        "model": st["ai"]["model"],
        "apiKey": "",
        "clientId": st["drive"]["clientId"],
        "clientSecret": "",
        "folder": st["drive"]["folder"],
    });
    app.ui.dialog = Some(Dialog::new(KIND, fields));
}

/// Save what the fields say (typed secrets replace the stored ones).
fn save(app: &mut VectorcraftApp, d: &Dialog) -> Result<Value, String> {
    let mut ai = json!({ "baseUrl": d.str("baseUrl"), "model": d.str("model") });
    if !d.str("apiKey").trim().is_empty() {
        ai["apiKey"] = json!(d.str("apiKey").trim());
    }
    app.run("cloud.setAiProvider", ai)?;
    let mut g = json!({ "clientId": d.str("clientId"), "folder": d.str("folder") });
    if !d.str("clientSecret").trim().is_empty() {
        g["clientSecret"] = json!(d.str("clientSecret").trim());
    }
    app.run("cloud.setGoogleClient", g)
}

fn confirm(app: &mut VectorcraftApp, d: &Dialog) -> Result<Value, String> {
    let r = save(app, d)?;
    app.ui.dialog = None;
    app.status(tl!("Cloud and AI settings saved"));
    Ok(r)
}

fn text_row(ui: &mut egui::Ui, d: &mut Dialog, key: &str, label: &str, hint: &str, password: bool) {
    widgets::label_row(ui, label, 150.0, |ui| {
        let mut v = d.str(key);
        let r = ui.add(egui::TextEdit::singleline(&mut v).password(password).hint_text(hint).desired_width(300.0));
        if r.changed() {
            d.fields.insert(key.into(), json!(v));
        }
    });
}

fn status_line(ui: &mut egui::Ui, ok: bool, text: &str) {
    let color = if ok { egui::Color32::from_rgb(0x3f, 0xa3, 0x5b) } else { egui::Color32::from_rgb(0xd9, 0x48, 0x3b) };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("●").color(color));
        ui.label(text);
    });
}

fn body(app: &mut VectorcraftApp, ui: &mut egui::Ui, d: &mut Dialog) -> bool {
    let st = app.run("cloud.status", json!({})).unwrap_or_default();
    let store = st["keyStore"].as_str().unwrap_or("the system keychain").to_string();
    if st["network"] == false {
        widgets::dim_label(ui, tl!("Cloud and AI features need the desktop app."));
        return false;
    }
    ui.label(egui::RichText::new(tl!("AI Variations")).strong());
    widgets::dim_label(
        ui,
        tl!(
            "An OpenAI-compatible images endpoint (/images/variations) and your API key. DLightroom includes no AI service: images are sent only when you click Generate Variations on a board."
        ),
    );
    text_row(ui, d, "baseUrl", tl!("Endpoint:"), "https://api.openai.com/v1", false);
    text_row(ui, d, "model", tl!("Model:"), "dall-e-2", false);
    text_row(ui, d, "apiKey", tl!("API Key:"), if st["ai"]["keyStored"] == true { tl!("stored; type to replace") } else { "" }, true);
    let key = st["ai"]["keyStored"] == true;
    ui.horizontal(|ui| {
        status_line(
            ui,
            st["ai"]["ready"] == true,
            &if key { format!("{} {store}", tl!("API key stored in")) } else { tl!("No API key stored").to_string() },
        );
        if key && ui.small_button(tl!("Remove Key")).clicked() {
            match app.run("cloud.setAiProvider", json!({ "apiKey": "" })) {
                Ok(_) => app.status(tl!("API key removed")),
                Err(e) => app.status(e),
            }
        }
    });
    ui.add_space(10.0);
    ui.separator();
    ui.label(egui::RichText::new(tl!("Google Drive")).strong());
    widgets::dim_label(
        ui,
        tl!(
            "Export for Screens can upload what it exports to a Drive folder. Use your own OAuth client (Google Cloud console › Credentials › OAuth client ID › Desktop app); DLightroom can only see the files it creates."
        ),
    );
    text_row(ui, d, "clientId", tl!("Client ID:"), "…apps.googleusercontent.com", false);
    text_row(
        ui,
        d,
        "clientSecret",
        tl!("Client Secret:"),
        if st["drive"]["clientSecretStored"] == true { tl!("stored; type to replace") } else { "" },
        true,
    );
    text_row(ui, d, "folder", tl!("Drive Folder:"), "DLightroom Exports", false);
    let connected = st["drive"]["connected"] == true;
    let has_client = !d.str("clientId").trim().is_empty();
    ui.horizontal(|ui| {
        status_line(ui, connected, if connected { tl!("Connected") } else { tl!("Disconnected") });
        if connected {
            if ui.small_button(tl!("Disconnect")).clicked() {
                match app.run("cloud.drive.disconnect", json!({})) {
                    Ok(_) => app.status(tl!("Google Drive disconnected")),
                    Err(e) => app.status(e),
                }
            }
        } else if ui
            .add_enabled(has_client, egui::Button::new(tl!("Connect…")))
            .on_disabled_hover_text(tl!("Enter your OAuth client ID first"))
            .clicked()
        {
            connect(app, d);
        }
    });
    crate::boards_view::jobs(app, ui);
    false
}

/// Save the client, then sign in in the browser.
fn connect(app: &mut VectorcraftApp, d: &Dialog) {
    let r = save(app, d).and_then(|_| app.run("cloud.drive.connect", json!({})));
    match r {
        Ok(r) => {
            let url = r["url"].as_str().unwrap_or_default().to_string();
            match app.services.open_url.as_mut() {
                Some(open) => open(&url),
                None => app.status(format!("Open this address to sign in: {url}")),
            }
        }
        Err(e) => app.status(e),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use vectorcraft_cloud::http::MockTransport;
    use vectorcraft_cloud::secrets::{MemoryStore, SecretStore};
    use vectorcraft_engine::Session;

    #[test]
    fn saving_keeps_secrets_out_of_the_fields_and_shows_disconnected() {
        let store = Arc::new(MemoryStore::default());
        let mut app = VectorcraftApp::new(Session::new(), crate::Services::default());
        app.session.cloud = vectorcraft_engine::cmd::cloud::Cloud::with(Arc::new(MockTransport::default()), store.clone());
        app.run("edit.cloudSettings", json!({})).unwrap();
        let mut d = app.ui.dialog.take().unwrap();
        assert_eq!(d.kind, KIND);
        d.fields.insert("apiKey".into(), json!("sk-abc"));
        d.fields.insert("clientId".into(), json!("1-x.apps.googleusercontent.com"));
        confirm(&mut app, &d).unwrap();
        assert_eq!(store.get(vectorcraft_engine::cmd::cloud::AI_KEY).unwrap().as_deref(), Some("sk-abc"));
        open(&mut app);
        let d = app.ui.dialog.as_ref().unwrap();
        assert_eq!(d.str("apiKey"), "", "a stored key isn't shown");
        assert_eq!(d.str("clientId"), "1-x.apps.googleusercontent.com");
        let st = app.run("cloud.status", json!({})).unwrap();
        assert_eq!((st["ai"]["ready"].as_bool(), st["drive"]["connected"].as_bool()), (Some(true), Some(false)));
        let mut bad = Dialog::new(KIND, json!({"baseUrl": "ftp://x", "model": "", "apiKey": "", "clientId": "", "clientSecret": "", "folder": ""}));
        assert!(confirm(&mut app, &bad).is_err(), "a bad endpoint keeps the dialog open");
        bad.fields.insert("baseUrl".into(), json!("https://ai.example.com/v1"));
        assert!(confirm(&mut app, &bad).is_ok());
    }
}
