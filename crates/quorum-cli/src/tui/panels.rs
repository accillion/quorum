//! Rendering. Pure functions that consume `&AppState` and a ratatui
//! `Frame`. Body pane is **plain wrapped text** — no markdown parser,
//! no bold/italic/code (v1.0 §4.3.1 explicitly drops the v0.1 promise).

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::state::{AppState, Modal, View};
use quorum_core::memory::PromotionState;
use quorum_core::review::{FindingSource, Severity};

/// Top-level frame layout: main row (list + body) + status bar.
pub fn render(frame: &mut Frame, state: &AppState, session_id: &str, db_path: &str) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[0]);

    match state.view {
        View::Main => {
            render_list_pane(frame, main[0], state);
            render_body_pane(frame, main[1], state);
        }
        View::History => {
            render_history_list(frame, main[0], state);
            render_history_body(frame, main[1], state);
        }
    }
    render_status_bar(frame, chunks[1], state, session_id);

    // Modal overlays render last so they paint on top.
    match state.modal {
        Modal::DismissReason => render_dismiss_reason(frame, area, state),
        Modal::DismissNote => render_dismiss_note(frame, area, state),
        Modal::Help => render_help(frame, area, session_id, db_path, state.view),
        Modal::Error => render_error(frame, area, state),
        Modal::PromoteText => render_promote_text(frame, area, state),
        Modal::DemoteConfirm => render_demote_confirm(frame, area, state),
        Modal::None => {}
    }
}

fn render_list_pane(frame: &mut Frame, area: Rect, state: &AppState) {
    let title = format!(
        "Findings ({}; {} dismissed)",
        state.findings.len(),
        state.session_dismissed_count
    );
    let inner_width = area.width.saturating_sub(4) as usize; // borders + selection marker
    let lines: Vec<Line> = state
        .findings
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let prefix = if i == state.selected { "▶ " } else { "  " };
            let sev = format!("[{}]", severity_letter(f.severity));
            let label = format!("{prefix}{sev} {}", f.title);
            let row = truncate_display(&label, inner_width);
            if i == state.selected {
                Line::from(Span::styled(
                    row,
                    Style::default().add_modifier(Modifier::REVERSED),
                ))
            } else {
                Line::from(row)
            }
        })
        .collect();
    let widget = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(widget, area);
}

fn render_body_pane(frame: &mut Frame, area: Rect, state: &AppState) {
    let Some(f) = state.selected_finding() else {
        let widget = Paragraph::new("").block(Block::default().borders(Borders::ALL).title(""));
        frame.render_widget(widget, area);
        return;
    };
    let title = truncate_display(&f.title, area.width.saturating_sub(4) as usize);

    let attribution = {
        let models = if f.supported_by.is_empty() {
            "(no model attribution)".to_string()
        } else {
            f.supported_by.join(" + ")
        };
        let kind = match f.source {
            FindingSource::Divergence => "divergence",
            FindingSource::Agreement => "agreement",
            FindingSource::Assumption => "assumption",
        };
        let conf = f
            .confidence
            .map(|c| format!(" · confidence {:.2}", c))
            .unwrap_or_default();
        format!("{models} · {kind}{conf}")
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        attribution,
        Style::default().add_modifier(Modifier::DIM),
    )));
    lines.push(Line::from("")); // spacer
    for raw in f.body.lines() {
        lines.push(Line::from(raw.to_string()));
    }

    let widget = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((state.body_scroll, 0))
        .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(widget, area);
}

fn render_status_bar(frame: &mut Frame, area: Rect, state: &AppState, session_id: &str) {
    let session_tail = session_short(session_id);
    let hint = if let Some(msg) = &state.status_message {
        msg.clone()
    } else {
        match state.view {
            View::Main => "j/k nav · d dismiss · u undo · H history · q quit · ? help".into(),
            View::History => {
                // Spec §5.2: `H back | p promote | D demote | x delete |
                // / filter | ? help`. We omit `x delete` and `/ filter`
                // since those are not Stage-5 ACs; the spec's status text
                // is descriptive of the eventual view, not Stage 5's gate.
                "j/k nav · p promote · D demote · H/Esc back · q quit · ? help".into()
            }
        }
    };
    let counter = match state.view {
        View::Main => format!(
            "({}; {} dismissed)",
            state.findings.len(),
            state.session_dismissed_count
        ),
        View::History => format!("[history: {} rows]", state.history_rows.len()),
    };
    let text = format!("{hint}  {counter}  session {session_tail}");
    frame.render_widget(Paragraph::new(text), area);
}

/// Phase 1C Stage 5 — dismissal-history table. Columns:
///   `<short-hash (8 chars)>  <state (1 char)>  <recurrence>  <title>`.
/// Spec §5.2; rows sorted by `last_seen_at DESC` upstream
/// (`MemoryStore::list_all`'s default sort).
fn render_history_list(frame: &mut Frame, area: Rect, state: &AppState) {
    let title = format!("Dismissals ({})", state.history_rows.len());
    if state.history_rows.is_empty() {
        let widget = Paragraph::new("(no dismissals)")
            .block(Block::default().borders(Borders::ALL).title(title));
        frame.render_widget(widget, area);
        return;
    }
    let inner_width = area.width.saturating_sub(4) as usize;
    let lines: Vec<Line> = state
        .history_rows
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let prefix = if i == state.history_selected {
                "▶ "
            } else {
                "  "
            };
            let hex = d.finding_identity_hash.to_hex();
            let short = &hex[..8];
            let state_char = match d.promotion_state {
                PromotionState::Candidate => 'c',
                PromotionState::LocalOnly => 'L',
                PromotionState::PromotedConvention => 'P',
            };
            let row_text = format!(
                "{prefix}{short}  {state_char}  n={n:<3} {title}",
                n = d.recurrence_count,
                title = d.title_snapshot
            );
            let row = truncate_display(&row_text, inner_width);
            if i == state.history_selected {
                Line::from(Span::styled(
                    row,
                    Style::default().add_modifier(Modifier::REVERSED),
                ))
            } else {
                Line::from(row)
            }
        })
        .collect();
    let widget = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(widget, area);
}

/// Phase 1C Stage 5 — body pane for the history view. Spec §5.2:
/// "title + body_snapshot + state + most recent 5 transition log
/// entries." Transition rows are oldest-first from the store; we
/// surface the last 5 (most recent) by tailing.
fn render_history_body(frame: &mut Frame, area: Rect, state: &AppState) {
    let Some(d) = state.history_rows.get(state.history_selected) else {
        let widget = Paragraph::new("").block(Block::default().borders(Borders::ALL).title(""));
        frame.render_widget(widget, area);
        return;
    };
    let title = truncate_display(&d.title_snapshot, area.width.saturating_sub(4) as usize);
    let state_label = match d.promotion_state {
        PromotionState::Candidate => "candidate",
        PromotionState::LocalOnly => "local_only",
        PromotionState::PromotedConvention => "promoted_convention",
    };
    let mut lines: Vec<Line> = Vec::new();
    let header = format!(
        "state={} · recurrence={} · reason={}",
        state_label,
        d.recurrence_count,
        d.reason.as_db_str()
    );
    lines.push(Line::from(Span::styled(
        header,
        Style::default().add_modifier(Modifier::DIM),
    )));
    lines.push(Line::from(""));
    if let Some(body) = &d.body_snapshot {
        for raw in body.lines() {
            lines.push(Line::from(raw.to_string()));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "(no body snapshot)",
            Style::default().add_modifier(Modifier::DIM),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Transitions (most recent 5):",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    if state.history_transitions.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (none)",
            Style::default().add_modifier(Modifier::DIM),
        )));
    } else {
        let total = state.history_transitions.len();
        let start = total.saturating_sub(5);
        for t in &state.history_transitions[start..] {
            lines.push(Line::from(format!(
                "  {} → {} ({}; ts_ms={})",
                t.from_state.as_db_str(),
                t.to_state.as_db_str(),
                t.trigger.as_db_str(),
                t.ts_ms
            )));
        }
    }
    let widget = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((state.history_body_scroll, 0))
        .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(widget, area);
}

fn render_promote_text(frame: &mut Frame, area: Rect, state: &AppState) {
    let row_title = state
        .history_rows
        .get(state.history_selected)
        .map(|d| d.title_snapshot.clone())
        .unwrap_or_default();
    let title = format!("Promote: {row_title:.50}");
    let body_preview = if state.promote_text_buf.is_empty() {
        "(empty — Enter to commit title-only block)".to_string()
    } else {
        format!("> {}", state.promote_text_buf)
    };
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Convention body (Enter accepts, Esc cancels):",
            Style::default().add_modifier(Modifier::DIM),
        )),
        Line::from(""),
        Line::from(body_preview),
        Line::from(""),
        Line::from("  [enter] submit · [esc] cancel · [backspace] delete"),
    ];
    render_modal(frame, area, &title, lines, 78, 9);
}

fn render_demote_confirm(frame: &mut Frame, area: Rect, state: &AppState) {
    let (short, row_title) = state
        .history_rows
        .get(state.history_selected)
        .map(|d| {
            let hex = d.finding_identity_hash.to_hex();
            (hex[..12].to_string(), d.title_snapshot.clone())
        })
        .unwrap_or_else(|| (String::new(), String::new()));
    let title = "Demote convention".to_string();
    let lines = vec![
        Line::from(""),
        Line::from(format!("Demote {short}: {row_title}?")),
        Line::from(""),
        Line::from("  [Y] confirm (capital required) · [N / Esc] cancel"),
    ];
    render_modal(frame, area, &title, lines, 70, 7);
}

fn render_dismiss_reason(frame: &mut Frame, area: Rect, state: &AppState) {
    let title = state
        .modal_target_index
        .and_then(|i| state.findings.get(i))
        .map(|f| format!("Dismiss: {:.60}", f.title))
        .unwrap_or_else(|| "Dismiss".into());
    let lines = vec![
        Line::from(""),
        Line::from("  (f) false positive"),
        Line::from("  (i) intentional"),
        Line::from("  (s) out of scope"),
        Line::from("  (w) won't fix"),
        Line::from("  (o) other (free text)"),
        Line::from(""),
        Line::from("  [esc] cancel"),
    ];
    render_modal(frame, area, &title, lines, 60, 10);
}

fn render_dismiss_note(frame: &mut Frame, area: Rect, state: &AppState) {
    let title = "Dismiss (other) — note required".to_string();
    let body = format!("> {}", state.note_buf);
    let lines = vec![
        Line::from(""),
        Line::from(body),
        Line::from(""),
        Line::from("  [enter] submit · [esc] back · [backspace] delete"),
    ];
    render_modal(frame, area, &title, lines, 70, 6);
}

fn render_help(frame: &mut Frame, area: Rect, session_id: &str, db_path: &str, view: View) {
    let title = "Help".to_string();
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(""),
        Line::from("  j / ↓        next row"),
        Line::from("  k / ↑        previous row"),
        Line::from("  g            jump to first"),
        Line::from("  G            jump to last"),
        Line::from("  PgDn / PgUp  scroll body half-page"),
    ];
    match view {
        View::Main => {
            lines.push(Line::from("  d / Enter    dismiss the selected finding"));
            lines.push(Line::from(
                "  u            undo most recent in-session dismissal",
            ));
            lines.push(Line::from("  H            open dismissal-history view"));
            lines.push(Line::from("  q / Esc      quit"));
        }
        View::History => {
            lines.push(Line::from("  p            promote (local_only rows only)"));
            lines.push(Line::from(
                "  D            demote (promoted_convention rows only)",
            ));
            lines.push(Line::from("  H / Esc      back to main view"));
            lines.push(Line::from("  q            quit"));
        }
    }
    lines.push(Line::from("  ?            this help"));
    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        "  store: {} · session: {}",
        db_path,
        session_short(session_id)
    )));
    lines.push(Line::from(""));
    lines.push(Line::from("  (press any key to close)"));
    render_modal(frame, area, &title, lines, 70, 18);
}

fn render_error(frame: &mut Frame, area: Rect, state: &AppState) {
    let msg = state.status_message.clone().unwrap_or_default();
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            msg,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  (press any key to dismiss)"),
    ];
    render_modal(frame, area, "Error", lines, 60, 6);
}

fn render_modal(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    width: u16,
    height: u16,
) {
    let modal = centered_rect(area, width.min(area.width), height.min(area.height));
    frame.render_widget(Clear, modal);
    let widget = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title.to_string()),
    );
    frame.render_widget(widget, modal);
}

fn centered_rect(area: Rect, w: u16, h: u16) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

fn severity_letter(s: Severity) -> char {
    match s {
        Severity::High => 'H',
        Severity::Medium => 'M',
        Severity::Low => 'L',
        Severity::Info => 'I',
    }
}

/// Truncate `s` to at most `max_chars` *character* cells. Trailing
/// ellipsis if shortened. Avoids panics on multi-byte chars.
fn truncate_display(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let count: usize = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    let take = max_chars.saturating_sub(1);
    let mut out: String = s.chars().take(take).collect();
    out.push('…');
    out
}

fn session_short(id: &str) -> String {
    if id.len() > 12 {
        format!("…{}", &id[id.len() - 8..])
    } else {
        id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_handles_multibyte() {
        assert_eq!(truncate_display("café", 100), "café");
        assert_eq!(truncate_display("café", 3), "ca…");
    }

    #[test]
    fn session_short_collapses_long_ids() {
        let s = session_short("consensus_session_abcdef0123");
        assert!(s.starts_with('…'));
        assert!(s.ends_with("def0123"));
    }

    #[test]
    fn session_short_short_id_passthrough() {
        assert_eq!(session_short("short"), "short");
    }
}
