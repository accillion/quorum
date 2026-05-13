//! `.quorum/conventions.md` — trusted only if committed to git AND clean.
//!
//! Three states (spec §4.3.2):
//!   - Absent → returns `Ok(None)`, silent.
//!   - Present in worktree but not tracked OR with uncommitted changes
//!     → returns `Ok(None)`, caller may emit the "untracked/uncommitted"
//!     note.
//!   - Present, tracked, and byte-identical to `HEAD:.quorum/conventions.md`
//!     → returns `Ok(Some(content))`.

use std::path::Path;

pub enum ConventionsState {
    /// File absent in the worktree. Caller must NOT emit a note.
    Absent,
    /// File present in the worktree but ignored by Quorum. Caller emits
    /// `note: .quorum/conventions.md present but not committed ...`.
    PresentButIgnored,
    /// File trusted and ready to bundle.
    Trusted(String),
}

impl ConventionsState {
    /// Per-call helper for the bundle path (§6.2 bridge): `true` iff
    /// `.quorum/conventions.md` is tracked AND HEAD blob is byte-identical
    /// to the worktree. The bundle assembler consults this once per
    /// invocation to decide whether `promoted_convention` rows render in
    /// the conventions section (trusted) or fall back to the memory
    /// section (dirty/uncommitted/missing). No caching — `load()` is the
    /// source of truth and re-runs on every bundle assembly.
    pub fn is_trusted(&self) -> bool {
        matches!(self, ConventionsState::Trusted(_))
    }
}

// ---------------------------------------------------------------------------
// Phase 1C Stage 3 — managed-section parser.
//
// `.quorum/conventions.md` is owned by the user above and below the
// `<!-- quorum:managed-section v=1 -->` fence; Quorum owns the content
// between the open and close fence markers. The parser enumerates the
// per-block `<!-- quorum:convention id=<12-hex> v=<int> -->` markers and
// preserves outside-fence content as byte slices so the Stage 4 writer can
// rebuild the file without disturbing user prose. The parser is read-only:
// no mutation, no rewrite, no temp-file IO.

/// Per-block view inside the managed section.
#[derive(Debug, Clone)]
pub struct ParsedBlock<'a> {
    /// 12-hex prefix id from `<!-- quorum:convention id=<id> v=<v> -->`.
    /// Parser-validated as 12 lowercase hex chars (non-hex → diagnostic).
    pub id: &'a str,
    /// `v=<int>` from the open marker.
    pub version: u32,
    /// The `### Convention: <title>` line (if present). May be empty if the
    /// block's first non-blank line is not a header.
    pub header_line: &'a str,
    /// Everything between the open marker and the `<!-- /quorum:convention -->`
    /// close marker (header + body). Plain string slice into the input.
    pub body: &'a str,
    /// Byte range covered by the block, inclusive of open + close markers.
    /// Used by the Stage 4 writer for surgical replacement.
    pub byte_range_in_file: std::ops::Range<usize>,
}

/// Top-level view of a `.quorum/conventions.md` file.
#[derive(Debug, Clone)]
pub struct ParsedConventionsMd<'a> {
    /// Bytes above (and including the newline preceding) the managed-section
    /// open marker. When `fence_present == false`, holds the entire file.
    pub above_fence: &'a [u8],
    /// Bytes after the managed-section close marker. Empty when fence absent.
    pub below_fence: &'a [u8],
    /// True when both open + close markers were found.
    pub fence_present: bool,
    /// True when the file's first line is `<!-- quorum-managed-conventions-md v=1 -->`.
    /// §4.4 last bullet: optional-on-read, mandatory-on-write. Stage 3 only reads.
    pub first_line_marker_present: bool,
    /// Managed blocks found between the open + close fence markers. Empty
    /// when fence absent, fence has zero blocks, or all blocks were malformed.
    pub blocks: Vec<ParsedBlock<'a>>,
}

/// Parser diagnostic — surfaced via the AC 168 stderr warning path when
/// `quorum convention list --orphans` runs (Stage 3) and when Stage 4
/// promote/demote run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConventionParseError {
    /// Open marker found but no matching `<!-- /quorum:convention -->`
    /// before the next open marker or the section close.
    UnclosedBlock { id: String, start_byte: usize },
    /// `id=...` value is not 12 lowercase hex chars.
    BadBlockId { raw: String, start_byte: usize },
    /// `v=...` value is not a non-negative integer.
    BadBlockVersion { raw: String, start_byte: usize },
    /// More than one `<!-- quorum:managed-section v=1 -->` open marker in
    /// the file. We only parse the first; subsequent ones surface as a
    /// diagnostic. (Spec is silent on multi-section files; lean per the
    /// dispatch prompt §2 calibration: treat as malformed, parse first only.)
    DuplicateManagedSection { start_byte: usize },
}

const MARKER_OPEN_SECTION: &str = "<!-- quorum:managed-section v=1 -->";
const MARKER_CLOSE_SECTION: &str = "<!-- /quorum:managed-section -->";
const MARKER_FILE_FIRST_LINE: &str = "<!-- quorum-managed-conventions-md v=1 -->";
const MARKER_BLOCK_CLOSE: &str = "<!-- /quorum:convention -->";

/// Parse a `.quorum/conventions.md` byte buffer.
///
/// Returns a `ParsedConventionsMd` plus a list of diagnostics for any
/// malformed blocks. The function never panics on user-supplied content
/// (Stage 3 dispatch prompt §2 calibration).
pub fn parse_conventions_md(
    input: &[u8],
) -> (ParsedConventionsMd<'_>, Vec<ConventionParseError>) {
    let mut diags: Vec<ConventionParseError> = Vec::new();
    let first_line_marker_present = file_starts_with_marker(input);

    // Locate the managed-section open marker. Treat any byte sequence
    // matching the literal as the marker — we don't insist on it sitting on
    // its own line. Lippa-side Phase 1A users have authored these files by
    // hand and the spec §4.4 example places the marker on its own line, but
    // adversarial content inside a code fence would mislead a line-only
    // search. The byte search is robust and matches the writer (Stage 4).
    let Some(open_idx) = find_subslice(input, MARKER_OPEN_SECTION.as_bytes()) else {
        return (
            ParsedConventionsMd {
                above_fence: input,
                below_fence: &input[input.len()..input.len()],
                fence_present: false,
                first_line_marker_present,
                blocks: Vec::new(),
            },
            diags,
        );
    };
    // Multi-section diagnostic (lean per dispatch prompt §2: parse first only).
    let after_first_open = open_idx + MARKER_OPEN_SECTION.len();
    if let Some(dup) = find_subslice(&input[after_first_open..], MARKER_OPEN_SECTION.as_bytes()) {
        diags.push(ConventionParseError::DuplicateManagedSection {
            start_byte: after_first_open + dup,
        });
    }

    // Locate the close marker AFTER the open marker. If absent, treat the
    // fence as malformed and return everything-below as empty.
    let close_search_start = after_first_open;
    let Some(rel_close_idx) =
        find_subslice(&input[close_search_start..], MARKER_CLOSE_SECTION.as_bytes())
    else {
        // Open marker without close — treat as malformed; surface a
        // diagnostic and refuse to recognize the fence. The Stage 4 writer
        // will rewrite a clean fence on the next promote.
        return (
            ParsedConventionsMd {
                above_fence: input,
                below_fence: &input[input.len()..input.len()],
                fence_present: false,
                first_line_marker_present,
                blocks: Vec::new(),
            },
            diags,
        );
    };
    let close_idx = close_search_start + rel_close_idx;
    let close_end = close_idx + MARKER_CLOSE_SECTION.len();

    let above_fence = &input[..open_idx];
    let below_fence = &input[close_end..];
    let inside = &input[after_first_open..close_idx];

    // Parse blocks inside the managed section.
    let blocks = parse_blocks(input, after_first_open, inside, &mut diags);

    (
        ParsedConventionsMd {
            above_fence,
            below_fence,
            fence_present: true,
            first_line_marker_present,
            blocks,
        },
        diags,
    )
}

fn file_starts_with_marker(input: &[u8]) -> bool {
    let marker = MARKER_FILE_FIRST_LINE.as_bytes();
    if input.len() < marker.len() {
        return false;
    }
    if &input[..marker.len()] != marker {
        return false;
    }
    // Must be followed by EOF or a line terminator.
    match input.get(marker.len()) {
        None | Some(b'\n') | Some(b'\r') => true,
        _ => false,
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

fn parse_blocks<'a>(
    full_input: &'a [u8],
    inside_offset: usize,
    inside: &'a [u8],
    diags: &mut Vec<ConventionParseError>,
) -> Vec<ParsedBlock<'a>> {
    let mut blocks = Vec::new();
    let needle_open_prefix = b"<!-- quorum:convention id=";
    let mut cursor: usize = 0;
    while let Some(rel) = find_subslice(&inside[cursor..], needle_open_prefix) {
        let open_marker_start = cursor + rel;
        let abs_block_start = inside_offset + open_marker_start;
        // Find end of the open marker line (`-->`).
        let after_prefix = open_marker_start + needle_open_prefix.len();
        let Some(end_rel) = find_subslice(&inside[after_prefix..], b"-->") else {
            // Open prefix without a `-->` close — malformed, skip past the
            // prefix to avoid an infinite loop, surface a diagnostic.
            diags.push(ConventionParseError::UnclosedBlock {
                id: "<unterminated open marker>".to_string(),
                start_byte: abs_block_start,
            });
            cursor = after_prefix;
            continue;
        };
        let open_marker_end = after_prefix + end_rel + 3; // include `-->`
        let attrs_str = match std::str::from_utf8(&inside[after_prefix..after_prefix + end_rel]) {
            Ok(s) => s.trim(),
            Err(_) => {
                diags.push(ConventionParseError::BadBlockId {
                    raw: "<non-utf8 attrs>".to_string(),
                    start_byte: abs_block_start,
                });
                cursor = open_marker_end;
                continue;
            }
        };
        // attrs_str now looks like `a1b2c3d4e5f6 v=1` (with the leading
        // `id=` already past). Split off the id token + optional `v=<n>`.
        let mut tokens = attrs_str.split_ascii_whitespace();
        let id_raw = match tokens.next() {
            Some(s) => s,
            None => {
                diags.push(ConventionParseError::BadBlockId {
                    raw: String::new(),
                    start_byte: abs_block_start,
                });
                cursor = open_marker_end;
                continue;
            }
        };
        if !is_valid_short_hash(id_raw) {
            diags.push(ConventionParseError::BadBlockId {
                raw: id_raw.to_string(),
                start_byte: abs_block_start,
            });
            cursor = open_marker_end;
            continue;
        }
        // Optional `v=<int>` second token.
        let mut version: u32 = 1;
        if let Some(v_tok) = tokens.next() {
            if let Some(rest) = v_tok.strip_prefix("v=") {
                match rest.parse::<u32>() {
                    Ok(v) => version = v,
                    Err(_) => {
                        diags.push(ConventionParseError::BadBlockVersion {
                            raw: v_tok.to_string(),
                            start_byte: abs_block_start,
                        });
                        cursor = open_marker_end;
                        continue;
                    }
                }
            }
        }

        // Locate the close marker. Bound the search by the next open
        // marker (so a missing close is caught instead of swallowing the
        // next block).
        let body_start = open_marker_end;
        let next_open_rel = find_subslice(&inside[body_start..], needle_open_prefix);
        let close_rel = find_subslice(&inside[body_start..], MARKER_BLOCK_CLOSE.as_bytes());
        let (close_start_rel, found_close) = match (close_rel, next_open_rel) {
            (Some(c), Some(n)) if c < n => (c, true),
            (Some(c), None) => (c, true),
            _ => (next_open_rel.unwrap_or(inside.len() - body_start), false),
        };
        if !found_close {
            diags.push(ConventionParseError::UnclosedBlock {
                id: id_raw.to_string(),
                start_byte: abs_block_start,
            });
            // Advance past the open marker so we keep scanning for more
            // blocks (a missing close doesn't poison the rest of the file).
            cursor = open_marker_end;
            continue;
        }
        let body_end = body_start + close_start_rel;
        let close_end = body_end + MARKER_BLOCK_CLOSE.len();
        // Body and header are returned as &str slices into the original
        // file buffer (via `full_input`) so callers get matching lifetimes.
        let abs_body_start = inside_offset + body_start;
        let abs_body_end = inside_offset + body_end;
        let abs_block_end = inside_offset + close_end;
        let body_str = match std::str::from_utf8(&full_input[abs_body_start..abs_body_end]) {
            Ok(s) => s,
            Err(_) => {
                // Non-UTF-8 body — surface diagnostic, skip this block.
                diags.push(ConventionParseError::BadBlockId {
                    raw: format!("{} (non-utf8 body)", id_raw),
                    start_byte: abs_block_start,
                });
                cursor = close_end;
                continue;
            }
        };
        let header_line = first_nonblank_line(body_str);
        // `id_raw` borrows from `inside`, which shares lifetime `'a` with
        // `full_input`. No re-borrow needed.
        blocks.push(ParsedBlock {
            id: id_raw,
            version,
            header_line,
            body: body_str,
            byte_range_in_file: (inside_offset + open_marker_start)..abs_block_end,
        });
        cursor = close_end;
    }
    blocks
}

fn first_nonblank_line(s: &str) -> &str {
    for line in s.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if !trimmed.trim().is_empty() {
            return trimmed;
        }
    }
    ""
}

/// `true` iff `s` is exactly 12 ASCII lowercase hex chars. Phase 1C
/// short-hash convention (established in Stage 2 bundle render).
pub fn is_valid_short_hash(s: &str) -> bool {
    s.len() == 12 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

// ---------------------------------------------------------------------------
// Orphan detection (AC 169 / partial AC 168).
//
// `OrphanReport` walks the parser output against the `conventions` SQLite
// table to surface both directions: managed block in file but no SQLite
// row, and SQLite row with no managed block in file. Lives alongside the
// parser because both consumers (CLI `list --orphans`, Stage 4 promote/
// demote pre-flight) reuse the same shape.

/// A managed block in `.quorum/conventions.md` that has no matching row
/// in the SQLite `conventions` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOrphan {
    pub id: String,
    pub header_line: String,
    pub byte_range_in_file: std::ops::Range<usize>,
}

/// A SQLite `conventions` row whose `conventions_md_block_id` does not
/// match any managed block in `.quorum/conventions.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbOrphan {
    pub finding_identity_hash_hex: String,
    pub conventions_md_block_id: String,
    pub title_snapshot: String,
}

/// Combined orphan report for `list --orphans`.
#[derive(Debug, Clone)]
pub struct OrphanReport {
    /// Blocks in the file with no SQLite row.
    pub file_orphans: Vec<FileOrphan>,
    /// SQLite rows with no matching block in the file.
    pub db_orphans: Vec<DbOrphan>,
    /// Parser diagnostics from the file walk — surfaced as AC 168 stderr
    /// warnings. Empty when no malformed blocks were encountered.
    pub parser_diagnostics: Vec<ConventionParseError>,
    /// True when conventions.md did not exist at the path passed to
    /// [`detect_orphans`]. Caller decides how to render this — typically
    /// `(.quorum/conventions.md not found)` in human output.
    pub file_missing: bool,
    /// True when conventions.md existed but contained no managed-section
    /// fence (or a malformed fence). All `promoted_convention` rows
    /// therefore appear as db_orphans.
    pub fence_absent: bool,
}

/// A single SQLite `conventions` row, simplified for orphan detection.
/// Used as the input row shape — the caller (memory/sqlite.rs) provides
/// these from a SELECT join.
#[derive(Debug, Clone)]
pub struct ConventionRow {
    pub finding_identity_hash_hex: String,
    pub conventions_md_block_id: String,
    pub title_snapshot: String,
}

/// Detect orphans by parsing the file at `conventions_md_path` and
/// comparing block ids against `db_rows`.
///
/// `db_rows` is expected to contain rows from the SQLite `conventions`
/// table joined with the `dismissals.title_snapshot` for diagnostic
/// rendering. Caller decides the join shape; this fn only consumes the
/// flattened triple.
pub fn detect_orphans(
    conventions_md_path: &Path,
    db_rows: &[ConventionRow],
) -> OrphanReport {
    let bytes = match std::fs::read(conventions_md_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // File absent — every DB row is a db_orphan; no file_orphans.
            return OrphanReport {
                file_orphans: Vec::new(),
                db_orphans: db_rows
                    .iter()
                    .cloned()
                    .map(|r| DbOrphan {
                        finding_identity_hash_hex: r.finding_identity_hash_hex,
                        conventions_md_block_id: r.conventions_md_block_id,
                        title_snapshot: r.title_snapshot,
                    })
                    .collect(),
                parser_diagnostics: Vec::new(),
                file_missing: true,
                fence_absent: true,
            };
        }
        Err(_) => {
            // Treat any other IO error the same as missing — we don't have
            // a way to enumerate blocks, so DB rows surface as orphans.
            return OrphanReport {
                file_orphans: Vec::new(),
                db_orphans: db_rows
                    .iter()
                    .cloned()
                    .map(|r| DbOrphan {
                        finding_identity_hash_hex: r.finding_identity_hash_hex,
                        conventions_md_block_id: r.conventions_md_block_id,
                        title_snapshot: r.title_snapshot,
                    })
                    .collect(),
                parser_diagnostics: Vec::new(),
                file_missing: true,
                fence_absent: true,
            };
        }
    };

    let (parsed, parser_diagnostics) = parse_conventions_md(&bytes);
    let block_ids: std::collections::HashSet<&str> =
        parsed.blocks.iter().map(|b| b.id).collect();
    let db_block_ids: std::collections::HashSet<&str> =
        db_rows.iter().map(|r| r.conventions_md_block_id.as_str()).collect();

    let mut file_orphans: Vec<FileOrphan> = parsed
        .blocks
        .iter()
        .filter(|b| !db_block_ids.contains(b.id))
        .map(|b| FileOrphan {
            id: b.id.to_string(),
            header_line: b.header_line.to_string(),
            byte_range_in_file: b.byte_range_in_file.clone(),
        })
        .collect();
    file_orphans.sort_by(|a, b| a.id.cmp(&b.id));

    let mut db_orphans: Vec<DbOrphan> = db_rows
        .iter()
        .filter(|r| !block_ids.contains(r.conventions_md_block_id.as_str()))
        .cloned()
        .map(|r| DbOrphan {
            finding_identity_hash_hex: r.finding_identity_hash_hex,
            conventions_md_block_id: r.conventions_md_block_id,
            title_snapshot: r.title_snapshot,
        })
        .collect();
    db_orphans.sort_by(|a, b| a.conventions_md_block_id.cmp(&b.conventions_md_block_id));

    OrphanReport {
        file_orphans,
        db_orphans,
        parser_diagnostics,
        file_missing: false,
        fence_absent: !parsed.fence_present,
    }
}

pub fn load(repo_root: &Path) -> Result<ConventionsState, git2::Error> {
    let rel = ".quorum/conventions.md";
    let path = repo_root.join(".quorum").join("conventions.md");
    if !path.exists() {
        return Ok(ConventionsState::Absent);
    }
    // Open repo at root.
    let repo = git2::Repository::open(repo_root)?;
    let head = repo.head()?.peel_to_commit()?;
    let tree = head.tree()?;
    let entry = match tree.get_path(std::path::Path::new(rel)) {
        Ok(e) => e,
        Err(_) => return Ok(ConventionsState::PresentButIgnored),
    };
    let object = entry.to_object(&repo)?;
    let blob = match object.as_blob() {
        Some(b) => b,
        None => return Ok(ConventionsState::PresentButIgnored),
    };
    let head_bytes = blob.content().to_vec();

    let disk_bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return Ok(ConventionsState::PresentButIgnored),
    };
    if head_bytes == disk_bytes {
        let content = String::from_utf8(disk_bytes).unwrap_or_default();
        Ok(ConventionsState::Trusted(content))
    } else {
        Ok(ConventionsState::PresentButIgnored)
    }
}

#[cfg(test)]
mod parser_tests {
    use super::*;
    use tempfile::TempDir;

    const SAMPLE_BLOCK: &str = "\
<!-- quorum:convention id=a1b2c3d4e5f6 v=1 -->
### Convention: do not block on stylistic ABC

Function bodies under 8 lines do not require docstrings; the project
treats this as a settled stylistic call after three independent
dismissals on review.
<!-- /quorum:convention -->";

    fn wrap_managed(blocks: &str, above: &str, below: &str) -> String {
        format!(
            "{above}<!-- quorum:managed-section v=1 -->
{blocks}
<!-- /quorum:managed-section -->{below}"
        )
    }

    #[test]
    fn file_with_no_fence_returns_above_only() {
        let raw = b"# Project conventions\n\nWritten by hand, no Quorum content.\n";
        let (parsed, diags) = parse_conventions_md(raw);
        assert!(!parsed.fence_present);
        assert!(parsed.blocks.is_empty());
        assert_eq!(parsed.above_fence, raw);
        assert_eq!(parsed.below_fence, b"");
        assert!(diags.is_empty());
    }

    #[test]
    fn empty_fence_parses_with_zero_blocks() {
        let raw = wrap_managed("", "user prose above\n\n", "\n\nfooter\n");
        let (parsed, diags) = parse_conventions_md(raw.as_bytes());
        assert!(parsed.fence_present);
        assert!(parsed.blocks.is_empty(), "no blocks inside an empty fence");
        assert!(diags.is_empty());
        // Above-fence preserved byte-for-byte.
        assert_eq!(parsed.above_fence, b"user prose above\n\n");
        // Below-fence preserved byte-for-byte.
        assert_eq!(parsed.below_fence, b"\n\nfooter\n");
    }

    #[test]
    fn fence_with_blocks_enumerates_all() {
        let two_blocks = format!(
            "\n{SAMPLE_BLOCK}\n\n<!-- quorum:convention id=9876fedcba01 v=1 -->\n### Convention: another\n\nBody.\n<!-- /quorum:convention -->\n"
        );
        let raw = wrap_managed(&two_blocks, "intro\n", "\nafter\n");
        let (parsed, diags) = parse_conventions_md(raw.as_bytes());
        assert!(parsed.fence_present);
        assert_eq!(parsed.blocks.len(), 2);
        assert_eq!(parsed.blocks[0].id, "a1b2c3d4e5f6");
        assert_eq!(parsed.blocks[0].version, 1);
        assert!(parsed.blocks[0].header_line.starts_with("### Convention: do not"));
        assert_eq!(parsed.blocks[1].id, "9876fedcba01");
        assert!(diags.is_empty());
        // Byte ranges are inside the input bounds and ordered.
        let r0 = &parsed.blocks[0].byte_range_in_file;
        let r1 = &parsed.blocks[1].byte_range_in_file;
        assert!(r0.end <= r1.start);
        assert!(r1.end <= raw.len());
    }

    #[test]
    fn first_line_marker_optional_on_read() {
        let raw_with =
            "<!-- quorum-managed-conventions-md v=1 -->\n# After marker\n".to_string();
        let (p_with, _) = parse_conventions_md(raw_with.as_bytes());
        assert!(p_with.first_line_marker_present);

        let raw_without = "# No marker\n".to_string();
        let (p_without, _) = parse_conventions_md(raw_without.as_bytes());
        assert!(!p_without.first_line_marker_present);
    }

    #[test]
    fn malformed_block_missing_close_emits_diagnostic() {
        let bad = "
<!-- quorum:convention id=a1b2c3d4e5f6 v=1 -->
### Convention: dangling
body without close
<!-- quorum:convention id=9876fedcba01 v=1 -->
### Convention: second
body
<!-- /quorum:convention -->
";
        let raw = wrap_managed(bad, "", "");
        let (parsed, diags) = parse_conventions_md(raw.as_bytes());
        // The well-formed second block is still parsed; the malformed
        // first surfaces a diagnostic.
        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.blocks[0].id, "9876fedcba01");
        assert!(
            diags
                .iter()
                .any(|d| matches!(d, ConventionParseError::UnclosedBlock { id, .. } if id == "a1b2c3d4e5f6")),
            "expected UnclosedBlock diagnostic for first block"
        );
    }

    #[test]
    fn malformed_block_bad_id_emits_diagnostic_and_does_not_panic() {
        let bad = "\n<!-- quorum:convention id=NOTHEX v=1 -->\n### bad\n<!-- /quorum:convention -->\n";
        let raw = wrap_managed(bad, "", "");
        let (parsed, diags) = parse_conventions_md(raw.as_bytes());
        assert!(parsed.blocks.is_empty());
        assert!(diags
            .iter()
            .any(|d| matches!(d, ConventionParseError::BadBlockId { .. })));
    }

    #[test]
    fn round_trip_preserves_outside_fence_bytes() {
        // AC 149 round-trip property (parser side): above + reconstructed
        // fence + below == input when nothing was edited.
        let raw = wrap_managed(
            &format!("\n{SAMPLE_BLOCK}\n"),
            "header lines\nwith CRLF\r\nand a trailing tab\t\n",
            "\nfoot \nlines\n",
        );
        let input = raw.as_bytes();
        let (parsed, diags) = parse_conventions_md(input);
        assert!(diags.is_empty());
        // The reconstructed fence is "<!-- quorum:managed-section v=1 -->" +
        // everything between markers + "<!-- /quorum:managed-section -->".
        let open_idx = parsed.above_fence.len();
        let mut recomposed = Vec::new();
        recomposed.extend_from_slice(parsed.above_fence);
        recomposed.extend_from_slice(&input[open_idx..input.len() - parsed.below_fence.len()]);
        recomposed.extend_from_slice(parsed.below_fence);
        assert_eq!(recomposed, input, "byte-for-byte round-trip");
    }

    #[test]
    fn duplicate_managed_section_surfaces_diagnostic() {
        let inner = format!("\n{SAMPLE_BLOCK}\n");
        let raw = format!(
            "above\n<!-- quorum:managed-section v=1 -->{inner}<!-- /quorum:managed-section -->\nmiddle\n<!-- quorum:managed-section v=1 -->\n<!-- /quorum:managed-section -->\nfooter\n"
        );
        let (parsed, diags) = parse_conventions_md(raw.as_bytes());
        assert!(parsed.fence_present);
        assert_eq!(parsed.blocks.len(), 1, "only first section parsed");
        assert!(diags
            .iter()
            .any(|d| matches!(d, ConventionParseError::DuplicateManagedSection { .. })));
    }

    #[test]
    fn is_valid_short_hash_basics() {
        assert!(is_valid_short_hash("a1b2c3d4e5f6"));
        assert!(is_valid_short_hash("0123456789ab"));
        assert!(!is_valid_short_hash("A1B2C3D4E5F6"), "uppercase rejected");
        assert!(!is_valid_short_hash("a1b2c3d4e5f"), "11 chars rejected");
        assert!(!is_valid_short_hash("a1b2c3d4e5f6g"), "13 chars rejected");
        assert!(!is_valid_short_hash("a1b2c3d4e5fz"), "non-hex rejected");
    }

    #[test]
    fn detect_orphans_file_orphan_and_db_orphan() {
        // Build a conventions.md with one block "a1b2..." and a DB row
        // "9876..." — both should appear as orphans.
        let td = TempDir::new().unwrap();
        let path = td.path().join("conventions.md");
        let raw = wrap_managed(&format!("\n{SAMPLE_BLOCK}\n"), "", "");
        std::fs::write(&path, raw).unwrap();
        let db_rows = vec![ConventionRow {
            finding_identity_hash_hex: "9".repeat(64),
            conventions_md_block_id: "9876fedcba01".into(),
            title_snapshot: "title for db orphan".into(),
        }];
        let report = detect_orphans(&path, &db_rows);
        assert!(!report.file_missing);
        assert!(!report.fence_absent);
        assert_eq!(report.file_orphans.len(), 1);
        assert_eq!(report.file_orphans[0].id, "a1b2c3d4e5f6");
        assert_eq!(report.db_orphans.len(), 1);
        assert_eq!(report.db_orphans[0].conventions_md_block_id, "9876fedcba01");
    }

    #[test]
    fn detect_orphans_no_file_treats_all_db_rows_as_orphans() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("does-not-exist.md");
        let db_rows = vec![ConventionRow {
            finding_identity_hash_hex: "a".repeat(64),
            conventions_md_block_id: "aaaaaaaaaaaa".into(),
            title_snapshot: "t".into(),
        }];
        let report = detect_orphans(&path, &db_rows);
        assert!(report.file_missing);
        assert!(report.fence_absent);
        assert_eq!(report.file_orphans.len(), 0);
        assert_eq!(report.db_orphans.len(), 1);
    }

    #[test]
    fn detect_orphans_full_match_is_clean() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("conventions.md");
        let raw = wrap_managed(&format!("\n{SAMPLE_BLOCK}\n"), "", "");
        std::fs::write(&path, raw).unwrap();
        let db_rows = vec![ConventionRow {
            finding_identity_hash_hex: "a".repeat(64),
            conventions_md_block_id: "a1b2c3d4e5f6".into(),
            title_snapshot: "matching".into(),
        }];
        let report = detect_orphans(&path, &db_rows);
        assert!(report.file_orphans.is_empty());
        assert!(report.db_orphans.is_empty());
        assert!(report.parser_diagnostics.is_empty());
    }

    #[test]
    fn detect_orphans_malformed_block_surfaces_parser_diagnostic() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("conventions.md");
        let bad = "\n<!-- quorum:convention id=a1b2c3d4e5f6 v=1 -->\nno close marker here\n";
        let raw = wrap_managed(bad, "", "");
        std::fs::write(&path, raw).unwrap();
        let report = detect_orphans(&path, &[]);
        assert!(!report.parser_diagnostics.is_empty());
    }
}
