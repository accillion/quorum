//! The corpus: case files, their validation and the manifest (ACs
//! 242–246). The format is specified in `bench/CORPUS.md`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::claims::MAX_WORDS;

/// Clean cases must be at least 12 months old at the design freeze
/// (AC 243). The freeze is 2026-11-23, so the cutoff is this date.
pub const CLEAN_CUTOFF: &str = "2025-11-23";
pub const MAX_DIFF_FILES: usize = 5;
pub const MAX_DIFF_LINES: usize = 300;
pub const ALLOWED_LICENCES: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "MIT OR Apache-2.0",
    "Apache-2.0 OR MIT",
];
pub const LANGUAGES: &[&str] = &["rust", "typescript", "python"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Bug,
    Clean,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseToml {
    pub schema: u32,
    pub id: String,
    pub kind: Kind,
    pub language: String,
    pub source_repo: String,
    pub source_commit: String,
    pub source_commit_url: String,
    pub source_commit_date: String,
    pub base_commit: String,
    #[serde(default)]
    pub removed_paths: Vec<String>,
    #[serde(default)]
    pub base_reverts: Vec<String>,
    pub case_base_tree: String,
    pub case_commit_tree: String,
    pub licence: String,
    pub licence_path: String,
    pub licence_notice: String,
    #[serde(default)]
    pub agent_files: Vec<String>,
    pub dataset: String,
    #[serde(default)]
    pub dataset_instance: String,
    /// Free-text facts a reader of the results needs (Gate B rulings).
    #[serde(default)]
    pub notes: Vec<String>,
    /// The case base differs from the upstream fixed tree beyond
    /// `removed_paths` and `base_reverts`: each `base_overrides` path takes
    /// its content from `base/<path>` in the case directory (Gate B
    /// ruling 2: the fix's explanatory comments are removed).
    #[serde(default)]
    pub edited: bool,
    #[serde(default)]
    pub edit_reason: String,
    #[serde(default)]
    pub base_overrides: Vec<String>,
    #[serde(default)]
    pub repro: Option<Repro>,
    #[serde(default)]
    pub clean: Option<CleanCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repro {
    pub reproduced: bool,
    pub image: String,
    pub command: String,
    pub test_files: Vec<String>,
    pub fixed_exit: i32,
    pub reverted_exit: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanCheck {
    pub head_checked: String,
    pub head_checked_date: String,
    pub retained_share: f64,
    #[serde(default)]
    pub later_commits: Vec<String>,
    pub verdict: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defect {
    pub id: String,
    pub severity: Severity,
    pub files: Vec<String>,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    #[serde(default)]
    pub defect: Vec<Defect>,
}

#[derive(Debug, Clone)]
pub struct Case {
    pub dir: PathBuf,
    pub meta: CaseToml,
    pub expected: Option<Expected>,
    pub diff: Vec<u8>,
}

impl Case {
    pub fn id(&self) -> &str {
        &self.meta.id
    }
    pub fn defects(&self) -> &[Defect] {
        self.expected
            .as_ref()
            .map(|e| e.defect.as_slice())
            .unwrap_or(&[])
    }
    /// `<owner>/<repo>` of the source repository: the bootstrap's cluster.
    pub fn source_repo_key(&self) -> String {
        self.meta
            .source_repo
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .rsplit('/')
            .take(2)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("/")
            .to_ascii_lowercase()
    }
}

#[derive(thiserror::Error, Debug)]
pub enum CorpusError {
    #[error("{case}: {msg}")]
    Invalid { case: String, msg: String },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

fn invalid(case: &str, msg: impl Into<String>) -> CorpusError {
    CorpusError::Invalid {
        case: case.into(),
        msg: msg.into(),
    }
}

fn is_hex40(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Paths in `diff --git a/<p> b/<q>` headers (both sides) and the count of
/// changed lines.
pub fn diff_stats(diff: &str) -> (BTreeSet<String>, usize) {
    let mut paths = BTreeSet::new();
    let mut lines = 0;
    for l in diff.lines() {
        if let Some(rest) = l.strip_prefix("diff --git ") {
            for part in rest.split(' ') {
                if let Some(p) = part.strip_prefix("a/").or_else(|| part.strip_prefix("b/")) {
                    paths.insert(p.to_string());
                }
            }
        } else if (l.starts_with('+') && !l.starts_with("+++"))
            || (l.starts_with('-') && !l.starts_with("---"))
        {
            lines += 1;
        }
    }
    (paths, lines)
}

/// A conservative test-file heuristic, used only to refuse a diff.patch
/// that still carries a test file. Corpus authors list test files
/// explicitly in `removed_paths`.
pub fn looks_like_test_path(p: &str) -> bool {
    let lower = p.to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    let in_test_dir = lower.split('/').any(|seg| {
        matches!(
            seg,
            "tests" | "test" | "__tests__" | "__snapshots__" | "testdata" | "test_data"
        )
    });
    in_test_dir
        || file.starts_with("test_")
        || file.ends_with("_test.py")
        || file.ends_with("_test.rs")
        || file.ends_with("_tests.rs")
        || file == "tests.rs"
        || file.contains(".test.")
        || file.contains(".spec.")
        || file.ends_with(".snap")
}

/// How a file's line comments start, by extension: `//`-style, `#`-style,
/// or none known (Markdown, data files).
fn comment_style(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" | "go" | "java"
        | "c" | "h" | "cc" | "cpp" | "hpp" | "kt" | "swift" | "scss" | "css" | "vue" | "astro"
        | "svelte" => Some("slash"),
        "py" | "pyi" | "toml" | "yaml" | "yml" | "sh" | "bash" | "rb" => Some("hash"),
        _ => None,
    }
}

/// Whether a source line is a comment and nothing else.
pub fn is_comment_only(line: &str, style: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    match style {
        "slash" => {
            t.starts_with("//")
                || t.starts_with("/*")
                || t.starts_with("*/")
                || t == "*"
                || t.starts_with("* ")
                || (t.starts_with("{/*") && t.ends_with("*/}"))
        }
        // `#!` and Rust-like `#[` never occur in these languages as code
        // that matters here, but a shebang is not a comment about a fix.
        "hash" => t.starts_with('#') && !t.starts_with("#!"),
        _ => false,
    }
}

/// Split a code line into (code, trailing comment) when a comment follows
/// code on the same line, scanning past string and character literals so
/// `"http://x"` or `'#'` is not a comment. `None` when there is no
/// trailing comment, or no code before it. Rust `'a` lifetimes are told
/// apart from character literals by the closing quote.
pub fn split_trailing_comment<'a>(line: &'a str, path: &str) -> Option<(&'a str, &'a str)> {
    let style = comment_style(path)?;
    let rust = path.ends_with(".rs");
    let b = line.as_bytes();
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        let comment_starts = match style {
            "slash" => c == b'/' && matches!(b.get(i + 1), Some(b'/') | Some(b'*')),
            _ => c == b'#',
        };
        if comment_starts {
            let code = line[..i].trim_end();
            return (!code.trim().is_empty()).then_some((code, &line[i..]));
        }
        match c {
            b'"' | b'`' if style == "slash" || c == b'"' => quote = Some(c),
            b'\'' => {
                // A Rust lifetime (`'a`, `'static`) has no closing quote
                // within a character literal's length.
                let is_char_lit = !rust || {
                    let rest = &b[i + 1..];
                    (rest.len() >= 2 && rest[1] == b'\'')
                        || (rest.first() == Some(&b'\\')
                            && rest.iter().skip(1).take(10).any(|&x| x == b'\''))
                };
                if is_char_lit {
                    quote = Some(c);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Removed lines that are code followed by a comment (Gate B, PR #22
/// ruling 1): (path, 1-based old line number, line, code part).
pub fn trailing_comment_removed_lines(diff: &str) -> Vec<(String, usize, String, String)> {
    removed_lines(diff)
        .into_iter()
        .filter_map(|(p, n, t)| {
            let (code, _) = split_trailing_comment(&t, &p)?;
            let code = code.to_string();
            Some((p, n, t, code))
        })
        .collect()
}

/// Every removed line of a diff: (path, 1-based old line number, text).
pub fn removed_lines(diff: &str) -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    let mut path: Option<String> = None;
    let mut old_line = 0usize;
    for l in diff.lines() {
        if let Some(rest) = l.strip_prefix("diff --git ") {
            path = rest
                .split(' ')
                .next()
                .and_then(|a| a.strip_prefix("a/"))
                .map(str::to_string);
            continue;
        }
        if let Some(h) = l.strip_prefix("@@ -") {
            old_line = h
                .split([',', ' '])
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            continue;
        }
        if l.starts_with("--- ") || l.starts_with("+++ ") || l.starts_with("index ") {
            continue;
        }
        match l.as_bytes().first() {
            Some(b'-') => {
                if let Some(p) = &path {
                    out.push((p.clone(), old_line, l[1..].to_string()));
                }
                old_line += 1;
            }
            Some(b' ') => old_line += 1,
            _ => {}
        }
    }
    out
}

/// Comment-only lines on the removed side of a diff (Gate B ruling 2):
/// in a bug case these are comments the fix added, which the reverted diff
/// shows being deleted, and which explain the answer. Returns (path, the
/// line's 1-based number in the old file, the line text).
pub fn comment_only_removed_lines(diff: &str) -> Vec<(String, usize, String)> {
    removed_lines(diff)
        .into_iter()
        .filter(|(p, _, t)| comment_style(p).is_some_and(|s| is_comment_only(t, s)))
        .collect()
}

fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

pub fn load_case(dir: &Path) -> Result<Case, CorpusError> {
    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let meta_text = std::fs::read_to_string(dir.join("case.toml"))?;
    let meta: CaseToml =
        toml::from_str(&meta_text).map_err(|e| invalid(&dir_name, format!("case.toml: {e}")))?;
    let c = &meta.id;
    if *c != dir_name {
        return Err(invalid(
            &dir_name,
            format!("id `{c}` does not match its directory"),
        ));
    }
    if meta.schema != 1 {
        return Err(invalid(c, "schema must be 1"));
    }
    if !LANGUAGES.contains(&meta.language.as_str()) {
        return Err(invalid(c, format!("language `{}`", meta.language)));
    }
    if !ALLOWED_LICENCES.contains(&meta.licence.as_str()) {
        return Err(invalid(
            c,
            format!(
                "licence `{}` is not MIT, Apache-2.0 or BSD (AC 244/245)",
                meta.licence
            ),
        ));
    }
    for h in [
        &meta.source_commit,
        &meta.base_commit,
        &meta.case_base_tree,
        &meta.case_commit_tree,
    ] {
        if !is_hex40(h) {
            return Err(invalid(c, format!("`{h}` is not a 40-hex object id")));
        }
    }
    if meta.licence_notice.trim().is_empty() {
        return Err(invalid(c, "licence_notice is empty"));
    }
    for f in &meta.agent_files {
        if f != "AGENTS.md" && f != "CLAUDE.md" {
            return Err(invalid(c, format!("agent_files entry `{f}`")));
        }
    }
    if meta.edited == meta.base_overrides.is_empty()
        || (meta.edited && meta.edit_reason.trim().is_empty())
    {
        return Err(invalid(c, "edited = true needs base_overrides and an edit_reason, and base_overrides needs edited = true"));
    }
    for p in &meta.base_overrides {
        if meta.removed_paths.contains(p) || meta.base_reverts.contains(p) {
            return Err(invalid(
                c,
                format!("base_overrides path `{p}` is also removed or reverted"),
            ));
        }
        if !dir.join("base").join(p).is_file() {
            return Err(invalid(c, format!("base/{p} is missing")));
        }
    }
    let diff = std::fs::read(dir.join("diff.patch"))?;
    let diff_text = String::from_utf8_lossy(&diff);
    if diff_text.contains("\r\n") {
        return Err(invalid(
            c,
            "diff.patch has CRLF line endings: regenerate it with autocrlf off",
        ));
    }
    let (paths, lines) = diff_stats(&diff_text);
    if paths.is_empty() {
        return Err(invalid(c, "diff.patch has no file"));
    }
    if paths.len() > MAX_DIFF_FILES || lines > MAX_DIFF_LINES {
        return Err(invalid(c, format!("diff.patch touches {} files / {lines} lines (max {MAX_DIFF_FILES} / {MAX_DIFF_LINES})", paths.len())));
    }
    for p in &paths {
        if looks_like_test_path(p)
            || meta.removed_paths.contains(p)
            || meta.base_reverts.contains(p)
        {
            return Err(invalid(
                c,
                format!("diff.patch touches `{p}`, a test, removed or reverted path"),
            ));
        }
    }
    let expected_path = dir.join("expected.toml");
    let expected = if expected_path.exists() {
        let t = std::fs::read_to_string(&expected_path)?;
        Some(
            toml::from_str::<Expected>(&t)
                .map_err(|e| invalid(c, format!("expected.toml: {e}")))?,
        )
    } else {
        None
    };
    match meta.kind {
        Kind::Bug => {
            if meta.base_commit != meta.source_commit {
                return Err(invalid(c, "a bug case's base_commit is its fix commit"));
            }
            let exp = expected
                .as_ref()
                .ok_or_else(|| invalid(c, "bug case without expected.toml"))?;
            if exp.defect.is_empty() {
                return Err(invalid(c, "expected.toml has no defect"));
            }
            let mut ids = BTreeSet::new();
            for d in &exp.defect {
                if !ids.insert(d.id.clone()) {
                    return Err(invalid(c, format!("duplicate defect id {}", d.id)));
                }
                if word_count(&d.title) > MAX_WORDS || d.description.trim().is_empty() {
                    return Err(invalid(
                        c,
                        format!(
                            "defect {}: title over {MAX_WORDS} words or empty description",
                            d.id
                        ),
                    ));
                }
            }
            let repro = meta
                .repro
                .as_ref()
                .ok_or_else(|| invalid(c, "bug case without [repro]"))?;
            if meta.clean.is_some() {
                return Err(invalid(c, "bug case with a [clean] table"));
            }
            for t in &repro.test_files {
                if !meta.removed_paths.contains(t) {
                    return Err(invalid(
                        c,
                        format!("repro test file `{t}` is not in removed_paths"),
                    ));
                }
                if !dir.join("repro").join("files").join(t).is_file() {
                    return Err(invalid(c, format!("repro/files/{t} is missing")));
                }
            }
            if repro.reproduced && (repro.fixed_exit != 0 || repro.reverted_exit == 0) {
                return Err(invalid(
                    c,
                    "reproduced = true needs fixed_exit 0 and a non-zero reverted_exit",
                ));
            }
            if repro.reproduced && repro.test_files.is_empty() {
                return Err(invalid(c, "reproduced = true with no test files"));
            }
        }
        Kind::Clean => {
            if expected.is_some() || meta.repro.is_some() {
                return Err(invalid(
                    c,
                    "a clean case has no expected.toml and no [repro]",
                ));
            }
            let chk = meta
                .clean
                .as_ref()
                .ok_or_else(|| invalid(c, "clean case without [clean]"))?;
            if meta.source_commit_date.as_str() > CLEAN_CUTOFF {
                return Err(invalid(
                    c,
                    format!(
                        "clean commit dated {} is newer than {CLEAN_CUTOFF} (AC 243)",
                        meta.source_commit_date
                    ),
                ));
            }
            if chk.retained_share < 0.8 || !is_hex40(&chk.head_checked) {
                return Err(invalid(
                    c,
                    "clean check: retained_share below 0.8 or bad head_checked",
                ));
            }
        }
    }
    Ok(Case {
        dir: dir.to_path_buf(),
        meta,
        expected,
        diff,
    })
}

pub fn load_corpus(cases_dir: &Path) -> Result<Vec<Case>, CorpusError> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(cases_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("case.toml").is_file())
        .collect();
    dirs.sort();
    dirs.iter().map(|d| load_case(d)).collect()
}

/// Corpus-level requirements (AC 242, AC 243). Each string is a failure.
pub fn corpus_problems(cases: &[Case]) -> Vec<String> {
    let mut p = Vec::new();
    let bugs = cases.iter().filter(|c| c.meta.kind == Kind::Bug).count();
    let clean = cases.len() - bugs;
    if bugs < 30 {
        p.push(format!("{bugs} bug cases; AC 243 needs at least 30"));
    }
    if clean < 10 {
        p.push(format!("{clean} clean cases; AC 243 needs at least 10"));
    }
    let langs: BTreeSet<&str> = cases.iter().map(|c| c.meta.language.as_str()).collect();
    if langs.len() < 3
        || !langs.contains("rust")
        || !(langs.contains("python") || langs.contains("typescript"))
    {
        p.push(format!(
            "languages {langs:?}; AC 243 needs three including Rust and Python or TypeScript"
        ));
    }
    let agent = cases
        .iter()
        .filter(|c| !c.meta.agent_files.is_empty())
        .count();
    if agent < 8 {
        p.push(format!(
            "{agent} cases with AGENTS.md or CLAUDE.md at the base; AC 242 needs at least 8"
        ));
    }
    p
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_string();
        let r = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if e.file_type()?.is_dir() {
            walk(&e.path(), &r, out)?;
        } else {
            out.push((r, e.path()));
        }
    }
    Ok(())
}

/// SHA-256 of a case directory, as `bench/CORPUS.md` defines it.
pub fn case_content_hash(dir: &Path) -> std::io::Result<String> {
    let mut files = Vec::new();
    walk(dir, "", &mut files)?;
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut h = Sha256::new();
    for (rel, path) in files {
        let bytes = std::fs::read(path)?;
        h.update(rel.as_bytes());
        h.update([0u8]);
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The text of `bench/MANIFEST.toml`.
pub fn manifest_text(cases: &[Case]) -> std::io::Result<String> {
    let mut s = String::from(
        "# Corpus manifest (AC 242). Generated by `quorum-bench manifest`; do not edit.\n# Each hash is the SHA-256 of the case directory as bench/CORPUS.md defines it.\n\n",
    );
    for c in cases {
        s.push_str(&format!(
            "[[case]]\nid = \"{}\"\nkind = \"{}\"\nlanguage = \"{}\"\nsource_repo = \"{}\"\nreproduced = {}\nagent_files = {}\ncontent_sha256 = \"{}\"\n\n",
            c.meta.id,
            if c.meta.kind == Kind::Bug { "bug" } else { "clean" },
            c.meta.language,
            c.source_repo_key(),
            c.meta.repro.as_ref().map(|r| r.reproduced).unwrap_or(false),
            !c.meta.agent_files.is_empty(),
            case_content_hash(&c.dir)?
        ));
    }
    Ok(s)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
pub(crate) mod testcase {
    //! A tiny, valid bug case built in a temp dir, for tests across the
    //! crate. Its trees are computed with git2 from a local upstream.
    use super::*;

    pub const BASE_SRC: &str = "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
    pub const BUGGY_SRC: &str = "pub fn add(a: i32, b: i32) -> i32 {\n    a - b\n}\n";
    pub const TEST_SRC: &str = "#[test]\nfn adds_two_numbers_correctly_for_the_bench() {\n    assert_eq!(lib::add(2, 2), 4);\n}\n";

    pub fn write_case(cases_dir: &Path, id: &str, toml_extra: &str) -> PathBuf {
        let d = cases_dir.join(id);
        std::fs::create_dir_all(d.join("repro/files/tests")).unwrap();
        std::fs::write(d.join("repro/files/tests/add.rs"), TEST_SRC).unwrap();
        std::fs::write(
            d.join("diff.patch"),
            "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@\n pub fn add(a: i32, b: i32) -> i32 {\n-    a + b\n+    a - b\n }\n",
        )
        .unwrap();
        std::fs::write(
            d.join("expected.toml"),
            "[[defect]]\nid = \"D1\"\nseverity = \"high\"\nfiles = [\"src/lib.rs\"]\ntitle = \"add subtracts instead of adding\"\ndescription = \"add returns a - b.\"\n",
        )
        .unwrap();
        let z = "0".repeat(40);
        std::fs::write(
            d.join("case.toml"),
            format!(
                "schema = 1\nid = \"{id}\"\nkind = \"bug\"\nlanguage = \"rust\"\nsource_repo = \"https://example.invalid/o/r\"\nsource_commit = \"{z}\"\nsource_commit_url = \"u\"\nsource_commit_date = \"2026-01-01\"\nbase_commit = \"{z}\"\nremoved_paths = [\"tests/add.rs\"]\ncase_base_tree = \"{z}\"\ncase_commit_tree = \"{z}\"\nlicence = \"MIT\"\nlicence_path = \"LICENSE\"\nlicence_notice = \"Copyright (c) Example\"\nagent_files = [\"AGENTS.md\"]\ndataset = \"hand-picked\"\n{toml_extra}\n[repro]\nreproduced = true\nimage = \"img\"\ncommand = \"cargo test\"\ntest_files = [\"tests/add.rs\"]\nfixed_exit = 0\nreverted_exit = 101\n"
            ),
        )
        .unwrap();
        d
    }
}

#[cfg(test)]
mod tests {
    use super::testcase::*;
    use super::*;

    #[test]
    fn a_valid_case_loads() {
        let td = tempfile::TempDir::new().unwrap();
        let d = write_case(td.path(), "rs-x-01", "");
        let c = load_case(&d).unwrap();
        assert_eq!(c.defects().len(), 1);
        assert_eq!(c.source_repo_key(), "o/r");
    }

    #[test]
    fn negative_controls_bad_cases_are_refused() {
        let td = tempfile::TempDir::new().unwrap();
        // A diff that still touches a test file.
        let d = write_case(td.path(), "rs-x-02", "");
        std::fs::write(d.join("diff.patch"), "diff --git a/tests/add.rs b/tests/add.rs\n--- a/tests/add.rs\n+++ b/tests/add.rs\n@@ -1 +1 @@\n-x\n+y\n").unwrap();
        assert!(load_case(&d)
            .unwrap_err()
            .to_string()
            .contains("test, removed or reverted"));
        // A copyleft licence.
        let d = write_case(td.path(), "rs-x-03", "");
        let t = std::fs::read_to_string(d.join("case.toml"))
            .unwrap()
            .replace("licence = \"MIT\"", "licence = \"GPL-3.0\"");
        std::fs::write(d.join("case.toml"), t).unwrap();
        assert!(load_case(&d).unwrap_err().to_string().contains("GPL-3.0"));
        // CRLF in the patch.
        let d = write_case(td.path(), "rs-x-04", "");
        let p = std::fs::read_to_string(d.join("diff.patch"))
            .unwrap()
            .replace('\n', "\r\n");
        std::fs::write(d.join("diff.patch"), p).unwrap();
        assert!(load_case(&d).unwrap_err().to_string().contains("CRLF"));
        // A repro test file that is not removed from the base.
        let d = write_case(td.path(), "rs-x-05", "");
        let t = std::fs::read_to_string(d.join("case.toml"))
            .unwrap()
            .replace("removed_paths = [\"tests/add.rs\"]", "removed_paths = []");
        std::fs::write(d.join("case.toml"), t).unwrap();
        assert!(load_case(&d)
            .unwrap_err()
            .to_string()
            .contains("not in removed_paths"));
        // Directory and id disagree.
        let d = write_case(td.path(), "rs-x-06", "");
        std::fs::rename(&d, td.path().join("rs-x-07")).unwrap();
        assert!(load_case(&td.path().join("rs-x-07")).is_err());
    }

    #[test]
    fn content_hash_changes_with_any_byte_and_ignores_nothing() {
        let td = tempfile::TempDir::new().unwrap();
        let d = write_case(td.path(), "rs-x-01", "");
        let h1 = case_content_hash(&d).unwrap();
        assert_eq!(h1, case_content_hash(&d).unwrap());
        std::fs::write(d.join("repro/output-fixed.txt"), "ok").unwrap();
        let h2 = case_content_hash(&d).unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn corpus_level_requirements() {
        let td = tempfile::TempDir::new().unwrap();
        write_case(td.path(), "rs-x-01", "");
        let cases = load_corpus(td.path()).unwrap();
        let p = corpus_problems(&cases);
        assert!(p.iter().any(|m| m.contains("bug cases")));
        assert!(p.iter().any(|m| m.contains("clean cases")));
        assert!(p.iter().any(|m| m.contains("languages")));
        assert!(p.iter().any(|m| m.contains("AGENTS.md")));
    }

    #[test]
    fn comment_only_removed_lines_are_found_per_language() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -10,4 +10,2 @@ fn f() {\n     let x = 1;\n-    // Guard against an empty slice: the fix.\n-    if v.is_empty() { return; } // trailing\n     g();\ndiff --git a/p/b.py b/p/b.py\n--- a/p/b.py\n+++ b/p/b.py\n@@ -3,2 +3,1 @@\n-# keep the stderr loop alive\n x = 1\ndiff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -1,1 +1,0 @@\n-# Title\n";
        let hits = comment_only_removed_lines(diff);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!((hits[0].0.as_str(), hits[0].1), ("src/a.rs", 11));
        assert_eq!((hits[1].0.as_str(), hits[1].1), ("p/b.py", 3));
        for (line, style, want) in [
            ("/// doc", "slash", true),
            (" * block", "slash", true),
            ("{/* jsx */}", "slash", true),
            ("x = 1 // trailing", "slash", false),
            ("#!/bin/sh", "hash", false),
            ("", "slash", false),
        ] {
            assert_eq!(is_comment_only(line, style), want, "{line:?}");
        }
    }

    #[test]
    fn trailing_comments_are_split_outside_literals() {
        let cases = [
            (
                "src/a.rs",
                "            b'\\\\' => i += 1, // escaped brace is data",
                Some("            b'\\\\' => i += 1,"),
            ),
            (
                "src/a.ts",
                "  if (gen !== ref.current) return; // stale",
                Some("  if (gen !== ref.current) return;"),
            ),
            ("src/a.ts", "  const u = 'http://x'; ", None),
            (
                "src/a.ts",
                "  const u = \"http://x\"; // a url",
                Some("  const u = \"http://x\";"),
            ),
            (
                "src/a.rs",
                "fn f<'a>(x: &'a str) -> &'a str { x } // lifetime",
                Some("fn f<'a>(x: &'a str) -> &'a str { x }"),
            ),
            (
                "src/a.rs",
                "let c = '/'; let d = 1; /* why */",
                Some("let c = '/'; let d = 1;"),
            ),
            (
                "p/a.py",
                "x = '#not a comment'  # but this is",
                Some("x = '#not a comment'"),
            ),
            ("p/a.py", "# only a comment", None),
            ("README.md", "text // not code", None),
        ];
        for (p, line, want) in cases {
            assert_eq!(
                split_trailing_comment(line, p).map(|x| x.0),
                want,
                "{p}: {line}"
            );
        }
        let diff = "diff --git a/src/a.ts b/src/a.ts\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -5,2 +5,1 @@\n-  return; // stale\n   x();\n";
        let hits = trailing_comment_removed_lines(diff);
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].1, hits[0].3.as_str()), (5, "  return;"));
    }

    #[test]
    fn edited_cases_need_overrides_and_a_reason() {
        let td = tempfile::TempDir::new().unwrap();
        let d = write_case(td.path(), "rs-x-01", "edited = true\n");
        assert!(load_case(&d)
            .unwrap_err()
            .to_string()
            .contains("edited = true"));
        let d = write_case(
            td.path(),
            "rs-x-02",
            "edited = true\nedit_reason = \"r\"\nbase_overrides = [\"src/lib.rs\"]\n",
        );
        assert!(load_case(&d)
            .unwrap_err()
            .to_string()
            .contains("base/src/lib.rs is missing"));
        std::fs::create_dir_all(d.join("base/src")).unwrap();
        std::fs::write(d.join("base/src/lib.rs"), BASE_SRC).unwrap();
        assert!(load_case(&d).is_ok());
    }

    #[test]
    fn test_path_heuristic() {
        for t in [
            "tests/a.rs",
            "src/foo_test.py",
            "pkg/__tests__/x.ts",
            "a/b.test.ts",
            "x.spec.ts",
            "crates/a/src/tests.rs",
            "snap/x.snap",
            "tests/test_x.py",
        ] {
            assert!(looks_like_test_path(t), "{t}");
        }
        for t in ["src/lib.rs", "src/testing_utils_not.rs", "latest.py"] {
            assert!(!looks_like_test_path(t), "{t}");
        }
    }
}
