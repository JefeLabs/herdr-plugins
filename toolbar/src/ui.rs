//! Layout and rendering: a 3-column activity bar on the right edge, the
//! selected tool's screen (or a message) to its left, an optional footer.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget, Wrap};
use tui_term::vt100;
use tui_term::widget::{Cursor, PseudoTerminal};

use crate::pty::ToolState;

pub const BAR_WIDTH: u16 = 3;
const RELOAD_GLYPH: &str = "⟳";
const ARMED_GLYPH: &str = "^]";

pub struct BarItem<'a> {
    pub icon: &'a str,
    pub name: &'a str,
    pub state: &'a ToolState,
    pub selected: bool,
}

pub struct Frame<'a> {
    pub items: Vec<BarItem<'a>>,
    /// The selected tool's screen; `None` shows `message` instead.
    pub screen: Option<&'a vt100::Screen>,
    pub message: Option<&'a str>,
    /// Painted over the last content row (e.g. "exited (code 3), enter to restart").
    pub footer: Option<&'a str>,
    pub prefix_armed: bool,
}

/// `(content, bar)`: the bar is the rightmost `BAR_WIDTH` columns.
pub fn split(area: Rect) -> (Rect, Rect) {
    let bar_w = BAR_WIDTH.min(area.width);
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
pub fn content_size(area: Rect, footer: bool) -> (u16, u16) {
    let (content, _) = split(area);
    let rows = if footer {
        content.height.saturating_sub(1)
    } else {
        content.height
    };
    (rows.max(1), content.width.max(1))
}

pub fn render(frame: &Frame, area: Rect, buf: &mut Buffer) {
    let (content, bar) = split(area);
    render_content(frame, content, buf);
    render_bar(frame, bar, buf);
}

fn render_content(frame: &Frame, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    match (frame.screen, frame.message) {
        (Some(screen), _) => {
            let mut cursor = Cursor::default();
            if frame.prefix_armed {
                cursor.hide();
            }
            PseudoTerminal::new(screen).cursor(cursor).render(area, buf);
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

fn render_bar(frame: &Frame, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    for (i, item) in frame.items.iter().enumerate() {
        let y = area.y + i as u16;
        if y >= area.y + area.height.saturating_sub(1) {
            break; // keep the last row for the reload/armed marker
        }
        let row = Rect {
            y,
            height: 1,
            ..area
        };
        let mut style = Style::default();
        if item.selected {
            style = style.add_modifier(Modifier::REVERSED);
        }
        if matches!(item.state, ToolState::Exited { .. } | ToolState::Failed(_)) {
            style = style.add_modifier(Modifier::DIM);
        }
        buf.set_style(row, style);
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
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
            })
            .collect()
    }

    fn draw(frame: &Frame) -> Terminal<TestBackend> {
        let mut t = Terminal::new(TestBackend::new(40, 8)).unwrap();
        t.draw(|f| render(frame, f.area(), f.buffer_mut())).unwrap();
        t
    }

    #[test]
    fn split_reserves_three_columns_on_the_right() {
        let (c, b) = split(Rect::new(0, 0, 40, 8));
        assert_eq!(b, Rect::new(37, 0, 3, 8));
        assert_eq!(c, Rect::new(0, 0, 37, 8));
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
}
