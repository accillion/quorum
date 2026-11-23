//! The one scrubbing filter (AC 247, AC 251): every manifest, recording and
//! raw-output archive passes through [`Scrubber::scrub`] before it is
//! written or published.
//!
//! It removes, in order:
//! 1. every known secret value (the keys and cookie this process holds);
//! 2. the value of any HTTP header line that carries credentials
//!    (`Cookie`, `Set-Cookie`, `Authorization`, API-key headers);
//! 3. key-shaped tokens (`Bearer …`, vendor key prefixes, `session=…`);
//! 4. the string value of credential-named JSON keys;
//! 5. the query string of any URL that carries a signature or token
//!    (presigned S3, GCS and Azure SAS URLs, `?token=`).
//!
//! No regex crate: the matchers are small hand-written scanners, each
//! with its own test.

pub const REDACTED: &str = "[REDACTED]";

/// Header names whose value is always removed (compared lowercase).
const CREDENTIAL_HEADERS: &[&str] = &[
    "cookie",
    "set-cookie",
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "api-key",
    "x-goog-api-key",
    "x-amz-security-token",
];

/// Token prefixes and the minimum length of the token body after them.
/// The prefix must start the text or follow a byte that cannot be part of
/// a token, so `task-…` never matches `sk-`.
pub(crate) const KEY_PREFIXES: &[(&str, usize)] = &[
    ("sk-ant-", 20),
    ("sk-proj-", 20),
    ("sk-", 20),
    ("xai-", 20),
    ("AIza", 30),
    ("ghp_", 20),
    ("gho_", 20),
    ("github_pat_", 20),
    ("lpa_", 16),
    ("Bearer ", 16),
    ("bearer ", 16),
];

/// JSON keys whose string value is removed (compared lowercase).
const CREDENTIAL_JSON_KEYS: &[&str] = &[
    "cookie",
    "set-cookie",
    "authorization",
    "token",
    "access_token",
    "refresh_token",
    "id_token",
    "session",
    "session_token",
    "share_token",
    "api_key",
    "apikey",
    "x-api-key",
    "password",
    "secret",
    "client_secret",
];

/// Query parameters that mark a URL as presigned or token-bearing.
const SIGNED_QUERY_PARAMS: &[&str] = &[
    "x-amz-signature",
    "x-amz-credential",
    "x-amz-security-token",
    "x-goog-signature",
    "x-goog-credential",
    "signature",
    "sig",
    "token",
    "access_token",
    "key",
    "apikey",
    "api_key",
    "session",
];

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'+' | b'/' | b'=')
}

/// Byte ranges of key-shaped tokens: prefix + at least `min` token bytes.
pub(crate) fn find_prefixed_tokens(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for &(prefix, min) in KEY_PREFIXES {
        let mut from = 0;
        while let Some(rel) = text[from..].find(prefix) {
            let start = from + rel;
            from = start + 1;
            let b = if start > 0 { bytes[start - 1] } else { b' ' };
            if (b.is_ascii_alphanumeric() || b == b'_' || b == b'-') && !prefix.ends_with(' ') {
                continue;
            }
            let body_start = start + prefix.len();
            let mut end = body_start;
            while end < bytes.len() && is_token_byte(bytes[end]) {
                end += 1;
            }
            if end - body_start >= min && !out.iter().any(|&(s, e)| start < e && s < end) {
                out.push((start, end));
            }
        }
    }
    out.sort();
    out
}

/// Byte ranges of the values of `session` cookie pairs: 8+ cookie-value
/// bytes (token characters and `%`) after `session=`. A placeholder such
/// as `<value>` or the `[REDACTED]` marker starts with a non-token byte
/// and does not match.
pub(crate) fn find_session_cookies(text: &str) -> Vec<(usize, usize)> {
    const NEEDLE: &str = "session=";
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = text[from..].find(NEEDLE) {
        let start = from + rel;
        from = start + 1;
        if start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
            continue;
        }
        let vstart = start + NEEDLE.len();
        let mut end = vstart;
        while end < bytes.len() && (is_token_byte(bytes[end]) || bytes[end] == b'%') {
            end += 1;
        }
        if end - vstart >= 8 {
            out.push((vstart, end));
        }
    }
    out
}

fn replace_ranges(text: &str, mut ranges: Vec<(usize, usize)>) -> String {
    if ranges.is_empty() {
        return text.to_string();
    }
    ranges.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in ranges {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (s, e) in merged {
        out.push_str(&text[at..s]);
        out.push_str(REDACTED);
        at = e;
    }
    out.push_str(&text[at..]);
    out
}

fn scrub_header_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        let hit = CREDENTIAL_HEADERS
            .iter()
            .find(|h| lower.starts_with(*h) && lower[h.len()..].trim_start().starts_with(':'));
        match hit {
            Some(_) => {
                let indent = &line[..line.len() - trimmed.len()];
                let colon = trimmed.find(':').unwrap_or(trimmed.len());
                let eol = if line.ends_with("\r\n") {
                    "\r\n"
                } else if line.ends_with('\n') {
                    "\n"
                } else {
                    ""
                };
                out.push_str(indent);
                out.push_str(trimmed[..colon].trim_end());
                out.push_str(": ");
                out.push_str(REDACTED);
                out.push_str(eol);
            }
            None => out.push_str(line),
        }
    }
    out
}

/// End of a JSON string starting at `open` (the index of its opening
/// quote): the index of the closing quote, honouring backslash escapes.
fn json_string_end(bytes: &[u8], open: usize) -> Option<usize> {
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

fn find_json_credential_values(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        let Some(kend) = json_string_end(bytes, i) else {
            break;
        };
        let key = text[i + 1..kend].to_ascii_lowercase();
        let mut j = kend + 1;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b':' && CREDENTIAL_JSON_KEYS.contains(&key.as_str()) {
            j += 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'"' {
                if let Some(vend) = json_string_end(bytes, j) {
                    if vend > j + 1 {
                        out.push((j + 1, vend));
                    }
                    i = vend + 1;
                    continue;
                }
            }
        }
        i = kend + 1;
    }
    out
}

fn find_signed_url_queries(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    for scheme in ["https://", "http://"] {
        let mut from = 0;
        while let Some(rel) = text[from..].find(scheme) {
            let start = from + rel;
            let mut end = start;
            while end < bytes.len()
                && !bytes[end].is_ascii_whitespace()
                && !matches!(bytes[end], b'"' | b'\'' | b'<' | b'>' | b'`' | b')')
            {
                end += 1;
            }
            from = end.max(start + 1);
            let url = &text[start..end];
            let Some(q) = url.find('?') else { continue };
            let query = &url[q + 1..];
            let signed = query.split('&').any(|pair| {
                let name = pair.split('=').next().unwrap_or("").to_ascii_lowercase();
                SIGNED_QUERY_PARAMS.contains(&name.as_str())
            });
            if signed {
                out.push((start + q + 1, end));
            }
        }
    }
    out
}

/// The scrubbing filter. Holds the exact secret values this process knows.
#[derive(Default, Clone)]
pub struct Scrubber {
    known: Vec<String>,
}

impl Scrubber {
    pub fn new(known_values: Vec<String>) -> Self {
        let mut known: Vec<String> = known_values.into_iter().filter(|v| v.len() >= 8).collect();
        // Longest first, so a value that contains another is removed whole.
        known.sort_by_key(|v| std::cmp::Reverse(v.len()));
        known.dedup();
        Self { known }
    }

    pub fn scrub(&self, text: &str) -> String {
        let mut s = text.to_string();
        for v in &self.known {
            if s.contains(v.as_str()) {
                s = s.replace(v.as_str(), REDACTED);
            }
        }
        let s = scrub_header_lines(&s);
        let mut ranges = find_prefixed_tokens(&s);
        ranges.extend(find_session_cookies(&s));
        ranges.extend(find_json_credential_values(&s));
        ranges.extend(find_signed_url_queries(&s));
        replace_ranges(&s, ranges)
    }

    /// Scrub every string inside a JSON value, recursively. Object keys
    /// named like credentials lose their value whatever its type.
    pub fn scrub_json(&self, v: &serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        match v {
            Value::String(s) => Value::String(self.scrub(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.scrub_json(x)).collect()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, x)| {
                        if CREDENTIAL_JSON_KEYS.contains(&k.to_ascii_lowercase().as_str())
                            && !x.is_null()
                        {
                            (k.clone(), Value::String(REDACTED.into()))
                        } else {
                            (k.clone(), self.scrub_json(x))
                        }
                    })
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a key-shaped value at runtime so this file itself never
    /// contains one (the key guard scans tracked files under bench/).
    fn fake(prefix: &str, n: usize) -> String {
        format!("{prefix}{}", "Ab9".repeat(n / 3 + 1))
    }

    #[test]
    fn known_values_are_removed_exactly() {
        let cookie = "c00kie-value-123456";
        let s = Scrubber::new(vec![cookie.into()]);
        let out = s.scrub(&format!("a {cookie} b {cookie}"));
        assert!(!out.contains(cookie));
        assert_eq!(out.matches(REDACTED).count(), 2);
    }

    #[test]
    fn short_known_values_are_ignored() {
        let s = Scrubber::new(vec!["abc".into()]);
        assert_eq!(s.scrub("abc abc"), "abc abc");
    }

    #[test]
    fn credential_header_lines_lose_their_value() {
        let s = Scrubber::default();
        let text = "GET / HTTP/1.1\r\nCookie: session=x; other=y\r\nAuthorization: Basic Zm9v\r\nX-Api-Key : k\nAccept: */*\n";
        let out = s.scrub(text);
        assert!(out.contains("Cookie: [REDACTED]\r\n"));
        assert!(out.contains("Authorization: [REDACTED]\r\n"));
        assert!(out.contains("X-Api-Key: [REDACTED]\n"));
        assert!(out.contains("Accept: */*"));
        assert!(!out.contains("Zm9v"));
    }

    #[test]
    fn key_shaped_tokens_are_removed() {
        let s = Scrubber::default();
        for prefix in ["sk-", "sk-ant-", "xai-", "AIza", "ghp_", "lpa_"] {
            let k = fake(prefix, 40);
            let out = s.scrub(&format!("key={k} end"));
            assert!(!out.contains(&k), "{prefix} survived: {out}");
            assert!(out.ends_with(" end"));
        }
        let bearer = format!("Bearer {}", "x1".repeat(20));
        assert!(!s.scrub(&bearer).contains("x1x1"));
    }

    #[test]
    fn prefix_inside_a_word_is_not_a_key() {
        let s = Scrubber::default();
        let text = format!("task-{}", "a".repeat(30));
        assert_eq!(s.scrub(&text), text);
        let short = "sk-short";
        assert_eq!(s.scrub(short), short);
    }

    #[test]
    fn session_cookie_pairs_are_removed() {
        let s = Scrubber::default();
        let out = s.scrub(&format!("cookie jar: {}=abcdefgh12345; path=/", "session"));
        assert_eq!(out, "cookie jar: session=[REDACTED]; path=/");
        // a different name ending in `session` is not the cookie
        assert_eq!(
            s.scrub("my_session=abcdefgh12345"),
            "my_session=abcdefgh12345"
        );
    }

    #[test]
    fn json_credential_values_are_removed() {
        let s = Scrubber::default();
        let out = s.scrub(r#"{"token": "abc\"def", "name": "kept", "Authorization":"x"}"#);
        assert_eq!(
            out,
            r#"{"token": "[REDACTED]", "name": "kept", "Authorization":"[REDACTED]"}"#
        );
    }

    #[test]
    fn presigned_url_queries_are_removed() {
        let s = Scrubber::default();
        let url =
            "https://b.s3.amazonaws.com/o.json?X-Amz-Credential=AK%2F&X-Amz-Signature=deadbeef";
        let out = s.scrub(&format!("see {url} now"));
        assert_eq!(out, "see https://b.s3.amazonaws.com/o.json?[REDACTED] now");
        let plain = "https://github.com/o/r/pull/1?tab=files";
        assert_eq!(s.scrub(plain), plain);
        let sas = "https://x.blob.core.windows.net/c/b?sv=1&sig=abc";
        assert!(!s.scrub(sas).contains("sig=abc"));
    }

    #[test]
    fn scrub_json_recurses_and_blanks_credential_keys() {
        let s = Scrubber::new(vec!["supersecretvalue".into()]);
        let v = serde_json::json!({
            "cookie": {"nested": 1},
            "list": ["ok", "supersecretvalue"],
            "headers": {"Set-Cookie": format!("{}=abcdefghijk", "session")}
        });
        let out = s.scrub_json(&v).to_string();
        assert!(!out.contains("supersecretvalue"));
        assert!(!out.contains("abcdefghijk"));
        assert!(out.contains("\"cookie\":\"[REDACTED]\""));
    }
}
