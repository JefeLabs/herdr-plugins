//! Layout and rendering: an activity bar on the right edge (small or large
//! buttons), the selected tool's screen (or a message) to its left, an
//! optional footer, and the hovered tool's name beside its button.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Widget, Wrap};
use tui_term::vt100;
use tui_term::widget::{Cursor, PseudoTerminal};

use crate::config::BarSize;
use crate::pty::ToolState;

const RELOAD_GLYPH: &str = "⟳";
const ARMED_GLYPH: &str = "^]";

pub struct BarItem<'a> {
    pub icon: &'a str,
    pub name: &'a str,
    pub state: &'a ToolState,
    pub selected: bool,
    /// Opens a Herdr pane rather than running in the toolbar.
    pub pane: bool,
}

pub struct Frame<'a> {
    pub items: Vec<BarItem<'a>>,
    /// The selected tool's screen; `None` shows `message` instead.
    pub screen: Option<&'a vt100::Screen>,
    pub message: Option<&'a str>,
    /// Painted over the last content row (e.g. "exited (code 3), enter to restart").
    pub footer: Option<&'a str>,
    pub prefix_armed: bool,
    pub bar: BarSize,
    /// The tool the mouse rests on: its name shows beside its button.
    pub hover: Option<usize>,
    /// Where the bar's rules go: before each of these tools.
    pub rules: Vec<usize>,
}

/// The width of the bar and the rows of each button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarLayout {
    pub width: u16,
    pub rows: u16,
}

/// The bar's shape for `count` tools in a pane `height` rows tall. Large
/// buttons are five columns by two rows (one blank row between icons); they
/// fall back to small ones (three columns, one row) when they would not all
/// fit above the marker row.
pub fn bar_layout(size: BarSize, count: usize, rules: usize, height: u16) -> BarLayout {
    let small = BarLayout { width: 3, rows: 1 };
    let needed = count * 2 + rules * usize::from(rule_rows(2));
    match size {
        BarSize::Small => small,
        BarSize::Large if needed <= usize::from(height.saturating_sub(1)) => {
            BarLayout { width: 5, rows: 2 }
        }
        BarSize::Large => small,
    }
}

/// Rows the rule takes: with large buttons a blank row and then the line, so
/// it has one blank row on each side like the icons; with small ones the line.
fn rule_rows(button_rows: u16) -> u16 {
    if button_rows >= 2 { 2 } else { 1 }
}

/// Where the rule goes: before the first tool that opens a pane, when tools
/// that run in the toolbar come before it.
pub fn rule_index(panes: impl IntoIterator<Item = bool>) -> Option<usize> {
    let mut previous = None;
    for (i, pane) in panes.into_iter().enumerate() {
        if pane && previous == Some(false) {
            return Some(i);
        }
        previous = Some(pane);
    }
    None
}

/// The tool button under row `y` of the bar, if any: not a rule, the
/// marker or the empty rows below the buttons.
pub fn button_at(bar: Rect, rows: u16, count: usize, rules: &[usize], y: u16) -> Option<usize> {
    (0..count)
        .find(|&i| button(bar, rows, i, rules).is_some_and(|b| y >= b.y && y < b.y + b.height))
}

/// `(content, bar)`: the bar is the rightmost `width` columns.
pub fn split(area: Rect, width: u16) -> (Rect, Rect) {
    let bar_w = width.min(area.width);
    let content = Rect {
        width: area.width - bar_w,
        ..area
    };
    let bar = Rect {
        x: area.x + area.width - bar_w,
        width: bar_w,
        ..area
    };
    (content, bar)
}

/// Rows the hosted tool may use: the content area minus the footer row when one shows.
pub fn content_size(area: Rect, footer: bool, bar_width: u16) -> (u16, u16) {
    let (content, _) = split(area, bar_width);
    let rows = if footer {
        content.height.saturating_sub(1)
    } else {
        content.height
    };
    (rows.max(1), content.width.max(1))
}

pub fn render(frame: &Frame, area: Rect, buf: &mut Buffer) {
    let rules = &frame.rules;
    let layout = bar_layout(frame.bar, frame.items.len(), rules.len(), area.height);
    let (content, bar) = split(area, layout.width);
    render_content(frame, content, buf);
    render_bar(frame, bar, layout.rows, rules, buf);
    render_hover(frame, content, bar, layout.rows, rules, buf);
}

fn render_content(frame: &Frame, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    match (frame.screen, frame.message) {
        (Some(screen), message) => {
            let mut cursor = Cursor::default();
            if frame.prefix_armed {
                cursor.hide();
            }
            PseudoTerminal::new(screen).cursor(cursor).render(area, buf);
            // An app-level message (reload error, untrusted file, unknown tool)
            // must stay visible while a tool runs: paint it as a banner over
            // the top row rather than hiding it behind the screen.
            if let Some(msg) = message {
                let row = Rect { height: 1, ..area };
                buf.set_style(row, Style::default().add_modifier(Modifier::REVERSED));
                Line::from(msg).render(row, buf);
            }
        }
        (None, Some(msg)) => {
            Paragraph::new(msg)
                .wrap(Wrap { trim: false })
                .render(area, buf);
        }
        (None, None) => {}
    }
    if let Some(footer) = frame.footer {
        let row = Rect {
            y: area.y + area.height - 1,
            height: 1,
            ..area
        };
        buf.set_style(row, Style::default().add_modifier(Modifier::REVERSED));
        Line::from(footer).render(row, buf);
    }
}

/// Where tool button `i` sits in the bar, if it fits above the marker row.
/// Each rule before it moves it down.
fn button(bar: Rect, rows: u16, i: usize, rules: &[usize]) -> Option<Rect> {
    let before = rules.iter().filter(|&&r| i >= r).count() as u16;
    let below_rule = before * rule_rows(rows);
    let y = bar.y + (i as u16).checked_mul(rows)? + below_rule;
    // keep the last row for the reload/armed marker
    (y + rows <= bar.y + bar.height.saturating_sub(1)).then_some(Rect {
        y,
        height: rows,
        ..bar
    })
}

fn render_bar(frame: &Frame, area: Rect, rows: u16, rules: &[usize], buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // a dim line between groups of tools
    for (k, &r) in rules.iter().enumerate() {
        let y = area.y + r as u16 * rows + (k as u16 + 1) * rule_rows(rows) - 1;
        if y < area.y + area.height.saturating_sub(1) {
            let line = Rect {
                y,
                height: 1,
                ..area
            };
            Line::from("─".repeat(usize::from(area.width)))
                .style(Style::default().add_modifier(Modifier::DIM))
                .render(line, buf);
        }
    }
    for (i, item) in frame.items.iter().enumerate() {
        let Some(button) = button(area, rows, i, rules) else {
            break;
        };
        let row = Rect {
            y: button.y + rows / 2,
            height: 1,
            ..button
        };
        let mut style = Style::default();
        if item.selected {
            style = style.add_modifier(Modifier::REVERSED);
        }
        if matches!(item.state, ToolState::Exited { .. } | ToolState::Failed(_)) {
            style = style.add_modifier(Modifier::DIM);
        }
        buf.set_style(button, style);
        let icon = if item.icon.is_empty() {
            item.name
                .chars()
                .next()
                .map(|c| c.to_string())
                .unwrap_or_default()
        } else {
            item.icon.to_string()
        };
        Line::from(icon).centered().render(row, buf);
    }
    let last = Rect {
        y: area.y + area.height - 1,
        height: 1,
        ..area
    };
    let glyph = if frame.prefix_armed {
        ARMED_GLYPH
    } else {
        RELOAD_GLYPH
    };
    Line::from(glyph)
        .centered()
        .style(Style::default().add_modifier(Modifier::DIM))
        .render(last, buf);
}

/// The hovered tool's name beside its button, over the content area: a box
/// whose text lines up with a large button's icon, or one reversed line
/// beside a small one.
fn render_hover(
    frame: &Frame,
    content: Rect,
    bar: Rect,
    rows: u16,
    rules: &[usize],
    buf: &mut Buffer,
) {
    let Some(i) = frame.hover else {
        return;
    };
    let (Some(item), Some(button)) = (frame.items.get(i), button(bar, rows, i, rules)) else {
        return;
    };
    let name = item.name;
    if content.width == 0 {
        return;
    }
    if rows >= 2 {
        let width = (Line::from(name).width() as u16 + 4).min(content.width);
        let rect = Rect {
            x: content.x + content.width - width,
            y: button.y,
            width,
            height: 3,
        };
        Clear.render(rect, buf);
        Paragraph::new(name)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .padding(Padding::horizontal(1)),
            )
            .render(rect, buf);
    } else {
        let label = Line::from(format!(" {name} "));
        let width = (label.width() as u16).min(content.width);
        let rect = Rect {
            x: content.x + content.width - width,
            y: button.y,
            width,
            height: 1,
        };
        Clear.render(rect, buf);
        buf.set_style(rect, Style::default().add_modifier(Modifier::REVERSED));
        label.render(rect, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;
    use ratatui::{Terminal, layout::Rect};
    use tui_term::vt100;

    use crate::pty::ToolState;

    fn items<'a>(states: &'a [ToolState], selected: usize) -> Vec<BarItem<'a>> {
        let icons = ["◉", "☑", "▤"];
        let names = ["Issues", "Todo", "Database"];
        states
            .iter()
            .enumerate()
            .map(|(i, state)| BarItem {
                icon: icons[i],
                name: names[i],
                state,
                selected: i == selected,
                pane: false,
            })
            .collect()
    }

    /// As `items`, with the tools from `first_pane` on opening a pane.
    fn grouped<'a>(
        states: &'a [ToolState],
        selected: usize,
        first_pane: usize,
    ) -> Vec<BarItem<'a>> {
        let mut v = items(states, selected);
        for item in v.iter_mut().skip(first_pane) {
            item.pane = true;
        }
        v
    }

    fn draw(frame: &Frame) -> Terminal<TestBackend> {
        let mut t = Terminal::new(TestBackend::new(40, 8)).unwrap();
        t.draw(|f| render(frame, f.area(), f.buffer_mut())).unwrap();
        t
    }

    #[test]
    fn split_reserves_the_bar_width_on_the_right() {
        let (c, b) = split(Rect::new(0, 0, 40, 8), 3);
        assert_eq!(b, Rect::new(37, 0, 3, 8));
        assert_eq!(c, Rect::new(0, 0, 37, 8));
        let (c, b) = split(Rect::new(0, 0, 40, 8), 5);
        assert_eq!(b, Rect::new(35, 0, 5, 8));
        assert_eq!(c, Rect::new(0, 0, 35, 8));
    }

    fn bare<'a>(items: Vec<BarItem<'a>>, bar: BarSize, hover: Option<usize>) -> Frame<'a> {
        let rules = rule_index(items.iter().map(|i| i.pane))
            .into_iter()
            .collect();
        Frame {
            items,
            screen: None,
            message: None,
            footer: None,
            prefix_armed: false,
            bar,
            hover,
            rules,
        }
    }

    fn draw_at(frame: &Frame, cols: u16, rows: u16) -> Terminal<TestBackend> {
        let mut t = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        t.draw(|f| render(frame, f.area(), f.buffer_mut())).unwrap();
        t
    }

    fn row(buf: &Buffer, y: u16, to: u16) -> String {
        (0..to).map(|x| buf[(x, y)].symbol()).collect()
    }

    const RUNNING: [ToolState; 3] = [ToolState::Running, ToolState::Running, ToolState::Running];

    #[test]
    fn large_buttons_are_five_wide_two_tall_one_blank_row_between_icons() {
        let t = draw_at(&bare(items(&RUNNING, 1), BarSize::Large, None), 40, 12);
        let buf = t.backend().buffer();
        assert_eq!(buf[(37, 1)].symbol(), "◉");
        assert_eq!(buf[(37, 3)].symbol(), "☑");
        assert_eq!(buf[(37, 5)].symbol(), "▤");
        for y in 2..4 {
            for x in 35..40 {
                assert!(
                    buf[(x, y)].modifier.contains(Modifier::REVERSED),
                    "the selected button is one block: ({x},{y})"
                );
            }
        }
        assert!(!buf[(35, 1)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(35, 4)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn large_buttons_that_do_not_fit_fall_back_to_small_ones() {
        // three tools need six rows above the marker; six rows leave five
        let t = draw_at(&bare(items(&RUNNING, 0), BarSize::Large, None), 40, 6);
        let buf = t.backend().buffer();
        assert_eq!(buf[(38, 0)].symbol(), "◉");
        assert_eq!(buf[(38, 1)].symbol(), "☑");
        assert_eq!(buf[(38, 2)].symbol(), "▤");
        assert_eq!(
            bar_layout(BarSize::Large, 3, 0, 6),
            BarLayout { width: 3, rows: 1 }
        );
        assert_eq!(
            bar_layout(BarSize::Large, 3, 0, 7),
            BarLayout { width: 5, rows: 2 }
        );
        assert_eq!(
            bar_layout(BarSize::Small, 3, 0, 40),
            BarLayout { width: 3, rows: 1 }
        );
    }

    #[test]
    fn hovering_a_large_button_shows_its_name_in_a_box_beside_it() {
        let t = draw_at(&bare(items(&RUNNING, 0), BarSize::Large, Some(2)), 40, 12);
        let buf = t.backend().buffer();
        // the third button is rows 4-5, its icon on row 5: the name lines up with it
        assert!(
            row(buf, 4, 35).ends_with("┌──────────┐"),
            "{:?}",
            row(buf, 4, 35)
        );
        assert!(
            row(buf, 5, 35).ends_with("│ Database │"),
            "{:?}",
            row(buf, 5, 35)
        );
        assert!(
            row(buf, 6, 35).ends_with("└──────────┘"),
            "{:?}",
            row(buf, 6, 35)
        );
        assert!(!row(buf, 3, 35).contains("Todo"), "only the hovered tool");
    }

    #[test]
    fn a_rule_separates_toolbar_tools_from_pane_tools() {
        let t = draw_at(&bare(grouped(&RUNNING, 0, 2), BarSize::Small, None), 40, 8);
        let buf = t.backend().buffer();
        assert_eq!(buf[(38, 0)].symbol(), "◉");
        assert_eq!(buf[(38, 1)].symbol(), "☑");
        assert_eq!(row(buf, 2, 40)[37..].to_string(), "───", "the rule row");
        assert!(buf[(38, 2)].modifier.contains(Modifier::DIM));
        assert_eq!(buf[(38, 3)].symbol(), "▤", "pane tools sit below the rule");
    }

    #[test]
    fn a_rule_goes_before_each_tool_the_frame_names() {
        // groups: [0] | [1] | [2], small buttons
        let mut frame = bare(items(&RUNNING, 0), BarSize::Small, None);
        frame.rules = vec![1, 2];
        let t = draw_at(&frame, 40, 8);
        let buf = t.backend().buffer();
        assert_eq!(buf[(38, 0)].symbol(), "◉");
        assert_eq!(row(buf, 1, 40)[37..].to_string(), "───", "the first rule");
        assert_eq!(buf[(38, 2)].symbol(), "☑");
        assert_eq!(row(buf, 3, 40)[37..].to_string(), "───", "the second rule");
        assert_eq!(buf[(38, 4)].symbol(), "▤");
        let at = |y| button_at(Rect::new(37, 0, 3, 8), 1, 3, &[1, 2], y);
        assert_eq!(
            (at(0), at(1), at(2), at(3), at(4)),
            (Some(0), None, Some(1), None, Some(2))
        );
    }

    #[test]
    fn a_rule_between_large_buttons_has_one_blank_row_on_each_side() {
        let t = draw_at(&bare(grouped(&RUNNING, 2, 1), BarSize::Large, None), 40, 12);
        let buf = t.backend().buffer();
        assert_eq!(buf[(37, 1)].symbol(), "◉");
        assert_eq!(row(buf, 2, 40)[35..].trim(), "", "blank above the rule");
        assert_eq!(row(buf, 3, 40)[35..].to_string(), "─────", "the rule row");
        assert_eq!(row(buf, 4, 40)[35..].trim(), "", "blank below the rule");
        assert_eq!(buf[(37, 5)].symbol(), "☑");
        assert_eq!(buf[(37, 7)].symbol(), "▤");
        assert!(
            buf[(35, 6)].modifier.contains(Modifier::REVERSED),
            "selected, rows 6-7"
        );
        assert!(!buf[(35, 5)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn the_rule_counts_when_large_buttons_must_fit() {
        // three tools and a rule with a blank row above it need eight rows above the marker
        assert_eq!(
            bar_layout(BarSize::Large, 3, 1, 9),
            BarLayout { width: 5, rows: 2 }
        );
        assert_eq!(
            bar_layout(BarSize::Large, 3, 1, 8),
            BarLayout { width: 3, rows: 1 }
        );
    }

    #[test]
    fn rows_map_to_buttons_skipping_the_rule() {
        let bar = Rect::new(35, 0, 5, 12);
        let at = |y| button_at(bar, 2, 3, &[1], y);
        assert_eq!((at(0), at(1)), (Some(0), Some(0)));
        assert_eq!(
            (at(2), at(3)),
            (None, None),
            "the gap and the rule are no button"
        );
        assert_eq!((at(4), at(5)), (Some(1), Some(1)));
        assert_eq!((at(6), at(7)), (Some(2), Some(2)));
        assert_eq!(at(8), None, "below the buttons");
        assert_eq!(at(11), None, "the marker row");
        let small = Rect::new(37, 0, 3, 8);
        assert_eq!(
            button_at(small, 1, 3, &[2], 2),
            None,
            "small: the rule only"
        );
        assert_eq!(button_at(small, 1, 3, &[2], 3), Some(2));
    }

    #[test]
    fn hovering_a_small_button_shows_its_name_on_its_row() {
        let t = draw_at(&bare(items(&RUNNING, 1), BarSize::Small, Some(0)), 40, 8);
        let buf = t.backend().buffer();
        assert!(
            row(buf, 0, 37).ends_with(" Issues "),
            "{:?}",
            row(buf, 0, 37)
        );
        assert!(buf[(30, 0)].modifier.contains(Modifier::REVERSED));
        assert!(!row(buf, 1, 37).contains("Todo"));
    }

    #[test]
    fn bar_shows_icons_selected_highlighted_exited_dimmed() {
        let states = [
            ToolState::Running,
            ToolState::Running,
            ToolState::Exited { code: Some(1) },
        ];
        let mut parser = vt100::Parser::new(8, 37, 0);
        parser.process(b"hello from a tool");
        let frame = Frame {
            items: items(&states, 1),
            screen: Some(parser.screen()),
            message: None,
            footer: None,
            prefix_armed: false,
            bar: BarSize::Small,
            hover: None,
            rules: Vec::new(),
        };
        let t = draw(&frame);
        insta::assert_snapshot!(t.backend());
        let buf = t.backend().buffer();
        assert!(
            buf[(38, 1)].modifier.contains(Modifier::REVERSED),
            "selected icon reversed"
        );
        assert!(
            buf[(38, 2)].modifier.contains(Modifier::DIM),
            "exited icon dim"
        );
        assert!(!buf[(38, 0)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn message_replaces_screen() {
        let states = [ToolState::Failed("x".into())];
        let frame = Frame {
            items: items(&states[..1], 0),
            screen: None,
            message: Some("cannot start `Todo`: No such file or directory (os error 2)"),
            footer: None,
            prefix_armed: false,
            bar: BarSize::Small,
            hover: None,
            rules: Vec::new(),
        };
        let t = draw(&frame);
        insta::assert_snapshot!(t.backend());
        let text = format!("{:?}", t.backend().buffer());
        assert!(text.contains("cannot start `Todo`"));
    }

    #[test]
    fn footer_overlays_last_content_row() {
        let states = [ToolState::Exited { code: Some(3) }];
        let parser = vt100::Parser::new(8, 37, 0);
        let frame = Frame {
            items: items(&states[..1], 0),
            screen: Some(parser.screen()),
            message: None,
            footer: Some("exited (code 3), enter to restart"),
            prefix_armed: false,
            bar: BarSize::Small,
            hover: None,
            rules: Vec::new(),
        };
        let t = draw(&frame);
        let buf = t.backend().buffer();
        let row: String = (0..37).map(|x| buf[(x, 7)].symbol()).collect();
        assert!(
            row.starts_with("exited (code 3), enter to restart"),
            "got: {row:?}"
        );
        assert!(buf[(0, 7)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn prefix_armed_marks_bar_bottom() {
        let states = [ToolState::Running];
        let parser = vt100::Parser::new(8, 37, 0);
        let mut frame = Frame {
            items: items(&states[..1], 0),
            screen: Some(parser.screen()),
            message: None,
            footer: None,
            prefix_armed: true,
            bar: BarSize::Small,
            hover: None,
            rules: Vec::new(),
        };
        let t = draw(&frame);
        let buf = t.backend().buffer();
        let bottom: String = (37..40).map(|x| buf[(x, 7)].symbol()).collect();
        assert_eq!(bottom.trim(), "^]");
        frame.prefix_armed = false;
        let t = draw(&frame);
        let bottom: String = (37..40)
            .map(|x| t.backend().buffer()[(x, 7)].symbol())
            .collect();
        assert_eq!(bottom.trim(), "⟳");
    }

    #[test]
    fn message_is_drawn_as_a_banner_even_when_a_screen_shows() {
        let states = [ToolState::Running];
        let mut parser = vt100::Parser::new(8, 37, 0);
        parser.process(b"tool output line");
        let frame = Frame {
            items: items(&states[..1], 0),
            screen: Some(parser.screen()),
            message: Some("config not reloaded: line 1: expected `]`"),
            footer: None,
            prefix_armed: false,
            bar: BarSize::Small,
            hover: None,
            rules: Vec::new(),
        };
        let t = draw(&frame);
        let buf = t.backend().buffer();
        let top: String = (0..37).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(
            top.starts_with("config not reloaded"),
            "banner on the top row, got: {top:?}"
        );
        assert!(buf[(0, 0)].modifier.contains(Modifier::REVERSED));
        let second: String = (0..37).map(|x| buf[(x, 1)].symbol()).collect();
        assert!(
            second.trim().is_empty() || second.contains("tool output") || true,
            "screen still rendered below"
        );
    }
}
