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
    /// An intent shown here because it affects this group (it lives elsewhere).
    pub reference: bool,
    /// For a group: intents living elsewhere that affect it or its sub-groups.
    pub refs: usize,
    /// Updated by a reconcile and not yet read: drawn with a leading `*`.
    pub unread: bool,
    /// Every acceptance criterion ticked: the only case drawn with `✓`.
    pub complete: bool,
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
            let counts = match (row.count, row.refs) {
                (0, 0) => String::new(),
                (n, 0) => format!(" ({n})"),
                (0, r) => format!(" (+{r})"),
                (n, r) => format!(" ({n} +{r})"),
            };
            if !counts.is_empty() {
                spans.push(Span::styled(
                    counts,
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }
        } else if row.reference {
            spans.push(Span::styled(
                format!("↳ {}", sanitize(row.label)),
                Style::default().add_modifier(Modifier::DIM),
            ));
        } else if row.broken {
            spans.push(Span::styled(
                format!("! {}", sanitize(row.label)),
                Style::default().add_modifier(Modifier::DIM),
            ));
        } else {
            // ✓ is kept for complete; a sent intent reads as an arrow
            let mark = match row.state {
                _ if row.complete => "✓ ",
                Some(State::Sent) => "→ ",
                Some(State::Ready) => "● ",
                Some(State::Refining) => "◐ ",
                _ => "○ ",
            };
            spans.push(Span::raw(mark));
            if row.unread {
                spans.push(Span::styled(
                    "* ",
                    Style::default().add_modifier(Modifier::BOLD),
                ));
            }
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
                sanitize(crate::config::kind_label(&intent.fm.kind)),
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

    fn checklist_text(items: &[crate::store::Criterion], selected: usize, h: u16) -> String {
        grouped_checklist_text(items, &[], selected, h)
    }

    fn grouped_checklist_text(
        items: &[crate::store::Criterion],
        groups: &[(usize, String)],
        selected: usize,
        h: u16,
    ) -> String {
        let mut t = Terminal::new(TestBackend::new(40, h)).unwrap();
        t.draw(|f| {
            render_checklist(
                items,
                "Acceptance criteria",
                "none",
                true,
                selected,
                &[],
                &[],
                groups,
                f.area(),
                f.buffer_mut(),
            )
        })
        .unwrap();
        let buf = t.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..40)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// One checklist drawn unfocused at `w`×`h`: its rows and the terminal.
    fn checklist_term(
        items: &[crate::store::Criterion],
        w: u16,
        h: u16,
    ) -> (Terminal<TestBackend>, Vec<String>) {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| {
            render_checklist(
                items,
                "Acceptance criteria",
                "none",
                false,
                0,
                &[],
                &[],
                &[],
                f.area(),
                f.buffer_mut(),
            )
        })
        .unwrap();
        let rows = (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| t.backend().buffer()[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        (t, rows)
    }

    fn modifier_at(t: &Terminal<TestBackend>, rows: &[String], needle: &str) -> Modifier {
        let y = rows
            .iter()
            .position(|r| r.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not in {rows:?}"));
        let x = rows[y][..rows[y].find(needle).unwrap()].chars().count();
        t.backend().buffer()[(x as u16, y as u16)].modifier
    }

    #[test]
    fn criteria_render_markdown_like_the_description() {
        let items = [crate::store::Criterion {
            text: "Opens **Schedule a Meeting** via `POST /api/x`.".into(),
            done: false,
        }];
        let (t, rows) = checklist_term(&items, 60, 4);
        assert!(
            rows.iter()
                .any(|r| r == "[ ] Opens Schedule a Meeting via POST /api/x."),
            "{rows:?}"
        );
        assert!(modifier_at(&t, &rows, "Schedule").contains(Modifier::BOLD));
        assert!(modifier_at(&t, &rows, "POST").contains(Modifier::REVERSED));
        assert!(!modifier_at(&t, &rows, "Opens").contains(Modifier::BOLD));
    }

    #[test]
    fn styles_carry_across_a_wrapped_line() {
        let items = [crate::store::Criterion {
            text: "Plain words then **a bold run that wraps onto the next line** and plain again"
                .into(),
            done: false,
        }];
        let (t, rows) = checklist_term(&items, 34, 6);
        let all = rows.join("\n");
        assert!(!all.contains("**"), "{all}");
        // the bold run starts on one line and continues on the next
        assert!(
            modifier_at(&t, &rows, "a bold").contains(Modifier::BOLD),
            "{all}"
        );
        assert!(
            modifier_at(&t, &rows, "next").contains(Modifier::BOLD),
            "{all}"
        );
        assert!(
            !modifier_at(&t, &rows, "plain again").contains(Modifier::BOLD),
            "{all}"
        );
    }

    #[test]
    fn preconditions_render_markdown_too() {
        let it =
            intent_with(&SECTIONS_BODY.replace("- [x] pre one", "- [x] pre **one** with `code`"));
        let (_t, rows) = detail(&it, Pane::Preconditions, Pane::Preconditions, 80, 24);
        assert!(
            rows.iter().any(|r| r == "[x] pre one with code"),
            "{rows:?}"
        );
    }

    #[test]
    fn a_long_checklist_scrolls_to_the_selection_and_says_what_is_hidden() {
        let items: Vec<crate::store::Criterion> = (0..10)
            .map(|i| crate::store::Criterion {
                text: format!("item {i}"),
                done: i < 2,
            })
            .collect();
        // a top border and five lines inside: four items and the hint
        let top = checklist_text(&items, 0, 6);
        assert!(
            top.contains("[x] item 0") && top.contains("[ ] item 3"),
            "{top}"
        );
        assert!(!top.contains("item 4"), "{top}");
        assert!(top.contains("↓ 6 more"), "{top}");
        let low = checklist_text(&items, 8, 6);
        assert!(low.contains("item 8"), "the selection stays in view: {low}");
        assert!(low.contains("↑ 5") && low.contains("↓ 1 more"), "{low}");
        // nothing hidden, no hint
        let all = checklist_text(&items[..3], 0, 6);
        assert!(!all.contains("more"), "{all}");
    }

    fn criterion(text: &str, done: bool) -> crate::store::Criterion {
        crate::store::Criterion {
            text: text.into(),
            done,
        }
    }

    #[test]
    fn criteria_are_drawn_under_their_area_with_its_own_count() {
        let items = [
            criterion("own one", false),
            criterion("header button", true),
            criterion("modal", false),
            criterion("banner", false),
        ];
        // an area heading with nothing under it yet, then one with a criterion
        let groups = [
            (1, "org/candidates".to_string()),
            (3, "org/empty".to_string()),
            (3, "family/dashboard".to_string()),
        ];
        let text = grouped_checklist_text(&items, &groups, 0, 10);
        let lines: Vec<&str> = text
            .lines()
            .map(|l| l.trim_matches(|c: char| c == '│' || c == ' '))
            .filter(|l| !l.is_empty() && !l.contains('─'))
            .collect();
        assert_eq!(
            lines,
            vec![
                "[ ] own one",
                "org/candidates (1/2)",
                "[x] header button",
                "[ ] modal",
                "org/empty (0/0)",
                "family/dashboard (0/1)",
                "[ ] banner",
            ],
            "{text}"
        );
    }

    #[test]
    fn group_headers_take_lines_but_the_selection_stays_in_view() {
        let items: Vec<_> = (0..6)
            .map(|i| criterion(&format!("item {i}"), false))
            .collect();
        let groups = [
            (0, "a".to_string()),
            (2, "b".to_string()),
            (4, "c".to_string()),
        ];
        // five lines inside the border: four rows and the hint
        let low = grouped_checklist_text(&items, &groups, 5, 6);
        assert!(low.contains("item 5"), "{low}");
        assert!(
            low.contains("c (0/2)"),
            "the selection's header shows: {low}"
        );
        assert!(!low.contains("item 0"), "{low}");
        assert!(low.contains("↑ 3"), "{low}");
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
                reference: false,
                refs: 0,
                unread: false,
                complete: false,
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
                reference: false,
                refs: 0,
                unread: false,
                complete: false,
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
                reference: false,
                refs: 0,
                unread: false,
                complete: false,
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
    fn the_detail_names_the_kind_as_bug_feature_or_idea() {
        for (kind, shown) in [
            ("issue", "kind: bug"),
            ("enhancement", "kind: feature"),
            ("idea", "kind: idea"),
        ] {
            let it = crate::store::Intent {
                path: std::path::PathBuf::from("/x/a.md"),
                fm: serde_yaml_ng::from_str(&format!("title: T\nkind: {kind}\n")).unwrap(),
                body: String::new(),
                loaded_updated: chrono::Local::now(),
            };
            let mut t = Terminal::new(TestBackend::new(60, 6)).unwrap();
            t.draw(|f| render_detail(&it, &[], f.area(), f.buffer_mut()))
                .unwrap();
            assert!(text_of(&t).contains(shown), "{kind}: {}", text_of(&t));
        }
    }

    const SECTIONS_BODY: &str = "## Description\nThe description.\n\n## Preconditions\n- [x] pre one\n- [ ] pre two\n\n## Acceptance criteria\n- [x] crit one\n- [ ] crit two\n- [ ] crit three\n\n## Open questions\n- q one?\n  → yes\n- q two?\n\n## Artifacts\n- spec: docs/spec.md\n";

    fn intent_with(body: &str) -> crate::store::Intent {
        crate::store::Intent {
            path: std::path::PathBuf::from("/x/a.md"),
            fm: serde_yaml_ng::from_str("title: T\nkind: idea\n").unwrap(),
            body: body.into(),
            loaded_updated: chrono::Local::now(),
        }
    }

    /// The detail view drawn at `w`×`h`: its rows, and the terminal for styles.
    fn detail(
        it: &crate::store::Intent,
        focus: Pane,
        open: Pane,
        w: u16,
        h: u16,
    ) -> (Terminal<TestBackend>, Vec<String>) {
        let view = DetailView {
            intent: it,
            attachments: &[],
            focus,
            description_scroll: 0,
            preconditions_selected: 0,
            criteria_selected: 0,
            question_selected: 0,
            artifacts_selected: 0,
            found: &[],
            answering: None,
            open,
            prs: &[],
            prs_selected: 0,
            demos_selected: 0,
        };
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| render_intent(&view, f.area(), f.buffer_mut()))
            .unwrap();
        let rows = (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| t.backend().buffer()[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        (t, rows)
    }

    fn row_of(rows: &[String], needle: &str) -> usize {
        rows.iter()
            .position(|r| r.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not found:\n{}", rows.join("\n")))
    }

    const LISTS_BODY: &str = "## Description\nThe description.\n\n## Preconditions\n- [x] pre one\n\n## Acceptance criteria\n- [x] crit one\n- [ ] crit two\n\n## Open questions\n- q one?\n\n## Artifacts\n- spec: docs/spec.md\n- plan: docs/plan.md\n- pr: #5828\n- pr: #5829\n- demo: story-a (criteria: 1)\n- issue: #5833\n";

    fn lists_prs() -> Vec<PrRow> {
        use crate::progress::PrState;
        vec![
            PrRow {
                target: "#5828".into(),
                state: PrState::Merged,
                title: Some("Notify the family".into()),
                found: false,
            },
            PrRow {
                target: "#5829".into(),
                state: PrState::Open,
                title: None,
                found: false,
            },
            PrRow {
                target: "#5840".into(),
                state: PrState::Merged,
                title: Some("Fix the sign-in".into()),
                found: true,
            },
        ]
    }

    /// The detail view with linked and found pull requests, demos and artifacts.
    fn lists_detail(
        focus: Pane,
        open: Pane,
        selected: usize,
    ) -> (Terminal<TestBackend>, Vec<String>) {
        let it = intent_with(LISTS_BODY);
        let prs = lists_prs();
        let found = [
            crate::store::Artifact {
                kind: "demo".into(),
                target: "story-b".into(),
            },
            crate::store::Artifact {
                kind: "adr".into(),
                target: "docs/adr/1.md".into(),
            },
        ];
        let view = DetailView {
            intent: &it,
            attachments: &[],
            focus,
            description_scroll: 0,
            preconditions_selected: 0,
            criteria_selected: 0,
            question_selected: 0,
            artifacts_selected: selected,
            found: &found,
            answering: None,
            open,
            prs: &prs,
            prs_selected: selected,
            demos_selected: selected,
        };
        let (w, h) = (80u16, 30u16);
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| render_intent(&view, f.area(), f.buffer_mut()))
            .unwrap();
        let rows = (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| t.backend().buffer()[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        (t, rows)
    }

    #[test]
    fn pull_requests_and_demos_have_their_own_sections() {
        let (_t, rows) = lists_detail(Pane::Tree, Pane::Criteria, 0);
        let all = rows.join("\n");
        let titles = [
            "▸ Preconditions (1/1)",
            "▾ Acceptance criteria (1/2)",
            "▸ Demo Videos (1 linked, 1 found)",
            "▸ Open questions (1 open)",
            "▸ Pull requests (1/2 merged, 1 found)",
            "▸ Artifacts (3 linked, 1 found)",
        ];
        let at: Vec<usize> = titles.iter().map(|t| row_of(&rows, t)).collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]), "in this order:\n{all}");
    }

    #[test]
    fn the_pull_requests_section_shows_each_ones_state_and_title() {
        let (t, rows) = lists_detail(Pane::PullRequests, Pane::PullRequests, 1);
        let all = rows.join("\n");
        for line in [
            "merged  #5828  Notify the family",
            "open    #5829",
            "merged? #5840  Fix the sign-in   (found · L links)",
        ] {
            assert!(rows.iter().any(|r| r == line), "missing {line:?}:\n{all}");
        }
        for elsewhere in ["docs/spec.md", "story-a", "#5833"] {
            assert!(
                !all.contains(elsewhere),
                "{elsewhere:?} is not a PR:\n{all}"
            );
        }
        // the selection is the section's own
        let y = row_of(&rows, "open    #5829") as u16;
        assert!(
            t.backend().buffer()[(0, y)]
                .modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn the_demos_section_lists_linked_then_found_stories() {
        let (_t, rows) = lists_detail(Pane::Demos, Pane::Demos, 0);
        let all = rows.join("\n");
        let linked = row_of(&rows, "▶ story-a (criteria: 1)");
        let found = row_of(&rows, "▶ story-b   (found · L links)");
        assert!(linked < found, "{all}");
        assert!(
            !all.contains("#5828") && !all.contains("docs/spec.md"),
            "{all}"
        );
    }

    #[test]
    fn artifacts_keep_the_rest() {
        let (_t, rows) = lists_detail(Pane::Artifacts, Pane::Artifacts, 0);
        let all = rows.join("\n");
        for line in [
            "spec     docs/spec.md",
            "plan     docs/plan.md",
            "issue    #5833",
            "adr?     docs/adr/1.md   (found · L links)",
        ] {
            assert!(rows.iter().any(|r| r == line), "missing {line:?}:\n{all}");
        }
        for moved in ["#5828", "story-a", "story-b"] {
            assert!(
                !all.contains(moved),
                "{moved:?} has its own section:\n{all}"
            );
        }
    }

    #[test]
    fn empty_pull_request_and_demo_sections_say_so() {
        let it = intent_with(SECTIONS_BODY.replace("- spec: docs/spec.md\n", "").as_str());
        let (_t, rows) = detail(&it, Pane::Tree, Pane::PullRequests, 80, 24);
        let all = rows.join("\n");
        assert!(all.contains("▾ Pull requests (0)"), "{all}");
        assert!(all.contains("▸ Demo Videos (0)"), "{all}");
        assert!(all.contains("none yet"), "{all}");
    }

    #[test]
    fn only_the_open_section_expands_under_the_description() {
        let it = intent_with(SECTIONS_BODY);
        let (_t, rows) = detail(&it, Pane::Tree, Pane::Criteria, 80, 24);
        let all = rows.join("\n");
        // every section keeps its title and count; only the open one shows its items
        for title in [
            "▸ Preconditions (1/2)",
            "▾ Acceptance criteria (1/3)",
            "▸ Open questions (1 open)",
            "▸ Artifacts (1)",
        ] {
            assert!(all.contains(title), "missing {title:?}:\n{all}");
        }
        assert!(
            all.contains("crit one") && all.contains("crit three"),
            "{all}"
        );
        for hidden in ["pre one", "q one?", "docs/spec.md"] {
            assert!(!all.contains(hidden), "{hidden:?} is collapsed:\n{all}");
        }
        // stacked in order right under the description; the closed ones below
        // the open section sit at the bottom
        let desc = row_of(&rows, "The description.");
        assert_eq!(row_of(&rows, "▸ Preconditions"), desc + 1, "{all}");
        assert!(row_of(&rows, "▾ Acceptance criteria") > desc + 1);
        assert_eq!(row_of(&rows, "▸ Demo Videos"), 20, "{all}");
        assert_eq!(row_of(&rows, "▸ Open questions"), 21, "{all}");
        assert_eq!(row_of(&rows, "▸ Pull requests"), 22, "{all}");
        assert_eq!(row_of(&rows, "▸ Artifacts"), 23, "{all}");

        let (_t, rows) = detail(&it, Pane::Tree, Pane::Artifacts, 80, 24);
        let all = rows.join("\n");
        assert!(
            all.contains("▾ Artifacts (1)") && all.contains("docs/spec.md"),
            "{all}"
        );
        assert!(!all.contains("crit one"), "{all}");
        assert!(all.contains("▸ Acceptance criteria (1/3)"), "{all}");
    }

    #[test]
    fn the_open_section_is_highlighted_while_it_has_focus() {
        let it = intent_with(SECTIONS_BODY);
        let reversed = |t: &Terminal<TestBackend>, rows: &[String], needle: &str| {
            let y = row_of(rows, needle) as u16;
            let x = rows[y as usize].find(needle).unwrap();
            let x = rows[y as usize][..x].chars().count() as u16;
            t.backend().buffer()[(x, y)]
                .modifier
                .contains(Modifier::REVERSED)
        };
        let (t, rows) = detail(&it, Pane::Criteria, Pane::Criteria, 80, 24);
        assert!(reversed(&t, &rows, "▾ Acceptance criteria"));
        assert!(!reversed(&t, &rows, "▸ Preconditions"));
        let (t, rows) = detail(&it, Pane::Tree, Pane::Criteria, 80, 24);
        assert!(!reversed(&t, &rows, "▾ Acceptance criteria"));
    }

    #[test]
    fn a_long_description_leaves_room_for_the_open_section() {
        let long: String = (0..30).map(|i| format!("line {i}\n\n")).collect();
        let body = SECTIONS_BODY.replace("The description.\n", &long);
        let it = intent_with(&body);
        let (_t, rows) = detail(&it, Pane::Tree, Pane::Criteria, 80, 30);
        let all = rows.join("\n");
        // at most a third of the height for the description (it scrolls)
        assert!(row_of(&rows, "▸ Preconditions") <= 2 + 10, "{all}");
        assert!(all.contains("crit three"), "{all}");
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
            open: Pane::Questions,
            prs: &[],
            prs_selected: 0,
            demos_selected: 0,
        };
        let mut t = Terminal::new(TestBackend::new(50, 30)).unwrap();
        t.draw(|f| render_intent(&view, f.area(), f.buffer_mut()))
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
    Demos,
    Questions,
    PullRequests,
    Artifacts,
}

/// `wrap_text` for styled spans: the same greedy wrap, with each word keeping
/// its styles (a word may change style midway) and each space between two
/// words keeping the style it had, so a code span stays one piece.
fn wrap_spans(spans: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>> {
    let w = width.max(1);
    // words as styled chars, each with the style of the space before it
    let mut words: Vec<(Style, Vec<(char, Style)>)> = Vec::new();
    let mut word: Vec<(char, Style)> = Vec::new();
    let mut gap = Style::default();
    for sp in spans {
        for ch in sp.content.chars() {
            if ch.is_whitespace() {
                if !word.is_empty() {
                    words.push((gap, std::mem::take(&mut word)));
                }
                gap = sp.style;
            } else {
                word.push((ch, sp.style));
            }
        }
    }
    if !word.is_empty() {
        words.push((gap, word));
    }
    let mut lines: Vec<Vec<(char, Style)>> = Vec::new();
    let mut line: Vec<(char, Style)> = Vec::new();
    for (gap, word) in words {
        let mut rest = &word[..];
        while rest.len() > w {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            lines.push(rest[..w].to_vec());
            rest = &rest[w..];
        }
        if !line.is_empty() && line.len() + 1 + rest.len() > w {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push((' ', gap));
        }
        line.extend_from_slice(rest);
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    // back to spans: one per run of a single style
    lines
        .into_iter()
        .map(|chars| {
            let mut out: Vec<Span<'static>> = Vec::new();
            for (ch, st) in chars {
                match out.last_mut() {
                    Some(last) if last.style == st => last.content.to_mut().push(ch),
                    _ => out.push(Span::styled(ch.to_string(), st)),
                }
            }
            out
        })
        .collect()
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
    /// The section open under the description (an accordion): the others
    /// show only their title line.
    pub open: Pane,
    /// Pull requests, linked first then found by the repo search.
    pub prs: &'a [PrRow],
    pub prs_selected: usize,
    pub demos_selected: usize,
}

/// A pull request in the detail view: linked by a `pr:` line or found by the
/// repo search, with the state and title `gh` gave.
#[derive(Debug, Clone, PartialEq)]
pub struct PrRow {
    pub target: String,
    pub state: crate::progress::PrState,
    pub title: Option<String>,
    pub found: bool,
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
pub fn render_intent(v: &DetailView, area: Rect, buf: &mut Buffer) {
    use crate::store::{criteria, questions};
    let it = v.intent;
    let qs = questions(&it.body);
    let cs = criteria(&it.body);
    let ps = crate::store::preconditions(&it.body);
    let arts = crate::store::artifacts(&it.body);
    let fresh = crate::store::new_criteria(it);
    let desc = description_text(&it.body);
    // the description takes what it needs, up to a third of the height (it
    // scrolls); below it the four sections are an accordion: the open one
    // gets the rest, the others a title line each
    let desc_rows = wrap_text(&desc, area.width as usize).len() as u16;
    let desc_h = (desc_rows + 1).clamp(2, (area.height / 3).max(4));
    let sections = [
        Pane::Preconditions,
        Pane::Criteria,
        Pane::Demos,
        Pane::Questions,
        Pane::PullRequests,
        Pane::Artifacts,
    ];
    let mut constraints = vec![Constraint::Length(2), Constraint::Length(desc_h)];
    constraints.extend(sections.iter().map(|s| {
        if *s == v.open {
            Constraint::Min(2)
        } else {
            Constraint::Length(1)
        }
    }));
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
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
                sanitize(crate::config::kind_label(&it.fm.kind)),
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

    let block = pane_block("Description", v.focus == Pane::Description);
    let inner = block.inner(layout[1]);
    block.render(layout[1], buf);
    Paragraph::new(crate::markdown::render_markdown(&desc))
        .wrap(Wrap { trim: false })
        .scroll((v.description_scroll, 0))
        .render(inner, buf);

    let marker = |s: Pane| if s == v.open { "▾" } else { "▸" };
    let names = [
        format!("{} Preconditions", marker(Pane::Preconditions)),
        format!("{} Acceptance criteria", marker(Pane::Criteria)),
    ];
    // the list sections' rows: pull requests and demos have their own,
    // everything else stays in Artifacts
    let in_section = |pane: Pane, found: bool| -> Vec<&crate::store::Artifact> {
        let from: &[crate::store::Artifact] = if found { v.found } else { &arts };
        from.iter()
            .filter(|a| list_section(&a.kind) == pane)
            .collect()
    };
    let demo_rows: Vec<ItemRow> = [false, true]
        .into_iter()
        .flat_map(|found| {
            in_section(Pane::Demos, found)
                .into_iter()
                .map(move |a| ItemRow {
                    tag: "▶".into(),
                    text: a.target.clone(),
                    found,
                })
        })
        .collect();
    let artifact_rows: Vec<ItemRow> = [false, true]
        .into_iter()
        .flat_map(|found| {
            in_section(Pane::Artifacts, found)
                .into_iter()
                .map(move |a| ItemRow {
                    tag: if found {
                        format!("{}?", a.kind)
                    } else {
                        a.kind.clone()
                    },
                    text: a.target.clone(),
                    found,
                })
        })
        .collect();
    let pr_rows: Vec<ItemRow> = v
        .prs
        .iter()
        .map(|p| ItemRow {
            tag: format!(
                "{}{}",
                pr_state_label(p.state),
                if p.found { "?" } else { "" }
            ),
            text: match &p.title {
                Some(t) => format!("{}  {t}", p.target),
                None => p.target.clone(),
            },
            found: p.found,
        })
        .collect();
    let count_title = |name: &str, rows: &[ItemRow]| {
        let found = rows.iter().filter(|r| r.found).count();
        let linked = rows.len() - found;
        if found == 0 {
            format!("{name} ({linked})")
        } else {
            format!("{name} ({linked} linked, {found} found)")
        }
    };
    let pr_title = {
        let linked: Vec<&PrRow> = v.prs.iter().filter(|p| !p.found).collect();
        let found = v.prs.len() - linked.len();
        let merged = linked
            .iter()
            .filter(|p| p.state == crate::progress::PrState::Merged)
            .count();
        match (linked.len(), found) {
            (0, 0) => "Pull requests (0)".to_string(),
            (0, f) => format!("Pull requests (0 linked, {f} found)"),
            (n, 0) => format!("Pull requests ({merged}/{n} merged)"),
            (n, f) => format!("Pull requests ({merged}/{n} merged, {f} found)"),
        }
    };
    let titles = [
        checklist_title(&names[0], &ps, &[]),
        checklist_title(&names[1], &cs, &fresh),
        format!(
            "{} {}",
            marker(Pane::Demos),
            count_title("Demo Videos", &demo_rows)
        ),
        format!(
            "{} Open questions ({} open)",
            marker(Pane::Questions),
            it.open_questions()
        ),
        format!("{} {pr_title}", marker(Pane::PullRequests)),
        format!(
            "{} {}",
            marker(Pane::Artifacts),
            count_title("Artifacts", &artifact_rows)
        ),
    ];
    for (i, section) in sections.into_iter().enumerate() {
        let rect = layout[2 + i];
        if section != v.open {
            pane_block(&titles[i], false).render(rect, buf);
            continue;
        }
        match section {
            Pane::Preconditions => render_checklist(
                &ps,
                &names[0],
                "none — what must already be true for this to be worth building?",
                v.focus == Pane::Preconditions,
                v.preconditions_selected,
                &[],
                &[],
                &[],
                rect,
                buf,
            ),
            Pane::Criteria => {
                // ▶ on the criteria an app-demo shows
                let demoed: Vec<usize> = arts
                    .iter()
                    .filter(|a| a.kind.eq_ignore_ascii_case("demo"))
                    .flat_map(|a| crate::demos::demo_target(&a.target).1)
                    .filter_map(|n| n.checked_sub(1))
                    .collect();
                render_checklist(
                    &cs,
                    &names[1],
                    "none yet — add `- [ ]` items in the editor or let polish propose them",
                    v.focus == Pane::Criteria,
                    v.criteria_selected,
                    &demoed,
                    &fresh,
                    &crate::store::criteria_groups(&it.body),
                    rect,
                    buf,
                )
            }
            Pane::Demos => render_items(
                &titles[i],
                v.focus == Pane::Demos,
                v.demos_selected,
                &demo_rows,
                2,
                "none yet — app-demos stories that show the criteria working (a adds one: story-id (criteria: 1, 4))",
                rect,
                buf,
            ),
            Pane::Questions => render_questions(v, &qs, &titles[i], rect, buf),
            Pane::PullRequests => render_items(
                &titles[i],
                v.focus == Pane::PullRequests,
                v.prs_selected,
                &pr_rows,
                8,
                "none yet — agents add a `pr:` line for each pull request they open (a adds one by hand)",
                rect,
                buf,
            ),
            _ => render_items(
                &titles[i],
                v.focus == Pane::Artifacts,
                v.artifacts_selected,
                &artifact_rows,
                9,
                "none yet — specs, plans, ADRs and tickets the agents produce land here (a adds one by hand)",
                rect,
                buf,
            ),
        }
    }
}

/// Which list section an artifact line belongs to: pull requests and demos
/// have their own, everything else stays in Artifacts.
pub fn list_section(kind: &str) -> Pane {
    if kind.eq_ignore_ascii_case("pr") {
        Pane::PullRequests
    } else if kind.eq_ignore_ascii_case("demo") {
        Pane::Demos
    } else {
        Pane::Artifacts
    }
}

fn pr_state_label(state: crate::progress::PrState) -> &'static str {
    use crate::progress::PrState;
    match state {
        PrState::Merged => "merged",
        PrState::Open => "open",
        PrState::Closed => "closed",
        PrState::Unknown => "…",
    }
}

/// One row of a list section: a short tag column, its text, and whether it
/// was found rather than linked.
struct ItemRow {
    tag: String,
    text: String,
    found: bool,
}

/// A list section (Demos, Pull requests, Artifacts): its rows under the
/// title, scrolled to the selection; found rows are dim and say `L` links
/// them.
#[allow(clippy::too_many_arguments)]
fn render_items(
    title: &str,
    focused: bool,
    selected: usize,
    rows: &[ItemRow],
    tag_width: usize,
    empty: &str,
    area: Rect,
    buf: &mut Buffer,
) {
    let block = pane_block(title, focused);
    let inner = block.inner(area);
    block.render(area, buf);
    let dim = Style::default().add_modifier(Modifier::DIM);
    if rows.is_empty() {
        Paragraph::new(Span::styled(empty, dim))
            .wrap(Wrap { trim: false })
            .render(inner, buf);
        return;
    }
    let skip = if selected >= inner.height as usize {
        selected + 1 - inner.height as usize
    } else {
        0
    };
    for (k, (i, row)) in rows.iter().enumerate().skip(skip).enumerate() {
        let y = inner.y + k as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let rect = Rect {
            y,
            height: 1,
            ..inner
        };
        if focused && i == selected {
            buf.set_style(rect, Style::default().add_modifier(Modifier::REVERSED));
        }
        let (text_style, hint) = if row.found {
            (dim, "   (found · L links)")
        } else {
            (Style::default(), "")
        };
        Line::from(vec![
            Span::styled(format!("{:<tag_width$}", sanitize(&row.tag)), dim),
            Span::styled(sanitize(&row.text), text_style),
            Span::styled(hint, dim),
        ])
        .render(rect, buf);
    }
}

/// The Open questions section: each question wrapped, its answer under it.
fn render_questions(
    v: &DetailView,
    qs: &[crate::store::Question],
    title: &str,
    area: Rect,
    buf: &mut Buffer,
) {
    let block = pane_block(title, v.focus == Pane::Questions);
    let inner = block.inner(area);
    block.render(area, buf);
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
/// A checklist's title with its counts: `Name (done/total)`, plus how many
/// are new when some are.
fn checklist_title(name: &str, items: &[crate::store::Criterion], fresh: &[usize]) -> String {
    let counts = format!(
        "{}/{}",
        items.iter().filter(|c| c.done).count(),
        items.len()
    );
    if fresh.is_empty() {
        format!("{name} ({counts})")
    } else {
        format!("{name} ({counts} · {} new)", fresh.len())
    }
}

/// A checklist pane (Preconditions, Acceptance criteria). `marked` holds the
/// (0-based) items an app-demo shows; they get `▶` after the checkbox.
/// `fresh` holds items added by someone else and not yet acknowledged: drawn
/// bold with a `+`, and counted in the title. `groups` are sub-headings (an
/// area each) with the index of their first item: drawn as bold headers with
/// their own counts.
#[allow(clippy::too_many_arguments)]
fn render_checklist(
    items: &[crate::store::Criterion],
    title: &str,
    empty: &str,
    focused: bool,
    selected: usize,
    marked: &[usize],
    fresh: &[usize],
    groups: &[(usize, String)],
    area: Rect,
    buf: &mut Buffer,
) {
    let block = pane_block(&checklist_title(title, items, fresh), focused);
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
    // each item's wrapped lines; scroll so the selection is in view and say
    // how many items are hidden above and below
    let width = (inner.width as usize).saturating_sub(4);
    let wrapped: Vec<Vec<Vec<Span<'static>>>> = items
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let extra = 2 * (usize::from(marked.contains(&i)) + usize::from(fresh.contains(&i)));
            wrap_spans(
                &crate::markdown::inline_spans(&c.text),
                width.saturating_sub(extra),
            )
        })
        .collect();
    // group headers sit above their first item and count toward its height
    let headers_at = |i: usize| groups.iter().filter(|(first, _)| *first == i).count();
    let heights: Vec<usize> = (0..items.len())
        .map(|i| wrapped[i].len() + headers_at(i))
        .collect();
    let height = inner.height as usize;
    let total: usize = heights.iter().sum();
    let overflow = total > height;
    let room = if overflow {
        height.saturating_sub(1).max(1)
    } else {
        height
    };
    let sel = if focused {
        selected.min(items.len() - 1)
    } else {
        0
    };
    let mut first = 0;
    while first < sel && heights[first..=sel].iter().sum::<usize>() > room {
        first += 1;
    }
    let mut y = inner.y;
    let mut used = 0usize;
    let mut last_shown = first;
    'items: for (i, c) in items.iter().enumerate().skip(first) {
        for (g, (start, name)) in groups.iter().enumerate().filter(|(_, (s, _))| *s == i) {
            if used >= room {
                break 'items;
            }
            // a group ends where the next one starts (an empty one ends at once)
            let end = groups.get(g + 1).map_or(items.len(), |(s, _)| *s);
            let members = &items[*start..end.max(*start)];
            let done = members.iter().filter(|c| c.done).count();
            Line::styled(
                format!("{} ({done}/{})", sanitize(name), members.len()),
                Style::default().add_modifier(Modifier::BOLD),
            )
            .render(
                Rect {
                    y,
                    height: 1,
                    ..inner
                },
                buf,
            );
            y += 1;
            used += 1;
        }
        let box_ = if c.done { "[x] " } else { "[ ] " };
        for (j, piece) in wrapped[i].iter().enumerate() {
            if used >= room {
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
            let tags = format!(
                "{}{}",
                if fresh.contains(&i) { "+ " } else { "" },
                if marked.contains(&i) { "▶ " } else { "" }
            );
            let lead = if j == 0 {
                format!("{box_}{tags}")
            } else {
                " ".repeat(4 + tags.chars().count())
            };
            let style = if fresh.contains(&i) {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let mut line = vec![Span::styled(lead, style)];
            line.extend(
                piece
                    .iter()
                    .map(|sp| Span::styled(sp.content.clone(), sp.style.patch(style))),
            );
            Line::from(line).render(rect, buf);
            y += 1;
            used += 1;
        }
        last_shown = i;
    }
    if overflow {
        let above = first;
        let below = items.len() - 1 - last_shown;
        let mut parts = Vec::new();
        if above > 0 {
            parts.push(format!("↑ {above}"));
        }
        if below > 0 {
            parts.push(format!("↓ {below}"));
        }
        if !parts.is_empty() {
            let rect = Rect {
                y: inner.y + inner.height - 1,
                height: 1,
                ..inner
            };
            Line::styled(
                format!("{} more (j/k)", parts.join(" · ")),
                Style::default().add_modifier(Modifier::DIM),
            )
            .render(rect, buf);
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

/// One row of the full-screen intent list.
pub struct ListRow<'a> {
    /// `raw`, `?2`, `ready`, `sent`.
    pub status: String,
    /// Delivery progress once sent.
    pub progress: Option<crate::progress::Progress>,
    /// The sent agent's live state (`working`, `idle`, `needs you`, `done`,
    /// `closed`); `None` before sending.
    pub agent: Option<String>,
    /// Time since the intent was sent, e.g. `15h ago`.
    pub running: Option<String>,
    pub title: &'a str,
    /// Updated by a reconcile and not yet read: the title gets a leading `*`.
    pub unread: bool,
    /// The group path in the tree, e.g. `skoolscout/org`.
    pub group: String,
    /// The sent agent's worktree path and slot, once it is in one.
    pub worktree: Option<String>,
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

const LIST_HINTS: &str = "v tree · S sort · f filter · s send · g agent";
const STATUS_W: usize = 6;
const WHEN_W: usize = 12;
const AGENT_W: usize = 11;
const RUNNING_W: usize = 7;
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

/// The list layout: a header, then one card per intent, each opening
/// with a horizontal rule:
///
/// ```text
/// ────────────────────────────────────────
///  Title
///  group path
///  TIMESTAMP │ STATUS │ AGENT │ SENT AGO │ STAGES label
///  worktree path │ slot N          (once the agent is in a worktree)
///
///  description, up to two wrapped lines
/// ```
///
/// Returns where each drawn card landed, `(row index, first line, end line)`,
/// so a mouse click can be mapped back to a card.
pub fn render_intent_list(
    rows: &[ListRow],
    selected: usize,
    header: &str,
    empty: &str,
    area: Rect,
    buf: &mut Buffer,
) -> Vec<(usize, u16, u16)> {
    let mut hits = Vec::new();
    if area.height < 3 || area.width < 20 {
        return hits;
    }
    let dim = Style::default().add_modifier(Modifier::DIM);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let w = area.width as usize;
    // the header wins; the key hints (also on the status line) show only if
    // they fit beside it
    let hints_w = Span::raw(LIST_HINTS).width();
    let head = fit(header, w);
    let head_w = Span::raw(head.as_str()).width();
    let show_hints = head_w + 2 + hints_w <= w;
    let gap = if show_hints { w - head_w - hints_w } else { 0 };
    Line::from(vec![
        Span::styled(head, bold),
        Span::raw(" ".repeat(gap)),
        Span::styled(if show_hints { LIST_HINTS } else { "" }, dim),
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
        Paragraph::new(Span::styled(empty.to_string(), dim)).render(body, buf);
        return hits;
    }
    let inner_w = w.saturating_sub(2);
    let descs: Vec<Vec<String>> = rows
        .iter()
        .map(|r| card_description(r.description, inner_w))
        .collect();
    // rule + title + group + fields + [worktree] + blank + description
    let height = |i: usize| 1 + 3 + usize::from(rows[i].worktree.is_some()) + 1 + descs[i].len();
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
        let rule_y = y;
        put(Line::styled("─".repeat(w), dim), &mut y);
        let card_top = y;
        // the title, and its path in the tree under it
        let star = if r.unread { "* " } else { "" };
        put(
            Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!("{star}{}", fit(r.title, inner_w.saturating_sub(star.len()))),
                    bold,
                ),
            ]),
            &mut y,
        );
        put(
            Line::from(vec![
                Span::raw(" "),
                Span::styled(fit(&r.group, inner_w), dim),
            ]),
            &mut y,
        );

        // metadata: timestamp │ status │ agent │ running │ progress
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
        match r.running.as_deref() {
            Some(t) => fields.push(Span::raw(format!("{:<RUNNING_W$}", fit(t, RUNNING_W)))),
            None => fields.push(Span::styled(format!("{:<RUNNING_W$}", "—"), dim)),
        }
        fields.push(sep());
        // progress gets what is left: bar and label (cut with …), else the
        // bar alone, else the label cut to fit; never part of the bar
        let room = w.saturating_sub(fields.iter().map(Span::width).sum());
        match &r.progress {
            Some(p) if p.complete => fields.push(Span::raw(fit("✓ complete", room))),
            Some(p) => {
                let bar_w = Span::raw(crate::progress::stage_bar(p)).width();
                if room >= bar_w + 2 {
                    fields.extend(bar_spans(p));
                    let label_w = (room - bar_w - 1).min(24);
                    fields.push(Span::raw(format!(" {}", fit(&p.label, label_w))));
                } else if room >= bar_w {
                    fields.extend(bar_spans(p));
                } else {
                    fields.push(Span::raw(fit(&p.label, room)));
                }
            }
            None => fields.push(Span::styled(fit("—", room), dim)),
        }
        put(Line::from(fields), &mut y);

        // the agent's worktree path under the metadata, once it is in one;
        // then a blank line before the description
        if let Some(w) = &r.worktree {
            put(
                Line::from(vec![Span::raw(" "), Span::styled(fit(w, inner_w), dim)]),
                &mut y,
            );
        }
        put(Line::raw(""), &mut y);

        // description
        for d in &descs[i] {
            put(
                Line::from(vec![Span::raw(" "), Span::styled(d.clone(), dim)]),
                &mut y,
            );
        }
        hits.push((i, rule_y, y.min(bottom)));
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
    hits
}

/// The progress bar as one plain run: the kinds are carried by the glyphs,
/// since Herdr does not keep colour or dim on block characters.
fn bar_spans(p: &crate::progress::Progress) -> Vec<Span<'static>> {
    vec![Span::raw(crate::progress::stage_bar(p))]
}
