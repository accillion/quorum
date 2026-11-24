//! Frozen prompt files: `bench/prompts/single-v1.md`, `bench/RUBRIC.md`
//! and `bench/rubrics/*.md`. Each is a readable Markdown document whose
//! model-facing text sits between markers, so the exact bytes sent are
//! unambiguous:
//!
//! ```text
//! version: 1
//! <!-- SYSTEM BEGIN -->
//! ...system text...
//! <!-- SYSTEM END -->
//! <!-- USER BEGIN -->
//! ...user template with {{PLACEHOLDERS}}...
//! <!-- USER END -->
//! ```
//!
//! One newline after a BEGIN marker and one before an END marker belong
//! to the markers, not the text.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub version: u32,
    pub system: String,
    pub user: String,
    /// SHA-256 of the whole file: the digest the freeze records.
    pub sha256: String,
}

#[derive(thiserror::Error, Debug)]
pub enum TemplateError {
    #[error("{0}: {1}")]
    Bad(String, String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("placeholder {{{{{0}}}}} was not filled")]
    Unfilled(String),
}

fn section(text: &str, name: &str, file: &str) -> Result<String, TemplateError> {
    let begin = format!("<!-- {name} BEGIN -->");
    let end = format!("<!-- {name} END -->");
    let b = text
        .find(&begin)
        .ok_or_else(|| TemplateError::Bad(file.into(), format!("no {begin}")))?;
    let rest = &text[b + begin.len()..];
    let e = rest
        .find(&end)
        .ok_or_else(|| TemplateError::Bad(file.into(), format!("no {end}")))?;
    let body = &rest[..e];
    let body = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix('\n'))
        .unwrap_or(body);
    let body = body
        .strip_suffix("\r\n")
        .or_else(|| body.strip_suffix('\n'))
        .unwrap_or(body);
    if body.contains('\r') {
        return Err(TemplateError::Bad(
            file.into(),
            "CRLF in a prompt section".into(),
        ));
    }
    Ok(body.to_string())
}

impl Template {
    pub fn parse(text: &str, file: &str) -> Result<Self, TemplateError> {
        let version = text
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("version:")
                    .map(|v| v.trim().to_string())
            })
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| TemplateError::Bad(file.into(), "no `version: N` line".into()))?;
        Ok(Self {
            version,
            system: section(text, "SYSTEM", file)?,
            user: section(text, "USER", file)?,
            sha256: crate::corpus::sha256_hex(text.as_bytes()),
        })
    }

    pub fn load(path: &Path) -> Result<Self, TemplateError> {
        let bytes = std::fs::read(path)?;
        Self::parse(
            &String::from_utf8_lossy(&bytes),
            &path.display().to_string(),
        )
    }

    /// The user text with every `{{NAME}}` replaced. Values are inserted
    /// verbatim and never re-scanned, so case content cannot inject a
    /// placeholder.
    pub fn render_user(&self, values: &BTreeMap<&str, &str>) -> Result<String, TemplateError> {
        let mut out = String::with_capacity(self.user.len());
        let mut rest = self.user.as_str();
        while let Some(i) = rest.find("{{") {
            out.push_str(&rest[..i]);
            let after = &rest[i + 2..];
            let j = after
                .find("}}")
                .ok_or_else(|| TemplateError::Unfilled("<unterminated>".into()))?;
            let name = &after[..j];
            let v = values
                .get(name)
                .ok_or_else(|| TemplateError::Unfilled(name.into()))?;
            out.push_str(v);
            rest = &after[j + 2..];
        }
        out.push_str(rest);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "# Title\nversion: 3\n\n<!-- SYSTEM BEGIN -->\nsys line\n<!-- SYSTEM END -->\n\n<!-- USER BEGIN -->\nQ:\n{{BUNDLE}}\n<!-- USER END -->\n";

    #[test]
    fn sections_and_rendering() {
        let t = Template::parse(T, "t").unwrap();
        assert_eq!(
            (t.version, t.system.as_str(), t.user.as_str()),
            (3, "sys line", "Q:\n{{BUNDLE}}")
        );
        let mut v = BTreeMap::new();
        v.insert("BUNDLE", "data with {{BUNDLE}} inside");
        assert_eq!(
            t.render_user(&v).unwrap(),
            "Q:\ndata with {{BUNDLE}} inside"
        );
        assert!(matches!(
            t.render_user(&BTreeMap::new()),
            Err(TemplateError::Unfilled(_))
        ));
    }

    #[test]
    fn missing_markers_or_version_are_errors() {
        assert!(Template::parse(&T.replace("version: 3", ""), "t").is_err());
        assert!(Template::parse(&T.replace("<!-- USER END -->", ""), "t").is_err());
    }
}
