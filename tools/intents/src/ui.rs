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
    /// A file that failed to parse; `label` is the file name.
    pub broken: bool,
}

/// Control characters other than tab/newline become a visible placeholder so
/// intent text or an agent reply cannot drive the terminal.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '·'
            } else {
                c
            }
        })
        .collect()
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
                sanitize(row.label),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            if row.count > 0 {
                spans.push(Span::styled(
                    format!(" ({})", row.count),
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }
        } else if row.broken {
            spans.push(Span::styled(
                format!("! {}", sanitize(row.label)),
                Style::default().add_modifier(Modifier::DIM),
            ));
        } else {
            let mark = match row.state {
                Some(State::Ready) => "● ",
                Some(State::Sent) => "✓ ",
                Some(State::Refining) => "◐ ",
                _ => "○ ",
            };
            spans.push(Span::raw(mark));
            spans.push(Span::raw(sanitize(row.label)));
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
            format!("# {}", sanitize(&intent.fm.title)),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "kind: {}   state: {}{}{}",
                sanitize(&intent.fm.kind),
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
    for l in sanitize(&intent.body).lines() {
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

/// Rough wrapped-line count for `text` in `width` columns.
fn wrapped_lines(text: &str, width: usize) -> usize {
    let w = width.max(1);
    text.lines()
        .map(|l| l.chars().count().max(1).div_ceil(w))
        .sum::<usize>()
        .max(1)
}

/// Transcript above an input line. `running` shows a spinner on the last entry;
/// `scroll` is how many wrapped lines back from the tail to show.
#[allow(clippy::too_many_arguments)] // a plain render function over the chat state's fields
pub fn render_chat(
    transcript: &[(String, String)],
    input: &str,
    running: bool,
    hint: Option<&str>,
    provider: &str,
    scroll: u16,
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
    let mut wrapped_total = 0usize;
    let width = body.width as usize;
    for (i, (role, text)) in transcript.iter().enumerate() {
        let is_last = i + 1 == transcript.len();
        let head = if role == "user" { "you" } else { provider };
        let head_line = format!("{head}{}", if is_last && running { " …" } else { "" });
        lines.push(Line::from(Span::styled(
            head_line,
            Style::default().add_modifier(Modifier::BOLD),
        )));
        wrapped_total += 1;
        let clean = sanitize(text);
        for l in clean.lines() {
            lines.push(Line::from(l.to_string()));
        }
        wrapped_total += wrapped_lines(&clean, width);
        lines.push(Line::from(""));
        wrapped_total += 1;
    }
    // keep the tail visible, measured in wrapped lines, then back off by `scroll`
    let visible = body.height as usize;
    let max_scroll = wrapped_total.saturating_sub(visible);
    let offset = max_scroll.saturating_sub(scroll as usize);
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((offset as u16, 0))
        .render(body, buf);
    let prompt = sanitize(&match hint {
        Some(h) => format!("› {input}   [{h}]"),
        None => format!("› {input}"),
    });
    buf.set_style(input_row, Style::default().add_modifier(Modifier::REVERSED));
    Line::from(prompt).render(input_row, buf);
}

/// Unified diff with context folding; `scroll` is the first visible line.
pub fn render_diff(old: &str, new: &str, scroll: u16, area: Rect, buf: &mut Buffer) {
    use similar::{ChangeTag, TextDiff};
    let diff = TextDiff::from_lines(old, new);
    let (mut added, mut removed) = (0usize, 0usize);
    let mut lines: Vec<Line> = Vec::new();
    for group in diff.grouped_ops(2) {
        for op in group {
            for change in diff.iter_changes(&op) {
                let (sign, style) = match change.tag() {
                    ChangeTag::Delete => {
                        removed += 1;
                        (
                            "-",
                            Style::default().add_modifier(Modifier::DIM | Modifier::CROSSED_OUT),
                        )
                    }
                    ChangeTag::Insert => {
                        added += 1;
                        ("+", Style::default().add_modifier(Modifier::BOLD))
                    }
                    ChangeTag::Equal => (" ", Style::default()),
                };
                lines.push(Line::from(Span::styled(
                    format!("{sign} {}", sanitize(change.value().trim_end_matches('\n'))),
                    style,
                )));
            }
        }
        lines.push(Line::from(Span::styled(
            "  ⋯",
            Style::default().add_modifier(Modifier::DIM),
        )));
    }
    let title = format!(
        " apply proposal? +{added} −{removed} · Enter = yes, Esc = back, PgUp/PgDn scroll "
    );
    let block = Block::default().borders(Borders::TOP).title(title);
    let inner = block.inner(area);
    block.render(area, buf);
    Paragraph::new(lines).scroll((scroll, 0)).render(inner, buf);
}

pub fn render_status(text: &str, area: Rect, buf: &mut Buffer) {
    buf.set_style(area, Style::default().add_modifier(Modifier::REVERSED));
    Line::from(sanitize(text)).render(area, buf);
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
        Line::from(format!(" {}", sanitize(item))).render(rect, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn text_of(t: &Terminal<TestBackend>) -> String {
        format!("{:?}", t.backend().buffer())
    }

    #[test]
    fn titles_and_broken_rows_are_sanitised() {
        let rows = vec![
            TreeRow {
                depth: 0,
                label: "evil\x1b]0;x\x07group",
                is_group: true,
                collapsed: false,
                questions: 0,
                state: None,
                count: 0,
                broken: false,
            },
            TreeRow {
                depth: 0,
                label: "bad\x1b[2Jfile.md",
                is_group: false,
                collapsed: false,
                questions: 0,
                state: None,
                count: 0,
                broken: true,
            },
            TreeRow {
                depth: 0,
                label: "ti\x1btle",
                is_group: false,
                collapsed: false,
                questions: 0,
                state: Some(State::Raw),
                count: 0,
                broken: false,
            },
        ];
        let mut t = Terminal::new(TestBackend::new(40, 5)).unwrap();
        t.draw(|f| render_tree(&rows, 0, 0, f.area(), f.buffer_mut()))
            .unwrap();
        let text = text_of(&t);
        assert!(
            !text.contains('\u{1b}') && !text.contains('\u{7}'),
            "raw controls in tree: {text}"
        );
    }

    #[test]
    fn detail_header_and_status_are_sanitised() {
        let mut it = crate::store::Intent {
            path: std::path::PathBuf::from("/x/a.md"),
            fm: serde_yaml_ng::from_str("title: \"t\\e]52;c;evil\\a\"\nkind: \"k\\e[2J\"\n")
                .unwrap(),
            body: String::new(),
            loaded_updated: chrono::Local::now(),
        };
        it.fm.title = "t\x1b]52;c;evil\x07".into();
        it.fm.kind = "k\x1b[2J".into();
        let mut t = Terminal::new(TestBackend::new(60, 6)).unwrap();
        t.draw(|f| render_detail(&it, &[], f.area(), f.buffer_mut()))
            .unwrap();
        assert!(!text_of(&t).contains('\u{1b}'), "raw ESC in detail");
        let mut t = Terminal::new(TestBackend::new(60, 1)).unwrap();
        t.draw(|f| render_status("msg \x1b[31mred", f.area(), f.buffer_mut()))
            .unwrap();
        assert!(!text_of(&t).contains('\u{1b}'), "raw ESC in status");
    }
}
