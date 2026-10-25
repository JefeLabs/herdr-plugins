//! The in-pane prefix (default `Ctrl-]`) and the translation of crossterm
//! key events into the bytes a hosted terminal program expects.

use anyhow::bail;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What the app should do with a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send this key to the selected tool (encoded by `key_to_bytes` with the
    /// tool's current cursor mode).
    Forward(KeyEvent),
    /// Zero-based tool index.
    Select(usize),
    Next,
    Prev,
    Reload,
    Restart,
    Quit,
    /// Swallow the key (it armed or cancelled the prefix).
    Ignore,
}

/// A single key with modifiers, parsed from `"ctrl-]"`, `"alt-t"`, ...
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyChord {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let mut modifiers = KeyModifiers::NONE;
        let mut parts: Vec<&str> = text.split('-').collect();
        // "ctrl--" style edge cases are not supported; a trailing empty part is an error.
        let last = match parts.pop() {
            Some(l) if !l.is_empty() => l,
            _ => bail!("prefix `{text}`: missing key"),
        };
        for m in parts {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "c" => modifiers |= KeyModifiers::CONTROL,
                "alt" | "meta" | "m" => modifiers |= KeyModifiers::ALT,
                "shift" | "s" => modifiers |= KeyModifiers::SHIFT,
                other => bail!("prefix `{text}`: unknown modifier `{other}`"),
            }
        }
        let mut chars = last.chars();
        let code = match (chars.next(), chars.next()) {
            (Some(c), None) => KeyCode::Char(c),
            _ => match last.to_ascii_lowercase().as_str() {
                "esc" | "escape" => KeyCode::Esc,
                "space" => KeyCode::Char(' '),
                "tab" => KeyCode::Tab,
                other => bail!("prefix `{text}`: unknown key `{other}`"),
            },
        };
        Ok(Self { code, modifiers })
    }

    /// Compare by the bytes a terminal would send, not by crossterm's decoding:
    /// crossterm reports the byte 0x1d (Ctrl-]) as Char('5') + CONTROL, so a
    /// chord written "ctrl-]" must still match it.
    fn matches(&self, key: &KeyEvent) -> bool {
        if key.code == self.code && key.modifiers == self.modifiers {
            return true;
        }
        let chord = KeyEvent::new(self.code, self.modifiers);
        match (key_to_bytes(key, false), key_to_bytes(&chord, false)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }
}

/// Two-key prefix state machine.
#[derive(Debug)]
pub struct Prefix {
    armed: bool,
    chord: KeyChord,
}

impl Prefix {
    pub fn new(prefix: &str) -> anyhow::Result<Self> {
        Ok(Self {
            armed: false,
            chord: KeyChord::parse(prefix)?,
        })
    }

    pub fn armed(&self) -> bool {
        self.armed
    }

    pub fn handle(&mut self, key: KeyEvent) -> Action {
        if !self.armed {
            if self.chord.matches(&key) {
                self.armed = true;
                return Action::Ignore;
            }
            return Action::Forward(key);
        }
        self.armed = false;
        if self.chord.matches(&key) {
            return Action::Forward(key);
        }
        match key.code {
            KeyCode::Char(c @ '1'..='9') if key.modifiers.is_empty() => {
                Action::Select(c as usize - '1' as usize)
            }
            KeyCode::Char('j') => Action::Next,
            KeyCode::Char('k') => Action::Prev,
            KeyCode::Char('r') => Action::Reload,
            KeyCode::Char('x') => Action::Restart,
            KeyCode::Char('q') => Action::Quit,
            _ => Action::Ignore,
        }
    }
}

/// Encode a key the way an xterm-compatible terminal would send it.
/// `app_cursor` is the tool's DECCKM state (`vt100::Screen::application_cursor`):
/// when set, unmodified arrows and Home/End use SS3 (`ESC O A`) as ncurses expects.
/// Modified special keys use xterm's `CSI 1 ; m X` / `CSI n ; m ~` form.
/// Returns `None` for keys that have no byte form.
pub fn key_to_bytes(key: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let modifier = 1 + (shift as u8) + (alt as u8) * 2 + (ctrl as u8) * 4;

    // Keys whose encoding is a final letter: CSI letter, SS3 letter, or CSI 1;m letter.
    let letter = match key.code {
        KeyCode::Up => Some(b'A'),
        KeyCode::Down => Some(b'B'),
        KeyCode::Right => Some(b'C'),
        KeyCode::Left => Some(b'D'),
        KeyCode::Home => Some(b'H'),
        KeyCode::End => Some(b'F'),
        KeyCode::F(1) => Some(b'P'),
        KeyCode::F(2) => Some(b'Q'),
        KeyCode::F(3) => Some(b'R'),
        KeyCode::F(4) => Some(b'S'),
        _ => None,
    };
    if let Some(l) = letter {
        let is_f = matches!(key.code, KeyCode::F(_));
        return Some(if modifier > 1 {
            format!("\x1b[1;{modifier}{}", l as char).into_bytes()
        } else if is_f || app_cursor {
            vec![0x1b, b'O', l]
        } else {
            vec![0x1b, b'[', l]
        });
    }

    // Keys whose encoding is CSI number ~ (with ;m before the tilde when modified).
    let tilde = match key.code {
        KeyCode::Insert => Some(2),
        KeyCode::Delete => Some(3),
        KeyCode::PageUp => Some(5),
        KeyCode::PageDown => Some(6),
        KeyCode::F(5) => Some(15),
        KeyCode::F(6) => Some(17),
        KeyCode::F(7) => Some(18),
        KeyCode::F(8) => Some(19),
        KeyCode::F(9) => Some(20),
        KeyCode::F(10) => Some(21),
        KeyCode::F(11) => Some(23),
        KeyCode::F(12) => Some(24),
        _ => None,
    };
    if let Some(n) = tilde {
        return Some(if modifier > 1 { format!("\x1b[{n};{modifier}~") } else { format!("\x1b[{n}~") }.into_bytes());
    }

    let mut out: Vec<u8> = Vec::with_capacity(4);
    if alt {
        out.push(0x1b);
    }
    let body: Vec<u8> = match key.code {
        KeyCode::Char(ch) if ctrl => vec![control_byte(ch)?],
        KeyCode::Char(ch) => ch.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![9],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        _ => return None,
    };
    out.extend(body);
    Some(out)
}

/// Bytes to send for pasted text. ESC and every C0 control except tab, newline and
/// carriage return are removed, so a paste can neither close the bracket early nor
/// smuggle a control sequence. Markers are added only when the tool asked for
/// bracketed paste (DECSET 2004).
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
        .collect();
    if bracketed {
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        clean.into_bytes()
    }
}

fn control_byte(ch: char) -> Option<u8> {
    let upper = ch.to_ascii_uppercase();
    Some(match upper {
        '2' | '@' | ' ' => 0,
        '3' | '[' => 27,
        '4' | '\\' => 28,
        '5' | ']' => 29,
        '6' | '^' => 30,
        '7' | '-' | '_' => 31,
        '8' | '?' => 127,
        'A'..='Z' => upper as u8 - 64,
        _ => return None,
    })
}


#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyCode::*, KeyEvent, KeyModifiers};

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const CONTROL: KeyModifiers = KeyModifiers::CONTROL;
    const ALT: KeyModifiers = KeyModifiers::ALT;
    const SHIFT: KeyModifiers = KeyModifiers::SHIFT;

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }
    fn p() -> Prefix {
        Prefix::new("ctrl-]").unwrap()
    }
    /// crossterm 0.29 decodes the byte 0x1d (Ctrl-]) as Char('5') + CONTROL,
    /// never as Char(']') + CONTROL. Tests must use what crossterm produces.
    fn ctrl_bracket() -> KeyEvent {
        k(Char('5'), CONTROL)
    }
    fn arm(p: &mut Prefix) {
        assert!(matches!(p.handle(ctrl_bracket()), Action::Ignore));
        assert!(p.armed());
    }

    #[test]
    fn plain_keys_forward_the_event() {
        assert!(matches!(p().handle(k(Char('a'), NONE)), Action::Forward(e) if e.code == Char('a')));
    }

    #[test]
    fn prefix_arms_on_the_event_crossterm_actually_produces() {
        let mut p = p();
        assert!(matches!(p.handle(k(Char('5'), CONTROL)), Action::Ignore));
        assert!(p.armed());
    }

    #[test]
    fn prefix_then_digit_selects_zero_based() {
        let mut p = p();
        arm(&mut p);
        assert!(matches!(p.handle(k(Char('3'), NONE)), Action::Select(2)));
        assert!(!p.armed());
    }

    #[test]
    fn prefix_twice_sends_literal_0x1d() {
        let mut p = p();
        arm(&mut p);
        let Action::Forward(e) = p.handle(ctrl_bracket()) else { panic!("expected Forward") };
        assert_eq!(key_to_bytes(&e, false), Some(vec![0x1d]));
        assert!(!p.armed());
    }

    #[test]
    fn prefix_then_unknown_is_ignored_and_disarms() {
        let mut p = p();
        arm(&mut p);
        assert!(matches!(p.handle(k(Char('z'), NONE)), Action::Ignore));
        assert!(!p.armed());
    }

    #[test]
    fn jk_r_x_q_map() {
        let mut p = p();
        for (ch, want) in [('j', Action::Next), ('k', Action::Prev), ('r', Action::Reload), ('x', Action::Restart), ('q', Action::Quit)] {
            arm(&mut p);
            assert_eq!(p.handle(k(Char(ch), NONE)), want, "key {ch}");
        }
    }

    #[test]
    fn alt_prefix_matches_too() {
        let mut p = Prefix::new("alt-t").unwrap();
        assert!(matches!(p.handle(k(Char('t'), ALT)), Action::Ignore));
        assert!(p.armed());
    }

    #[test]
    fn encodes_arrows_and_ctrl_chars() {
        assert_eq!(key_to_bytes(&k(Up, NONE), false), Some(vec![27, 91, 65]));
        assert_eq!(key_to_bytes(&k(Char('c'), CONTROL), false), Some(vec![3]));
        assert_eq!(key_to_bytes(&k(Char('x'), ALT), false), Some(vec![27, b'x']));
        assert_eq!(key_to_bytes(&k(F(5), NONE), false), Some(b"\x1b[15~".to_vec()));
        assert_eq!(key_to_bytes(&k(Enter, NONE), false), Some(vec![b'\r']));
        assert_eq!(key_to_bytes(&k(Backspace, NONE), false), Some(vec![0x7f]));
        assert_eq!(key_to_bytes(&k(Char('é'), NONE), false), Some("é".as_bytes().to_vec()));
        assert_eq!(key_to_bytes(&k(Char(' '), CONTROL), false), Some(vec![0]));
    }

    #[test]
    fn application_cursor_mode_uses_ss3_for_arrows_and_home_end() {
        assert_eq!(key_to_bytes(&k(Up, NONE), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(key_to_bytes(&k(Left, NONE), true), Some(b"\x1bOD".to_vec()));
        assert_eq!(key_to_bytes(&k(Home, NONE), true), Some(b"\x1bOH".to_vec()));
        assert_eq!(key_to_bytes(&k(End, NONE), true), Some(b"\x1bOF".to_vec()));
        // not affected by the mode
        assert_eq!(key_to_bytes(&k(PageUp, NONE), true), Some(b"\x1b[5~".to_vec()));
    }

    #[test]
    fn modifiers_on_special_keys_use_xterm_encoding() {
        assert_eq!(key_to_bytes(&k(Left, CONTROL), false), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(key_to_bytes(&k(Right, SHIFT), false), Some(b"\x1b[1;2C".to_vec()));
        assert_eq!(key_to_bytes(&k(Up, ALT), false), Some(b"\x1b[1;3A".to_vec()));
        assert_eq!(key_to_bytes(&k(F(5), SHIFT), false), Some(b"\x1b[15;2~".to_vec()));
        assert_eq!(key_to_bytes(&k(F(1), CONTROL), false), Some(b"\x1b[1;5P".to_vec()));
        assert_eq!(key_to_bytes(&k(Delete, CONTROL), false), Some(b"\x1b[3;5~".to_vec()));
        // in application cursor mode a modified arrow still uses CSI
        assert_eq!(key_to_bytes(&k(Left, CONTROL), true), Some(b"\x1b[1;5D".to_vec()));
    }

    #[test]
    fn bad_prefix_string_is_an_error() {
        assert!(Prefix::new("super-]").is_err());
        assert!(Prefix::new("ctrl-").is_err());
        assert!(Prefix::new("alt-t").is_ok());
    }

    #[test]
    fn paste_strips_escapes_and_wraps_only_when_bracketed() {
        // A paste must not be able to end the bracket early or inject control sequences.
        let hostile = "ls\x1b[201~\x1b]0;x\x07rm -rf ~\n\x01\ttab\r";
        assert_eq!(
            paste_bytes(hostile, false),
            b"ls[201~]0;xrm -rf ~\n\ttab\r".to_vec()
        );
        let wrapped = paste_bytes("hi\x1b", true);
        assert_eq!(wrapped, b"\x1b[200~hi\x1b[201~".to_vec());
        assert_eq!(paste_bytes("plain", false), b"plain".to_vec());
    }
}
