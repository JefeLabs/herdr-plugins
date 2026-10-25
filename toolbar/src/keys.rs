//! The in-pane prefix (default `Ctrl-]`) and the translation of crossterm
//! key events into the bytes a hosted terminal program expects.

use anyhow::bail;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What the app should do with a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Write these bytes to the selected tool.
    Forward(Vec<u8>),
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

    fn matches(&self, key: &KeyEvent) -> bool {
        key.code == self.code && key.modifiers == self.modifiers
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
        Ok(Self { armed: false, chord: KeyChord::parse(prefix)? })
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
            return key_to_bytes(&key).map(Action::Forward).unwrap_or(Action::Ignore);
        }
        self.armed = false;
        if self.chord.matches(&key) {
            return key_to_bytes(&key).map(Action::Forward).unwrap_or(Action::Ignore);
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
/// Returns `None` for keys that have no byte form (modifier-only, media keys).
pub fn key_to_bytes(key: &KeyEvent) -> Option<Vec<u8>> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
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
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::F(n) => function_key(n)?,
        _ => return None,
    };
    out.extend(body);
    Some(out)
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

fn function_key(n: u8) -> Option<Vec<u8>> {
    Some(match n {
        1 => b"\x1bOP".to_vec(),
        2 => b"\x1bOQ".to_vec(),
        3 => b"\x1bOR".to_vec(),
        4 => b"\x1bOS".to_vec(),
        5 => b"\x1b[15~".to_vec(),
        6 => b"\x1b[17~".to_vec(),
        7 => b"\x1b[18~".to_vec(),
        8 => b"\x1b[19~".to_vec(),
        9 => b"\x1b[20~".to_vec(),
        10 => b"\x1b[21~".to_vec(),
        11 => b"\x1b[23~".to_vec(),
        12 => b"\x1b[24~".to_vec(),
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

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }
    fn p() -> Prefix {
        Prefix::new("ctrl-]").unwrap()
    }
    fn arm(p: &mut Prefix) {
        assert!(matches!(p.handle(k(Char(']'), CONTROL)), Action::Ignore));
        assert!(p.armed());
    }

    #[test]
    fn plain_keys_forward() {
        assert!(matches!(p().handle(k(Char('a'), NONE)), Action::Forward(b) if b == b"a"));
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
        assert!(matches!(p.handle(k(Char(']'), CONTROL)), Action::Forward(b) if b == vec![0x1d]));
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
    fn encodes_arrows_and_ctrl_chars() {
        assert_eq!(key_to_bytes(&k(Up, NONE)), Some(vec![27, 91, 65]));
        assert_eq!(key_to_bytes(&k(Char('c'), CONTROL)), Some(vec![3]));
        assert_eq!(key_to_bytes(&k(Char('x'), ALT)), Some(vec![27, b'x']));
        assert_eq!(key_to_bytes(&k(F(5), NONE)), Some(b"\x1b[15~".to_vec()));
        assert_eq!(key_to_bytes(&k(Enter, NONE)), Some(vec![b'\r']));
        assert_eq!(key_to_bytes(&k(Backspace, NONE)), Some(vec![0x7f]));
        assert_eq!(key_to_bytes(&k(Char('é'), NONE)), Some("é".as_bytes().to_vec()));
        assert_eq!(key_to_bytes(&k(Char(' '), CONTROL)), Some(vec![0]));
    }

    #[test]
    fn bad_prefix_string_is_an_error() {
        assert!(Prefix::new("super-]").is_err());
        assert!(Prefix::new("ctrl-").is_err());
        assert!(Prefix::new("alt-t").is_ok());
    }
}
