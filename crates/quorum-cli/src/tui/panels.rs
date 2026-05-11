//! Rendering. Pure functions that consume `&AppState` and a ratatui
//! `Frame`. Body pane is **plain wrapped text** — no markdown parser,
//! no bold/italic/code (v1.0 §4.3.1 explicitly drops the v0.1 promise).

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::state::{AppState, Modal};
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

    render_list_pane(frame, main[0], state);
    render_body_pane(frame, main[1], state);
    render_status_bar(frame, chunks[1], state, session_id);

    // Modal overlays render last so they paint on top.
    match state.modal {
        Modal::DismissReason => render_dismiss_reason(frame, area, state),
        Modal::DismissNote => render_dismiss_note(frame, area, state),
        Modal::Help => render_help(frame, area, session_id, db_path),
        Modal::Error => render_error(frame, area, state),
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
    let counter = format!(
        "({}; {} dismissed)",
        state.findings.len(),
        state.session_dismissed_count
    );
    let hint = if let Some(msg) = &state.status_message {
        msg.clone()
    } else {
        "j/k nav · d dismiss · u undo · q quit · ? help".into()
    };
    let text = format!("{hint}  {counter}  session {session_tail}");
    frame.render_widget(Paragraph::new(text), area);
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

fn render_help(frame: &mut Frame, area: Rect, session_id: &str, db_path: &str) {
    let title = "Help".to_string();
    let lines = vec![
        Line::from(""),
        Line::from("  j / ↓        next finding"),
        Line::from("  k / ↑        previous finding"),
        Line::from("  g            jump to first"),
        Line::from("  G            jump to last"),
        Line::from("  PgDn / PgUp  scroll body half-page"),
        Line::from("  d / Enter    dismiss the selected finding"),
        Line::from("  u            undo most recent in-session dismissal"),
        Line::from("  q / Esc      quit"),
        Line::from("  ?            this help"),
        Line::from(""),
        Line::from(format!(
            "  store: {} · session: {}",
            db_path,
            session_short(session_id)
        )),
        Line::from(""),
        Line::from("  (press any key to close)"),
    ];
    render_modal(frame, area, &title, lines, 70, 16);
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
