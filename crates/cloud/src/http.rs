//! HTTP requests behind a [`Transport`], so the clients can be tested with a scripted
//! [`MockTransport`] and the desktop app sends them with [`NativeTransport`] (ureq over rustls).
//! The web build has [`Unavailable`].

use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Largest response body read (bigger ones are an error, not a memory spike).
pub const MAX_RESPONSE_BYTES: u64 = 96 << 20;

/// An HTTP request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn new(method: &str, url: impl Into<String>) -> Self {
        Self { method: method.into(), url: url.into(), headers: vec![], body: vec![] }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    /// A header's value (names compared ignoring case).
    pub fn header_value(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// An HTTP response: any status (4xx and 5xx are responses, not errors).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self { status, headers: vec![], body: body.into() }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body as JSON.
    pub fn json(&self) -> Result<serde_json::Value, String> {
        serde_json::from_slice(&self.body).map_err(|e| format!("the server's answer isn't JSON ({e})"))
    }

    /// The server's own error message, when its body carries one (`error.message`, `error` or
    /// `error_description`), else the status.
    pub fn error_message(&self) -> String {
        let v = serde_json::from_slice::<serde_json::Value>(&self.body).ok();
        let msg = v.as_ref().and_then(|v| {
            v.pointer("/error/message")
                .and_then(|m| m.as_str())
                .or_else(|| v.get("error_description").and_then(|m| m.as_str()))
                .or_else(|| v.get("error").and_then(|m| m.as_str()))
                .map(String::from)
        });
        match msg {
            Some(m) => format!("HTTP {}: {m}", self.status),
            None => format!("HTTP {}", self.status),
        }
    }
}

/// Something that sends requests. An `Err` is a failure to talk to the server at all (no network,
/// DNS, TLS, timeout); every answer the server gives is an `Ok`.
pub trait Transport: Send + Sync {
    fn send(&self, req: &Request) -> Result<Response, String>;
}

/// Requests over the network: ureq with rustls and Mozilla's root certificates.
#[cfg(not(target_arch = "wasm32"))]
pub struct NativeTransport {
    agent: ureq::Agent,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeTransport {
    /// With a timeout for each whole request.
    pub fn new(timeout: Duration) -> Self {
        let config = ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).user_agent("DLightroom").build();
        Self { agent: config.into() }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for NativeTransport {
    fn default() -> Self {
        Self::new(Duration::from_secs(120))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Transport for NativeTransport {
    fn send(&self, req: &Request) -> Result<Response, String> {
        let mut builder = ureq::http::Request::builder().method(req.method.as_str()).uri(req.url.as_str());
        for (n, v) in &req.headers {
            builder = builder.header(n.as_str(), v.as_str());
        }
        let request = builder.body(req.body.clone()).map_err(|e| format!("bad request: {e}"))?;
        let resp = self.agent.run(request).map_err(|e| format!("can't reach {}: {e}", host_of(&req.url)))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().iter().filter_map(|(n, v)| Some((n.as_str().to_string(), v.to_str().ok()?.to_string()))).collect();
        let body = resp
            .into_body()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(|e| format!("the answer from {} couldn't be read: {e}", host_of(&req.url)))?;
        Ok(Response { status, headers, body })
    }
}

/// The host part of `url`, for messages (no paths, queries or tokens in them).
pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

/// The web build: nothing reaches the network from here.
pub struct Unavailable;

impl Transport for Unavailable {
    fn send(&self, _: &Request) -> Result<Response, String> {
        Err("network features need the desktop app".into())
    }
}

/// The transport the desktop app uses: [`NativeTransport`]; on the web, [`Unavailable`].
pub fn default_transport() -> Arc<dyn Transport> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Arc::new(NativeTransport::default())
    }
    #[cfg(target_arch = "wasm32")]
    {
        Arc::new(Unavailable)
    }
}

/// A transport that answers from a script, recording the requests: for tests.
#[derive(Clone, Default)]
pub struct MockTransport {
    answers: Arc<Mutex<Vec<Result<Response, String>>>>,
    pub requests: Arc<Mutex<Vec<Request>>>,
}

impl MockTransport {
    /// Answers given in order, one per request.
    pub fn new(answers: Vec<Result<Response, String>>) -> Self {
        Self { answers: Arc::new(Mutex::new(answers)), requests: Arc::default() }
    }

    pub fn sent(&self) -> Vec<Request> {
        self.requests.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

impl Transport for MockTransport {
    fn send(&self, req: &Request) -> Result<Response, String> {
        if let Ok(mut r) = self.requests.lock() {
            r.push(req.clone());
        }
        let mut answers = self.answers.lock().map_err(|_| "mock poisoned".to_string())?;
        if answers.is_empty() {
            return Err("no more scripted answers".into());
        }
        answers.remove(0)
    }
}

/// `s` percent-encoded for a URL query or a form body (RFC 3986 unreserved characters kept).
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `application/x-www-form-urlencoded` body of `pairs`.
pub fn form(pairs: &[(&str, &str)]) -> Vec<u8> {
    pairs.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect::<Vec<_>>().join("&").into_bytes()
}

/// A `multipart/form-data` body: its content type (with the boundary) and bytes.
pub struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

impl Multipart {
    pub fn new() -> Self {
        // A fixed boundary; `finish` refuses a body whose parts happen to contain it.
        Self { boundary: "dlightroom-part-boundary-7f3a9c1e5b".into(), body: vec![] }
    }

    pub fn text(mut self, name: &str, value: &str) -> Self {
        self.body.extend_from_slice(format!("--{}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n", self.boundary).as_bytes());
        self
    }

    pub fn file(mut self, name: &str, filename: &str, mime: &str, data: &[u8]) -> Self {
        self.body.extend_from_slice(
            format!(
                "--{}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n",
                self.boundary
            )
            .as_bytes(),
        );
        self.body.extend_from_slice(data);
        self.body.extend_from_slice(b"\r\n");
        self
    }

    /// (content type, body). Fails if a part happens to contain the boundary.
    pub fn finish(mut self) -> Result<(String, Vec<u8>), String> {
        let marker = format!("--{}", self.boundary);
        let count = self.body.windows(marker.len()).filter(|w| *w == marker.as_bytes()).count();
        let parts = self.body.windows(b"Content-Disposition".len()).filter(|w| *w == b"Content-Disposition").count();
        if count != parts {
            return Err("a part of the upload contains the multipart boundary".into());
        }
        self.body.extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        Ok((format!("multipart/form-data; boundary={}", self.boundary), self.body))
    }
}

impl Default for Multipart {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_and_forms() {
        assert_eq!(encode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
        assert_eq!(String::from_utf8(form(&[("grant_type", "refresh_token"), ("x", "1 2")])).unwrap(), "grant_type=refresh_token&x=1%202");
        assert_eq!(host_of("https://oauth2.googleapis.com/token?x=secret"), "oauth2.googleapis.com");
    }

    #[test]
    fn multipart_bodies_carry_every_part() {
        let (ct, body) = Multipart::new().text("n", "2").file("image", "a.png", "image/png", b"\x89PNG").finish().unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(ct.starts_with("multipart/form-data; boundary="));
        assert!(text.contains("name=\"n\"\r\n\r\n2\r\n") && text.contains("filename=\"a.png\"") && text.ends_with("--\r\n"));
        let evil = Multipart::new().file("f", "x", "text/plain", b"--dlightroom-part-boundary-7f3a9c1e5b");
        assert!(evil.finish().is_err());
    }

    #[test]
    fn error_messages_come_from_the_body() {
        assert_eq!(Response::new(401, r#"{"error":{"message":"Incorrect API key"}}"#).error_message(), "HTTP 401: Incorrect API key");
        assert_eq!(Response::new(400, r#"{"error":"invalid_grant","error_description":"Bad Request"}"#).error_message(), "HTTP 400: Bad Request");
        assert_eq!(Response::new(503, "busy").error_message(), "HTTP 503");
    }

    #[test]
    fn the_mock_answers_in_order_and_records() {
        let m = MockTransport::new(vec![Ok(Response::new(200, "a")), Err("down".into())]);
        assert_eq!(m.send(&Request::new("GET", "https://x/1")).unwrap().body, b"a");
        assert!(m.send(&Request::new("GET", "https://x/2")).is_err());
        assert!(m.send(&Request::new("GET", "https://x/3")).is_err(), "out of answers");
        assert_eq!(m.sent().len(), 3);
    }
}
