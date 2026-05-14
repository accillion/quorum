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
pub fn parse_conventions_md(input: &[u8]) -> (ParsedConventionsMd<'_>, Vec<ConventionParseError>) {
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
    let Some(rel_close_idx) = find_subslice(
        &input[close_search_start..],
        MARKER_CLOSE_SECTION.as_bytes(),
    ) else {
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
    matches!(input.get(marker.len()), None | Some(b'\n') | Some(b'\r'))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
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
pub fn detect_orphans(conventions_md_path: &Path, db_rows: &[ConventionRow]) -> OrphanReport {
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
    let block_ids: std::collections::HashSet<&str> = parsed.blocks.iter().map(|b| b.id).collect();
    let db_block_ids: std::collections::HashSet<&str> = db_rows
        .iter()
        .map(|r| r.conventions_md_block_id.as_str())
        .collect();

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

// ---------------------------------------------------------------------------
// Phase 1C Stage 4 — writer + atomic file write helper (AC 137/148/149/150).
//
// The writer is the inverse of `parse_conventions_md`: it emits a canonical
// byte layout that round-trips through the parser when the input was itself
// produced by this writer. Above-fence and below-fence bytes from
// `ParsedConventionsMd` are emitted verbatim (AC 149 — byte-for-byte
// preservation). The managed section is rewritten from the supplied
// `BlockToWrite` list; line endings honor the caller-detected `LineEnding`.
//
// Atomicity: `atomic_write_managed_section` writes via a temp file in the
// same directory as the destination, then `fs::rename` for the visible-side
// commit (B5 ordering). The caller (Stage 4 `promote` / `demote`) wraps the
// SQLite COMMIT after the rename returns Ok.

const MARKER_DO_NOT_EDIT: &str =
    "<!-- DO NOT EDIT MANUALLY: this section is rewritten by `quorum convention promote/demote`. -->";
const MARKER_DEMOTE_HINT: &str =
    "<!-- To remove an auto-derived convention, run `quorum convention demote <hash>`. -->";

/// Line-ending convention for a conventions.md file. Detected from existing
/// content via [`LineEnding::detect`]; a fresh file defaults to LF (spec §4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
        }
    }

    /// Detect the line ending from raw bytes. The first line break in the
    /// buffer wins: `\r\n` => CRLF, lone `\n` => LF. Empty / no-line-break
    /// input falls back to LF (spec §4.4: "On a new file, defaults to LF").
    pub fn detect(input: &[u8]) -> LineEnding {
        let mut i = 0;
        while i < input.len() {
            if input[i] == b'\n' {
                if i > 0 && input[i - 1] == b'\r' {
                    return LineEnding::Crlf;
                }
                return LineEnding::Lf;
            }
            i += 1;
        }
        LineEnding::Lf
    }
}

/// One managed block, fully described for the writer. `id` is the 12-hex
/// prefix of `finding_identity_hash`; `version` is the on-disk format
/// version (always 1 in Phase 1C). `title` becomes the `### Convention:
/// <title>` header line; `body` is the user-supplied prose (may span
/// multiple lines; may be empty for title-only promotes per §4.4).
#[derive(Debug, Clone)]
pub struct BlockToWrite<'a> {
    pub id: &'a str,
    pub version: u32,
    pub title: &'a str,
    pub body: &'a str,
}

impl<'a> BlockToWrite<'a> {
    /// Round-trip helper: reconstruct a `BlockToWrite` from a `ParsedBlock`
    /// produced by `parse_conventions_md` on this writer's own output.
    ///
    /// The canonical body layout this writer emits is:
    /// ```text
    /// <LE>### Convention: <TITLE><LE>            (when body empty)
    /// <LE>### Convention: <TITLE><LE><LE><BODY><LE>  (when body non-empty)
    /// ```
    /// where `<LE>` is the file's line ending. `from_parsed_block` reverses
    /// this layout to recover `title` and `body` as borrowed slices of the
    /// original file. Returns `None` if the block's body does not match
    /// the canonical layout (e.g., user hand-edited it inside the fence).
    pub fn from_parsed_block(pb: &ParsedBlock<'a>) -> Option<Self> {
        let mut body = pb.body;
        // Strip one leading line break.
        body = body.strip_prefix("\r\n").or_else(|| body.strip_prefix('\n'))?;
        // First line is the header.
        let (first_line, rest) = match body.find('\n') {
            Some(idx) => (&body[..idx], &body[idx + 1..]),
            None => (body, ""),
        };
        let header = first_line.trim_end_matches('\r');
        let title = header.strip_prefix("### Convention: ")?;
        if rest.is_empty() {
            // No body.
            return Some(BlockToWrite {
                id: pb.id,
                version: pb.version,
                title,
                body: "",
            });
        }
        // Expect a blank line before the body.
        let mut after_blank = rest;
        after_blank = after_blank
            .strip_prefix("\r\n")
            .or_else(|| after_blank.strip_prefix('\n'))?;
        // Strip exactly one trailing line break before the close marker.
        let body_part = after_blank
            .strip_suffix("\r\n")
            .or_else(|| after_blank.strip_suffix('\n'))
            .unwrap_or(after_blank);
        Some(BlockToWrite {
            id: pb.id,
            version: pb.version,
            title,
            body: body_part,
        })
    }
}

/// Render `parsed.above_fence + managed-section + parsed.below_fence` into
/// a fresh byte buffer.
///
/// Behavior (spec §4.4):
/// - `parsed.above_fence` emitted byte-for-byte (AC 149).
/// - `parsed.below_fence` emitted byte-for-byte (AC 149).
/// - Section fence auto-created if `parsed.fence_present == false`
///   (AC 148).
/// - First-line marker `<!-- quorum-managed-conventions-md v=1 -->` written
///   only on fresh files (`parsed.above_fence` empty AND fence absent).
///   Preserved by `parsed.above_fence` byte-passthrough when already present.
/// - Line endings: every inserted line uses `le.as_str()` (AC 150).
/// - Duplicate-id `blocks_to_write` is a caller bug. In debug builds we
///   panic via `debug_assert!`; in release the writer emits all entries
///   (last-wins behavior on re-parse).
pub fn render_conventions_md(
    parsed: &ParsedConventionsMd<'_>,
    blocks_to_write: &[BlockToWrite<'_>],
    le: LineEnding,
) -> Vec<u8> {
    debug_assert!(
        unique_ids(blocks_to_write),
        "render_conventions_md called with duplicate-id blocks; caller must dedupe"
    );

    let mut out: Vec<u8> = Vec::with_capacity(
        parsed.above_fence.len() + parsed.below_fence.len() + 256 + blocks_to_write.len() * 256,
    );
    let lebytes = le.as_str().as_bytes();

    let fresh_file = parsed.above_fence.is_empty() && !parsed.fence_present;
    if fresh_file {
        // §4.4 last bullet: the first-line marker is written on freshly-
        // created files only. Existing files without it keep their absence
        // (parsed.above_fence carries the existing first bytes).
        out.extend_from_slice(MARKER_FILE_FIRST_LINE.as_bytes());
        out.extend_from_slice(lebytes);
    } else {
        out.extend_from_slice(parsed.above_fence);
    }

    // Managed-section open marker + DO NOT EDIT comments + blank line.
    out.extend_from_slice(MARKER_OPEN_SECTION.as_bytes());
    out.extend_from_slice(lebytes);
    out.extend_from_slice(MARKER_DO_NOT_EDIT.as_bytes());
    out.extend_from_slice(lebytes);
    out.extend_from_slice(MARKER_DEMOTE_HINT.as_bytes());
    out.extend_from_slice(lebytes);

    // Blocks, separated by one blank line. Each block is rendered as:
    //   <LE><!-- quorum:convention id=ID v=V --><LE>### Convention: T<LE>
    //   [<LE><body><LE>] <!-- /quorum:convention --><LE>
    for block in blocks_to_write {
        out.extend_from_slice(lebytes); // blank line before each block
        emit_block(&mut out, block, lebytes);
        out.extend_from_slice(lebytes);
    }

    // Managed-section close marker. Always preceded by exactly one blank
    // line (one extra LE) whether or not there were blocks — keeps the
    // empty-section layout symmetric with the no-block fresh-file case.
    if blocks_to_write.is_empty() {
        out.extend_from_slice(lebytes);
    }
    out.extend_from_slice(MARKER_CLOSE_SECTION.as_bytes());

    out.extend_from_slice(parsed.below_fence);
    out
}

fn unique_ids(blocks: &[BlockToWrite<'_>]) -> bool {
    let mut seen = std::collections::HashSet::with_capacity(blocks.len());
    blocks.iter().all(|b| seen.insert(b.id))
}

fn emit_block(out: &mut Vec<u8>, block: &BlockToWrite<'_>, le: &[u8]) {
    out.extend_from_slice(b"<!-- quorum:convention id=");
    out.extend_from_slice(block.id.as_bytes());
    out.extend_from_slice(b" v=");
    out.extend_from_slice(block.version.to_string().as_bytes());
    out.extend_from_slice(b" -->");
    out.extend_from_slice(le);
    out.extend_from_slice(b"### Convention: ");
    out.extend_from_slice(block.title.as_bytes());
    out.extend_from_slice(le);
    if !block.body.is_empty() {
        out.extend_from_slice(le);
        out.extend_from_slice(block.body.as_bytes());
        if !block.body.ends_with('\n') {
            out.extend_from_slice(le);
        }
    }
    out.extend_from_slice(b"<!-- /quorum:convention -->");
}

/// Atomic-on-same-volume write: write `bytes` to a sibling temp file, then
/// `fs::rename` into place. Spec §3.2 T2 step 2 — the file-side commit
/// point. The SQLite COMMIT MUST run only after this returns `Ok` (B5
/// ordering from the v0.2 adjudication).
///
/// The temp file lives in `path.parent()` so the rename stays on the same
/// filesystem (POSIX) / volume (Windows `MoveFileExW`). On rename failure
/// the temp file is removed (best-effort) and the IO error returned.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "atomic_write: target path has no parent",
        )
    })?;
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let file_name = path
        .file_name()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "atomic_write: target path has no file name",
            )
        })?
        .to_string_lossy();
    let temp_name = format!(".{file_name}.tmp.{pid}.{nanos}");
    let temp_path = parent.join(temp_name);

    // Write bytes to the temp file, fsync, then rename.
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&temp_path)?;
        f.write_all(bytes)?;
        // Best-effort fsync to ensure the bytes hit disk before the rename
        // is visible — Windows tolerates a missing fsync, but POSIX-on-EXT4
        // can leave a zero-length file if power-loss races the rename.
        let _ = f.sync_all();
    }
    if let Err(e) = std::fs::rename(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod stage4_test_seam {
    //! AC 175 crash harness: a `#[cfg(test)]` panic seam fired between
    //! `atomic_write` returning Ok and the caller's SQLite COMMIT. Lets
    //! `crates/quorum-cli/tests/convention_write.rs` verify the
    //! file-ahead-of-SQLite recovery contract by panicking the promote
    //! transaction mid-flight and asserting (a) the file holds the new
    //! block and (b) SQLite state is still `local_only`, then (c)
    //! idempotent re-promote heals.
    use std::sync::atomic::{AtomicBool, Ordering};
    pub static FAIL_AFTER_RENAME: AtomicBool = AtomicBool::new(false);

    pub fn maybe_panic_after_rename() {
        if FAIL_AFTER_RENAME.load(Ordering::SeqCst) {
            panic!("AC 175 crash harness fired between fs::rename and SQLite COMMIT");
        }
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
        assert!(parsed.blocks[0]
            .header_line
            .starts_with("### Convention: do not"));
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
        let raw_with = "<!-- quorum-managed-conventions-md v=1 -->\n# After marker\n".to_string();
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
        let bad =
            "\n<!-- quorum:convention id=NOTHEX v=1 -->\n### bad\n<!-- /quorum:convention -->\n";
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

#[cfg(test)]
mod writer_tests {
    //! Phase 1C Stage 4 — writer + atomic_write coverage (AC 137/148/149/150).

    use super::*;
    use tempfile::TempDir;

    fn parse_then_render(bytes: &[u8], le: LineEnding) -> Vec<u8> {
        let (parsed, diags) = parse_conventions_md(bytes);
        assert!(diags.is_empty(), "fixture must parse cleanly: {diags:?}");
        let blocks: Vec<BlockToWrite<'_>> = parsed
            .blocks
            .iter()
            .map(|pb| {
                BlockToWrite::from_parsed_block(pb).unwrap_or_else(|| {
                    panic!("from_parsed_block failed on canonical fixture block id={}", pb.id)
                })
            })
            .collect();
        render_conventions_md(&parsed, &blocks, le)
    }

    fn write_then_round_trip(blocks: &[BlockToWrite<'_>], le: LineEnding) -> Vec<u8> {
        // Fresh-file render → first-line marker + fence + blocks.
        let empty = ParsedConventionsMd {
            above_fence: b"",
            below_fence: b"",
            fence_present: false,
            first_line_marker_present: false,
            blocks: Vec::new(),
        };
        render_conventions_md(&empty, blocks, le)
    }

    #[test]
    fn line_ending_detect_basic() {
        assert_eq!(LineEnding::detect(b""), LineEnding::Lf);
        assert_eq!(LineEnding::detect(b"no breaks here"), LineEnding::Lf);
        assert_eq!(LineEnding::detect(b"line1\nline2"), LineEnding::Lf);
        assert_eq!(LineEnding::detect(b"line1\r\nline2"), LineEnding::Crlf);
        // Mixed: first wins.
        assert_eq!(LineEnding::detect(b"first\nthen\r\nrest"), LineEnding::Lf);
        assert_eq!(LineEnding::detect(b"first\r\nthen\nrest"), LineEnding::Crlf);
    }

    #[test]
    fn fresh_file_emits_first_line_marker_and_fence() {
        let blocks: Vec<BlockToWrite<'_>> = Vec::new();
        let bytes = write_then_round_trip(&blocks, LineEnding::Lf);
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(
            s.starts_with("<!-- quorum-managed-conventions-md v=1 -->\n"),
            "fresh file starts with first-line marker; got: {s:?}"
        );
        assert!(s.contains("<!-- quorum:managed-section v=1 -->\n"));
        assert!(s.contains("<!-- /quorum:managed-section -->"));
        // Re-parse: marker preserved, fence present, zero blocks.
        let (parsed, diags) = parse_conventions_md(&bytes);
        assert!(diags.is_empty());
        assert!(parsed.first_line_marker_present);
        assert!(parsed.fence_present);
        assert!(parsed.blocks.is_empty());
    }

    #[test]
    fn existing_file_without_marker_preserves_marker_absence() {
        // User authored a conventions.md with no Quorum marker — promotion
        // appends fence only, keeping the user's first line intact.
        let above = b"# Project conventions\n\nA hand-rolled file.\n";
        let parsed = ParsedConventionsMd {
            above_fence: above,
            below_fence: b"",
            fence_present: false,
            first_line_marker_present: false,
            blocks: Vec::new(),
        };
        let block = BlockToWrite {
            id: "a1b2c3d4e5f6",
            version: 1,
            title: "test",
            body: "",
        };
        let bytes = render_conventions_md(&parsed, &[block], LineEnding::Lf);
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(
            !s.starts_with("<!-- quorum-managed-conventions-md"),
            "marker absence preserved on existing file"
        );
        assert!(s.starts_with("# Project conventions\n"));
        assert!(s.contains("### Convention: test\n"));
    }

    #[test]
    fn empty_body_block_canonical_layout() {
        let block = BlockToWrite {
            id: "a1b2c3d4e5f6",
            version: 1,
            title: "title-only",
            body: "",
        };
        let bytes = write_then_round_trip(&[block], LineEnding::Lf);
        let s = std::str::from_utf8(&bytes).unwrap();
        // Expect the open marker followed by header line, then close marker
        // with NO blank line in between (empty-body case).
        assert!(s.contains("<!-- quorum:convention id=a1b2c3d4e5f6 v=1 -->\n### Convention: title-only\n<!-- /quorum:convention -->"));
    }

    #[test]
    fn non_empty_body_block_canonical_layout() {
        let block = BlockToWrite {
            id: "a1b2c3d4e5f6",
            version: 1,
            title: "with body",
            body: "Hello\nbody.",
        };
        let bytes = write_then_round_trip(&[block], LineEnding::Lf);
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.contains(
            "<!-- quorum:convention id=a1b2c3d4e5f6 v=1 -->\n### Convention: with body\n\nHello\nbody.\n<!-- /quorum:convention -->"
        ), "got: {s}");
    }

    #[test]
    fn round_trip_lf_single_block() {
        // AC 149 round-trip: writer-produced bytes round-trip through the
        // parser and back.
        let blocks = [BlockToWrite {
            id: "a1b2c3d4e5f6",
            version: 1,
            title: "the title",
            body: "first line\nsecond line",
        }];
        let first = write_then_round_trip(&blocks, LineEnding::Lf);
        let second = parse_then_render(&first, LineEnding::Lf);
        assert_eq!(first, second, "round-trip is byte-equal (LF)");
    }

    #[test]
    fn round_trip_crlf_single_block() {
        let blocks = [BlockToWrite {
            id: "a1b2c3d4e5f6",
            version: 1,
            title: "crlf-title",
            body: "windows body",
        }];
        let first = write_then_round_trip(&blocks, LineEnding::Crlf);
        let second = parse_then_render(&first, LineEnding::Crlf);
        assert_eq!(first, second, "round-trip is byte-equal (CRLF)");
        // Confirm CRLF actually emitted.
        assert!(first.windows(2).any(|w| w == b"\r\n"), "CRLF must appear");
    }

    #[test]
    fn round_trip_two_blocks() {
        let blocks = [
            BlockToWrite {
                id: "a1b2c3d4e5f6",
                version: 1,
                title: "first",
                body: "body A",
            },
            BlockToWrite {
                id: "9876fedcba01",
                version: 1,
                title: "second",
                body: "",
            },
        ];
        let first = write_then_round_trip(&blocks, LineEnding::Lf);
        let second = parse_then_render(&first, LineEnding::Lf);
        assert_eq!(first, second, "two-block round-trip byte-equal");
    }

    #[test]
    fn above_fence_bytes_preserved_byte_for_byte() {
        // AC 149: arbitrary above-fence content survives.
        let above = b"# header with weird \xc3\xa9 chars\n\nand tabs\there\nplus trailing\r\n";
        let parsed = ParsedConventionsMd {
            above_fence: above,
            below_fence: b"\n\nfooter line\n",
            fence_present: false,
            first_line_marker_present: false,
            blocks: Vec::new(),
        };
        let block = BlockToWrite {
            id: "a1b2c3d4e5f6",
            version: 1,
            title: "t",
            body: "b",
        };
        let bytes = render_conventions_md(&parsed, &[block], LineEnding::Lf);
        assert!(bytes.starts_with(above));
        assert!(bytes.ends_with(b"\n\nfooter line\n"));
    }

    #[test]
    fn from_parsed_block_recovers_canonical_layout() {
        // Round-trip a hand-built canonical file through parse → from_parsed
        // → render and verify byte equality.
        let blocks = [BlockToWrite {
            id: "abcdef012345",
            version: 1,
            title: "round-trip me",
            body: "some\nbody\nlines",
        }];
        let original = write_then_round_trip(&blocks, LineEnding::Lf);
        let (parsed, diags) = parse_conventions_md(&original);
        assert!(diags.is_empty());
        assert_eq!(parsed.blocks.len(), 1);
        let recovered = BlockToWrite::from_parsed_block(&parsed.blocks[0]).unwrap();
        assert_eq!(recovered.title, "round-trip me");
        assert_eq!(recovered.body, "some\nbody\nlines");
    }

    #[test]
    fn from_parsed_block_returns_none_on_non_canonical() {
        // A hand-edited block whose body doesn't start with the canonical
        // `<LE>### Convention: ...` layout should not round-trip silently.
        let raw = "above\n<!-- quorum:managed-section v=1 -->\n<!-- quorum:convention id=a1b2c3d4e5f6 v=1 -->\nHand-edited body — no header line.\n<!-- /quorum:convention -->\n<!-- /quorum:managed-section -->\n";
        let (parsed, _) = parse_conventions_md(raw.as_bytes());
        assert_eq!(parsed.blocks.len(), 1);
        // Header_line is non-empty (parser picks first non-blank line) but
        // doesn't start with `### Convention: `, so from_parsed_block
        // refuses.
        assert!(BlockToWrite::from_parsed_block(&parsed.blocks[0]).is_none());
    }

    #[test]
    fn atomic_write_happy_path() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("conventions.md");
        atomic_write(&path, b"hello world").unwrap();
        let got = std::fs::read(&path).unwrap();
        assert_eq!(got, b"hello world");
        // No temp leftover.
        let entries: Vec<_> = std::fs::read_dir(td.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1, "no temp file leaked");
    }

    #[test]
    fn atomic_write_overwrites_existing() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("conventions.md");
        std::fs::write(&path, b"old content").unwrap();
        atomic_write(&path, b"new content").unwrap();
        let got = std::fs::read(&path).unwrap();
        assert_eq!(got, b"new content");
    }

    #[test]
    fn atomic_write_rejects_path_without_parent() {
        // An empty / root path has no parent — we surface InvalidInput
        // instead of silently writing to CWD.
        let err = atomic_write(Path::new(""), b"x").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
}
