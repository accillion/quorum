//! `Secret` — wraps the session cookie. Debug/Display redact; only
//! `expose()` returns the inner value.
//!
//! Derives `Clone` so `LippaClient::renewed_cookie()` can return
//! `Option<Secret>` without moving from under the std::sync::Mutex
//! that mirrors the cookie. Cloning a String is cheap.

#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: String) -> Self {
        Self(s)
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

impl PartialEq for Secret {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for Secret {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts() {
        let s = Secret::new("hunter2".into());
        assert_eq!(format!("{s:?}"), "Secret(<redacted>)");
        assert_eq!(format!("{s}"), "<redacted>");
        assert_eq!(s.expose(), "hunter2");
    }

    #[test]
    fn equality_via_inner() {
        assert_eq!(Secret::new("a".into()), Secret::new("a".into()));
        assert_ne!(Secret::new("a".into()), Secret::new("b".into()));
    }
}
