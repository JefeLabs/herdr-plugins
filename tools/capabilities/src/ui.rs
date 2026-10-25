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
        if role == "user" {
            for l in clean.lines() {
                lines.push(Line::from(l.to_string()));
            }
            wrapped_total += wrapped_lines(&clean, width);
        } else {
            // agent replies are Markdown; render them
            let rendered = crate::markdown::render_markdown(&clean);
            let w = width.max(1);
            wrapped_total += rendered
                .iter()
                .map(|l| l.to_string().chars().count().max(1).div_ceil(w))
                .sum::<usize>()
                .max(1);
            lines.extend(rendered);
        }
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
    #[test]
    fn long_questions_wrap_so_they_can_be_read_in_full() {
        let long = "Is `Pending` a generic status shared across all folders, and would we want a distinct Skipped state instead of reusing it everywhere?";
        let body = format!(
            "## Description\nx\n\n## Open questions\n- {long}\n  → a long answer that also needs to wrap across the narrow pane width to be legible\n- short?\n"
        );
        let it = crate::store::Intent {
            path: std::path::PathBuf::from("/x/a.md"),
            fm: serde_yaml_ng::from_str("title: T\nkind: idea\n").unwrap(),
            body,
            loaded_updated: chrono::Local::now(),
        };
        let view = DetailView {
            intent: &it,
            attachments: &[],
            focus: Pane::Questions,
            description_scroll: 0,
            preconditions_selected: 0,
            criteria_selected: 0,
            question_selected: 0,
            artifacts_selected: 0,
            found: &[],
            answering: None,
        };
        let mut t = Terminal::new(TestBackend::new(50, 30)).unwrap();
        t.draw(|f| render_capability(&view, f.area(), f.buffer_mut()))
            .unwrap();
        let rows: Vec<String> = (0..30)
            .map(|y| {
                (0..50)
                    .map(|x| t.backend().buffer()[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        let joined = rows.join("\n");
        assert!(
            joined.contains("reusing it"),
            "tail of the long question must be visible:\n{joined}"
        );
        assert!(
            joined.contains("legible"),
            "tail of the answer must be visible:\n{joined}"
        );
        assert!(
            joined.contains("? short?"),
            "the next question still shows:\n{joined}"
        );
        assert!(rows.iter().all(|r| r.chars().count() <= 50));
        // the whole selected question is highlighted, not just its first line
        let first = rows
            .iter()
            .position(|r| r.contains("✓ Is `Pending`"))
            .unwrap_or_else(|| panic!("selected question row not found:\n{joined}"));
        for y in first..first + 2 {
            assert!(
                t.backend().buffer()[(2, y as u16)]
                    .modifier
                    .contains(Modifier::REVERSED),
                "row {y} of the selected question is highlighted"
            );
        }
    }
}

/// Which of the three detail panes has keyboard focus (or the tree).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Tree,
    Description,
    Preconditions,
    Criteria,
    Questions,
    Artifacts,
}

/// Greedy word wrap to `width` columns; a word longer than the width is split.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let w = width.max(1);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        let mut len = 0usize;
        for word in para.split_whitespace() {
            let mut rest = word;
            while rest.chars().count() > w {
                if len > 0 {
                    out.push(std::mem::take(&mut line));
                    len = 0;
                }
                let cut: String = rest.chars().take(w).collect();
                out.push(cut.clone());
                rest = &rest[cut.len()..];
            }
            let wl = rest.chars().count();
            if len > 0 && len + 1 + wl > w {
                out.push(std::mem::take(&mut line));
                len = 0;
            }
            if len > 0 {
                line.push(' ');
                len += 1;
            }
            line.push_str(rest);
            len += wl;
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Everything the three-pane detail view needs from the app.
pub struct DetailView<'a> {
    pub intent: &'a Intent,
    pub attachments: &'a [(String, u64)],
    pub focus: Pane,
    pub description_scroll: u16,
    pub preconditions_selected: usize,
    pub criteria_selected: usize,
    pub question_selected: usize,
    pub artifacts_selected: usize,
    /// Artifacts found by scanning the repo but not yet linked; listed after the linked ones.
    pub found: &'a [crate::store::Artifact],
    /// `(question index, text so far)` while an answer is being typed.
    pub answering: Option<(usize, &'a str)>,
}

fn pane_block(title: &str, focused: bool) -> Block<'static> {
    let style = if focused {
        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().add_modifier(Modifier::DIM)
    };
    Block::default()
        .borders(Borders::TOP)
        .title(Span::styled(format!(" {title} "), style))
}

/// Header, then Description / Acceptance criteria / Open questions as three
/// separately scrollable, separately focusable panes.
pub fn render_capability(v: &DetailView, area: Rect, buf: &mut Buffer) {
    use crate::store::{criteria, questions};
    let it = v.intent;
    let qs = questions(&it.body);
    let cs = criteria(&it.body);
    let text_width = (area.width as usize).saturating_sub(2); // after the "? " prefix
    let q_rows: u16 = qs
        .iter()
        .map(|q| {
            wrap_text(&q.text, text_width).len()
                + q.answer
                    .as_ref()
                    .map(|a| wrap_text(a, text_width.saturating_sub(2)).len())
                    .unwrap_or(0)
        })
        .sum::<usize>() as u16
        + if v.answering.is_some() { 2 } else { 0 };
    let ps = crate::store::preconditions(&it.body);
    let rows_of = |items: &[crate::store::Criterion]| {
        items
            .iter()
            .map(|c| wrap_text(&c.text, text_width.saturating_sub(2)).len())
            .sum::<usize>() as u16
    };
    let c_rows = rows_of(&cs);
    let p_rows = rows_of(&ps);
    let arts = crate::store::artifacts(&it.body);
    let third = (area.height / 3).max(3);
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length((p_rows + 1).clamp(2, third.min(8))),
            Constraint::Length((c_rows + 1).clamp(2, third.min(8))),
            Constraint::Length((q_rows + 1).clamp(2, third + 2)),
            Constraint::Length(((arts.len() + v.found.len()) as u16 + 1).clamp(2, 8)),
        ])
        .split(area);

    // header
    let open = it.open_questions();
    let header = vec![
        Line::from(Span::styled(
            format!("# {}", sanitize(&it.fm.title)),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "kind: {}   state: {}{}{}{}",
                sanitize(&it.fm.kind),
                it.fm.state.label(),
                if open > 0 {
                    format!("   ?{open}")
                } else {
                    String::new()
                },
                it.fm
                    .sent_to
                    .as_ref()
                    .map(|s| format!("   sent → {} ({})", s.handoff, s.workspace_id))
                    .unwrap_or_default(),
                if v.attachments.is_empty() {
                    String::new()
                } else {
                    format!("   {} attachment(s)", v.attachments.len())
                }
            ),
            Style::default().add_modifier(Modifier::DIM),
        )),
    ];
    Paragraph::new(header).render(layout[0], buf);

    // description: everything outside the two list sections
    // preconditions
    render_checklist(
        &ps,
        "Preconditions",
        "none — what must already be true for this to be worth building?",
        v.focus == Pane::Preconditions,
        v.preconditions_selected,
        layout[2],
        buf,
    );

    let block = pane_block("Description", v.focus == Pane::Description);
    let inner = block.inner(layout[1]);
    block.render(layout[1], buf);
    let desc = description_text(&it.body);
    Paragraph::new(crate::markdown::render_markdown(&desc))
        .wrap(Wrap { trim: false })
        .scroll((v.description_scroll, 0))
        .render(inner, buf);

    // criteria
    let block = pane_block(
        &format!(
            "Acceptance criteria ({}/{})",
            cs.iter().filter(|c| c.done).count(),
            cs.len()
        ),
        v.focus == Pane::Criteria,
    );
    let inner = block.inner(layout[3]);
    block.render(layout[3], buf);
    if cs.is_empty() {
        Paragraph::new(Span::styled(
            "none yet — add `- [ ]` items in the editor or let polish propose them",
            Style::default().add_modifier(Modifier::DIM),
        ))
        .render(inner, buf);
    }
    let mut y = inner.y;
    'crit: for (i, c) in cs.iter().enumerate() {
        let box_ = if c.done { "[x] " } else { "[ ] " };
        for (j, piece) in wrap_text(&sanitize(&c.text), (inner.width as usize).saturating_sub(4))
            .into_iter()
            .enumerate()
        {
            if y >= inner.y + inner.height {
                break 'crit;
            }
            let rect = Rect {
                y,
                height: 1,
                ..inner
            };
            if v.focus == Pane::Criteria && i == v.criteria_selected {
                buf.set_style(rect, Style::default().add_modifier(Modifier::REVERSED));
            }
            Line::from(format!("{}{piece}", if j == 0 { box_ } else { "    " })).render(rect, buf);
            y += 1;
        }
    }

    // questions
    let block = pane_block(
        &format!("Open questions ({} open)", open),
        v.focus == Pane::Questions,
    );
    let inner = block.inner(layout[4]);
    block.render(layout[4], buf);
    if qs.is_empty() {
        Paragraph::new(Span::styled(
            "none",
            Style::default().add_modifier(Modifier::DIM),
        ))
        .render(inner, buf);
    }
    // one entry per wrapped line, tagged with its question index
    let mut rows: Vec<(usize, Line)> = Vec::new();
    let qw = (inner.width as usize).saturating_sub(2);
    for (i, q) in qs.iter().enumerate() {
        let mark = if q.answer.is_some() { "✓" } else { "?" };
        for (j, piece) in wrap_text(&sanitize(&q.text), qw).into_iter().enumerate() {
            let prefix = if j == 0 {
                format!("{mark} ")
            } else {
                "  ".to_string()
            };
            rows.push((i, Line::from(format!("{prefix}{piece}"))));
        }
        if let Some((idx, text)) = v.answering
            && idx == i
        {
            rows.push((
                i,
                Line::from(Span::styled(
                    format!("  › {}▏", sanitize(text)),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
            ));
            rows.push((
                i,
                Line::from(Span::styled(
                    "    Enter saves · Esc cancels",
                    Style::default().add_modifier(Modifier::DIM),
                )),
            ));
        } else if let Some(a) = &q.answer {
            for (j, piece) in wrap_text(&sanitize(a), qw.saturating_sub(2))
                .into_iter()
                .enumerate()
            {
                let prefix = if j == 0 { "  → " } else { "    " };
                rows.push((
                    i,
                    Line::from(Span::styled(
                        format!("{prefix}{piece}"),
                        Style::default().add_modifier(Modifier::DIM),
                    )),
                ));
            }
        }
    }
    // keep the selected question's first row visible
    let first_sel = rows
        .iter()
        .position(|(i, _)| *i == v.question_selected)
        .unwrap_or(0);
    let visible = inner.height as usize;
    let skip = if first_sel >= visible {
        first_sel + 1 - visible
    } else {
        0
    };
    for (k, (i, line)) in rows.into_iter().skip(skip).enumerate() {
        let y = inner.y + k as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let rect = Rect {
            y,
            height: 1,
            ..inner
        };
        if v.focus == Pane::Questions && i == v.question_selected && v.answering.is_none() {
            buf.set_style(rect, Style::default().add_modifier(Modifier::REVERSED));
        }
        line.render(rect, buf);
    }

    // artifacts: linked first, then those found in the repo (dim, "found")
    let title = if v.found.is_empty() {
        format!("Artifacts ({})", arts.len())
    } else {
        format!("Artifacts ({} linked, {} found)", arts.len(), v.found.len())
    };
    let block = pane_block(&title, v.focus == Pane::Artifacts);
    let inner = block.inner(layout[5]);
    block.render(layout[5], buf);
    let all: Vec<(&crate::store::Artifact, bool)> = arts
        .iter()
        .map(|a| (a, false))
        .chain(v.found.iter().map(|a| (a, true)))
        .collect();
    if all.is_empty() {
        Paragraph::new(Span::styled("none yet — specs, plans, ADRs and tickets the agents produce land here (a adds one by hand)", Style::default().add_modifier(Modifier::DIM))).wrap(Wrap { trim: false }).render(inner, buf);
    }
    let skip = if v.artifacts_selected >= inner.height as usize {
        v.artifacts_selected + 1 - inner.height as usize
    } else {
        0
    };
    for (k, (i, (a, is_found))) in all.iter().enumerate().skip(skip).enumerate() {
        let y = inner.y + k as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let rect = Rect {
            y,
            height: 1,
            ..inner
        };
        if v.focus == Pane::Artifacts && i == v.artifacts_selected {
            buf.set_style(rect, Style::default().add_modifier(Modifier::REVERSED));
        }
        let (tag, target_style, hint) = if *is_found {
            (
                format!("{:<9}", format!("{}?", sanitize(&a.kind))),
                Style::default().add_modifier(Modifier::DIM),
                "   (found · L links)",
            )
        } else {
            (format!("{:<9}", sanitize(&a.kind)), Style::default(), "")
        };
        Line::from(vec![
            Span::styled(tag, Style::default().add_modifier(Modifier::DIM)),
            Span::styled(sanitize(&a.target), target_style),
            Span::styled(hint, Style::default().add_modifier(Modifier::DIM)),
        ])
        .render(rect, buf);
    }
}

/// Read-only viewer for a Markdown artifact.
pub fn render_reader(
    title: &str,
    lines: &[Line<'static>],
    scroll: u16,
    area: Rect,
    buf: &mut Buffer,
) {
    let block = Block::default().borders(Borders::TOP).title(format!(
        " {} · Esc back · j/k PgUp/PgDn scroll ",
        sanitize(title)
    ));
    let inner = block.inner(area);
    block.render(area, buf);
    Paragraph::new(lines.to_vec())
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0))
        .render(inner, buf);
}

/// A titled checklist pane with wrapped items; the selected one is highlighted when focused.
fn render_checklist(
    items: &[crate::store::Criterion],
    title: &str,
    empty: &str,
    focused: bool,
    selected: usize,
    area: Rect,
    buf: &mut Buffer,
) {
    let block = pane_block(
        &format!(
            "{title} ({}/{})",
            items.iter().filter(|c| c.done).count(),
            items.len()
        ),
        focused,
    );
    let inner = block.inner(area);
    block.render(area, buf);
    if items.is_empty() {
        Paragraph::new(Span::styled(
            empty,
            Style::default().add_modifier(Modifier::DIM),
        ))
        .render(inner, buf);
        return;
    }
    let mut y = inner.y;
    'items: for (i, c) in items.iter().enumerate() {
        let box_ = if c.done { "[x] " } else { "[ ] " };
        for (j, piece) in wrap_text(&sanitize(&c.text), (inner.width as usize).saturating_sub(4))
            .into_iter()
            .enumerate()
        {
            if y >= inner.y + inner.height {
                break 'items;
            }
            let rect = Rect {
                y,
                height: 1,
                ..inner
            };
            if focused && i == selected {
                buf.set_style(rect, Style::default().add_modifier(Modifier::REVERSED));
            }
            Line::from(format!("{}{piece}", if j == 0 { box_ } else { "    " })).render(rect, buf);
            y += 1;
        }
    }
}

/// The body without the list sections.
pub fn description_text(body: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for line in body.lines() {
        if line.starts_with("## ") {
            let h = line.trim();
            skipping = h.eq_ignore_ascii_case("## Acceptance criteria")
                || h.eq_ignore_ascii_case("## Open questions")
                || h.eq_ignore_ascii_case("## Preconditions")
                || h.eq_ignore_ascii_case("## Artifacts");
            if skipping {
                continue;
            }
            if h.eq_ignore_ascii_case("## Description") {
                continue; // the pane title already says it
            }
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

/// One row of the full-screen capability list.
pub struct ListRow<'a> {
    /// `raw`, `?2`, `ready`, `sent`.
    pub status: String,
    /// `(fill 0..=10, label, complete)` once sent.
    pub progress: Option<(f32, String, bool)>,
    /// The sent agent's live state (`working`, `idle`, `needs you`, `done`,
    /// `closed`); `None` before sending.
    pub agent: Option<String>,
    pub title: &'a str,
    pub when: String,
    pub description: &'a str,
}

/// `text` cut to `max` display columns, ending in `…` when cut.
pub fn fit(text: &str, max: usize) -> String {
    let text = sanitize(text).replace(['\n', '\t'], " ");
    if Span::raw(text.as_str()).width() <= max {
        return text;
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = Span::raw(c.to_string()).width();
        if used + w + 1 > max {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

const LIST_HINTS: &str = "v tree · S sort · s send · g agent";
const STATUS_W: usize = 6;
const BAR_W: usize = 10;
const WHEN_W: usize = 12;
const AGENT_W: usize = 11;
/// Description lines shown on a card at most.
const DESC_LINES: usize = 2;

/// The description as it appears on a card: wrapped to `width`, at most
/// `DESC_LINES` lines, the last ending in `…` when more was cut.
fn card_description(text: &str, width: usize) -> Vec<String> {
    if text.trim().is_empty() {
        return vec!["(no description yet)".to_string()];
    }
    let lines = wrap_text(&sanitize(text), width.max(1));
    let mut out: Vec<String> = lines.iter().take(DESC_LINES).cloned().collect();
    if lines.len() > DESC_LINES
        && let Some(last) = out.last_mut()
    {
        let cut = fit(last, width.saturating_sub(1));
        *last = format!("{}…", cut.trim_end_matches('…').trim_end());
    }
    out
}

/// The list layout: a header, then one card per capability, each opening
/// with a horizontal rule:
///
/// ```text
/// ────────────────────────────────────────
///  Title
///  TIMESTAMP │ STATUS │ AGENT │ PROGRESS
///  description, up to two wrapped lines
/// ```
pub fn render_capability_list(
    rows: &[ListRow],
    selected: usize,
    header: &str,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.height < 3 || area.width < 20 {
        return;
    }
    let dim = Style::default().add_modifier(Modifier::DIM);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let w = area.width as usize;
    let hints_w = Span::raw(LIST_HINTS).width();
    let head = fit(header, w.saturating_sub(hints_w + 2));
    let gap = w.saturating_sub(Span::raw(head.as_str()).width() + hints_w);
    Line::from(vec![
        Span::styled(head, bold),
        Span::raw(" ".repeat(gap)),
        Span::styled(if gap > 0 { LIST_HINTS } else { "" }, dim),
    ])
    .render(Rect { height: 1, ..area }, buf);
    let body = Rect {
        y: area.y + 1,
        height: area.height - 1,
        ..area
    };
    if rows.is_empty() {
        Line::styled("─".repeat(w), dim).render(Rect { height: 1, ..body }, buf);
        let body = Rect {
            y: body.y + 1,
            height: body.height.saturating_sub(1),
            ..body
        };
        Paragraph::new(Span::styled(
            "no capabilities yet: v back to the tree, then a adds one",
            dim,
        ))
        .render(body, buf);
        return;
    }
    let inner_w = w.saturating_sub(2);
    let descs: Vec<Vec<String>> = rows
        .iter()
        .map(|r| card_description(r.description, inner_w))
        .collect();
    // rule + title + fields rows + description
    let height = |i: usize| 1 + 2 + descs[i].len();
    // first card shown: the selected card must fit completely
    let room = body.height as usize;
    let mut first = 0;
    while first < selected && (first..=selected).map(height).sum::<usize>() > room {
        first += 1;
    }
    let bottom = body.y + body.height;
    let sep = || Span::styled(" │ ", Style::default().add_modifier(Modifier::DIM));
    let mut y = body.y;
    for (i, r) in rows.iter().enumerate().skip(first) {
        if y >= bottom {
            break;
        }
        let mut put = |line: Line, y: &mut u16| {
            if *y < bottom {
                line.render(
                    Rect {
                        y: *y,
                        height: 1,
                        ..body
                    },
                    buf,
                );
            }
            *y += 1;
        };
        put(Line::styled("─".repeat(w), dim), &mut y);
        let card_top = y;
        // row one: the title alone
        put(
            Line::from(vec![
                Span::raw(" "),
                Span::styled(fit(r.title, inner_w), bold),
            ]),
            &mut y,
        );

        // row two: timestamp │ status │ agent │ progress
        let mut fields = vec![
            Span::raw(" "),
            Span::styled(format!("{:<WHEN_W$}", fit(&r.when, WHEN_W)), dim),
            sep(),
            Span::raw(format!("{:<STATUS_W$}", fit(&r.status, STATUS_W))),
            sep(),
        ];
        match r.agent.as_deref() {
            Some("needs you") => fields.push(Span::styled(
                format!("{:<AGENT_W$}", "! needs you"),
                bold.fg(ratatui::style::Color::Yellow),
            )),
            Some(a) => fields.push(Span::raw(format!("{:<AGENT_W$}", fit(a, AGENT_W)))),
            None => fields.push(Span::styled(format!("{:<AGENT_W$}", "—"), dim)),
        }
        fields.push(sep());
        match &r.progress {
            Some((_, _, true)) => fields.push(Span::raw("✓ complete")),
            Some((fill, label, false)) => {
                fields.push(Span::raw(crate::progress::bar(*fill, BAR_W)));
                fields.push(Span::raw(format!(" {}", fit(label, 24))));
            }
            None => fields.push(Span::styled("—", dim)),
        }
        put(Line::from(fields), &mut y);

        // description
        for d in &descs[i] {
            put(
                Line::from(vec![Span::raw(" "), Span::styled(d.clone(), dim)]),
                &mut y,
            );
        }
        if i == selected {
            let h = y.min(bottom).saturating_sub(card_top);
            buf.set_style(
                Rect {
                    y: card_top,
                    height: h,
                    ..body
                },
                Style::default().add_modifier(Modifier::REVERSED),
            );
        }
    }
}
