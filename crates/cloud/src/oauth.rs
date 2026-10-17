//! OAuth 2.0 for installed apps (RFC 8252): the system browser asks the user, the app listens on a
//! loopback address for the answer, and PKCE (RFC 7636, S256) keeps the code useful to this app
//! alone. No secret is built into DLightroom: the client id (and, for Google's "Desktop app"
//! clients, the client secret Google issues with it, which installed apps can't keep secret
//! anyway) come from the user's own OAuth client.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::http::{Request, Transport, encode, form};
use crate::job::Cancel;
use crate::util::{base64url, random_bytes};

/// An OAuth client: who the app is to the provider, and the provider's endpoints.
#[derive(Clone, Debug, PartialEq)]
pub struct OAuthClient {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub auth_url: String,
    pub token_url: String,
    pub scope: String,
}

/// A PKCE verifier and challenge, and the `state` that ties the answer to this request.
#[derive(Clone, Debug, PartialEq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

impl Pkce {
    pub fn new() -> Result<Self, String> {
        let verifier = base64url(&random_bytes(48)?);
        let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
        Ok(Self { verifier, challenge, state: base64url(&random_bytes(24)?) })
    }
}

/// Tokens the provider issued: the access token, when it expires (Unix seconds) and, when given,
/// the refresh token that gets new ones.
#[derive(Clone, Debug, PartialEq)]
pub struct Tokens {
    pub access_token: String,
    pub expires_at: u64,
    pub refresh_token: Option<String>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Tokens {
    /// Still good for at least a minute.
    pub fn fresh(&self) -> bool {
        self.expires_at > now() + 60
    }
}

impl OAuthClient {
    /// The browser address that asks the user, answering to `redirect_uri`.
    pub fn authorization_url(&self, pkce: &Pkce, redirect_uri: &str) -> String {
        let q = [
            ("client_id", self.client_id.as_str()),
            ("redirect_uri", redirect_uri),
            ("response_type", "code"),
            ("scope", self.scope.as_str()),
            ("code_challenge", pkce.challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", pkce.state.as_str()),
            // A refresh token, so the user signs in once.
            ("access_type", "offline"),
            ("prompt", "consent"),
        ];
        let query: Vec<String> = q.iter().map(|(k, v)| format!("{k}={}", encode(v))).collect();
        format!("{}?{}", self.auth_url, query.join("&"))
    }

    fn token_request<'a>(&'a self, t: &dyn Transport, mut pairs: Vec<(&'a str, &'a str)>) -> Result<Tokens, String> {
        pairs.push(("client_id", &self.client_id));
        if let Some(s) = &self.client_secret {
            pairs.push(("client_secret", s));
        }
        let r = Request::new("POST", self.token_url.clone()).header("Content-Type", "application/x-www-form-urlencoded").body(form(&pairs));
        let resp = t.send(&r)?;
        if !resp.ok() {
            return Err(format!("the sign-in was refused ({})", resp.error_message()));
        }
        let v = resp.json()?;
        let access_token =
            v.get("access_token").and_then(|a| a.as_str()).filter(|a| !a.is_empty()).ok_or("the answer has no access token")?.to_string();
        let expires_in = v.get("expires_in").and_then(|e| e.as_u64()).unwrap_or(3600).min(86_400 * 30);
        let refresh_token = v.get("refresh_token").and_then(|r| r.as_str()).filter(|r| !r.is_empty()).map(String::from);
        Ok(Tokens { access_token, expires_at: now() + expires_in, refresh_token })
    }

    /// Trade the authorization `code` for tokens.
    pub fn exchange(&self, t: &dyn Transport, code: &str, pkce: &Pkce, redirect_uri: &str) -> Result<Tokens, String> {
        self.token_request(
            t,
            vec![("grant_type", "authorization_code"), ("code", code), ("code_verifier", &pkce.verifier), ("redirect_uri", redirect_uri)],
        )
    }

    /// New tokens from a refresh token (the refresh token stays the same unless the answer says).
    pub fn refresh(&self, t: &dyn Transport, refresh_token: &str) -> Result<Tokens, String> {
        let mut tok = self.token_request(t, vec![("grant_type", "refresh_token"), ("refresh_token", refresh_token)])?;
        tok.refresh_token.get_or_insert_with(|| refresh_token.to_string());
        Ok(tok)
    }
}

/// A one-shot listener on 127.0.0.1 for the browser's redirect.
pub struct Loopback {
    listener: TcpListener,
    pub redirect_uri: String,
}

/// Largest request the loopback listener reads.
const MAX_REDIRECT_REQUEST: usize = 16 << 10;

impl Loopback {
    /// Listen on a free port of 127.0.0.1.
    pub fn bind() -> Result<Self, String> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("can't listen for the sign-in: {e}"))?;
        let port = listener.local_addr().map_err(|e| format!("can't listen for the sign-in: {e}"))?.port();
        listener.set_nonblocking(true).map_err(|e| format!("can't listen for the sign-in: {e}"))?;
        Ok(Self { listener, redirect_uri: format!("http://127.0.0.1:{port}") })
    }

    /// Wait for the browser to come back with the authorization code for `state`: the code, or why
    /// there is none (the user said no, the time ran out, or it was cancelled).
    pub fn wait(&self, state: &str, timeout: Duration, cancel: &Cancel) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        loop {
            cancel.check()?;
            if Instant::now() >= deadline {
                return Err("the sign-in wasn't finished in time".into());
            }
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if let Some(answer) = handle(stream, state) {
                        return answer;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(40)),
                Err(e) => return Err(format!("the sign-in couldn't be received: {e}")),
            }
        }
    }
}

/// Read one request from the browser and answer it: `None` for a request that isn't the redirect
/// (a favicon), else the code or the error it carries.
fn handle(mut stream: TcpStream, state: &str) -> Option<Result<String, String>> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = vec![0u8; MAX_REDIRECT_REQUEST];
    let mut len = 0;
    while len < buf.len() {
        match stream.read(&mut buf[len..]) {
            Ok(0) => break,
            Ok(n) => {
                len += n;
                if buf[..len].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(buf.get(..len).unwrap_or_default()).to_string();
    let target = text.lines().next().and_then(|l| l.strip_prefix("GET ")).and_then(|l| l.split(' ').next()).unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/" {
        reply(&mut stream, 404, "Not found");
        return None;
    }
    let param = |k: &str| query.split('&').filter_map(|kv| kv.split_once('=')).find(|(n, _)| *n == k).map(|(_, v)| decode(v));
    let answer = if param("state").as_deref() != Some(state) {
        Err("the sign-in answer didn't match this request".to_string())
    } else if let Some(e) = param("error") {
        Err(if e == "access_denied" { "access was not granted".to_string() } else { format!("the sign-in failed: {e}") })
    } else {
        param("code").filter(|c| !c.is_empty()).ok_or_else(|| "the sign-in answer has no code".to_string())
    };
    match &answer {
        Ok(_) => reply(&mut stream, 200, "DLightroom is connected. You can close this tab and go back to DLightroom."),
        Err(e) => reply(&mut stream, 400, &format!("DLightroom couldn't connect: {e}. You can close this tab.")),
    }
    Some(answer)
}

fn reply(stream: &mut TcpStream, status: u16, text: &str) {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>DLightroom</title></head><body style=\"font-family:system-ui;margin:3em\"><p>{}</p></body></html>",
        text.replace('&', "&amp;").replace('<', "&lt;")
    );
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        if status == 200 { "OK" } else { "Error" },
        body.len()
    );
    // The browser may have gone away: the answer is a courtesy.
    let _ = stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(body.as_bytes()));
}

/// Percent-decoding of a query value (`+` is a space).
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&c) = bytes.get(i) {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (c, hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
                continue;
            }
            (b'+', _) => out.push(b' '),
            (b, _) => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{MockTransport, Response};

    fn client() -> OAuthClient {
        OAuthClient {
            client_id: "123.apps.example".into(),
            client_secret: Some("shh".into()),
            auth_url: "https://accounts.example.com/auth".into(),
            token_url: "https://oauth.example.com/token".into(),
            scope: "https://www.googleapis.com/auth/drive.file".into(),
        }
    }

    #[test]
    fn pkce_challenge_is_the_s256_of_the_verifier() {
        let p = Pkce::new().unwrap();
        assert!(p.verifier.len() >= 43 && p.verifier.len() <= 128);
        assert_eq!(p.challenge, base64url(&Sha256::digest(p.verifier.as_bytes())));
        assert_ne!(Pkce::new().unwrap().state, p.state);
    }

    #[test]
    fn the_authorization_url_asks_for_offline_access_with_pkce() {
        let p = Pkce { verifier: "v".into(), challenge: "c".into(), state: "s".into() };
        let url = client().authorization_url(&p, "http://127.0.0.1:5000");
        assert!(url.starts_with("https://accounts.example.com/auth?client_id=123.apps.example"));
        for part in [
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A5000",
            "code_challenge=c",
            "code_challenge_method=S256",
            "state=s",
            "access_type=offline",
            "scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.file",
        ] {
            assert!(url.contains(part), "{part} in {url}");
        }
        assert!(!url.contains("shh"), "the secret never goes in the browser address");
    }

    #[test]
    fn codes_are_exchanged_and_tokens_refreshed() {
        let t = MockTransport::new(vec![
            Ok(Response::new(200, r#"{"access_token":"at1","expires_in":3599,"refresh_token":"rt"}"#)),
            Ok(Response::new(200, r#"{"access_token":"at2","expires_in":3599}"#)),
            Ok(Response::new(400, r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#)),
        ]);
        let p = Pkce { verifier: "ver".into(), challenge: "c".into(), state: "s".into() };
        let tok = client().exchange(&t, "the code", &p, "http://127.0.0.1:1").unwrap();
        assert_eq!((tok.access_token.as_str(), tok.refresh_token.as_deref()), ("at1", Some("rt")));
        assert!(tok.fresh());
        let body = String::from_utf8(t.sent()[0].body.clone()).unwrap();
        assert!(
            body.contains("grant_type=authorization_code")
                && body.contains("code=the%20code")
                && body.contains("code_verifier=ver")
                && body.contains("client_secret=shh")
        );
        let tok2 = client().refresh(&t, "rt").unwrap();
        assert_eq!((tok2.access_token.as_str(), tok2.refresh_token.as_deref()), ("at2", Some("rt")), "the refresh token is kept");
        assert!(client().refresh(&t, "rt").unwrap_err().contains("expired or revoked"));
    }

    fn browser(port_uri: &str, path_query: &str) -> String {
        let addr = port_uri.trim_start_matches("http://");
        let mut s = TcpStream::connect(addr).unwrap();
        write!(s, "GET {path_query} HTTP/1.1\r\nHost: {addr}\r\n\r\n").unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    #[test]
    fn the_loopback_listener_takes_the_code_for_its_state() {
        let lb = Loopback::bind().unwrap();
        let uri = lb.redirect_uri.clone();
        let t = std::thread::spawn(move || {
            assert!(browser(&uri, "/favicon.ico").starts_with("HTTP/1.1 404"));
            browser(&uri, "/?state=st&code=4%2F0AbC&scope=x")
        });
        let code = lb.wait("st", Duration::from_secs(10), &Cancel::default()).unwrap();
        assert_eq!(code, "4/0AbC");
        assert!(t.join().unwrap().contains("DLightroom is connected"));
    }

    #[test]
    fn a_refusal_a_wrong_state_and_cancelling_are_errors() {
        let lb = Loopback::bind().unwrap();
        let uri = lb.redirect_uri.clone();
        let t = std::thread::spawn(move || browser(&uri, "/?state=st&error=access_denied"));
        assert_eq!(lb.wait("st", Duration::from_secs(10), &Cancel::default()).unwrap_err(), "access was not granted");
        t.join().unwrap();
        let lb = Loopback::bind().unwrap();
        let uri = lb.redirect_uri.clone();
        let t = std::thread::spawn(move || browser(&uri, "/?state=other&code=x"));
        assert!(lb.wait("st", Duration::from_secs(10), &Cancel::default()).unwrap_err().contains("didn't match"));
        t.join().unwrap();
        let cancel = Cancel::default();
        cancel.cancel();
        assert_eq!(Loopback::bind().unwrap().wait("st", Duration::from_secs(10), &cancel).unwrap_err(), crate::job::CANCELLED);
        assert!(Loopback::bind().unwrap().wait("st", Duration::from_millis(50), &Cancel::default()).unwrap_err().contains("in time"));
    }

    #[test]
    fn query_values_decode() {
        assert_eq!(decode("a%2Fb+c%"), "a/b c%");
        assert_eq!(decode("%zz"), "%zz");
    }
}
