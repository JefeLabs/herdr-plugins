//! A small text area: lines, cursor, shift-selection, scrolling, rendering.
//! Positions are `(row, col)` in characters, not bytes.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

pub type Pos = (usize, usize);

#[derive(Debug, Clone, Default)]
pub struct TextArea {
    lines: Vec<String>,
    cursor: Pos,
    anchor: Option<Pos>,
    scroll: usize,
    /// Preserve a trailing newline on round trip.
    trailing_newline: bool,
}

impl TextArea {
    pub fn from_text(s: &str) -> Self {
        let trailing_newline = s.ends_with('\n');
        let mut lines: Vec<String> = s.split('\n').map(str::to_string).collect();
        if trailing_newline {
            lines.pop();
        }
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self {
            lines,
            cursor: (0, 0),
            anchor: None,
            scroll: 0,
            trailing_newline,
        }
    }

    pub fn text(&self) -> String {
        let mut t = self.lines.join("\n");
        if self.trailing_newline {
            t.push('\n');
        }
        t
    }

    pub fn cursor(&self) -> Pos {
        self.cursor
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn replace_all(&mut self, s: &str) {
        *self = Self::from_text(s);
    }

    fn line_len(&self, row: usize) -> usize {
        self.lines.get(row).map(|l| l.chars().count()).unwrap_or(0)
    }

    fn byte_idx(line: &str, col: usize) -> usize {
        line.char_indices()
            .nth(col)
            .map(|(i, _)| i)
            .unwrap_or(line.len())
    }

    fn ordered_selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        if a == self.cursor {
            return None;
        }
        Some(if a < self.cursor {
            (a, self.cursor)
        } else {
            (self.cursor, a)
        })
    }

    pub fn selection(&self) -> Option<String> {
        let (s, e) = self.ordered_selection()?;
        let mut out = String::new();
        for row in s.0..=e.0 {
            let line = &self.lines[row];
            let from = if row == s.0 {
                Self::byte_idx(line, s.1)
            } else {
                0
            };
            let to = if row == e.0 {
                Self::byte_idx(line, e.1)
            } else {
                line.len()
            };
            out.push_str(&line[from..to]);
            if row != e.0 {
                out.push('\n');
            }
        }
        Some(out)
    }

    fn delete_selection(&mut self) -> bool {
        let Some((s, e)) = self.ordered_selection() else {
            return false;
        };
        let head = {
            let l = &self.lines[s.0];
            l[..Self::byte_idx(l, s.1)].to_string()
        };
        let tail = {
            let l = &self.lines[e.0];
            l[Self::byte_idx(l, e.1)..].to_string()
        };
        self.lines.drain(s.0..=e.0);
        self.lines.insert(s.0, head + &tail);
        self.cursor = s;
        self.anchor = None;
        true
    }

    fn insert_char(&mut self, c: char) {
        self.delete_selection();
        let (r, col) = self.cursor;
        let idx = Self::byte_idx(&self.lines[r], col);
        self.lines[r].insert(idx, c);
        self.cursor.1 += 1;
    }

    fn insert_newline(&mut self) {
        self.delete_selection();
        let (r, col) = self.cursor;
        let idx = Self::byte_idx(&self.lines[r], col);
        let rest = self.lines[r].split_off(idx);
        self.lines.insert(r + 1, rest);
        self.cursor = (r + 1, 0);
    }

    fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        let (r, col) = self.cursor;
        if col > 0 {
            let idx = Self::byte_idx(&self.lines[r], col - 1);
            self.lines[r].remove(idx);
            self.cursor.1 -= 1;
        } else if r > 0 {
            let line = self.lines.remove(r);
            let prev_len = self.line_len(r - 1);
            self.lines[r - 1].push_str(&line);
            self.cursor = (r - 1, prev_len);
        }
    }

    fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        let (r, col) = self.cursor;
        if col < self.line_len(r) {
            let idx = Self::byte_idx(&self.lines[r], col);
            self.lines[r].remove(idx);
        } else if r + 1 < self.lines.len() {
            let next = self.lines.remove(r + 1);
            self.lines[r].push_str(&next);
        }
    }

    fn move_to(&mut self, pos: Pos, select: bool) {
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        let row = pos.0.min(self.lines.len() - 1);
        let col = pos.1.min(self.line_len(row));
        self.cursor = (row, col);
    }

    /// Handle a key; `height` is the visible row count, used for paging and scroll.
    pub fn handle(&mut self, key: KeyEvent, height: u16) -> bool {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let (r, c) = self.cursor;
        let page = (height as usize).max(1);
        let edited = match key.code {
            KeyCode::Char(ch) if !ctrl => {
                self.insert_char(ch);
                true
            }
            KeyCode::Char('a') if ctrl => {
                self.move_to((r, 0), false);
                false
            }
            KeyCode::Char('e') if ctrl => {
                self.move_to((r, usize::MAX), false);
                false
            }
            KeyCode::Enter => {
                self.insert_newline();
                true
            }
            KeyCode::Backspace => {
                self.backspace();
                true
            }
            KeyCode::Delete => {
                self.delete();
                true
            }
            KeyCode::Left => {
                if c > 0 {
                    self.move_to((r, c - 1), shift);
                } else if r > 0 {
                    self.move_to((r - 1, usize::MAX), shift);
                } else {
                    self.move_to((r, c), shift);
                }
                false
            }
            KeyCode::Right => {
                if c < self.line_len(r) {
                    self.move_to((r, c + 1), shift);
                } else if r + 1 < self.lines.len() {
                    self.move_to((r + 1, 0), shift);
                } else {
                    self.move_to((r, c), shift);
                }
                false
            }
            KeyCode::Up => {
                self.move_to((r.saturating_sub(1), c), shift);
                false
            }
            KeyCode::Down => {
                self.move_to((r + 1, c), shift);
                false
            }
            KeyCode::Home => {
                self.move_to((r, 0), shift);
                false
            }
            KeyCode::End => {
                self.move_to((r, usize::MAX), shift);
                false
            }
            KeyCode::PageUp => {
                self.move_to((r.saturating_sub(page), c), shift);
                false
            }
            KeyCode::PageDown => {
                self.move_to((r + page, c), shift);
                false
            }
            _ => false,
        };
        self.ensure_visible(page);
        edited
    }

    fn ensure_visible(&mut self, height: usize) {
        if self.cursor.0 < self.scroll {
            self.scroll = self.cursor.0;
        } else if self.cursor.0 >= self.scroll + height {
            self.scroll = self.cursor.0 + 1 - height;
        }
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer, focused: bool) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let sel = self.ordered_selection();
        let width = area.width as usize;
        // horizontal scroll keeps the cursor column visible
        let hscroll = self.cursor.1.saturating_sub(width.saturating_sub(1));
        for (i, row) in (self.scroll..self.scroll + area.height as usize).enumerate() {
            let y = area.y + i as u16;
            let Some(line) = self.lines.get(row) else {
                break;
            };
            for (j, ch) in line.chars().enumerate().skip(hscroll).take(width) {
                let x = area.x + (j - hscroll) as u16;
                let cell = &mut buf[(x, y)];
                // never hand a control character to the terminal (OSC title/clipboard writes)
                let shown = if ch.is_control() && ch != '\t' {
                    '·'
                } else if ch == '\t' {
                    ' '
                } else {
                    ch
                };
                cell.set_symbol(&shown.to_string());
                if let Some((s, e)) = sel
                    && (row, j) >= s
                    && (row, j) < e
                {
                    cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
                }
            }
            // selection covering the newline at the end of a fully selected line
            if let Some((s, e)) = sel
                && row >= s.0
                && row < e.0
            {
                let col = line.chars().count().saturating_sub(hscroll);
                if col < width {
                    buf[(area.x + col as u16, y)]
                        .set_style(Style::default().add_modifier(Modifier::REVERSED));
                }
            }
        }
        if focused
            && self.cursor.0 >= self.scroll
            && self.cursor.0 < self.scroll + area.height as usize
        {
            let x = area.x + (self.cursor.1 - hscroll).min(width - 1) as u16;
            let y = area.y + (self.cursor.0 - self.scroll) as u16;
            buf[(x, y)].set_style(
                Style::default().add_modifier(Modifier::REVERSED | Modifier::SLOW_BLINK),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyCode::*, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;
    use ratatui::{Terminal, layout::Rect};

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const SHIFT: KeyModifiers = KeyModifiers::SHIFT;

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }
    fn type_str(ta: &mut TextArea, s: &str) {
        for c in s.chars() {
            if c == '\n' {
                ta.handle(k(Enter, NONE), 5);
            } else {
                ta.handle(k(Char(c), NONE), 5);
            }
        }
    }

    #[test]
    fn typing_and_newlines_round_trip() {
        let mut ta = TextArea::from_text("");
        type_str(&mut ta, "ab\ncd");
        assert_eq!(ta.text(), "ab\ncd");
        assert_eq!(ta.cursor(), (1, 2));
        assert_eq!(
            TextArea::from_text("x\ny\n").text(),
            "x\ny\n",
            "trailing newline kept"
        );
    }

    #[test]
    fn backspace_joins_lines_and_delete_removes_forward() {
        let mut ta = TextArea::from_text("ab\ncd");
        ta.handle(k(Down, NONE), 5);
        ta.handle(k(Home, NONE), 5);
        ta.handle(k(Backspace, NONE), 5);
        assert_eq!(ta.text(), "abcd");
        assert_eq!(ta.cursor(), (0, 2));
        ta.handle(k(Delete, NONE), 5);
        assert_eq!(ta.text(), "abd");
    }

    #[test]
    fn shift_right_selects_and_selection_text() {
        let mut ta = TextArea::from_text("hello world");
        ta.handle(k(Right, SHIFT), 5);
        ta.handle(k(Right, SHIFT), 5);
        ta.handle(k(Right, SHIFT), 5);
        assert_eq!(ta.selection().as_deref(), Some("hel"));
        ta.handle(k(End, SHIFT), 5);
        assert_eq!(ta.selection().as_deref(), Some("hello world"));
    }

    #[test]
    fn arrow_without_shift_clears_selection_and_typing_replaces_it() {
        let mut ta = TextArea::from_text("hello");
        ta.handle(k(Right, SHIFT), 5);
        ta.handle(k(Right, SHIFT), 5);
        ta.handle(k(Left, NONE), 5);
        assert!(ta.selection().is_none());
        ta.handle(k(End, NONE), 5);
        ta.handle(k(Left, SHIFT), 5);
        ta.handle(k(Left, SHIFT), 5);
        ta.handle(k(Char('X'), NONE), 5);
        assert_eq!(ta.text(), "helX");
    }

    #[test]
    fn page_down_scrolls_keeping_cursor_visible() {
        let text: String = (0..30).map(|i| format!("line {i}\n")).collect();
        let mut ta = TextArea::from_text(&text);
        ta.handle(k(PageDown, NONE), 5);
        assert_eq!(ta.cursor().0, 5);
        for _ in 0..3 {
            ta.handle(k(PageDown, NONE), 5);
        }
        assert_eq!(ta.cursor().0, 20);
        assert!(
            ta.scroll() <= 20 && ta.scroll() + 5 > 20,
            "scroll {} keeps row 20 visible",
            ta.scroll()
        );
        ta.handle(k(PageUp, NONE), 5);
        assert_eq!(ta.cursor().0, 15);
    }

    #[test]
    fn unicode_cursor_moves_by_char() {
        let mut ta = TextArea::from_text("héllo");
        ta.handle(k(Right, NONE), 5);
        ta.handle(k(Right, NONE), 5);
        ta.handle(k(Backspace, NONE), 5);
        assert_eq!(ta.text(), "hllo");
    }

    #[test]
    fn render_marks_selection_reversed_and_shows_cursor() {
        let mut ta = TextArea::from_text("abc def\nsecond");
        ta.handle(k(Right, SHIFT), 5);
        ta.handle(k(Right, SHIFT), 5);
        let mut t = Terminal::new(TestBackend::new(20, 5)).unwrap();
        t.draw(|f| ta.render(Rect::new(0, 0, 20, 5), f.buffer_mut(), true))
            .unwrap();
        let buf = t.backend().buffer();
        assert!(buf[(0, 0)].modifier.contains(Modifier::REVERSED));
        assert!(buf[(1, 0)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(3, 0)].modifier.contains(Modifier::REVERSED));
        assert_eq!(buf[(0, 1)].symbol(), "s");
    }

    #[test]
    fn replace_all_resets_cursor_and_selection() {
        let mut ta = TextArea::from_text("old");
        ta.handle(k(Right, SHIFT), 5);
        ta.replace_all("brand new");
        assert_eq!(ta.text(), "brand new");
        assert_eq!(ta.cursor(), (0, 0));
        assert!(ta.selection().is_none());
    }

    #[test]
    fn control_characters_render_as_placeholders() {
        let ta = TextArea::from_text("a\x1b]0;evil\x07b\tc");
        let mut t = Terminal::new(TestBackend::new(20, 2)).unwrap();
        t.draw(|f| ta.render(Rect::new(0, 0, 20, 2), f.buffer_mut(), false))
            .unwrap();
        let row: String = (0..20)
            .map(|x| t.backend().buffer()[(x, 0)].symbol().to_string())
            .collect();
        assert!(
            !row.contains('\x1b') && !row.contains('\x07'),
            "raw controls leaked: {row:?}"
        );
        assert!(row.starts_with("a·"), "placeholder for ESC: {row:?}");
    }
}
