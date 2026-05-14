//! TUI main loop (Phase 1B Stage 2).
//!
//! Decomposition:
//!   - [`state`] — pure `AppState`, key handler returning [`Command`]s,
//!     all input-validation rules. Unit-tested.
//!   - [`panels`] — ratatui rendering functions. Unit-tested for the
//!     handful of text-shape invariants.
//!   - [`dismiss_prompt`] — placeholder; the modal logic lives entirely
//!     in [`state`] for testability. Kept as a doc anchor.
//!   - this file — owns the terminal lifecycle (raw mode + alt screen),
//!     the [`MemoryStore`] handle, and the event/render loop.
//!
//! Terminal restoration is the critical correctness invariant
//! (v1.0 §4.3.5, AC 117). [`TuiSession`]'s `Drop` impl runs
//! `disable_raw_mode()` + `LeaveAlternateScreen` on every exit path. We
//! also install a panic hook at setup that calls the same restore
//! before unwinding, so a panic anywhere inside the loop still surfaces
//! a cooked terminal to the user. The hook is restored on Drop so test
//! harnesses aren't left with a poisoned global.

pub mod dismiss_prompt;
pub mod panels;
pub mod state;

use std::io::{self, Stdout, Write};
use std::path::Path;
use std::time::Duration;

use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::sync::Mutex;

use quorum_core::archive::SuppressionSummary;
use quorum_core::conventions::{
    atomic_write, parse_conventions_md, render_conventions_md, BlockToWrite, LineEnding,
};
use quorum_core::memory::{
    Dismissal, FindingIdentityHash, MemoryError, MemoryStore, PromoteOutcome, PromotionState,
    ShortHashResolution,
};
use quorum_core::review::Review;

#[allow(unused_imports)] // re-exported for future TUI extensions / integration tests
pub use state::View;
pub use state::{AppState, Command, Modal};

#[derive(thiserror::Error, Debug)]
pub enum TuiError {
    #[error("terminal io: {0}")]
    Io(#[from] io::Error),
    #[error("memory store: {0}")]
    Memory(#[from] MemoryError),
}

/// Outcome of a TUI session. The caller uses `newly_suppressed` to
/// merge with the filter-site `suppressed_findings` for the archive,
/// and `kept_findings` becomes the post-TUI `review.findings`.
pub struct TuiOutcome {
    pub kept_findings: Vec<quorum_core::review::Finding>,
    pub newly_suppressed: Vec<SuppressionSummary>,
}

/// Run the TUI against a pre-filtered review (the filter site dropped
/// already-active dismissals; this loop only handles fresh dismissals
/// the user makes during the session).
///
/// The store is the same handle the filter site used. `--no-expire` is
/// captured in `AppState` so the modal commit forwards the right
/// `expires_in` to `MemoryStore::dismiss`.
pub fn run(
    review: &Review,
    store: &dyn MemoryStore,
    repo_head_sha: &str,
    branch: &str,
    no_expire: bool,
    repo_root: &Path,
) -> Result<TuiOutcome, TuiError> {
    let mut state = AppState::new(review.findings.clone(), no_expire);
    let session_id = review.session_id.clone();
    let db_path_str = db_path_label(store);
    let mut session = TuiSession::enter()?;

    let mut newly_suppressed: Vec<SuppressionSummary> = Vec::new();

    loop {
        session.terminal.draw(|f| {
            panels::render(f, &state, &session_id, &db_path_str);
        })?;

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let ev = event::read()?;
        let key = match ev {
            Event::Key(k) => k,
            _ => continue,
        };

        match state.on_key(key) {
            Command::None => continue,
            Command::Quit => break,
            Command::Dismiss {
                finding_index,
                reason,
                note,
            } => {
                let f = match state.findings.get(finding_index).cloned() {
                    Some(f) => f,
                    None => continue,
                };
                let expires = if state.no_expire {
                    None
                } else {
                    Some(time::Duration::days(365))
                };
                match store.dismiss(&f, repo_head_sha, branch, reason, note.clone(), expires) {
                    Ok(id) => {
                        let summary = SuppressionSummary {
                            finding_identity_hash: quorum_core::memory::finding_identity_hash(&f)
                                .to_hex(),
                            title_snapshot: f.title.clone(),
                            source_type_snapshot: source_kind(f.source).to_string(),
                            reason: reason.as_db_str().to_string(),
                            dismissed_at: time::OffsetDateTime::now_utc()
                                .format(&time::format_description::well_known::Rfc3339)
                                .unwrap_or_default(),
                        };
                        newly_suppressed.push(summary);
                        state.apply_committed_dismissal(finding_index, id);
                    }
                    Err(MemoryError::AlreadyDismissed) => {
                        state.apply_already_dismissed(finding_index);
                    }
                    Err(MemoryError::OtherWithoutNote) | Err(MemoryError::InvalidNote) => {
                        state.status_message = Some(
                            "invalid note rejected by storage; press any key to continue".into(),
                        );
                        state.modal = Modal::Error;
                    }
                    Err(e) => {
                        state.status_message = Some(format!("dismiss failed: {e}"));
                        state.modal = Modal::Error;
                    }
                }
            }
            Command::Undo => {
                if let Some(entry) = state.undo_stack.pop() {
                    let id = entry.dismissal_id;
                    match store.delete(id) {
                        Ok(_) => {
                            // Remove the corresponding entry from
                            // newly_suppressed so the archive reflects
                            // the post-TUI state.
                            let hash =
                                quorum_core::memory::finding_identity_hash(&entry.finding).to_hex();
                            newly_suppressed.retain(|s| s.finding_identity_hash != hash);
                            state.apply_undo(entry);
                        }
                        Err(e) => {
                            state.undo_stack.push(entry);
                            state.status_message = Some(format!("undo failed: {e}"));
                            state.modal = Modal::Error;
                        }
                    }
                }
            }
            Command::OpenHistory => match store.list_all() {
                Ok(rows) => {
                    let follow_up = state.apply_history_loaded(rows);
                    if let Command::LoadTransitions(hash) = follow_up {
                        match store.load_transitions(&hash) {
                            Ok(ts) => state.apply_transitions_loaded(ts),
                            Err(e) => {
                                state.apply_write_failed(format!("load_transitions: {e}"));
                            }
                        }
                    }
                }
                Err(e) => {
                    state.status_message = Some(format!("list_all failed: {e}"));
                    state.modal = Modal::Error;
                }
            },
            Command::LoadTransitions(hash) => match store.load_transitions(&hash) {
                Ok(ts) => state.apply_transitions_loaded(ts),
                Err(e) => {
                    state.apply_write_failed(format!("load_transitions: {e}"));
                }
            },
            Command::Promote { hash, body } => match tui_promote(store, repo_root, &hash, &body) {
                Ok(title) => {
                    let hex = hash.to_hex();
                    let short = &hex[..12];
                    state.apply_promote_committed(hash, short, &title);
                }
                Err(msg) => state.apply_write_failed(msg),
            },
            Command::Demote { hash } => match tui_demote(store, repo_root, &hash) {
                Ok(()) => {
                    let hex = hash.to_hex();
                    let short = &hex[..12];
                    state.apply_demote_committed(hash, short);
                }
                Err(msg) => state.apply_write_failed(msg),
            },
        }
    }

    // Final render so the user sees the "M dismissed" delta if anything
    // happened on the last tick. Then the guard's Drop restores the
    // terminal.
    session.terminal.draw(|f| {
        panels::render(f, &state, &session_id, &db_path_str);
    })?;

    let kept_findings = state.findings;
    Ok(TuiOutcome {
        kept_findings,
        newly_suppressed,
    })
}

fn source_kind(s: quorum_core::review::FindingSource) -> &'static str {
    match s {
        quorum_core::review::FindingSource::Divergence => "divergence",
        quorum_core::review::FindingSource::Agreement => "agreement",
        quorum_core::review::FindingSource::Assumption => "assumption",
    }
}

/// Label shown in the help overlay (spec §4.3.4 footer). `MemoryStore`
/// is trait-typed in the loop, so we can't cast back to
/// `LocalSqliteMemoryStore::path()` here. Status display only — the
/// actual store handle is what's load-bearing.
fn db_path_label(_store: &dyn MemoryStore) -> String {
    ".quorum/dismissals.sqlite".to_string()
}

/// Phase 1C Stage 5 — TUI-side T2 promote orchestrator (spec §3.2 T2,
/// AC 158). Path A choice: the TUI does the file-then-SQLite
/// orchestration directly using public `quorum_core::conventions`
/// helpers rather than calling `commands::convention::promote`. This
/// avoids stderr emission inside the alt-screen buffer; the CLI
/// orchestrator's pre-flight diagnostics (AC 168) are skipped here as
/// non-AC TUI surface (documented in the Stage 5 close report).
///
/// Returns the row's title on success (for status-bar feedback) or a
/// short error message on failure.
fn tui_promote(
    store: &dyn MemoryStore,
    repo_root: &Path,
    hash: &FindingIdentityHash,
    body: &str,
) -> Result<String, String> {
    // 1. Resolve the row using the public short-hash API (the full
    //    64-hex always resolves as Exact or NotFound — the trait
    //    documents this contract).
    let dismissal = resolve_full_hash(store, hash)?;
    if dismissal.promotion_state != PromotionState::LocalOnly {
        return Err(format!(
            "promote: row {} is now {} (snapshot stale; quit and re-invoke to refresh)",
            &hash.to_hex()[..12],
            dismissal.promotion_state.as_db_str()
        ));
    }

    let hex = dismissal.finding_identity_hash.to_hex();
    let block_id: String = hex[..12].to_string();
    let title_for_block = &dismissal.title_snapshot;

    // 2. Read + parse conventions.md (diagnostics silently dropped —
    //    AC 168 stderr is a CLI surface, not a TUI surface).
    let conv_path = repo_root.join(".quorum").join("conventions.md");
    let existing_bytes = match std::fs::read(&conv_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("read {conv_path:?}: {e}")),
    };
    let le = LineEnding::detect(&existing_bytes);
    let (parsed, _diagnostics) = parse_conventions_md(&existing_bytes);

    // 3. Build the new block set (idempotent re-promote: replace if
    //    block_id already present, else append).
    let mut to_write: Vec<BlockToWrite<'_>> = Vec::with_capacity(parsed.blocks.len() + 1);
    let mut replaced = false;
    for pb in &parsed.blocks {
        if pb.id == block_id {
            to_write.push(BlockToWrite {
                id: &block_id,
                version: 1,
                title: title_for_block,
                body,
            });
            replaced = true;
        } else if let Some(bt) = BlockToWrite::from_parsed_block(pb) {
            to_write.push(bt);
        }
        // Non-canonical blocks: skip silently (CLI logs a warning to
        // stderr but the TUI status bar can't carry per-block notes
        // cleanly. Mirrors AC 168 simplification.)
    }
    if !replaced {
        to_write.push(BlockToWrite {
            id: &block_id,
            version: 1,
            title: title_for_block,
            body,
        });
    }

    // 4. Render + atomic_write (file-first per spec §3.2 T2).
    let new_bytes = render_conventions_md(&parsed, &to_write, le);
    std::fs::create_dir_all(repo_root.join(".quorum"))
        .map_err(|e| format!("create .quorum/: {e}"))?;
    atomic_write(&conv_path, &new_bytes).map_err(|e| format!("atomic_write: {e}"))?;

    // 5. AC 175 crash seam — symmetric with the CLI promote path. No-op
    //    in production (zero-cost AtomicBool probe).
    quorum_core::conventions::stage4_test_seam::maybe_panic_after_rename();

    // 6. SQLite commit. Title-only resolution (body == "") matches the
    //    CLI promote: store the title verbatim to satisfy the
    //    `conventions.convention_text` ≥1-byte CHECK constraint.
    let convention_text_for_db: &str = if body.is_empty() {
        title_for_block
    } else {
        body
    };
    let ts_ms = current_unix_millis();
    let outcome = store
        .commit_promote(hash, convention_text_for_db, &block_id, ts_ms)
        .map_err(|e| format!("commit_promote: {e}"))?;
    match outcome {
        PromoteOutcome::Committed => Ok(dismissal.title_snapshot.clone()),
        PromoteOutcome::StateDrifted => Err(
            "promote: SQLite state was not local_only at COMMIT time (drift); file write \
             succeeded but DB did not advance. Run `quorum convention list --orphans` to \
             reconcile."
                .to_string(),
        ),
    }
}

/// Phase 1C Stage 5 — TUI-side T3 demote orchestrator (spec §3.2 T3,
/// AC 159).
fn tui_demote(
    store: &dyn MemoryStore,
    repo_root: &Path,
    hash: &FindingIdentityHash,
) -> Result<(), String> {
    let dismissal = resolve_full_hash(store, hash)?;
    if dismissal.promotion_state != PromotionState::PromotedConvention {
        return Err(format!(
            "demote: row {} is now {} (snapshot stale)",
            &hash.to_hex()[..12],
            dismissal.promotion_state.as_db_str()
        ));
    }

    let hex = dismissal.finding_identity_hash.to_hex();
    let block_id: String = hex[..12].to_string();
    let conv_path = repo_root.join(".quorum").join("conventions.md");

    // Remove the managed block from conventions.md if the file is
    // present; mirrors the CLI's §10 Q7 lean (missing file → no file
    // write, DB still advances).
    match std::fs::read(&conv_path) {
        Ok(existing_bytes) => {
            let le = LineEnding::detect(&existing_bytes);
            let (parsed, _diagnostics) = parse_conventions_md(&existing_bytes);
            let to_write: Vec<BlockToWrite<'_>> = parsed
                .blocks
                .iter()
                .filter(|pb| pb.id != block_id)
                .filter_map(BlockToWrite::from_parsed_block)
                .collect();
            let new_bytes = render_conventions_md(&parsed, &to_write, le);
            atomic_write(&conv_path, &new_bytes).map_err(|e| format!("atomic_write: {e}"))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Q7 lean: file missing → skip file write, advance DB only.
        }
        Err(e) => return Err(format!("read {conv_path:?}: {e}")),
    }

    let ts_ms = current_unix_millis();
    let outcome = store
        .commit_demote(hash, ts_ms)
        .map_err(|e| format!("commit_demote: {e}"))?;
    match outcome {
        quorum_core::memory::DemoteOutcome::Committed => Ok(()),
        quorum_core::memory::DemoteOutcome::StateDrifted => Err(
            "demote: SQLite state was not promoted_convention at COMMIT time (drift); file \
             write succeeded but DB did not advance."
                .to_string(),
        ),
    }
}

/// Current unix epoch milliseconds. Mirrors the CLI helper of the same
/// shape; duplicated here so the TUI doesn't reach into `commands::`
/// internals.
fn current_unix_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Resolve a full `FindingIdentityHash` to the underlying row through
/// the public `find_by_short_hash` trait method. The 64-hex form is
/// documented as always resolving to `Exact` or `NotFound` (never
/// `Ambiguous`), so the Ambiguous arm is treated as a backend invariant
/// violation rather than a user-facing message.
fn resolve_full_hash(
    store: &dyn MemoryStore,
    hash: &FindingIdentityHash,
) -> Result<Dismissal, String> {
    let hex = hash.to_hex();
    match store.find_by_short_hash(&hex) {
        Ok(ShortHashResolution::Exact(d)) => Ok(*d),
        Ok(ShortHashResolution::NotFound) => {
            Err(format!("row {} not found (snapshot stale?)", &hex[..12]))
        }
        Ok(ShortHashResolution::Ambiguous(_)) => Err(format!(
            "row {} resolved ambiguously on full 64-hex — backend invariant violated",
            &hex[..12]
        )),
        Err(e) => Err(format!("find_by_short_hash: {e}")),
    }
}

/// RAII guard for the alt-screen / raw-mode pair. Restoration runs on
/// every exit path, including panic unwind (the global panic hook
/// installed during setup also restores; we store the previous hook so
/// Drop can put it back).
pub struct TuiSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    raw_was_enabled: bool,
}

impl TuiSession {
    pub fn enter() -> Result<Self, TuiError> {
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        enable_raw_mode()?;
        install_panic_hook();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            raw_was_enabled: true,
        })
    }
}

impl Drop for TuiSession {
    fn drop(&mut self) {
        // Best-effort: do not panic in Drop. Each step is conditional
        // on the previous so partial setup still gets fully torn down.
        if self.raw_was_enabled {
            let _ = disable_raw_mode();
        }
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = io::stdout().flush();
        restore_panic_hook();
    }
}

/// Global slot holding the prior panic hook so we can restore it when
/// the TuiSession drops. `Mutex` because Rust's panic-hook API is
/// thread-global and we may be entered from any thread (in practice the
/// CLI is single-threaded, but the guard is cheap).
type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync>;
static PRIOR_HOOK: Mutex<Option<PanicHook>> = Mutex::new(None);

fn install_panic_hook() {
    let mut slot = PRIOR_HOOK.lock().unwrap();
    if slot.is_some() {
        return; // already installed by an outer TuiSession
    }
    let prev = std::panic::take_hook();
    *slot = Some(prev);
    drop(slot);
    std::panic::set_hook(Box::new(|info| {
        // Restore cooked mode + main screen before letting the prior
        // hook print the backtrace. If anything here fails we still
        // proceed — losing terminal state on a panic is better than
        // suppressing the panic message entirely.
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = io::stdout().flush();
        let guard = PRIOR_HOOK.lock().unwrap();
        if let Some(prev) = guard.as_ref() {
            prev(info);
        }
    }));
}

fn restore_panic_hook() {
    let mut slot = PRIOR_HOOK.lock().unwrap();
    if let Some(prev) = slot.take() {
        std::panic::set_hook(prev);
    }
}

#[cfg(test)]
mod tests {
    //! Loop-level smoke tests live in the integration suite
    //! `crates/quorum-cli/tests/tui_smoke.rs` — they need a
    //! `TestBackend` plus a real `LocalSqliteMemoryStore`, which is
    //! integration-tier surface. State and rendering unit tests live
    //! in `state.rs` and `panels.rs`.
    //!
    //! Phase 1C Stage 5 — the `tui_promote` / `tui_demote` helpers are
    //! private to this module (they live alongside the loop because they
    //! orchestrate file-then-SQLite writes from inside the TUI). State
    //! tests can't reach them; integration tests in `tests/` can't reach
    //! binary-internal items. So we test the helpers directly here
    //! against a real `LocalSqliteMemoryStore` rooted in a `TempDir`.

    use super::*;
    use quorum_core::memory::identity::finding_identity_hash;
    use quorum_core::memory::{
        DismissalReason, FindingIdentityHash, LocalSqliteMemoryStore, PromotionState,
    };
    use quorum_core::review::{Finding, FindingSource, Severity};
    use tempfile::TempDir;

    fn finding(title: &str) -> Finding {
        Finding {
            severity: Severity::High,
            title: title.to_string(),
            body: format!("body for {title}"),
            source: FindingSource::Divergence,
            supported_by: vec!["m".into()],
            confidence: Some(0.9),
        }
    }

    /// Seed a dismissal and (optionally) force its promotion_state via
    /// direct SQL — same fixture pattern as `convention_write.rs`.
    fn seed(
        store: &LocalSqliteMemoryStore,
        title: &str,
        state: PromotionState,
    ) -> FindingIdentityHash {
        let f = finding(title);
        store
            .dismiss(
                &f,
                "head-sha",
                "main",
                DismissalReason::FalsePositive,
                None,
                None,
            )
            .unwrap();
        let h = finding_identity_hash(&f);
        if state != PromotionState::Candidate {
            let conn = rusqlite::Connection::open(store.path()).unwrap();
            conn.execute(
                "UPDATE dismissals SET promotion_state = ?1 WHERE finding_identity_hash = ?2",
                rusqlite::params![state.as_db_str(), h.to_hex()],
            )
            .unwrap();
        }
        h
    }

    fn store_in(td: &TempDir) -> LocalSqliteMemoryStore {
        // The store creates `.quorum/dismissals.sqlite` under `root/`.
        LocalSqliteMemoryStore::new(td.path()).unwrap()
    }

    #[test]
    fn tui_promote_writes_file_and_flips_state() {
        // AC 158 — full T2 round-trip via the TUI orchestrator.
        let td = TempDir::new().unwrap();
        let store = store_in(&td);
        let hash = seed(
            &store,
            "no blocking on stylistic ABC",
            PromotionState::LocalOnly,
        );

        let title = tui_promote(&store, td.path(), &hash, "Reject ABC stylistic findings.")
            .expect("promote succeeds");
        assert_eq!(title, "no blocking on stylistic ABC");

        // SQLite advanced.
        let resolved = store.find_by_short_hash(&hash.to_hex()).expect("find ok");
        match resolved {
            ShortHashResolution::Exact(d) => {
                assert_eq!(d.promotion_state, PromotionState::PromotedConvention);
            }
            other => panic!("expected Exact, got {other:?}"),
        }

        // File side: managed block written.
        let conv = std::fs::read_to_string(td.path().join(".quorum").join("conventions.md"))
            .expect("conventions.md exists");
        let short = &hash.to_hex()[..12];
        assert!(
            conv.contains(short),
            "conventions.md should reference block id {short}; got:\n{conv}"
        );
        assert!(conv.contains("Reject ABC stylistic findings."));
    }

    #[test]
    fn tui_promote_empty_body_falls_back_to_title_for_db() {
        // Mirrors the CLI title-only resolution (Stage 4 close report
        // note). `convention_text` must be ≥1 byte; passing empty body
        // through the TUI stores the title verbatim.
        let td = TempDir::new().unwrap();
        let store = store_in(&td);
        let hash = seed(&store, "title only convention", PromotionState::LocalOnly);

        tui_promote(&store, td.path(), &hash, "").expect("promote succeeds");

        let conn = rusqlite::Connection::open(store.path()).unwrap();
        let text: String = conn
            .query_row(
                "SELECT convention_text FROM conventions WHERE finding_identity_hash = ?1",
                rusqlite::params![hash.to_hex()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(text, "title only convention");
    }

    #[test]
    fn tui_promote_rejects_non_local_only_state() {
        let td = TempDir::new().unwrap();
        let store = store_in(&td);
        let hash = seed(&store, "still a candidate", PromotionState::Candidate);

        let err = tui_promote(&store, td.path(), &hash, "body").expect_err("should reject");
        assert!(err.contains("candidate"), "unexpected error: {err}");
    }

    #[test]
    fn tui_demote_removes_block_and_flips_state() {
        // AC 159 — full T3 round-trip via the TUI orchestrator.
        // Setup: promote first (so conventions.md has the block), then
        // demote and verify the block is gone + SQLite is back to
        // local_only.
        let td = TempDir::new().unwrap();
        let store = store_in(&td);
        let hash = seed(&store, "demote me", PromotionState::LocalOnly);
        tui_promote(&store, td.path(), &hash, "body to be removed").unwrap();

        let conv_before =
            std::fs::read_to_string(td.path().join(".quorum").join("conventions.md")).unwrap();
        let short = &hash.to_hex()[..12];
        assert!(conv_before.contains(short));

        tui_demote(&store, td.path(), &hash).expect("demote succeeds");

        let conv_after =
            std::fs::read_to_string(td.path().join(".quorum").join("conventions.md")).unwrap();
        assert!(
            !conv_after.contains(short),
            "demote should remove the managed block; remaining file:\n{conv_after}"
        );

        let resolved = store.find_by_short_hash(&hash.to_hex()).unwrap();
        match resolved {
            ShortHashResolution::Exact(d) => {
                assert_eq!(d.promotion_state, PromotionState::LocalOnly);
            }
            other => panic!("expected Exact, got {other:?}"),
        }
    }

    #[test]
    fn tui_demote_missing_file_still_advances_db() {
        // §10 Q7 lean — file missing → SQLite still advances. Mirrors
        // the CLI demote path.
        let td = TempDir::new().unwrap();
        let store = store_in(&td);
        let hash = seed(&store, "no file demote", PromotionState::PromotedConvention);
        // Ensure conventions.md is absent (the seed didn't write it).
        let conv = td.path().join(".quorum").join("conventions.md");
        assert!(!conv.exists());

        tui_demote(&store, td.path(), &hash).expect("demote succeeds even without file");

        let resolved = store.find_by_short_hash(&hash.to_hex()).unwrap();
        match resolved {
            ShortHashResolution::Exact(d) => {
                assert_eq!(d.promotion_state, PromotionState::LocalOnly);
            }
            other => panic!("expected Exact, got {other:?}"),
        }
    }

    #[test]
    fn tui_demote_rejects_non_promoted_state() {
        let td = TempDir::new().unwrap();
        let store = store_in(&td);
        let hash = seed(&store, "still local_only", PromotionState::LocalOnly);
        let err = tui_demote(&store, td.path(), &hash).expect_err("should reject");
        assert!(err.contains("local_only"), "unexpected error: {err}");
    }

    #[test]
    fn ac_161_panic_hook_chain_no_new_set_hook_in_stage_5_code() {
        // AC 161 — Phase 1B's panic-hook chain is the canonical install
        // site. Stage 5 code must not call std::panic::set_hook or
        // take_hook anywhere. This test guards against regression by
        // asserting that, with the global hook in its default state,
        // running tui_promote does NOT swap it out.
        let original = std::panic::take_hook();
        // Install a sentinel hook keyed on a global flag; assert it
        // remains installed after a successful promote (which is the
        // most code-path coverage Stage 5 has on its own write side).
        use std::sync::atomic::{AtomicBool, Ordering};
        static SENTINEL_FIRED: AtomicBool = AtomicBool::new(false);
        std::panic::set_hook(Box::new(|_info| {
            SENTINEL_FIRED.store(true, Ordering::SeqCst);
        }));

        let td = TempDir::new().unwrap();
        let store = store_in(&td);
        let hash = seed(&store, "ac 161 row", PromotionState::LocalOnly);
        let _ = tui_promote(&store, td.path(), &hash, "body").unwrap();

        // Trigger a panic so the currently-installed hook fires; if
        // tui_promote had swapped the hook, our sentinel wouldn't fire.
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            panic!("post-promote sentinel probe");
        }));
        assert!(r.is_err());
        assert!(
            SENTINEL_FIRED.load(Ordering::SeqCst),
            "tui_promote must not call std::panic::set_hook — Phase 1B's chain is canonical"
        );

        std::panic::set_hook(original);
    }
}
