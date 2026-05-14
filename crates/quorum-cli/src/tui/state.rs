//! Pure TUI state + event handler. No I/O, no terminal — everything in
//! this module is exercised by `tui_smoke.rs` against `TestBackend`.
//!
//! The loop in [`super`] is responsible for raw mode, alt-screen, panic
//! hook, and the actual `dismiss()` calls into [`MemoryStore`]. State
//! reflects what the next render needs; commands signal what the loop
//! should execute on this tick.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use quorum_core::memory::{
    Dismissal, DismissalId, DismissalReason, FindingIdentityHash, PromotionState,
    StateTransitionRow,
};
use quorum_core::review::Finding;

/// What the loop should execute after the latest key event. The loop owns
/// the [`MemoryStore`] handle and the in-session undo stack; the state
/// just *describes* what to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    None,
    /// User wants to quit (q / Esc from main list, never from a modal).
    Quit,
    /// Commit a dismissal of the currently selected finding.
    Dismiss {
        finding_index: usize,
        reason: DismissalReason,
        note: Option<String>,
    },
    /// Pop the in-session undo stack and call `MemoryStore::delete`.
    Undo,
    /// Phase 1C Stage 5 — load the dismissal-history snapshot from the
    /// store (`list_all` + `load_transitions` for the initial cursor row).
    /// The loop populates state via [`AppState::apply_history_loaded`].
    OpenHistory,
    /// Phase 1C Stage 5 — refresh the transition-log strip for the row at
    /// `history_selected`. Fires whenever the history cursor moves.
    LoadTransitions(FindingIdentityHash),
    /// Phase 1C Stage 5 — execute T2 from the TUI promote modal. The loop
    /// performs the file-then-SQLite orchestration (parse conventions.md,
    /// atomic_write, `MemoryStore::commit_promote`) and calls
    /// [`AppState::apply_promote_committed`] or [`AppState::apply_write_failed`].
    Promote {
        hash: FindingIdentityHash,
        body: String,
    },
    /// Phase 1C Stage 5 — execute T3 from the TUI demote-confirm modal.
    /// The loop removes the managed block from conventions.md and calls
    /// `MemoryStore::commit_demote`.
    Demote {
        hash: FindingIdentityHash,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modal {
    None,
    DismissReason,
    DismissNote, // free-text input for `o` (other)
    Help,
    /// Briefly shown when something illegal was tried (esc dismisses).
    Error,
    /// Phase 1C Stage 5 — single-line text input for the promote body.
    /// Active only inside [`View::History`] on `local_only` rows.
    PromoteText,
    /// Phase 1C Stage 5 — `Y/N` confirmation for demote. Active only
    /// inside [`View::History`] on `promoted_convention` rows.
    DemoteConfirm,
}

/// Phase 1C Stage 5 — top-level view selector. The Phase 1B per-review
/// finding list is [`View::Main`]; the new dismissal-history table is
/// [`View::History`]. Spec §5.2 / AC 157: `H` toggles, `Esc` from history
/// returns to main, `q` from any view quits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Main,
    History,
}

/// In-session undo entry — the dismissal id (so the loop can call
/// `delete`) plus enough context to surface the finding back to the user
/// when undone.
#[derive(Debug, Clone)]
pub struct UndoEntry {
    pub dismissal_id: DismissalId,
    /// The finding's index in the original (pre-dismiss) list, so the
    /// loop can re-insert it at the same position.
    pub original_index: usize,
    pub finding: Finding,
}

pub struct AppState {
    pub findings: Vec<Finding>,
    pub selected: usize,
    pub body_scroll: u16,
    pub modal: Modal,
    pub note_buf: String,
    /// The selected finding's index at the moment the dismiss modal
    /// opened. The user can't change selection while the modal is up,
    /// but we capture this explicitly so the [`Command::Dismiss`] event
    /// is unambiguous when the loop fires.
    pub modal_target_index: Option<usize>,
    pub help_visible: bool,
    pub status_message: Option<String>,
    pub undo_stack: Vec<UndoEntry>,
    /// Filled in by the loop when the cli was invoked with
    /// `--no-expire`. Forwarded to `MemoryStore::dismiss`.
    pub no_expire: bool,
    /// Counter of dismissals committed during this TUI session; used
    /// only for the in-loop "M dismissed" status display.
    pub session_dismissed_count: u32,
    /// Phase 1C Stage 5 — current top-level view ([`View::Main`] for the
    /// Phase 1B finding list, [`View::History`] for the dismissal-history
    /// snapshot).
    pub view: View,
    /// Phase 1C Stage 5 — snapshot of `dismissals` rows for the history
    /// view. Populated once via [`AppState::apply_history_loaded`] when
    /// `H` is first pressed; never live-refreshed mid-session (spec §2
    /// non-goal: "No TUI re-fetch.").
    pub history_rows: Vec<Dismissal>,
    pub history_selected: usize,
    pub history_body_scroll: u16,
    /// Phase 1C Stage 5 — transition log for the currently-selected
    /// history row, oldest-first. Refreshed by [`Command::LoadTransitions`]
    /// whenever the cursor moves.
    pub history_transitions: Vec<StateTransitionRow>,
    /// Phase 1C Stage 5 — single-line edit buffer for the promote-text
    /// modal. Pre-seeded with the row's title on open; user can clear and
    /// type a body, or accept Enter for title-only (spec §5.2).
    pub promote_text_buf: String,
}

impl AppState {
    pub fn new(findings: Vec<Finding>, no_expire: bool) -> Self {
        Self {
            findings,
            selected: 0,
            body_scroll: 0,
            modal: Modal::None,
            note_buf: String::new(),
            modal_target_index: None,
            help_visible: false,
            status_message: None,
            undo_stack: Vec::new(),
            no_expire,
            session_dismissed_count: 0,
            view: View::Main,
            history_rows: Vec::new(),
            history_selected: 0,
            history_body_scroll: 0,
            history_transitions: Vec::new(),
            promote_text_buf: String::new(),
        }
    }

    pub fn selected_finding(&self) -> Option<&Finding> {
        self.findings.get(self.selected)
    }

    /// Dispatch a key event. Returns a [`Command`] for the loop to
    /// honor on the next tick. Pure: no terminal, no memory store —
    /// just state.
    pub fn on_key(&mut self, key: KeyEvent) -> Command {
        // Ignore key-release events (Windows emits both Press and
        // Release; we want one-shot semantics).
        if key.kind == KeyEventKind::Release {
            return Command::None;
        }
        match self.modal {
            Modal::None => match self.view {
                View::Main => self.on_key_list(key),
                View::History => self.on_key_history(key),
            },
            Modal::Help => self.on_key_help(key),
            Modal::DismissReason => self.on_key_dismiss_reason(key),
            Modal::DismissNote => self.on_key_dismiss_note(key),
            Modal::Error => self.on_key_error(key),
            Modal::PromoteText => self.on_key_promote_text(key),
            Modal::DemoteConfirm => self.on_key_demote_confirm(key),
        }
    }

    fn on_key_list(&mut self, key: KeyEvent) -> Command {
        // Quit branch first.
        if matches!(key.code, KeyCode::Char('q')) || matches!(key.code, KeyCode::Esc) {
            return Command::Quit;
        }
        // Ctrl+C also requests exit so the restoration path runs (the
        // raw-mode terminal otherwise wouldn't forward SIGINT).
        if matches!(key.code, KeyCode::Char('c')) && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Command::Quit;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down
                if !self.findings.is_empty() && self.selected + 1 < self.findings.len() =>
            {
                self.selected += 1;
                self.body_scroll = 0;
            }
            KeyCode::Char('k') | KeyCode::Up if self.selected > 0 => {
                self.selected -= 1;
                self.body_scroll = 0;
            }
            KeyCode::Char('g') => {
                self.selected = 0;
                self.body_scroll = 0;
            }
            KeyCode::Char('G') if !self.findings.is_empty() => {
                self.selected = self.findings.len() - 1;
                self.body_scroll = 0;
            }
            KeyCode::PageDown => {
                // Spec §4.3.2: half-page. The pane height is unknown to
                // pure state, so we just advance a fixed step here; the
                // loop's render clamps anything past the body length.
                self.body_scroll = self.body_scroll.saturating_add(BODY_HALF_PAGE);
            }
            KeyCode::PageUp => {
                self.body_scroll = self.body_scroll.saturating_sub(BODY_HALF_PAGE);
            }
            KeyCode::Char('d') | KeyCode::Enter if !self.findings.is_empty() => {
                self.modal = Modal::DismissReason;
                self.modal_target_index = Some(self.selected);
                self.status_message = None;
            }
            KeyCode::Char('u') => {
                if !self.undo_stack.is_empty() {
                    return Command::Undo;
                }
                self.status_message = Some("nothing to undo".into());
            }
            KeyCode::Char('?') => {
                self.modal = Modal::Help;
                self.help_visible = true;
            }
            // Phase 1C Stage 5 — open the dismissal-history view (AC 157).
            // The loop fetches via `MemoryStore::list_all` and calls
            // `apply_history_loaded`.
            KeyCode::Char('H') => {
                self.status_message = None;
                return Command::OpenHistory;
            }
            _ => {}
        }
        Command::None
    }

    /// Phase 1C Stage 5 — key dispatch inside [`View::History`]. Spec §5.2:
    ///   * `H` / `Esc` → back to main view.
    ///   * `q` / Ctrl+C → quit TUI from any view.
    ///   * `j` / `k` / `↓` / `↑` / `g` / `G` — cursor navigation.
    ///   * `p` — open the promote modal (only on `local_only` rows).
    ///   * `D` (capital) — open the demote-confirm modal (only on
    ///     `promoted_convention` rows; capital-D avoids collision with
    ///     main-list `d`-for-dismiss muscle memory, §B6).
    fn on_key_history(&mut self, key: KeyEvent) -> Command {
        if matches!(key.code, KeyCode::Char('q')) {
            return Command::Quit;
        }
        if matches!(key.code, KeyCode::Char('c')) && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Command::Quit;
        }
        if matches!(key.code, KeyCode::Char('H')) || matches!(key.code, KeyCode::Esc) {
            self.view = View::Main;
            self.history_body_scroll = 0;
            self.status_message = None;
            return Command::None;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down
                if !self.history_rows.is_empty()
                    && self.history_selected + 1 < self.history_rows.len() =>
            {
                self.history_selected += 1;
                self.history_body_scroll = 0;
                if let Some(row) = self.history_rows.get(self.history_selected) {
                    return Command::LoadTransitions(row.finding_identity_hash);
                }
            }
            KeyCode::Char('k') | KeyCode::Up if self.history_selected > 0 => {
                self.history_selected -= 1;
                self.history_body_scroll = 0;
                if let Some(row) = self.history_rows.get(self.history_selected) {
                    return Command::LoadTransitions(row.finding_identity_hash);
                }
            }
            KeyCode::Char('g') => {
                self.history_selected = 0;
                self.history_body_scroll = 0;
                if let Some(row) = self.history_rows.first() {
                    return Command::LoadTransitions(row.finding_identity_hash);
                }
            }
            KeyCode::Char('G') if !self.history_rows.is_empty() => {
                self.history_selected = self.history_rows.len() - 1;
                self.history_body_scroll = 0;
                if let Some(row) = self.history_rows.get(self.history_selected) {
                    return Command::LoadTransitions(row.finding_identity_hash);
                }
            }
            KeyCode::PageDown => {
                self.history_body_scroll = self.history_body_scroll.saturating_add(BODY_HALF_PAGE);
            }
            KeyCode::PageUp => {
                self.history_body_scroll = self.history_body_scroll.saturating_sub(BODY_HALF_PAGE);
            }
            KeyCode::Char('p') => {
                // Spec §5.2 / AC 158: gate at the keystroke level. Only
                // `local_only` rows open the modal; other states emit a
                // status-bar note and DO NOT open the modal.
                let Some(row) = self.history_rows.get(self.history_selected) else {
                    return Command::None;
                };
                match row.promotion_state {
                    PromotionState::LocalOnly => {
                        // Seed the buffer with the title so Enter-with-no-edit
                        // produces a title-only block (spec §5.2: "Enter to
                        // accept title-only"). The renderer surfaces the
                        // seed so users see what they'd commit; clearing the
                        // line and pressing Enter also produces title-only.
                        self.promote_text_buf = row.title_snapshot.clone();
                        self.modal = Modal::PromoteText;
                        self.status_message = None;
                    }
                    PromotionState::Candidate => {
                        self.status_message = Some(
                            "promote requires local_only state (this row is candidate; dismiss \
                             it more or wait for auto-promote)"
                                .into(),
                        );
                    }
                    PromotionState::PromotedConvention => {
                        self.status_message =
                            Some("already a promoted_convention; demote first to update".into());
                    }
                }
            }
            KeyCode::Char('D') => {
                // Spec §5.2 / AC 159: gate at the keystroke level. Only
                // `promoted_convention` rows open the confirmation.
                let Some(row) = self.history_rows.get(self.history_selected) else {
                    return Command::None;
                };
                match row.promotion_state {
                    PromotionState::PromotedConvention => {
                        self.modal = Modal::DemoteConfirm;
                        self.status_message = None;
                    }
                    PromotionState::Candidate | PromotionState::LocalOnly => {
                        self.status_message = Some(
                            "demote requires promoted_convention state (use 'p' to promote a \
                             local_only row first)"
                                .into(),
                        );
                    }
                }
            }
            KeyCode::Char('?') => {
                self.modal = Modal::Help;
                self.help_visible = true;
            }
            _ => {}
        }
        Command::None
    }

    fn on_key_help(&mut self, _key: KeyEvent) -> Command {
        // Spec §4.3.4: any key dismisses.
        self.modal = Modal::None;
        self.help_visible = false;
        Command::None
    }

    fn on_key_dismiss_reason(&mut self, key: KeyEvent) -> Command {
        if matches!(key.code, KeyCode::Esc) {
            self.modal = Modal::None;
            self.modal_target_index = None;
            return Command::None;
        }
        let reason = match key.code {
            KeyCode::Char('f') => DismissalReason::FalsePositive,
            KeyCode::Char('i') => DismissalReason::Intentional,
            KeyCode::Char('s') => DismissalReason::OutOfScope,
            KeyCode::Char('w') => DismissalReason::WontFix,
            KeyCode::Char('o') => {
                self.modal = Modal::DismissNote;
                self.note_buf.clear();
                return Command::None;
            }
            _ => return Command::None,
        };
        let idx = self
            .modal_target_index
            .expect("modal_target_index set when DismissReason is open");
        self.modal = Modal::None;
        self.modal_target_index = None;
        Command::Dismiss {
            finding_index: idx,
            reason,
            note: None,
        }
    }

    fn on_key_dismiss_note(&mut self, key: KeyEvent) -> Command {
        match key.code {
            KeyCode::Esc => {
                self.modal = Modal::DismissReason; // back to the reason picker
                self.note_buf.clear();
                Command::None
            }
            KeyCode::Backspace => {
                self.note_buf.pop();
                Command::None
            }
            KeyCode::Enter => self.try_commit_note(),
            KeyCode::Char(c) => {
                self.append_note_char(c);
                Command::None
            }
            _ => Command::None,
        }
    }

    fn on_key_error(&mut self, _key: KeyEvent) -> Command {
        self.modal = Modal::None;
        self.status_message = None;
        Command::None
    }

    /// Phase 1C Stage 5 — single-line text editor for the promote modal.
    /// `Enter` submits (empty buffer = title-only block per spec §5.2);
    /// `Esc` cancels and discards the buffer. Newlines are rejected
    /// (we're a single-line input — the spec lists no multi-line key).
    fn on_key_promote_text(&mut self, key: KeyEvent) -> Command {
        match key.code {
            KeyCode::Esc => {
                self.modal = Modal::None;
                self.promote_text_buf.clear();
                Command::None
            }
            KeyCode::Backspace => {
                self.promote_text_buf.pop();
                Command::None
            }
            KeyCode::Enter => {
                let Some(row) = self.history_rows.get(self.history_selected) else {
                    self.modal = Modal::None;
                    self.promote_text_buf.clear();
                    return Command::None;
                };
                // Guard against state drift: the row may have transitioned
                // between modal-open and Enter (paranoid but cheap; the
                // snapshot doesn't refresh, but the SQLite side could have
                // moved if another process touched it).
                if row.promotion_state != PromotionState::LocalOnly {
                    self.status_message = Some(
                        "promote aborted: row is no longer local_only (snapshot stale; quit \
                         and re-invoke to refresh)"
                            .into(),
                    );
                    self.modal = Modal::Error;
                    self.promote_text_buf.clear();
                    return Command::None;
                }
                let hash = row.finding_identity_hash;
                // Trim trailing whitespace; the spec is silent on
                // leading-trim but title-only mode (empty after trim)
                // honors "Enter to accept title-only" cleanly.
                let body = std::mem::take(&mut self.promote_text_buf)
                    .trim_end()
                    .to_string();
                self.modal = Modal::None;
                Command::Promote { hash, body }
            }
            KeyCode::Char(c) => {
                // Reject newlines defensively — KeyCode::Enter is the
                // separate submit path; embedded \n/\r would only arrive
                // from a paste pipeline that shouldn't be used here.
                if c == '\n' || c == '\r' {
                    return Command::None;
                }
                // Strip control chars except tab→space (mirrors the
                // dismiss-note rules; keeps the modal robust to paste).
                if (c as u32) < 0x20 && c != '\t' {
                    return Command::None;
                }
                let ch = if c == '\t' { ' ' } else { c };
                // Soft cap to keep paste bombs from hanging the input;
                // the on-disk body cap is enforced elsewhere
                // (BODY_SNAPSHOT_MAX_BYTES = 2048).
                if self.promote_text_buf.len() + ch.len_utf8()
                    > quorum_core::memory::BODY_SNAPSHOT_MAX_BYTES
                {
                    return Command::None;
                }
                self.promote_text_buf.push(ch);
                Command::None
            }
            _ => Command::None,
        }
    }

    /// Phase 1C Stage 5 — `Y/N` confirmation for demote. Spec §5.2:
    /// capital `Y` confirms; anything else (lowercase `y`, `n`, `Esc`)
    /// cancels.
    fn on_key_demote_confirm(&mut self, key: KeyEvent) -> Command {
        match key.code {
            KeyCode::Char('Y') => {
                let Some(row) = self.history_rows.get(self.history_selected) else {
                    self.modal = Modal::None;
                    return Command::None;
                };
                if row.promotion_state != PromotionState::PromotedConvention {
                    self.status_message = Some(
                        "demote aborted: row is no longer promoted_convention (snapshot stale; \
                         quit and re-invoke to refresh)"
                            .into(),
                    );
                    self.modal = Modal::Error;
                    return Command::None;
                }
                let hash = row.finding_identity_hash;
                self.modal = Modal::None;
                Command::Demote { hash }
            }
            _ => {
                // Esc, n, lowercase y, anything else — cancel.
                self.modal = Modal::None;
                Command::None
            }
        }
    }

    /// Append a char to the note buffer per the §4.3.3 rules:
    ///   - control chars (other than \t which collapses to space) stripped
    ///   - embedded newlines (\n / \r) rejected with an error modal
    ///   - cap enforced inside the loop on commit; we soft-cap on input
    ///     so the user doesn't fill the buffer past 2KB and lose typing.
    fn append_note_char(&mut self, c: char) {
        if c == '\n' || c == '\r' {
            self.status_message = Some(NOTE_NEWLINE_REJECTED.into());
            self.modal = Modal::Error;
            return;
        }
        if (c as u32) < 0x20 && c != '\t' {
            // strip — silent per spec
            return;
        }
        let ch = if c == '\t' { ' ' } else { c };
        // 2KB hard cap — additional input past the cap is silently dropped;
        // the explicit too-long check at commit catches paste-bomb cases.
        if self.note_buf.len() + ch.len_utf8() > NOTE_MAX_BYTES {
            self.status_message = Some(NOTE_TOO_LONG.into());
            self.modal = Modal::Error;
            return;
        }
        self.note_buf.push(ch);
    }

    fn try_commit_note(&mut self) -> Command {
        let trimmed = self.note_buf.trim().to_string();
        if trimmed.is_empty() {
            self.status_message = Some(NOTE_REQUIRED.into());
            self.modal = Modal::Error;
            return Command::None;
        }
        if trimmed.len() > NOTE_MAX_BYTES {
            self.status_message = Some(NOTE_TOO_LONG.into());
            self.modal = Modal::Error;
            return Command::None;
        }
        let idx = self
            .modal_target_index
            .expect("modal_target_index set when DismissNote is open");
        self.modal = Modal::None;
        self.modal_target_index = None;
        let note = std::mem::take(&mut self.note_buf);
        Command::Dismiss {
            finding_index: idx,
            reason: DismissalReason::Other,
            note: Some(note),
        }
    }

    /// Called by the loop after `MemoryStore::dismiss` succeeded.
    /// Removes the finding from the visible list, advances selection to
    /// the next visible finding (no wrap), pushes the undo entry.
    pub fn apply_committed_dismissal(&mut self, finding_index: usize, dismissal_id: DismissalId) {
        if finding_index >= self.findings.len() {
            return;
        }
        let removed = self.findings.remove(finding_index);
        self.undo_stack.push(UndoEntry {
            dismissal_id,
            original_index: finding_index,
            finding: removed,
        });
        self.session_dismissed_count += 1;
        // Selection advance: clamp to remaining-length-1, no wrap.
        if self.findings.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.findings.len() {
            self.selected = self.findings.len() - 1;
        }
        self.body_scroll = 0;
    }

    /// Called after `MemoryStore::delete(id)` succeeded for an undo.
    /// Re-inserts the finding at its original index, selects it.
    pub fn apply_undo(&mut self, entry: UndoEntry) {
        let pos = entry.original_index.min(self.findings.len());
        self.findings.insert(pos, entry.finding);
        self.selected = pos;
        self.session_dismissed_count = self.session_dismissed_count.saturating_sub(1);
        self.body_scroll = 0;
    }

    /// Phase 1C Stage 5 — install the dismissal-history snapshot loaded
    /// by the loop on first `H` press. Switches the view, resets the
    /// cursor, and (if non-empty) signals the caller to also fetch the
    /// initial row's transition log via [`Command::LoadTransitions`].
    pub fn apply_history_loaded(&mut self, rows: Vec<Dismissal>) -> Command {
        self.history_rows = rows;
        self.history_selected = 0;
        self.history_body_scroll = 0;
        self.history_transitions.clear();
        self.view = View::History;
        self.status_message = None;
        if let Some(row) = self.history_rows.first() {
            Command::LoadTransitions(row.finding_identity_hash)
        } else {
            Command::None
        }
    }

    /// Phase 1C Stage 5 — install the transition log for the currently
    /// selected history row.
    pub fn apply_transitions_loaded(&mut self, rows: Vec<StateTransitionRow>) {
        self.history_transitions = rows;
    }

    /// Phase 1C Stage 5 — record a successful T2 promotion in the
    /// history snapshot. The SQLite row flipped to `promoted_convention`;
    /// reflect that locally so the user sees the new state without a
    /// full re-fetch (consistent with §2 "no live re-fetch" non-goal —
    /// we update only the row the user just touched).
    pub fn apply_promote_committed(
        &mut self,
        hash: FindingIdentityHash,
        short_hash: &str,
        title: &str,
    ) {
        for row in self.history_rows.iter_mut() {
            if row.finding_identity_hash == hash {
                row.promotion_state = PromotionState::PromotedConvention;
                break;
            }
        }
        self.status_message = Some(format!(
            "promoted {short_hash}: {title} — commit .quorum/conventions.md to apply"
        ));
    }

    /// Phase 1C Stage 5 — record a successful T3 demotion in the
    /// history snapshot.
    pub fn apply_demote_committed(&mut self, hash: FindingIdentityHash, short_hash: &str) {
        for row in self.history_rows.iter_mut() {
            if row.finding_identity_hash == hash {
                row.promotion_state = PromotionState::LocalOnly;
                break;
            }
        }
        self.status_message = Some(format!("demoted {short_hash} (now local_only)"));
    }

    /// Phase 1C Stage 5 — surface a write failure from the promote /
    /// demote orchestrator. Opens the error modal so the message is
    /// visible above the table.
    pub fn apply_write_failed(&mut self, msg: String) {
        self.status_message = Some(msg);
        self.modal = Modal::Error;
    }

    /// `dismiss()` returned `AlreadyDismissed` (defensive). Show the
    /// dedicated error modal but DROP the finding from the visible list
    /// — the user's intent (suppress it) is honored.
    pub fn apply_already_dismissed(&mut self, finding_index: usize) {
        if finding_index < self.findings.len() {
            self.findings.remove(finding_index);
        }
        if self.selected >= self.findings.len() && !self.findings.is_empty() {
            self.selected = self.findings.len() - 1;
        }
        self.status_message = Some(ALREADY_DISMISSED.into());
        self.modal = Modal::Error;
    }
}

/// Spec §4.3.3 — strict 2KB cap on note bytes (UTF-8).
pub const NOTE_MAX_BYTES: usize = quorum_core::memory::NOTE_MAX_BYTES;
const BODY_HALF_PAGE: u16 = 10; // tuned at render time

pub const NOTE_REQUIRED: &str = "note required for \"other\" reason";
pub const NOTE_TOO_LONG: &str = "note too long; 2KB max";
pub const NOTE_NEWLINE_REJECTED: &str = "note must be single-line; newline rejected";
pub const ALREADY_DISMISSED: &str = "already dismissed; press any key to continue";

#[cfg(test)]
mod tests {
    use super::*;
    use quorum_core::review::{FindingSource, Severity};
    use time::OffsetDateTime;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// Build a `Dismissal` fixture with the supplied promotion state +
    /// title. The hash is derived from `byte` so different rows in the
    /// same fixture compare unequal.
    fn dismissal(byte: u8, title: &str, state: PromotionState) -> Dismissal {
        Dismissal {
            id: DismissalId(byte as i64),
            finding_identity_hash: FindingIdentityHash([byte; 32]),
            title_snapshot: title.to_string(),
            body_snapshot: Some(format!("body snapshot for {title}")),
            source_type_snapshot: "divergence".into(),
            models_snapshot: vec!["m1".into(), "m2".into()],
            branch_snapshot: "main".into(),
            reason: DismissalReason::FalsePositive,
            note: None,
            dismissed_at: OffsetDateTime::UNIX_EPOCH,
            last_seen_at: OffsetDateTime::UNIX_EPOCH,
            last_seen_session_id: None,
            recurrence_count: 3,
            expires_at: None,
            repo_head_sha_first: "abcdef0123".into(),
            promotion_state: state,
        }
    }

    fn fixture() -> AppState {
        let mk = |t: &str, sev: Severity| Finding {
            severity: sev,
            title: t.to_string(),
            body: format!("body of {t}"),
            source: FindingSource::Divergence,
            supported_by: vec!["m1".into(), "m2".into()],
            confidence: Some(0.9),
        };
        AppState::new(
            vec![
                mk("alpha", Severity::High),
                mk("beta", Severity::High),
                mk("gamma", Severity::Medium),
                mk("delta", Severity::Low),
            ],
            false,
        )
    }

    #[test]
    fn j_k_navigate_with_no_wrap() {
        let mut s = fixture();
        assert_eq!(s.selected, 0);
        s.on_key(k(KeyCode::Char('j')));
        assert_eq!(s.selected, 1);
        s.on_key(k(KeyCode::Char('j')));
        s.on_key(k(KeyCode::Char('j')));
        s.on_key(k(KeyCode::Char('j'))); // tries past end; clamps
        assert_eq!(s.selected, 3);
        s.on_key(k(KeyCode::Char('k')));
        assert_eq!(s.selected, 2);
        for _ in 0..10 {
            s.on_key(k(KeyCode::Char('k')));
        }
        assert_eq!(s.selected, 0, "k clamps at zero, no wrap");
    }

    #[test]
    fn g_and_g_upper_jump_to_first_and_last() {
        let mut s = fixture();
        s.on_key(k(KeyCode::Char('G')));
        assert_eq!(s.selected, 3);
        s.on_key(k(KeyCode::Char('g')));
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn pgdn_pgup_advance_body_scroll() {
        let mut s = fixture();
        assert_eq!(s.body_scroll, 0);
        s.on_key(k(KeyCode::PageDown));
        assert!(s.body_scroll > 0);
        let high = s.body_scroll;
        s.on_key(k(KeyCode::PageUp));
        assert!(s.body_scroll < high);
    }

    #[test]
    fn d_opens_reason_modal_then_letters_commit() {
        let mut s = fixture();
        let cmd = s.on_key(k(KeyCode::Char('d')));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.modal, Modal::DismissReason);
        let cmd = s.on_key(k(KeyCode::Char('w')));
        match cmd {
            Command::Dismiss {
                reason: DismissalReason::WontFix,
                note: None,
                finding_index: 0,
            } => {}
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(s.modal, Modal::None);
    }

    #[test]
    fn enter_is_alias_for_d_in_list_mode() {
        let mut s = fixture();
        s.on_key(k(KeyCode::Enter));
        assert_eq!(s.modal, Modal::DismissReason);
    }

    #[test]
    fn o_opens_note_input_and_validates_rules() {
        let mut s = fixture();
        s.on_key(k(KeyCode::Char('d')));
        s.on_key(k(KeyCode::Char('o')));
        assert_eq!(s.modal, Modal::DismissNote);

        // Empty submission rejected.
        let cmd = s.on_key(k(KeyCode::Enter));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.modal, Modal::Error);
        assert_eq!(s.status_message.as_deref(), Some(NOTE_REQUIRED));
        // press any key to dismiss error → back to list
        s.on_key(k(KeyCode::Char('x')));
        assert_eq!(s.modal, Modal::None);

        // Re-enter the prompt and try newline rejection.
        s.on_key(k(KeyCode::Char('d')));
        s.on_key(k(KeyCode::Char('o')));
        s.on_key(k(KeyCode::Char('a')));
        s.on_key(k(KeyCode::Char('\n')));
        assert_eq!(s.modal, Modal::Error);
        assert_eq!(s.status_message.as_deref(), Some(NOTE_NEWLINE_REJECTED));
    }

    #[test]
    fn o_with_valid_note_commits_as_other() {
        let mut s = fixture();
        s.on_key(k(KeyCode::Char('d')));
        s.on_key(k(KeyCode::Char('o')));
        for c in "looks intentional".chars() {
            s.on_key(k(KeyCode::Char(c)));
        }
        let cmd = s.on_key(k(KeyCode::Enter));
        match cmd {
            Command::Dismiss {
                reason: DismissalReason::Other,
                note: Some(n),
                finding_index: 0,
            } => assert_eq!(n, "looks intentional"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn control_chars_stripped_tab_collapsed_to_space() {
        let mut s = fixture();
        s.on_key(k(KeyCode::Char('d')));
        s.on_key(k(KeyCode::Char('o')));
        s.on_key(k(KeyCode::Char('a')));
        s.on_key(k(KeyCode::Char('\t'))); // becomes space
        s.on_key(k(KeyCode::Char('\u{0007}'))); // BEL — stripped
        s.on_key(k(KeyCode::Char('b')));
        let cmd = s.on_key(k(KeyCode::Enter));
        match cmd {
            Command::Dismiss { note: Some(n), .. } => assert_eq!(n, "a b"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn note_2kb_cap_enforced_on_input() {
        let mut s = fixture();
        s.on_key(k(KeyCode::Char('d')));
        s.on_key(k(KeyCode::Char('o')));
        for _ in 0..NOTE_MAX_BYTES {
            s.on_key(k(KeyCode::Char('x')));
        }
        // One more must trigger the error modal.
        s.on_key(k(KeyCode::Char('y')));
        assert_eq!(s.modal, Modal::Error);
        assert_eq!(s.status_message.as_deref(), Some(NOTE_TOO_LONG));
    }

    #[test]
    fn esc_from_reason_modal_returns_to_list() {
        let mut s = fixture();
        s.on_key(k(KeyCode::Char('d')));
        s.on_key(k(KeyCode::Esc));
        assert_eq!(s.modal, Modal::None);
        assert!(s.modal_target_index.is_none());
    }

    #[test]
    fn esc_from_note_modal_returns_to_reason() {
        let mut s = fixture();
        s.on_key(k(KeyCode::Char('d')));
        s.on_key(k(KeyCode::Char('o')));
        s.on_key(k(KeyCode::Char('a')));
        s.on_key(k(KeyCode::Esc));
        assert_eq!(s.modal, Modal::DismissReason);
        assert!(s.note_buf.is_empty());
    }

    #[test]
    fn q_from_list_quits() {
        let mut s = fixture();
        let cmd = s.on_key(k(KeyCode::Char('q')));
        assert_eq!(cmd, Command::Quit);
    }

    #[test]
    fn esc_from_list_quits() {
        let mut s = fixture();
        let cmd = s.on_key(k(KeyCode::Esc));
        assert_eq!(cmd, Command::Quit);
    }

    #[test]
    fn ctrl_c_quits_through_restoration_path() {
        let mut s = fixture();
        let cmd = s.on_key(ctrl('c'));
        assert_eq!(cmd, Command::Quit);
    }

    #[test]
    fn help_toggles_then_any_key_dismisses() {
        let mut s = fixture();
        s.on_key(k(KeyCode::Char('?')));
        assert_eq!(s.modal, Modal::Help);
        assert!(s.help_visible);
        s.on_key(k(KeyCode::Char('x')));
        assert_eq!(s.modal, Modal::None);
        assert!(!s.help_visible);
    }

    #[test]
    fn undo_with_empty_stack_is_a_no_op_with_status() {
        let mut s = fixture();
        let cmd = s.on_key(k(KeyCode::Char('u')));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.status_message.as_deref(), Some("nothing to undo"));
    }

    #[test]
    fn apply_committed_dismissal_removes_and_advances_no_wrap() {
        let mut s = fixture();
        s.selected = 1; // "beta"
        let removed_title = s.findings[1].title.clone();
        s.apply_committed_dismissal(1, DismissalId(42));
        assert!(!s.findings.iter().any(|f| f.title == removed_title));
        assert_eq!(s.selected, 1, "next finding ('gamma') becomes selected");
        assert_eq!(s.undo_stack.len(), 1);
        assert_eq!(s.session_dismissed_count, 1);
    }

    #[test]
    fn apply_undo_restores_at_original_index() {
        let mut s = fixture();
        s.selected = 1;
        s.apply_committed_dismissal(1, DismissalId(42));
        let popped = s.undo_stack.pop().unwrap();
        s.apply_undo(popped);
        assert_eq!(s.findings[1].title, "beta");
        assert_eq!(s.selected, 1);
        assert_eq!(s.session_dismissed_count, 0);
    }

    #[test]
    fn undo_stack_unbounded_no_eviction() {
        // v0.1 had a 16-cap; v0.2/v1.0 drops it. Push 50 entries, all retained.
        let mut s = fixture();
        for i in 0..50u32 {
            s.undo_stack.push(UndoEntry {
                dismissal_id: DismissalId(i as i64),
                original_index: 0,
                finding: s.findings[0].clone(),
            });
        }
        assert_eq!(s.undo_stack.len(), 50);
    }

    #[test]
    fn key_release_events_are_ignored() {
        let mut s = fixture();
        let mut press = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        press.kind = KeyEventKind::Release;
        s.on_key(press);
        assert_eq!(s.selected, 0, "Release events do not advance selection");
    }

    // -------------------------------------------------------------------
    // Phase 1C Stage 5 — dismissal-history view + promote/demote modals
    // (AC 157 / 158 / 159).
    // -------------------------------------------------------------------

    fn shift(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)
    }

    fn history_fixture() -> AppState {
        let mut s = fixture();
        // Three rows covering each promotion state so the keystroke
        // gates for `p` / `D` are exercised exhaustively.
        let rows = vec![
            dismissal(0x11, "alpha rule", PromotionState::LocalOnly),
            dismissal(0x22, "beta candidate", PromotionState::Candidate),
            dismissal(0x33, "gamma promoted", PromotionState::PromotedConvention),
        ];
        let cmd = s.apply_history_loaded(rows);
        // Initial cursor row exists → loop fetches its transitions.
        match cmd {
            Command::LoadTransitions(_) => {}
            other => panic!("expected LoadTransitions after history load, got {other:?}"),
        }
        s
    }

    #[test]
    fn capital_h_from_main_emits_open_history_without_view_flip() {
        // AC 157: pressing H from main does NOT flip the view directly;
        // it signals the loop to fetch (the loop calls
        // apply_history_loaded which actually flips view).
        let mut s = fixture();
        let cmd = s.on_key(shift('H'));
        assert_eq!(cmd, Command::OpenHistory);
        assert_eq!(s.view, View::Main, "view flips inside apply_history_loaded");
    }

    #[test]
    fn apply_history_loaded_flips_view_and_requests_transitions() {
        // AC 157 — view flip happens inside apply_history_loaded, and
        // the returned Command tells the loop to fetch the first row's
        // transition log so the body pane has data on first render.
        let mut s = fixture();
        let rows = vec![dismissal(1, "row a", PromotionState::LocalOnly)];
        let cmd = s.apply_history_loaded(rows);
        assert_eq!(s.view, View::History);
        assert_eq!(s.history_selected, 0);
        match cmd {
            Command::LoadTransitions(h) => assert_eq!(h, FindingIdentityHash([1; 32])),
            other => panic!("expected LoadTransitions, got {other:?}"),
        }
    }

    #[test]
    fn apply_history_loaded_empty_emits_no_command() {
        let mut s = fixture();
        let cmd = s.apply_history_loaded(Vec::new());
        assert_eq!(s.view, View::History);
        assert!(s.history_rows.is_empty());
        assert_eq!(cmd, Command::None);
    }

    #[test]
    fn capital_h_from_history_returns_to_main() {
        // AC 157: H toggles back to main from history.
        let mut s = history_fixture();
        assert_eq!(s.view, View::History);
        let cmd = s.on_key(shift('H'));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.view, View::Main);
    }

    #[test]
    fn esc_from_history_returns_to_main_does_not_quit() {
        let mut s = history_fixture();
        let cmd = s.on_key(k(KeyCode::Esc));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.view, View::Main);
    }

    #[test]
    fn q_from_history_quits_tui() {
        // Spec §5.2: "q (which now quits the TUI from any view)."
        let mut s = history_fixture();
        let cmd = s.on_key(k(KeyCode::Char('q')));
        assert_eq!(cmd, Command::Quit);
    }

    #[test]
    fn ctrl_c_from_history_quits_tui() {
        let mut s = history_fixture();
        let cmd = s.on_key(ctrl('c'));
        assert_eq!(cmd, Command::Quit);
    }

    #[test]
    fn j_k_in_history_emit_load_transitions_each_move() {
        let mut s = history_fixture();
        assert_eq!(s.history_selected, 0);
        let cmd = s.on_key(k(KeyCode::Char('j')));
        assert_eq!(s.history_selected, 1);
        match cmd {
            Command::LoadTransitions(h) => assert_eq!(h, FindingIdentityHash([0x22; 32])),
            other => panic!("unexpected {other:?}"),
        }
        let cmd = s.on_key(k(KeyCode::Char('k')));
        assert_eq!(s.history_selected, 0);
        match cmd {
            Command::LoadTransitions(h) => assert_eq!(h, FindingIdentityHash([0x11; 32])),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn p_on_local_only_opens_promote_modal_with_title_seed() {
        // AC 158 — only local_only rows open the modal; buffer pre-seeded
        // with the title (Enter-with-no-edit = title-only block).
        let mut s = history_fixture();
        assert_eq!(s.history_selected, 0);
        let cmd = s.on_key(k(KeyCode::Char('p')));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.modal, Modal::PromoteText);
        assert_eq!(s.promote_text_buf, "alpha rule");
    }

    #[test]
    fn p_on_candidate_is_rejected_no_modal_opens() {
        // AC 158 — keystroke-level gate. Candidate rows produce a
        // status-bar note; modal does NOT open.
        let mut s = history_fixture();
        s.on_key(k(KeyCode::Char('j'))); // move to candidate row
        let cmd = s.on_key(k(KeyCode::Char('p')));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.modal, Modal::None);
        assert!(s
            .status_message
            .as_deref()
            .unwrap_or("")
            .starts_with("promote requires local_only"));
    }

    #[test]
    fn p_on_promoted_convention_is_rejected_no_modal_opens() {
        // AC 158 — promoted_convention rows reject `p` with a different
        // message ("demote first").
        let mut s = history_fixture();
        s.on_key(k(KeyCode::Char('j')));
        s.on_key(k(KeyCode::Char('j'))); // move to promoted row
        let cmd = s.on_key(k(KeyCode::Char('p')));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.modal, Modal::None);
        assert!(s
            .status_message
            .as_deref()
            .unwrap_or("")
            .starts_with("already a promoted_convention"));
    }

    #[test]
    fn promote_modal_enter_with_title_seed_commits_title_only() {
        // AC 158 — Enter-with-no-edit on the seeded buffer submits the
        // title as the body. (Title-only semantics: spec §5.2 says
        // "Enter to accept title-only"; the seed is the title, so
        // accepting unedited = title body.)
        let mut s = history_fixture();
        s.on_key(k(KeyCode::Char('p')));
        let cmd = s.on_key(k(KeyCode::Enter));
        match cmd {
            Command::Promote { hash, body } => {
                assert_eq!(hash, FindingIdentityHash([0x11; 32]));
                assert_eq!(body, "alpha rule");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(s.modal, Modal::None);
    }

    #[test]
    fn promote_modal_clear_and_enter_commits_empty_body() {
        // AC 158 — clearing the buffer (Backspace) and Enter gives an
        // empty body string. The loop interprets this as title-only and
        // forwards to MemoryStore::commit_promote with title fallback.
        let mut s = history_fixture();
        s.on_key(k(KeyCode::Char('p')));
        for _ in 0..s.promote_text_buf.len() + 1 {
            s.on_key(k(KeyCode::Backspace));
        }
        let cmd = s.on_key(k(KeyCode::Enter));
        match cmd {
            Command::Promote { body, .. } => assert_eq!(body, ""),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn promote_modal_user_edits_then_enter_commits_custom_body() {
        let mut s = history_fixture();
        s.on_key(k(KeyCode::Char('p')));
        // Replace the buffer with a custom body.
        s.promote_text_buf.clear();
        for c in "do not block on stylistic ABC".chars() {
            s.on_key(k(KeyCode::Char(c)));
        }
        let cmd = s.on_key(k(KeyCode::Enter));
        match cmd {
            Command::Promote { body, .. } => assert_eq!(body, "do not block on stylistic ABC"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn promote_modal_esc_cancels_and_clears_buffer() {
        let mut s = history_fixture();
        s.on_key(k(KeyCode::Char('p')));
        assert!(!s.promote_text_buf.is_empty());
        let cmd = s.on_key(k(KeyCode::Esc));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.modal, Modal::None);
        assert!(s.promote_text_buf.is_empty());
    }

    #[test]
    fn promote_modal_rejects_embedded_newlines_silently() {
        // Defense-in-depth: newlines via Char('\n') should be silently
        // dropped (Enter is the submit path, not a literal newline).
        let mut s = history_fixture();
        s.on_key(k(KeyCode::Char('p')));
        let before = s.promote_text_buf.len();
        let cmd = s.on_key(k(KeyCode::Char('\n')));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.promote_text_buf.len(), before);
        assert_eq!(s.modal, Modal::PromoteText);
    }

    #[test]
    fn capital_d_on_promoted_convention_opens_confirm_modal() {
        // AC 159 — only promoted_convention rows open the confirm modal.
        let mut s = history_fixture();
        s.on_key(k(KeyCode::Char('j')));
        s.on_key(k(KeyCode::Char('j'))); // navigate to promoted row
        let cmd = s.on_key(shift('D'));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.modal, Modal::DemoteConfirm);
    }

    #[test]
    fn capital_d_on_local_only_is_rejected_no_modal_opens() {
        let mut s = history_fixture();
        // Cursor at local_only.
        let cmd = s.on_key(shift('D'));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.modal, Modal::None);
        assert!(s
            .status_message
            .as_deref()
            .unwrap_or("")
            .starts_with("demote requires promoted_convention"));
    }

    #[test]
    fn capital_d_on_candidate_is_rejected_no_modal_opens() {
        let mut s = history_fixture();
        s.on_key(k(KeyCode::Char('j'))); // candidate
        let cmd = s.on_key(shift('D'));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.modal, Modal::None);
    }

    #[test]
    fn demote_confirm_y_uppercase_commits() {
        // AC 159 — only capital Y confirms.
        let mut s = history_fixture();
        s.on_key(k(KeyCode::Char('j')));
        s.on_key(k(KeyCode::Char('j')));
        s.on_key(shift('D'));
        let cmd = s.on_key(shift('Y'));
        match cmd {
            Command::Demote { hash } => assert_eq!(hash, FindingIdentityHash([0x33; 32])),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(s.modal, Modal::None);
    }

    #[test]
    fn demote_confirm_lowercase_y_cancels() {
        let mut s = history_fixture();
        s.on_key(k(KeyCode::Char('j')));
        s.on_key(k(KeyCode::Char('j')));
        s.on_key(shift('D'));
        let cmd = s.on_key(k(KeyCode::Char('y')));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.modal, Modal::None);
    }

    #[test]
    fn demote_confirm_n_or_esc_cancels() {
        for cancel in [KeyCode::Char('n'), KeyCode::Esc] {
            let mut s = history_fixture();
            s.on_key(k(KeyCode::Char('j')));
            s.on_key(k(KeyCode::Char('j')));
            s.on_key(shift('D'));
            let cmd = s.on_key(k(cancel));
            assert_eq!(cmd, Command::None);
            assert_eq!(s.modal, Modal::None);
        }
    }

    #[test]
    fn d_lowercase_in_history_is_a_no_op_not_dismiss() {
        // Capital-D collision-check: lowercase `d` does NOT trigger the
        // dismiss path inside the history view (that's main-view-only).
        let mut s = history_fixture();
        s.on_key(k(KeyCode::Char('j')));
        s.on_key(k(KeyCode::Char('j')));
        let cmd = s.on_key(k(KeyCode::Char('d')));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.modal, Modal::None, "lowercase d is unbound in history");
    }

    #[test]
    fn apply_promote_committed_flips_local_only_to_promoted_in_snapshot() {
        // After T2 succeeds, the snapshot reflects the new state so the
        // user sees the change without a refresh.
        let mut s = history_fixture();
        s.apply_promote_committed(
            FindingIdentityHash([0x11; 32]),
            "111111111111",
            "alpha rule",
        );
        assert_eq!(
            s.history_rows[0].promotion_state,
            PromotionState::PromotedConvention
        );
        assert!(s.status_message.is_some());
    }

    #[test]
    fn apply_demote_committed_flips_promoted_to_local_only_in_snapshot() {
        let mut s = history_fixture();
        s.apply_demote_committed(FindingIdentityHash([0x33; 32]), "333333333333");
        assert_eq!(s.history_rows[2].promotion_state, PromotionState::LocalOnly);
    }

    #[test]
    fn apply_write_failed_opens_error_modal() {
        let mut s = history_fixture();
        s.apply_write_failed("commit_promote: state drifted".into());
        assert_eq!(s.modal, Modal::Error);
        assert_eq!(
            s.status_message.as_deref(),
            Some("commit_promote: state drifted")
        );
    }

    #[test]
    fn history_g_capital_g_jump_to_first_last_emit_transitions() {
        let mut s = history_fixture();
        let cmd = s.on_key(shift('G'));
        assert_eq!(s.history_selected, 2);
        assert!(matches!(cmd, Command::LoadTransitions(_)));
        let cmd = s.on_key(k(KeyCode::Char('g')));
        assert_eq!(s.history_selected, 0);
        assert!(matches!(cmd, Command::LoadTransitions(_)));
    }

    #[test]
    fn empty_history_p_and_capital_d_are_safe_no_ops() {
        // Defensive: cursor at 0 with empty rows must not panic.
        let mut s = fixture();
        let _ = s.apply_history_loaded(Vec::new());
        let cmd = s.on_key(k(KeyCode::Char('p')));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.modal, Modal::None);
        let cmd = s.on_key(shift('D'));
        assert_eq!(cmd, Command::None);
        assert_eq!(s.modal, Modal::None);
    }
}
