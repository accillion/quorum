//! `quorum convention` — Phase 1C Stage 3 read surface.
//!
//! Stage 3 ships `list`, `show`, `history`. The write subcommands
//! (`promote`, `demote`, `prune`) land in Stage 4 and are intentionally
//! absent from the clap surface here — they're cleaner absent than stubbed.

use std::path::PathBuf;

use quorum_core::conventions::{detect_orphans, OrphanReport};
use quorum_core::memory::{
    Dismissal, LocalSqliteMemoryStore, MemoryStore, PromotionState, ShortHashResolution,
    StateTransitionRow, TransitionTrigger,
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
    println!(
        "{:<12}  {:<20}  {:>5}  {}",
        "HASH", "STATE", "RECUR", "TITLE"
    );
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
    let mut n = 0;
    for c in s.chars() {
        if n >= max_chars {
            out.push('…');
            return out;
        }
        out.push(c);
        n += 1;
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

fn format_diagnostic(d: &quorum_core::conventions::ConventionParseError) -> String {
    use quorum_core::conventions::ConventionParseError::*;
    match d {
        UnclosedBlock { id, start_byte } => format!(
            ".quorum/conventions.md: block id={id} at byte {start_byte} has no closing marker"
        ),
        BadBlockId { raw, start_byte } => format!(
            ".quorum/conventions.md: bad block id {raw:?} at byte {start_byte}"
        ),
        BadBlockVersion { raw, start_byte } => format!(
            ".quorum/conventions.md: bad block version {raw:?} at byte {start_byte}"
        ),
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
        ShortHashResolution::Exact(d) => Ok(d),
        ShortHashResolution::NotFound => Err(CliError::Config(format!(
            "no dismissal matches '{prefix}'"
        ))),
        ShortHashResolution::Ambiguous(matches) => {
            let mut msg = format!(
                "short-hash '{prefix}' is ambiguous; matches:\n"
            );
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
    emit_history(&transitions);
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
    emit_history(transitions);
}

fn emit_history(transitions: &[StateTransitionRow]) {
    if transitions.is_empty() {
        println!("(no transition history — dismissal predates schema v2)");
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
    let dt = time::OffsetDateTime::from_unix_timestamp_nanos((secs as i128) * 1_000_000_000 + nanos)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    dt.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| format!("{ms}ms"))
}
