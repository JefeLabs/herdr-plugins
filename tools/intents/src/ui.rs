//! Layout and widgets: group tree, intent detail, chat, diff, status line.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

use crate::store::{Intent, State};

/// One visible row of the tree.
pub struct TreeRow<'a> {
    pub depth: usize,
    pub label: &'a str,
    pub is_group: bool,
    pub collapsed: bool,
    pub questions: usize,
    pub state: Option<State>,
    pub count: usize,
}

/// `(left tree, right pane, status line)`.
pub fn split(area: Rect) -> (Rect, Rect, Rect) {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(v[0]);
    (h[0], h[1], v[1])
}

/// Right pane when the chat is open: `(intent above, chat below)`.
pub fn split_chat(area: Rect) -> (Rect, Rect) {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);
    (v[0], v[1])
}

pub fn render_tree(rows: &[TreeRow], selected: usize, scroll: usize, area: Rect, buf: &mut Buffer) {
    let block = Block::default().borders(Borders::RIGHT);
    let inner = block.inner(area);
    block.render(area, buf);
    for (i, row) in rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(inner.height as usize)
    {
        let y = inner.y + (i - scroll) as u16;
        let mut spans = vec![Span::raw("  ".repeat(row.depth))];
        if row.is_group {
            spans.push(Span::raw(if row.collapsed { "▸ " } else { "▾ " }));
            spans.push(Span::styled(
                row.label.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            if row.count > 0 {
                spans.push(Span::styled(
                    format!(" ({})", row.count),
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }
        } else {
            let mark = match row.state {
                Some(State::Ready) => "● ",
                Some(State::Sent) => "✓ ",
                Some(State::Refining) => "◐ ",
                _ => "○ ",
            };
            spans.push(Span::raw(mark));
            spans.push(Span::raw(row.label.to_string()));
            if row.questions > 0 {
                spans.push(Span::styled(
                    format!(" ?{}", row.questions),
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }
        }
        let line = Line::from(spans);
        let rect = Rect {
            y,
            height: 1,
            ..inner
        };
        if i == selected {
            buf.set_style(rect, Style::default().add_modifier(Modifier::REVERSED));
        }
        line.render(rect, buf);
    }
}

pub fn render_detail(intent: &Intent, attachments: &[(String, u64)], area: Rect, buf: &mut Buffer) {
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!("# {}", intent.fm.title),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "kind: {}   state: {}{}{}",
                intent.fm.kind,
                intent.fm.state.label(),
                if intent.open_questions() > 0 {
                    format!("   ?{}", intent.open_questions())
                } else {
                    String::new()
                },
                intent
                    .fm
                    .sent_to
                    .as_ref()
                    .map(|s| format!("   sent → {} ({})", s.handoff, s.workspace_id))
                    .unwrap_or_default()
            ),
            Style::default().add_modifier(Modifier::DIM),
        )),
        Line::from(""),
    ];
    for l in intent.body.lines() {
        lines.push(Line::from(l.to_string()));
    }
    if !attachments.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "## Attachments",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for (name, size) in attachments {
            lines.push(Line::from(format!("{name} ({})", human(*size))));
        }
    }
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .render(area, buf);
}

pub fn human(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

/// Transcript above an input line. `running` shows a spinner on the last entry.
pub fn render_chat(
    transcript: &[(String, String)],
    input: &str,
    running: bool,
    hint: Option<&str>,
    provider: &str,
    area: Rect,
    buf: &mut Buffer,
) {
    let block = Block::default()
        .borders(Borders::TOP)
        .title(format!(" refine · {provider} "));
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height < 2 {
        return;
    }
    let body = Rect {
        height: inner.height - 1,
        ..inner
    };
    let input_row = Rect {
        y: inner.y + inner.height - 1,
        height: 1,
        ..inner
    };
    let mut lines: Vec<Line> = Vec::new();
    for (i, (role, text)) in transcript.iter().enumerate() {
        let is_last = i + 1 == transcript.len();
        let head = if role == "user" { "you" } else { provider };
        lines.push(Line::from(Span::styled(
            format!("{head}{}", if is_last && running { " …" } else { "" }),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for l in text.lines() {
            lines.push(Line::from(l.to_string()));
        }
        lines.push(Line::from(""));
    }
    // keep the tail visible
    let total = lines.len();
    let skip = total.saturating_sub(body.height as usize);
    Paragraph::new(lines.into_iter().skip(skip).collect::<Vec<_>>())
        .wrap(Wrap { trim: false })
        .render(body, buf);
    let prompt = match hint {
        Some(h) => format!("› {input}   [{h}]"),
        None => format!("› {input}"),
    };
    buf.set_style(input_row, Style::default().add_modifier(Modifier::REVERSED));
    Line::from(prompt).render(input_row, buf);
}

pub fn render_diff(old: &str, new: &str, area: Rect, buf: &mut Buffer) {
    use similar::{ChangeTag, TextDiff};
    let block = Block::default()
        .borders(Borders::TOP)
        .title(" apply proposal? Enter = yes, Esc = back ");
    let inner = block.inner(area);
    block.render(area, buf);
    let diff = TextDiff::from_lines(old, new);
    let mut lines: Vec<Line> = Vec::new();
    for change in diff.iter_all_changes() {
        let (sign, style) = match change.tag() {
            ChangeTag::Delete => (
                "-",
                Style::default().add_modifier(Modifier::DIM | Modifier::CROSSED_OUT),
            ),
            ChangeTag::Insert => ("+", Style::default().add_modifier(Modifier::BOLD)),
            ChangeTag::Equal => (" ", Style::default()),
        };
        lines.push(Line::from(Span::styled(
            format!("{sign} {}", change.value().trim_end_matches('\n')),
            style,
        )));
    }
    Paragraph::new(lines).render(inner, buf);
}

pub fn render_status(text: &str, area: Rect, buf: &mut Buffer) {
    buf.set_style(area, Style::default().add_modifier(Modifier::REVERSED));
    Line::from(text.to_string()).render(area, buf);
}

pub fn render_list(title: &str, items: &[String], selected: usize, area: Rect, buf: &mut Buffer) {
    let block = Block::default()
        .borders(Borders::TOP)
        .title(format!(" {title} "));
    let inner = block.inner(area);
    block.render(area, buf);
    for (i, item) in items.iter().enumerate().take(inner.height as usize) {
        let rect = Rect {
            y: inner.y + i as u16,
            height: 1,
            ..inner
        };
        if i == selected {
            buf.set_style(rect, Style::default().add_modifier(Modifier::REVERSED));
        }
        Line::from(format!(" {item}")).render(rect, buf);
    }
}
