//! `quorum convention` — Phase 1C Stage 3 read surface.
//!
//! Stage 3 ships `list`, `show`, `history`. The write subcommands
//! (`promote`, `demote`, `prune`) land in Stage 4 and are intentionally
//! absent from the clap surface here — they're cleaner absent than stubbed.

use std::io::Write;
use std::path::{Path, PathBuf};

use quorum_core::conventions::{
    atomic_write, detect_orphans, parse_conventions_md, render_conventions_md, BlockToWrite,
    LineEnding, OrphanReport, ParsedConventionsMd,
};
use quorum_core::memory::{
    DemoteOutcome, Dismissal, LocalSqliteMemoryStore, MemoryStore, PromoteOutcome, PromotionState,
    ShortHashResolution, StateTransitionRow, TransitionTrigger,
};

use crate::exit::{CliError, Exit};

/// Resolve the directory under which `.quorum/` lives. Honors the
/// `--quorum-dir` flag (spec §5.1: "all `convention` subcommands honor
/// `--quorum-dir <path>` for tests") and falls back to the current
/// working directory.
fn resolve_quorum_root(quorum_dir: Option<&PathBuf>) -> Result<PathBuf, CliError> {
    if let Some(p) = quorum_dir {
        return Ok(p.clone());
    }
    std::env::current_dir().map_err(|e| CliError::Io(e.to_string()))
}

fn open_store(root: &std::path::Path) -> Result<LocalSqliteMemoryStore, CliError> {
    LocalSqliteMemoryStore::new(root).map_err(|e| CliError::Io(e.to_string()))
}

// ---------------------------------------------------------------------------
// `list`
// ---------------------------------------------------------------------------

pub fn list(
    quorum_dir: Option<&PathBuf>,
    state: Option<PromotionState>,
    orphans: bool,
    json: bool,
) -> Result<Exit, CliError> {
    let root = resolve_quorum_root(quorum_dir)?;
    if orphans {
        return list_orphans(&root, json);
    }
    let store = open_store(&root)?;
    let rows = store
        .list_by_state(state)
        .map_err(|e| CliError::Io(format!("dismissals query failed: {e}")))?;
    if json {
        emit_list_json(&rows);
    } else {
        emit_list_text(&rows);
    }
    Ok(Exit::Ok)
}

/// Stable JSON shape — order: `hash`, `short_hash`, `state`,
/// `recurrence_count`, `title`, `dismissed_at`, `last_seen_at`. Same row
/// order as the default text output so `jq` pipelines are deterministic.
fn emit_list_json(rows: &[Dismissal]) {
    let mut buf = String::from("[");
    for (i, d) in rows.iter().enumerate() {
        if i > 0 {
            buf.push(',');
        }
        let hex = d.finding_identity_hash.to_hex();
        let short = &hex[..12];
        let dismissed = d
            .dismissed_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        let last_seen = d
            .last_seen_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        // Hand-assemble so we never serialise body/note (audit-leak guard
        // — Stage 2 §4.6: notes are sensitive and stay off-disk-outside-DB).
        buf.push_str(&format!(
            "{{\"hash\":{},\"short_hash\":{},\"state\":{},\"recurrence_count\":{},\"title\":{},\"dismissed_at\":{},\"last_seen_at\":{}}}",
            json_str(&hex),
            json_str(short),
            json_str(d.promotion_state.as_db_str()),
            d.recurrence_count,
            json_str(&d.title_snapshot),
            json_str(&dismissed),
            json_str(&last_seen),
        ));
    }
    buf.push(']');
    println!("{buf}");
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

const TITLE_DISPLAY_WIDTH: usize = 60;

fn emit_list_text(rows: &[Dismissal]) {
    if rows.is_empty() {
        println!("(no dismissals)");
        return;
    }
    println!("{:<12}  {:<20}  {:>5}  TITLE", "HASH", "STATE", "RECUR");
    for d in rows {
        let hex = d.finding_identity_hash.to_hex();
        let short = &hex[..12];
        let title = truncate_for_display(&d.title_snapshot, TITLE_DISPLAY_WIDTH);
        println!(
            "{:<12}  {:<20}  {:>5}  {}",
            short,
            d.promotion_state.as_db_str(),
            d.recurrence_count,
            title
        );
    }
}

fn truncate_for_display(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (n, c) in s.chars().enumerate() {
        if n >= max_chars {
            out.push('…');
            return out;
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------------
// `--orphans` (AC 169; AC 168 stderr-warning partial)
// ---------------------------------------------------------------------------

fn list_orphans(root: &std::path::Path, json: bool) -> Result<Exit, CliError> {
    let store = open_store(root)?;
    let rows = store
        .list_conventions()
        .map_err(|e| CliError::Io(format!("conventions read failed: {e}")))?;
    let conv_md_path = root.join(".quorum").join("conventions.md");
    let report = detect_orphans(&conv_md_path, &rows);

    // AC 168 partial — emit parser diagnostics as stderr warnings.
    for d in &report.parser_diagnostics {
        eprintln!("warning: {}", format_diagnostic(d));
    }

    if json {
        emit_orphan_report_json(&report);
    } else {
        emit_orphan_report_text(&report);
    }
    Ok(Exit::Ok)
}

pub fn format_diagnostic(d: &quorum_core::conventions::ConventionParseError) -> String {
    use quorum_core::conventions::ConventionParseError::*;
    match d {
        UnclosedBlock { id, start_byte } => format!(
            ".quorum/conventions.md: block id={id} at byte {start_byte} has no closing marker"
        ),
        BadBlockId { raw, start_byte } => {
            format!(".quorum/conventions.md: bad block id {raw:?} at byte {start_byte}")
        }
        BadBlockVersion { raw, start_byte } => {
            format!(".quorum/conventions.md: bad block version {raw:?} at byte {start_byte}")
        }
        DuplicateManagedSection { start_byte } => format!(
            ".quorum/conventions.md: duplicate managed-section marker at byte {start_byte}; \
             only the first section is parsed"
        ),
    }
}

fn emit_orphan_report_text(report: &OrphanReport) {
    if report.file_missing {
        println!("(.quorum/conventions.md not found)");
    } else if report.fence_absent {
        println!("(no quorum-managed-section in .quorum/conventions.md)");
    }
    if report.file_orphans.is_empty() && report.db_orphans.is_empty() {
        if !report.file_missing && !report.fence_absent {
            println!("(no orphans)");
        }
        return;
    }
    if !report.file_orphans.is_empty() {
        println!("file orphans (block in conventions.md, no SQLite row):");
        for o in &report.file_orphans {
            let header = if o.header_line.is_empty() {
                "(no header)"
            } else {
                &o.header_line
            };
            println!("  {}  {}", o.id, header);
        }
    }
    if !report.db_orphans.is_empty() {
        println!("db orphans (SQLite row, no block in conventions.md):");
        for o in &report.db_orphans {
            println!("  {}  {}", o.conventions_md_block_id, o.title_snapshot);
        }
    }
}

fn emit_orphan_report_json(report: &OrphanReport) {
    let mut buf = String::new();
    buf.push('{');
    buf.push_str(&format!("\"file_missing\":{},", report.file_missing));
    buf.push_str(&format!("\"fence_absent\":{},", report.fence_absent));
    buf.push_str("\"file_orphans\":[");
    for (i, o) in report.file_orphans.iter().enumerate() {
        if i > 0 {
            buf.push(',');
        }
        buf.push_str(&format!(
            "{{\"id\":{},\"header_line\":{},\"byte_start\":{},\"byte_end\":{}}}",
            json_str(&o.id),
            json_str(&o.header_line),
            o.byte_range_in_file.start,
            o.byte_range_in_file.end
        ));
    }
    buf.push_str("],\"db_orphans\":[");
    for (i, o) in report.db_orphans.iter().enumerate() {
        if i > 0 {
            buf.push(',');
        }
        buf.push_str(&format!(
            "{{\"hash\":{},\"conventions_md_block_id\":{},\"title\":{}}}",
            json_str(&o.finding_identity_hash_hex),
            json_str(&o.conventions_md_block_id),
            json_str(&o.title_snapshot)
        ));
    }
    buf.push_str("]}");
    println!("{buf}");
}

// ---------------------------------------------------------------------------
// Short-hash resolution helper (WI-5; AC 163).
// ---------------------------------------------------------------------------

const MIN_SHORT_HASH_LEN: usize = 8;

/// Resolve a hex-prefix CLI argument to a single dismissal row, or emit a
/// `CliError::Config` (exit 2) with a precise diagnostic on rejection /
/// ambiguity / not-found. Pure presentation concern — wraps
/// `MemoryStore::find_by_short_hash`. Stage 4 will re-use this helper for
/// `promote` / `demote`.
pub fn resolve_short_hash(
    store: &LocalSqliteMemoryStore,
    prefix: &str,
) -> Result<Dismissal, CliError> {
    if prefix.len() < MIN_SHORT_HASH_LEN {
        return Err(CliError::Config(format!(
            "short-hash must be ≥ {} hex chars, got {}",
            MIN_SHORT_HASH_LEN,
            prefix.len()
        )));
    }
    if !prefix
        .bytes()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(CliError::Config(format!(
            "short-hash must be lowercase hex, got '{prefix}'"
        )));
    }
    match store
        .find_by_short_hash(prefix)
        .map_err(|e| CliError::Io(format!("short-hash lookup failed: {e}")))?
    {
        ShortHashResolution::Exact(d) => Ok(*d),
        ShortHashResolution::NotFound => {
            Err(CliError::Config(format!("no dismissal matches '{prefix}'")))
        }
        ShortHashResolution::Ambiguous(matches) => {
            let mut msg = format!("short-hash '{prefix}' is ambiguous; matches:\n");
            let shown = matches.iter().take(10);
            for d in shown {
                let hex = d.finding_identity_hash.to_hex();
                msg.push_str(&format!(
                    "  {}  {}\n",
                    &hex[..12],
                    truncate_for_display(&d.title_snapshot, TITLE_DISPLAY_WIDTH)
                ));
            }
            if matches.len() > 10 {
                msg.push_str(&format!("  ... and {} more\n", matches.len() - 10));
            }
            msg.pop(); // strip trailing newline so CliError display is tidy
            Err(CliError::Config(msg))
        }
    }
}

// ---------------------------------------------------------------------------
// `show <hash>` / `history <hash>` (WI-6).
// ---------------------------------------------------------------------------

pub fn show(quorum_dir: Option<&PathBuf>, hash_prefix: &str) -> Result<Exit, CliError> {
    let root = resolve_quorum_root(quorum_dir)?;
    let store = open_store(&root)?;
    let row = resolve_short_hash(&store, hash_prefix)?;
    let transitions = store
        .load_transitions(&row.finding_identity_hash)
        .map_err(|e| CliError::Io(format!("transition log read: {e}")))?;
    emit_show(&row, &transitions);
    Ok(Exit::Ok)
}

pub fn history(quorum_dir: Option<&PathBuf>, hash_prefix: &str) -> Result<Exit, CliError> {
    let root = resolve_quorum_root(quorum_dir)?;
    let store = open_store(&root)?;
    let row = resolve_short_hash(&store, hash_prefix)?;
    let transitions = store
        .load_transitions(&row.finding_identity_hash)
        .map_err(|e| CliError::Io(format!("transition log read: {e}")))?;
    emit_history(&transitions, row.promotion_state);
    Ok(Exit::Ok)
}

fn emit_show(row: &Dismissal, transitions: &[StateTransitionRow]) {
    let hex = row.finding_identity_hash.to_hex();
    let short = &hex[..12];
    println!("hash:        {hex}");
    println!("short-hash:  {short}");
    println!("title:       {}", row.title_snapshot);
    match &row.body_snapshot {
        Some(b) if !b.is_empty() => println!("body:        {b}"),
        _ => println!("body:        <no body snapshot>"),
    }
    println!("state:       {}", row.promotion_state.as_db_str());
    println!("recurrence:  {}", row.recurrence_count);
    let first_seen = row
        .dismissed_at
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let last_seen = row
        .last_seen_at
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    println!("first-seen:  {first_seen}");
    println!("last-seen:   {last_seen}");
    println!();
    println!("transitions:");
    emit_history(transitions, row.promotion_state);
}

fn emit_history(transitions: &[StateTransitionRow], current_state: PromotionState) {
    if transitions.is_empty() {
        // Distinguish a fresh v2 candidate row (initial state — no
        // transition row is ever written when entering `candidate`) from
        // a row whose state advanced past candidate without leaving a
        // transition trail (only possible for pre-v2 rows migrated under
        // the schema v2 epoch). BUG 3 fix: the old code reported every
        // empty-transitions row as "predates schema v2", which is
        // misleading on the most common case (fresh v2 candidates).
        if current_state == PromotionState::Candidate {
            println!("(no transitions yet — dismissal is in initial state)");
        } else {
            println!("(no transition history — dismissal predates schema v2)");
        }
        return;
    }
    for t in transitions {
        let ts = format_ts_ms(t.ts_ms);
        let rec = t
            .recurrence_at_transition
            .map(|n| format!(" (recurrence={n} at transition)"))
            .unwrap_or_default();
        println!(
            "  {ts}  {} → {}  via {}{}",
            t.from_state.as_db_str(),
            t.to_state.as_db_str(),
            trigger_label(t.trigger),
            rec
        );
    }
}

fn trigger_label(t: TransitionTrigger) -> &'static str {
    t.as_db_str()
}

fn format_ts_ms(ms: i64) -> String {
    // `ms` is unix epoch millis. Render in ISO-8601 UTC for human read.
    let secs = ms / 1000;
    let nanos = ((ms % 1000) * 1_000_000) as i128;
    let dt =
        time::OffsetDateTime::from_unix_timestamp_nanos((secs as i128) * 1_000_000_000 + nanos)
            .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    dt.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| format!("{ms}ms"))
}

// ---------------------------------------------------------------------------
// Stage 4 write surface: `promote`, `demote`, `prune` (AC 137/139/140/141/142).
// ---------------------------------------------------------------------------

/// Source for `quorum convention promote --text` / `--from-editor` /
/// (neither). Title-only promote (§4.4: no auto-body from `body_snapshot`)
/// is `BodySource::TitleOnly`.
#[derive(Debug, Clone)]
pub enum BodySource {
    /// `--text "<s>"`: use the supplied string verbatim.
    Text(String),
    /// `--from-editor`: spawn `$EDITOR` (test seam: `QUORUM_TEST_EDITOR_BODY`
    /// env var overrides the spawn and supplies the body directly).
    FromEditor,
    /// Neither flag — block carries only the title-derived `### Convention: …`
    /// header line; no body paragraph (§4.4, AC 139).
    TitleOnly,
}

/// Test-only env var for hermetic `--from-editor` coverage. When set,
/// supplies the body directly instead of spawning `$EDITOR`. Documented
/// inline so close-report readers see the seam choice.
const TEST_EDITOR_BODY_ENV: &str = "QUORUM_TEST_EDITOR_BODY";

/// Resolve a `BodySource` into the body string actually written to the
/// managed block. `TitleOnly` produces an empty body; `Text(s)` returns `s`
/// directly; `FromEditor` consults `QUORUM_TEST_EDITOR_BODY` (test seam)
/// then falls back to spawning `$EDITOR` on a temp template.
fn resolve_body(src: &BodySource, title: &str, quorum_dir: &Path) -> Result<String, CliError> {
    match src {
        BodySource::TitleOnly => Ok(String::new()),
        BodySource::Text(s) => {
            if s.is_empty() {
                return Err(CliError::Config(
                    "--text value is empty; pass --text \"<body>\" or omit --text for a \
                     title-only block"
                        .into(),
                ));
            }
            Ok(s.clone())
        }
        BodySource::FromEditor => {
            // Hermetic test seam.
            if let Ok(canned) = std::env::var(TEST_EDITOR_BODY_ENV) {
                if canned.trim().is_empty() {
                    return Err(CliError::Config(
                        "$QUORUM_TEST_EDITOR_BODY produced empty body".into(),
                    ));
                }
                return Ok(canned);
            }
            spawn_editor_for_body(title, quorum_dir)
        }
    }
}

fn spawn_editor_for_body(title: &str, quorum_dir: &Path) -> Result<String, CliError> {
    let editor = std::env::var("EDITOR").map_err(|_| {
        CliError::Config(
            "$EDITOR not set; cannot launch editor for --from-editor. Set EDITOR or use \
             --text \"<body>\" instead."
                .into(),
        )
    })?;
    let dir = quorum_dir.join(".quorum");
    std::fs::create_dir_all(&dir).map_err(|e| CliError::Io(e.to_string()))?;
    let template_path = dir.join(format!(
        ".convention_edit.{}.{}.md",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    // Template: a comment header + a blank line for the user to type the
    // body into. The title-derived comment line is stripped on read so
    // it doesn't leak into the committed block.
    let template = format!(
        "# Title: {title}\n# Lines starting with '#' will be ignored.\n# Enter the convention body below this line; save and exit.\n\n"
    );
    std::fs::write(&template_path, &template).map_err(|e| CliError::Io(e.to_string()))?;

    let status = std::process::Command::new(&editor)
        .arg(&template_path)
        .status()
        .map_err(|e| CliError::Io(format!("could not spawn $EDITOR={editor}: {e}")))?;
    if !status.success() {
        let _ = std::fs::remove_file(&template_path);
        return Err(CliError::Config(format!(
            "$EDITOR exited with status {status:?}; aborting promote"
        )));
    }
    let content =
        std::fs::read_to_string(&template_path).map_err(|e| CliError::Io(e.to_string()))?;
    let _ = std::fs::remove_file(&template_path);
    let body = strip_editor_comment_lines(&content);
    if body.trim().is_empty() {
        return Err(CliError::Config(
            "editor produced empty body; aborting promote".into(),
        ));
    }
    Ok(body)
}

fn strip_editor_comment_lines(content: &str) -> String {
    content
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Promote a dismissal from `local_only` to `promoted_convention` (T2).
/// File-then-SQLite ordering per spec §3.2 T2 + dispatch §2 (B5).
pub fn promote(
    quorum_dir: Option<&PathBuf>,
    hash_prefix: &str,
    body_source: BodySource,
) -> Result<Exit, CliError> {
    let root = resolve_quorum_root(quorum_dir)?;
    let store = open_store(&root)?;
    let row = resolve_short_hash(&store, hash_prefix)?;

    // 1. State guard: only local_only promotes (spec §3.2 T2 + AC 137).
    let hex = row.finding_identity_hash.to_hex();
    let short_hash = &hex[..12];
    match row.promotion_state {
        PromotionState::Candidate => {
            return Err(CliError::Config(format!(
                "{short_hash} is still a candidate (recurrence={n}); dismissed too few times to \
                 promote — wait for auto-promote after the next dismissal",
                n = row.recurrence_count
            )));
        }
        PromotionState::PromotedConvention => {
            return Err(CliError::Config(format!(
                "{short_hash} is already a promoted_convention; run \
                 `quorum convention demote {short_hash}` first to update its text"
            )));
        }
        PromotionState::LocalOnly => { /* fall through */ }
    }

    // 2. Body resolution (--text / --from-editor / title-only).
    let body = resolve_body(&body_source, &row.title_snapshot, &root)?;

    // 3. Pre-flight on conventions.md: parse, run orphan detection, surface
    //    diagnostics via stderr (AC 168 promote-side closure).
    let conv_path = root.join(".quorum").join("conventions.md");
    let existing_bytes = match std::fs::read(&conv_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(CliError::Io(format!("read {conv_path:?}: {e}"))),
    };
    let le = LineEnding::detect(&existing_bytes);
    let (parsed, diagnostics) = parse_conventions_md(&existing_bytes);
    for d in &diagnostics {
        eprintln!("warning: {}", format_diagnostic(d));
    }
    preflight_dirty_warning(&store, &parsed)?;

    // 4. Build the new block set. Idempotent re-promote: if a block for
    //    this hash already exists in the file (file-ahead-of-SQLite
    //    recovery), replace it; otherwise append.
    let title_for_block = &row.title_snapshot;
    let block_id: String = hex[..12].to_string();
    let mut to_write: Vec<BlockToWrite<'_>> = Vec::with_capacity(parsed.blocks.len() + 1);
    let mut replaced = false;
    for pb in &parsed.blocks {
        if pb.id == block_id {
            // Caller hash matches — replace with the fresh content.
            to_write.push(BlockToWrite {
                id: &block_id,
                version: 1,
                title: title_for_block,
                body: &body,
            });
            replaced = true;
        } else {
            // Preserve untouched blocks. If the on-disk layout is non-
            // canonical (user hand-edited), we fall back to title=header_line,
            // body=parsed.body — keeps the byte layout reasonable but the
            // exact-byte round-trip property only holds on canonical files.
            match BlockToWrite::from_parsed_block(pb) {
                Some(bt) => to_write.push(bt),
                None => {
                    eprintln!(
                        "warning: .quorum/conventions.md: block id={} has a non-canonical layout; \
                         re-rendering may reformat the block (untouched content otherwise preserved)",
                        pb.id
                    );
                    to_write.push(BlockToWrite {
                        id: pb.id,
                        version: pb.version,
                        title: pb.header_line.trim_start_matches("### Convention: "),
                        body: "",
                    });
                }
            }
        }
    }
    if !replaced {
        to_write.push(BlockToWrite {
            id: &block_id,
            version: 1,
            title: title_for_block,
            body: &body,
        });
    }

    // 5. Render bytes + atomic_write. File-first-rename, then SQLite.
    let new_bytes = render_conventions_md(&parsed, &to_write, le);
    std::fs::create_dir_all(root.join(".quorum")).map_err(|e| CliError::Io(e.to_string()))?;
    atomic_write(&conv_path, &new_bytes).map_err(|e| CliError::Io(e.to_string()))?;

    // 6. AC 175 test seam — panic harness fires between rename and COMMIT.
    // The seam is a no-op in production (the AtomicBool is always false
    // unless a test sets it). See `stage4_test_seam` for context.
    quorum_core::conventions::stage4_test_seam::maybe_panic_after_rename();

    // 7. SQLite-side commit.
    //    Title-only resolution: when `body` is empty (title-only block per
    //    §4.4), the `conventions.convention_text` CHECK demands ≥ 1 byte.
    //    Spec §4.4 is silent on what to store in this case; resolution per
    //    dispatch §2 halt-threshold-2 (obvious-from-context): store the
    //    title verbatim so the SQLite row carries the canonical
    //    convention text. Surfaced in the Stage 4 close report.
    let convention_text_for_db: &str = if body.is_empty() {
        title_for_block
    } else {
        body.as_str()
    };
    let ts_ms = current_unix_millis();
    let outcome = store
        .commit_promote(
            &row.finding_identity_hash,
            convention_text_for_db,
            &block_id,
            ts_ms,
        )
        .map_err(|e| CliError::Io(format!("commit_promote: {e}")))?;
    match outcome {
        PromoteOutcome::Committed => {
            println!(
                "promoted {short_hash} to convention: {title}",
                title = row.title_snapshot
            );
            eprintln!(
                "note: commit .quorum/conventions.md to move it from the memory section to the \
                 conventions section"
            );
            Ok(Exit::Ok)
        }
        PromoteOutcome::StateDrifted => Err(CliError::Config(format!(
            "{short_hash}: SQLite state was not local_only at COMMIT time (concurrent writer \
             or state drift); file write happened but DB did not advance. Re-run \
             `quorum convention promote {short_hash}` after `list --orphans` reconciliation."
        ))),
    }
}

/// Demote a dismissal from `promoted_convention` to `local_only` (T3).
pub fn demote(quorum_dir: Option<&PathBuf>, hash_prefix: &str) -> Result<Exit, CliError> {
    let root = resolve_quorum_root(quorum_dir)?;
    let store = open_store(&root)?;
    let row = resolve_short_hash(&store, hash_prefix)?;

    let hex = row.finding_identity_hash.to_hex();
    let short_hash = &hex[..12];
    if row.promotion_state != PromotionState::PromotedConvention {
        return Err(CliError::Config(format!(
            "{short_hash} is not in promoted_convention state (current: {}); demote requires \
             a previously-promoted row",
            row.promotion_state.as_db_str()
        )));
    }

    let conv_path = root.join(".quorum").join("conventions.md");
    let existing_bytes_opt = match std::fs::read(&conv_path) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(CliError::Io(format!("read {conv_path:?}: {e}"))),
    };

    let block_id: String = hex[..12].to_string();

    if let Some(existing_bytes) = existing_bytes_opt {
        // File present — parse, remove the matching block, atomic-write.
        let le = LineEnding::detect(&existing_bytes);
        let (parsed, diagnostics) = parse_conventions_md(&existing_bytes);
        for d in &diagnostics {
            eprintln!("warning: {}", format_diagnostic(d));
        }
        preflight_dirty_warning(&store, &parsed)?;

        let to_write: Vec<BlockToWrite<'_>> = parsed
            .blocks
            .iter()
            .filter(|pb| pb.id != block_id)
            .filter_map(BlockToWrite::from_parsed_block)
            .collect();
        let new_bytes = render_conventions_md(&parsed, &to_write, le);
        atomic_write(&conv_path, &new_bytes).map_err(|e| CliError::Io(e.to_string()))?;
    } else {
        // §10 Q7 lean: missing conventions.md → stderr warning + SQLite
        // state still proceeds. No file write to attempt.
        eprintln!(
            "warning: .quorum/conventions.md not found; SQLite state will advance but no file \
             write performed"
        );
    }

    let ts_ms = current_unix_millis();
    let outcome = store
        .commit_demote(&row.finding_identity_hash, ts_ms)
        .map_err(|e| CliError::Io(format!("commit_demote: {e}")))?;
    match outcome {
        DemoteOutcome::Committed => {
            println!("demoted {short_hash} (now local_only)");
            Ok(Exit::Ok)
        }
        DemoteOutcome::StateDrifted => Err(CliError::Config(format!(
            "{short_hash}: SQLite state was not promoted_convention at COMMIT time; \
             concurrent writer or state drift. File side has been updated; re-run \
             after `list --orphans` reconciliation."
        ))),
    }
}

/// Prune candidate dismissals older than the configured threshold (T4).
pub fn prune(quorum_dir: Option<&PathBuf>, dry_run: bool, yes: bool) -> Result<Exit, CliError> {
    let root = resolve_quorum_root(quorum_dir)?;
    let store = open_store(&root)?;

    // [memory] candidate_expire_days fallback (config absent → defaults).
    let candidate_expire_days = match quorum_core::config::read(&root) {
        Ok(cfg) => cfg.memory.candidate_expire_days,
        Err(quorum_core::config::ConfigError::NotFound(_)) => {
            quorum_core::config::MemoryConfig::default().candidate_expire_days
        }
        Err(e) => return Err(CliError::Config(format!("config: {e}"))),
    };
    if candidate_expire_days == 0 {
        eprintln!("prune disabled (candidate_expire_days=0); no rows pruned");
        return Ok(Exit::Ok);
    }
    let cutoff =
        time::OffsetDateTime::now_utc() - time::Duration::days(candidate_expire_days as i64);

    // Always preview candidates first (used by both --dry-run output and
    // the interactive confirmation prompt).
    let candidates = store
        .list_by_state(Some(PromotionState::Candidate))
        .map_err(|e| CliError::Io(format!("list_by_state: {e}")))?;
    let stale: Vec<&Dismissal> = candidates
        .iter()
        .filter(|d| d.last_seen_at < cutoff)
        .collect();

    if stale.is_empty() {
        println!(
            "no candidates older than {} days; nothing to prune",
            candidate_expire_days
        );
        return Ok(Exit::Ok);
    }

    if dry_run {
        println!(
            "would prune {} candidate(s) older than {} days:",
            stale.len(),
            candidate_expire_days
        );
        for d in &stale {
            let hex = d.finding_identity_hash.to_hex();
            println!(
                "  {}  {}",
                &hex[..12],
                truncate_for_display(&d.title_snapshot, TITLE_DISPLAY_WIDTH)
            );
        }
        return Ok(Exit::Ok);
    }

    if !yes && !confirm_prompt(stale.len(), candidate_expire_days)? {
        println!("aborted; no rows pruned");
        return Ok(Exit::Ok);
    }

    let pruned = store
        .prune_candidates(cutoff)
        .map_err(|e| CliError::Io(format!("prune_candidates: {e}")))?;
    println!("pruned {} candidate(s)", pruned);
    Ok(Exit::Ok)
}

fn confirm_prompt(count: usize, days: u32) -> Result<bool, CliError> {
    eprint!("Prune {count} candidate(s) older than {days} days? [y/N] ");
    std::io::stderr()
        .flush()
        .map_err(|e| CliError::Io(e.to_string()))?;
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| CliError::Io(e.to_string()))?;
    let ans = line.trim();
    Ok(ans.eq_ignore_ascii_case("y") || ans.eq_ignore_ascii_case("yes"))
}

/// AC 168 promote/demote pre-flight: surface in-file/SQLite divergence to
/// stderr. The Stage 4 dispatch §"WI-3 pre-flight" Q15 lean is simplified
/// per the dispatch's §2 latitude: any pre-flight discrepancy
/// (orphan blocks, missing rows) emits a warn-only stderr message.
/// Inside-vs-outside-fence detection is non-AC and deferred — surfaced in
/// the Stage 4 close report.
fn preflight_dirty_warning(
    store: &LocalSqliteMemoryStore,
    parsed: &ParsedConventionsMd<'_>,
) -> Result<(), CliError> {
    let db_rows = store
        .list_conventions()
        .map_err(|e| CliError::Io(format!("preflight list_conventions: {e}")))?;
    let file_block_ids: std::collections::HashSet<&str> =
        parsed.blocks.iter().map(|b| b.id).collect();
    let db_block_ids: std::collections::HashSet<&str> = db_rows
        .iter()
        .map(|r| r.conventions_md_block_id.as_str())
        .collect();
    for pb in &parsed.blocks {
        if !db_block_ids.contains(pb.id) {
            eprintln!(
                "warning: .quorum/conventions.md: orphan managed block id={} (no SQLite row); \
                 run `quorum convention list --orphans` for full report",
                pb.id
            );
        }
    }
    for r in &db_rows {
        if !file_block_ids.contains(r.conventions_md_block_id.as_str()) {
            eprintln!(
                "warning: SQLite row {} has no managed block in .quorum/conventions.md",
                r.conventions_md_block_id
            );
        }
    }
    Ok(())
}

fn current_unix_millis() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}
