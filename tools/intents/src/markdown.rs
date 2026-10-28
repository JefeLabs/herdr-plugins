//! Markdown to ratatui lines for the view panes: headings bold, bullets as
//! dots, bold/italic/code spans styled, code blocks dimmed, text sanitised.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::ui::sanitize;

pub fn render_markdown(text: &str) -> Vec<Line<'static>> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(text, opts);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut style = Style::default();
    let mut in_code_block = false;
    let mut list_stack: Vec<Option<u64>> = Vec::new(); // None = bullet, Some(n) = next number
    let mut pending_prefix: Option<String> = None;
    let mut heading = false;

    fn flush(lines: &mut Vec<Line<'static>>, current: &mut Vec<Span<'static>>) {
        if !current.is_empty() {
            lines.push(Line::from(std::mem::take(current)));
        }
    }
    fn blank(lines: &mut Vec<Line<'static>>) {
        if !lines.is_empty()
            && !lines
                .last()
                .map(|l| l.to_string().is_empty())
                .unwrap_or(true)
        {
            lines.push(Line::from(""));
        }
    }

    for ev in parser {
        match ev {
            Event::Start(Tag::Heading { .. }) => {
                blank(&mut lines);
                heading = true;
                style = style.add_modifier(Modifier::BOLD);
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(&mut lines, &mut current);
                heading = false;
                style = style.remove_modifier(Modifier::BOLD);
            }
            Event::Start(Tag::Paragraph) => {
                if list_stack.is_empty() {
                    blank(&mut lines);
                }
            }
            Event::End(TagEnd::Paragraph) => flush(&mut lines, &mut current),
            Event::Start(Tag::List(start)) => {
                if list_stack.is_empty() {
                    blank(&mut lines);
                }
                list_stack.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut lines, &mut current);
                list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut lines, &mut current);
                let depth = list_stack.len().saturating_sub(1);
                let indent = "  ".repeat(depth);
                let prefix = match list_stack.last_mut() {
                    Some(Some(n)) => {
                        let p = format!("{indent}{n}. ");
                        *n += 1;
                        p
                    }
                    _ => format!("{indent}• "),
                };
                pending_prefix = Some(prefix);
            }
            Event::End(TagEnd::Item) => flush(&mut lines, &mut current),
            Event::TaskListMarker(done) => {
                // replace the bullet with the checkbox
                pending_prefix =
                    pending_prefix.map(|p| p.replace("• ", if done { "[x] " } else { "[ ] " }));
            }
            Event::Start(Tag::Emphasis) => style = style.add_modifier(Modifier::ITALIC),
            Event::End(TagEnd::Emphasis) => style = style.remove_modifier(Modifier::ITALIC),
            Event::Start(Tag::Strong) => style = style.add_modifier(Modifier::BOLD),
            Event::End(TagEnd::Strong) => {
                if !heading {
                    style = style.remove_modifier(Modifier::BOLD);
                }
            }
            Event::Start(Tag::Strikethrough) => style = style.add_modifier(Modifier::CROSSED_OUT),
            Event::End(TagEnd::Strikethrough) => {
                style = style.remove_modifier(Modifier::CROSSED_OUT)
            }
            Event::Start(Tag::BlockQuote(_)) => pending_prefix = Some("│ ".to_string()),
            Event::End(TagEnd::BlockQuote(_)) => flush(&mut lines, &mut current),
            Event::Start(Tag::CodeBlock(kind)) => {
                blank(&mut lines);
                in_code_block = true;
                if let CodeBlockKind::Fenced(lang) = kind
                    && !lang.is_empty()
                {
                    lines.push(Line::from(Span::styled(
                        format!("  {lang}"),
                        Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC),
                    )));
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                flush(&mut lines, &mut current);
                in_code_block = false;
            }
            Event::Code(c) => {
                take_prefix(&mut current, &mut pending_prefix, style);
                current.push(Span::styled(
                    sanitize(&c),
                    style.add_modifier(Modifier::REVERSED),
                ));
            }
            Event::Text(t) => {
                if in_code_block {
                    for l in t.lines() {
                        lines.push(Line::from(Span::styled(
                            format!("  {}", sanitize(l)),
                            Style::default().add_modifier(Modifier::DIM),
                        )));
                    }
                } else {
                    take_prefix(&mut current, &mut pending_prefix, style);
                    current.push(Span::styled(sanitize(&t), style));
                }
            }
            Event::SoftBreak => current.push(Span::styled(" ", style)),
            Event::HardBreak => flush(&mut lines, &mut current),
            Event::Rule => {
                flush(&mut lines, &mut current);
                lines.push(Line::from(Span::styled(
                    "────────",
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
            Event::Start(Tag::Link { .. }) => style = style.add_modifier(Modifier::UNDERLINED),
            Event::End(TagEnd::Link) => style = style.remove_modifier(Modifier::UNDERLINED),
            Event::Html(h) | Event::InlineHtml(h) => {
                take_prefix(&mut current, &mut pending_prefix, style);
                current.push(Span::styled(
                    sanitize(&h),
                    style.add_modifier(Modifier::DIM),
                ));
            }
            _ => {}
        }
    }
    flush(&mut lines, &mut current);
    // drop a leading blank line
    if lines
        .first()
        .map(|l| l.to_string().is_empty())
        .unwrap_or(false)
    {
        lines.remove(0);
    }
    lines
}

/// One line of inline Markdown (a checklist item) as styled spans, the way
/// `render_markdown` styles them: bold, italic, struck through, link text
/// underlined, code reversed, markers dropped. Text that would parse as a
/// block (a heading, a list item, a quote, a rule) is shown as written.
pub fn inline_spans(text: &str) -> Vec<Span<'static>> {
    let raw = || vec![Span::raw(sanitize(text))];
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    let events: Vec<Event> = Parser::new_ext(text, opts).collect();
    if !matches!(events.first(), Some(Event::Start(Tag::Paragraph))) {
        return raw();
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut style = Style::default();
    for ev in events {
        match ev {
            Event::Start(Tag::Paragraph) | Event::End(TagEnd::Paragraph) => {}
            Event::Start(Tag::Strong) => style = style.add_modifier(Modifier::BOLD),
            Event::End(TagEnd::Strong) => style = style.remove_modifier(Modifier::BOLD),
            Event::Start(Tag::Emphasis) => style = style.add_modifier(Modifier::ITALIC),
            Event::End(TagEnd::Emphasis) => style = style.remove_modifier(Modifier::ITALIC),
            Event::Start(Tag::Strikethrough) => style = style.add_modifier(Modifier::CROSSED_OUT),
            Event::End(TagEnd::Strikethrough) => {
                style = style.remove_modifier(Modifier::CROSSED_OUT)
            }
            Event::Start(Tag::Link { .. }) => style = style.add_modifier(Modifier::UNDERLINED),
            Event::End(TagEnd::Link) => style = style.remove_modifier(Modifier::UNDERLINED),
            Event::Code(c) => spans.push(Span::styled(
                sanitize(&c),
                style.add_modifier(Modifier::REVERSED),
            )),
            Event::Text(t) | Event::InlineHtml(t) => spans.push(Span::styled(sanitize(&t), style)),
            Event::SoftBreak | Event::HardBreak => spans.push(Span::styled(" ", style)),
            // anything block-level (or an image) is not a single line of text
            _ => return raw(),
        }
    }
    spans
}

fn take_prefix(current: &mut Vec<Span<'static>>, pending: &mut Option<String>, style: Style) {
    if let Some(p) = pending.take() {
        current.push(Span::styled(
            p,
            style.remove_modifier(Modifier::BOLD | Modifier::ITALIC),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn spans(line: &Line) -> Vec<(String, Modifier)> {
        line.spans
            .iter()
            .map(|s| (s.content.to_string(), s.style.add_modifier))
            .collect()
    }

    #[test]
    fn headings_are_bold_and_lose_their_hashes() {
        let lines = render_markdown("# Title\n\n## Sub\n");
        let texts: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        assert_eq!(texts[0], "Title");
        assert!(spans(&lines[0])[0].1.contains(Modifier::BOLD));
        assert!(texts.iter().any(|t| t == "Sub"));
        assert!(!texts.iter().any(|t| t.contains('#')));
    }

    #[test]
    fn bullets_and_inline_styles() {
        let lines = render_markdown("- plain **bold** and *it* and `code`\n1. first\n");
        let first = &lines[0];
        let s = spans(first);
        assert!(
            first.to_string().starts_with("• plain "),
            "{:?}",
            first.to_string()
        );
        assert!(
            s.iter()
                .any(|(t, m)| t == "bold" && m.contains(Modifier::BOLD))
        );
        assert!(
            s.iter()
                .any(|(t, m)| t == "it" && m.contains(Modifier::ITALIC))
        );
        assert!(
            s.iter()
                .any(|(t, m)| t == "code" && m.contains(Modifier::REVERSED))
        );
        assert!(
            lines.iter().any(|l| l.to_string().starts_with("1. first")),
            "{:?}",
            lines.iter().map(|l| l.to_string()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn code_blocks_are_dim_and_kept_verbatim() {
        let lines = render_markdown("text\n\n```bash\nmake test\n```\n");
        let code = lines
            .iter()
            .find(|l| l.to_string().contains("make test"))
            .unwrap();
        assert!(spans(code)[0].1.contains(Modifier::DIM));
        assert_eq!(code.to_string().trim(), "make test");
    }

    #[test]
    fn paragraphs_are_separated_and_text_is_sanitised() {
        let lines = render_markdown("one\n\ntwo\x1b[2J\n");
        let texts: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        assert_eq!(
            texts,
            ["one", "", "two·[2J"],
            "ESC becomes a dot, the rest stays visible"
        );
    }

    #[test]
    fn task_list_items_keep_their_boxes() {
        let lines = render_markdown("- [ ] todo\n- [x] done\n");
        assert!(lines[0].to_string().starts_with("[ ] todo"), "{}", lines[0]);
        assert!(lines[1].to_string().starts_with("[x] done"), "{}", lines[1]);
    }

    fn inline(text: &str) -> Vec<(String, Modifier)> {
        inline_spans(text)
            .iter()
            .map(|s| (s.content.to_string(), s.style.add_modifier))
            .collect()
    }

    #[test]
    fn inline_spans_style_bold_italic_and_code_without_their_markers() {
        let got = inline("Opens **Schedule a Meeting** via `POST /x` and *maybe*");
        let text: String = got.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, "Opens Schedule a Meeting via POST /x and maybe");
        let style_of = |needle: &str| {
            got.iter()
                .find(|(t, _)| t == needle)
                .unwrap_or_else(|| panic!("no span {needle:?} in {got:?}"))
                .1
        };
        assert!(style_of("Schedule a Meeting").contains(Modifier::BOLD));
        assert!(style_of("POST /x").contains(Modifier::REVERSED));
        assert!(style_of("maybe").contains(Modifier::ITALIC));
        assert!(style_of("Opens ").is_empty());
    }

    #[test]
    fn inline_spans_leave_block_markers_as_text() {
        for raw in ["1. first", "# a heading", "- an item", "> quoted", "---"] {
            let text: String = inline(raw).into_iter().map(|(t, _)| t).collect();
            assert_eq!(text, raw, "{raw:?} is shown as written");
        }
        let text: String = inline("#5834 is open")
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(text, "#5834 is open");
    }
}
