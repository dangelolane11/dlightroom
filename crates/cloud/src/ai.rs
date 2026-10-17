//! AI image variations behind a provider interface. DLightroom ships no AI service and no
//! credentials: the user picks a provider and enters its endpoint, model and API key (kept in the
//! system keychain, see [`crate::secrets`]). Requests run as background jobs with progress and
//! cancellation; every result is what the provider sent back, never made up, and every failure is
//! reported as it happened.
//!
//! The provider included is [`OpenAiCompatible`]: the `images/variations` endpoint of the OpenAI
//! API, which other services and self-hosted servers also offer. Other providers implement
//! [`VariationProvider`].

use crate::http::{Multipart, Request, Transport, host_of};
use crate::job::{Cancel, Progress};
use crate::util::base64_decode;

/// Most variations asked for at once.
pub const MAX_COUNT: u32 = 4;
/// Largest image sent (the endpoint's limit is 4 MB).
pub const MAX_UPLOAD: usize = 4 << 20;
/// Largest image decoded to prepare an upload (pixels, either side).
const MAX_SOURCE_SIDE: u32 = 20_000;

/// What to ask for.
#[derive(Clone, Debug, PartialEq)]
pub struct VariationRequest {
    /// A square PNG ([`prepare_image`]).
    pub image_png: Vec<u8>,
    /// 1–4.
    pub count: u32,
    /// The side of the results in pixels: 256, 512 or 1024.
    pub size: u32,
}

/// A provider's settings, as the user entered them. The key is kept in the keychain, not with
/// the preferences.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettings {
    /// Which provider: [`OpenAiCompatible::ID`].
    pub provider: String,
    /// The API's base URL, such as `https://api.openai.com/v1`.
    pub base_url: String,
    pub model: String,
}

/// An AI service that makes variations of an image.
pub trait VariationProvider: Send + Sync {
    /// Ask for the variations: their image bytes (PNG), as many as the service returned.
    fn variations(&self, transport: &dyn Transport, req: &VariationRequest, progress: &Progress, cancel: &Cancel) -> Result<Vec<Vec<u8>>, String>;
    /// The provider's name, for the board's record of where an image came from.
    fn origin(&self) -> String;
}

/// The OpenAI Images API `POST {base}/images/variations` (and services compatible with it).
#[derive(Clone, Debug, PartialEq)]
pub struct OpenAiCompatible {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

impl OpenAiCompatible {
    pub const ID: &'static str = "openaiCompatible";
    pub const DEFAULT_BASE: &'static str = "https://api.openai.com/v1";
    pub const DEFAULT_MODEL: &'static str = "dall-e-2";

    /// The provider for `settings` and `api_key`, checked: an `https://` endpoint (or `http://`
    /// on this computer, for a local server) and a key unless the endpoint is local.
    pub fn new(settings: &ProviderSettings, api_key: &str) -> Result<Self, String> {
        let base = settings.base_url.trim().trim_end_matches('/').to_string();
        let local = base.starts_with("http://localhost") || base.starts_with("http://127.0.0.1");
        if !base.starts_with("https://") && !local {
            return Err("the endpoint must start with https:// (or http://localhost for a server on this computer)".into());
        }
        if host_of(&base).is_empty() {
            return Err("the endpoint has no host".into());
        }
        let key = api_key.trim();
        if key.is_empty() && !local {
            return Err("enter the provider's API key".into());
        }
        if key.chars().any(|c| c.is_control() || c == ' ') {
            return Err("the API key has spaces or control characters in it".into());
        }
        let model = settings.model.trim();
        Ok(Self { base_url: base, model: if model.is_empty() { Self::DEFAULT_MODEL.into() } else { model.into() }, api_key: key.into() })
    }
}

impl VariationProvider for OpenAiCompatible {
    fn origin(&self) -> String {
        format!("ai:{} ({})", host_of(&self.base_url), self.model)
    }

    fn variations(&self, t: &dyn Transport, req: &VariationRequest, progress: &Progress, cancel: &Cancel) -> Result<Vec<Vec<u8>>, String> {
        if !(1..=MAX_COUNT).contains(&req.count) {
            return Err(format!("ask for 1 to {MAX_COUNT} variations"));
        }
        if ![256, 512, 1024].contains(&req.size) {
            return Err("the size must be 256, 512 or 1024 pixels".into());
        }
        if req.image_png.len() > MAX_UPLOAD {
            return Err(format!("the image must be smaller than {} MB", MAX_UPLOAD >> 20));
        }
        cancel.check()?;
        progress.set(0.1, "Sending the image");
        let (ct, body) = Multipart::new()
            .file("image", "image.png", "image/png", &req.image_png)
            .text("n", &req.count.to_string())
            .text("size", &format!("{0}x{0}", req.size))
            .text("model", &self.model)
            .text("response_format", "b64_json")
            .finish()?;
        let mut r = Request::new("POST", format!("{}/images/variations", self.base_url)).header("Content-Type", ct).body(body);
        if !self.api_key.is_empty() {
            r = r.header("Authorization", format!("Bearer {}", self.api_key));
        }
        let resp = t.send(&r)?;
        cancel.check()?;
        if !resp.ok() {
            return Err(match resp.status {
                401 | 403 => format!("the provider refused the API key ({})", resp.error_message()),
                429 => format!("the provider is rate limiting or out of credit ({})", resp.error_message()),
                _ => format!("the provider couldn't make variations ({})", resp.error_message()),
            });
        }
        progress.set(0.7, "Reading the results");
        let v = resp.json()?;
        let data = v.get("data").and_then(|d| d.as_array()).ok_or("the provider's answer has no images")?;
        let mut out = vec![];
        for (i, item) in data.iter().take(MAX_COUNT as usize).enumerate() {
            cancel.check()?;
            let bytes = if let Some(b64) = item.get("b64_json").and_then(|b| b.as_str()) {
                base64_decode(b64).ok_or("an image in the answer isn't valid base64")?
            } else if let Some(url) = item.get("url").and_then(|u| u.as_str()) {
                progress.set(0.7 + 0.25 * i as f32 / data.len().max(1) as f32, "Downloading the results");
                if !url.starts_with("https://") {
                    return Err("the provider returned an image address that isn't https".into());
                }
                let got = t.send(&Request::new("GET", url))?;
                if !got.ok() {
                    return Err(format!("a result couldn't be downloaded ({})", got.error_message()));
                }
                got.body
            } else {
                return Err("an image in the answer has neither data nor an address".into());
            };
            image::guess_format(&bytes).map_err(|_| "the provider sent something that isn't an image".to_string())?;
            out.push(bytes);
        }
        if out.is_empty() {
            return Err("the provider returned no images".into());
        }
        progress.set(1.0, "Done");
        Ok(out)
    }
}

/// `bytes` (any image the boards take) as the endpoint wants it: a square PNG, cropped to its
/// centre, at most 1024 pixels a side and under [`MAX_UPLOAD`].
pub fn prepare_image(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let format = image::guess_format(bytes).map_err(|_| "not an image".to_string())?;
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_SIDE);
    limits.max_image_height = Some(MAX_SOURCE_SIDE);
    limits.max_alloc = Some(1 << 30);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| format!("the image can't be read: {e}"))?;
    let side = img.width().min(img.height());
    if side == 0 {
        return Err("the image is empty".into());
    }
    let square = img.crop_imm((img.width() - side) / 2, (img.height() - side) / 2, side, side);
    for target in [1024u32, 512, 256] {
        let scaled = if side > target { square.resize_exact(target, target, image::imageops::FilterType::Triangle) } else { square.clone() };
        let mut out = std::io::Cursor::new(vec![]);
        scaled.to_rgba8().write_to(&mut out, image::ImageFormat::Png).map_err(|e| format!("the image can't be prepared: {e}"))?;
        let out = out.into_inner();
        if out.len() < MAX_UPLOAD {
            return Ok(out);
        }
    }
    Err(format!("the image can't be made smaller than {} MB", MAX_UPLOAD >> 20))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{MockTransport, Response};
    use crate::util::base64_encode;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 100, 50, 255]));
        let mut out = std::io::Cursor::new(vec![]);
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn provider() -> OpenAiCompatible {
        OpenAiCompatible::new(
            &ProviderSettings { provider: OpenAiCompatible::ID.into(), base_url: "https://api.example.com/v1/".into(), model: String::new() },
            "sk-test",
        )
        .unwrap()
    }

    #[test]
    fn settings_are_checked() {
        let s = |url: &str| ProviderSettings { provider: OpenAiCompatible::ID.into(), base_url: url.into(), model: "m".into() };
        assert!(OpenAiCompatible::new(&s("http://api.example.com"), "k").is_err(), "plain http to another computer");
        assert!(OpenAiCompatible::new(&s("https://api.example.com"), "").is_err(), "no key");
        assert!(OpenAiCompatible::new(&s("https://api.example.com"), "a b").is_err());
        assert!(OpenAiCompatible::new(&s("http://localhost:8080/v1"), "").is_ok(), "a local server needs no key");
        assert_eq!(provider().base_url, "https://api.example.com/v1");
        assert_eq!(provider().model, OpenAiCompatible::DEFAULT_MODEL);
    }

    #[test]
    fn a_request_sends_the_image_and_reads_the_results() {
        let result = png(8, 8);
        let answer = format!(r#"{{"created":1,"data":[{{"b64_json":"{0}"}},{{"b64_json":"{0}"}}]}}"#, base64_encode(&result));
        let t = MockTransport::new(vec![Ok(Response::new(200, answer))]);
        let req = VariationRequest { image_png: prepare_image(&png(300, 200)).unwrap(), count: 2, size: 256 };
        let got = provider().variations(&t, &req, &Progress::default(), &Cancel::default()).unwrap();
        assert_eq!(got, vec![result.clone(), result]);
        let sent = &t.sent()[0];
        assert_eq!((sent.method.as_str(), sent.url.as_str()), ("POST", "https://api.example.com/v1/images/variations"));
        assert_eq!(sent.header_value("Authorization"), Some("Bearer sk-test"));
        let body = String::from_utf8_lossy(&sent.body);
        assert!(body.contains("name=\"n\"\r\n\r\n2") && body.contains("256x256") && body.contains("dall-e-2") && body.contains("b64_json"));
    }

    #[test]
    fn errors_are_reported_as_they_happened() {
        let req = VariationRequest { image_png: png(4, 4), count: 1, size: 512 };
        let run = |answer: Result<Response, String>| {
            provider().variations(&MockTransport::new(vec![answer]), &req, &Progress::default(), &Cancel::default())
        };
        assert!(run(Ok(Response::new(401, r#"{"error":{"message":"Incorrect API key provided"}}"#))).unwrap_err().contains("refused the API key"));
        assert!(run(Ok(Response::new(429, "{}"))).unwrap_err().contains("rate limiting"));
        assert!(run(Ok(Response::new(200, r#"{"data":[]}"#))).unwrap_err().contains("no images"));
        assert!(run(Ok(Response::new(200, r#"{"data":[{"b64_json":"bm90IGFuIGltYWdl"}]}"#))).unwrap_err().contains("isn't an image"));
        assert!(run(Err("can't reach api.example.com: timed out".into())).unwrap_err().contains("timed out"));
        let cancel = Cancel::default();
        cancel.cancel();
        let t = MockTransport::new(vec![]);
        assert_eq!(provider().variations(&t, &req, &Progress::default(), &cancel).unwrap_err(), crate::job::CANCELLED);
        assert!(t.sent().is_empty(), "nothing is sent once cancelled");
        let bad = VariationRequest { count: 9, ..req.clone() };
        assert!(provider().variations(&MockTransport::new(vec![]), &bad, &Progress::default(), &Cancel::default()).is_err());
    }

    #[test]
    fn results_given_as_addresses_are_downloaded() {
        let result = png(4, 4);
        let t = MockTransport::new(vec![
            Ok(Response::new(200, r#"{"data":[{"url":"https://cdn.example.com/a.png"}]}"#)),
            Ok(Response::new(200, result.clone())),
        ]);
        let req = VariationRequest { image_png: png(4, 4), count: 1, size: 1024 };
        assert_eq!(provider().variations(&t, &req, &Progress::default(), &Cancel::default()).unwrap(), vec![result]);
        assert_eq!(t.sent()[1].url, "https://cdn.example.com/a.png");
        assert!(t.sent()[1].header_value("Authorization").is_none(), "the key isn't sent to the image host");
    }

    #[test]
    fn images_are_prepared_square_and_small() {
        let out = prepare_image(&png(3000, 2000)).unwrap();
        let img = image::load_from_memory(&out).unwrap();
        assert_eq!((img.width(), img.height()), (1024, 1024));
        let small = image::load_from_memory(&prepare_image(&png(30, 50)).unwrap()).unwrap();
        assert_eq!((small.width(), small.height()), (30, 30));
        assert!(prepare_image(b"nope").is_err());
    }
}
