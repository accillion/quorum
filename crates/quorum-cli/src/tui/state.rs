//! Pure TUI state + event handler. No I/O, no terminal — everything in
//! this module is exercised by `tui_smoke.rs` against `TestBackend`.
//!
//! The loop in [`super`] is responsible for raw mode, alt-screen, panic
//! hook, and the actual `dismiss()` calls into [`MemoryStore`]. State
//! reflects what the next render needs; commands signal what the loop
//! should execute on this tick.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use quorum_core::memory::{DismissalId, DismissalReason};
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modal {
    None,
    DismissReason,
    DismissNote, // free-text input for `o` (other)
    Help,
    /// Briefly shown when something illegal was tried (esc dismisses).
    Error,
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
            Modal::None => self.on_key_list(key),
            Modal::Help => self.on_key_help(key),
            Modal::DismissReason => self.on_key_dismiss_reason(key),
            Modal::DismissNote => self.on_key_dismiss_note(key),
            Modal::Error => self.on_key_error(key),
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

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
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
}
