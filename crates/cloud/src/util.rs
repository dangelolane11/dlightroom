//! Base64 (standard and URL-safe) and secure random bytes.

const STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn encode_with(data: &[u8], table: &[u8; 64], pad: bool) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let chars = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for (i, c) in chars.iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(table[*c as usize]));
            } else if pad {
                out.push('=');
            }
        }
    }
    out
}

/// Standard base64 with padding.
pub fn base64_encode(data: &[u8]) -> String {
    encode_with(data, STD, true)
}

/// URL-safe base64 without padding (RFC 4648 §5), as PKCE uses it.
pub fn base64url(data: &[u8]) -> String {
    encode_with(data, URL, false)
}

/// Standard or URL-safe base64, padded or not, whitespace ignored; `None` for anything else.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// `n` bytes from the operating system's secure random source.
pub fn random_bytes(n: usize) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; n];
    #[cfg(not(target_arch = "wasm32"))]
    {
        getrandom::fill(&mut buf).map_err(|e| format!("no secure random numbers: {e}"))?;
        Ok(buf)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = &mut buf;
        Err("this needs the desktop app".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0xfb, 0xff, 0xfe]] {
            assert_eq!(base64_decode(&base64_encode(data)).unwrap(), data);
            assert_eq!(base64_decode(&base64url(data)).unwrap(), data);
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
        assert_eq!(base64_decode("not base64!"), None);
    }

    #[test]
    fn random_bytes_differ() {
        let (a, b) = (random_bytes(32).unwrap(), random_bytes(32).unwrap());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
