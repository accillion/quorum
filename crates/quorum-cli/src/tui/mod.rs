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
use quorum_core::memory::{MemoryError, MemoryStore};
use quorum_core::review::Review;

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
}
