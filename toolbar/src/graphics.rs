//! Kitty graphics pass-through for slot tools that ask for it
//! (`graphics = true`), such as terminal-browser.
//!
//! The slot's screen model (vt100) drops graphics commands, so they are cut
//! out of the tool's output first: the text goes to the screen model, each
//! whole command is kept with the tool's cursor at that point, and the app
//! writes it to Herdr placed at the matching cell of the slot. Herdr shows
//! images in its panes like any terminal. The checks a graphics tool makes
//! before drawing (may I draw images, how big is a cell in pixels) are
//! answered here, since the screen model answers none of them.

/// A run of a tool's output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// Bytes for the screen model.
    Text(Vec<u8>),
    /// One whole graphics command, `ESC _ G … ESC \`.
    Graphics(Vec<u8>),
}

/// Cuts graphics commands out of a tool's output, across read boundaries.
#[derive(Debug, Default)]
pub struct Splitter {
    /// The tail of the last read: an escape that may start a command, or a
    /// command still waiting for its terminator.
    pending: Vec<u8>,
}

const ESC: u8 = 0x1b;
/// A command that never ends is dropped rather than held forever.
const MAX_PENDING: usize = 64 << 20;

impl Splitter {
    /// Split `input` into text and whole graphics commands. A command (or an
    /// escape that may start one) cut off by the end of `input` waits for the
    /// rest.
    pub fn split(&mut self, input: &[u8]) -> Vec<Piece> {
        let mut buf = std::mem::take(&mut self.pending);
        buf.extend_from_slice(input);
        let mut out = Vec::new();
        let mut text = Vec::new();
        let mut i = 0;
        while i < buf.len() {
            if buf[i] != ESC {
                text.push(buf[i]);
                i += 1;
                continue;
            }
            // ESC _ G starts a graphics command; anything shorter may yet
            let start = &buf[i..];
            if start.len() < 3 && b"\x1b_G".starts_with(start) {
                self.pending = start.to_vec();
                break;
            }
            if start.starts_with(b"\x1b_G") {
                match start[3..].windows(2).position(|w| w == b"\x1b\\") {
                    Some(end) => {
                        if !text.is_empty() {
                            out.push(Piece::Text(std::mem::take(&mut text)));
                        }
                        let len = 3 + end + 2;
                        out.push(Piece::Graphics(start[..len].to_vec()));
                        i += len;
                        continue;
                    }
                    None => {
                        if start.len() <= MAX_PENDING {
                            self.pending = start.to_vec();
                        }
                        break;
                    }
                }
            }
            text.push(ESC);
            i += 1;
        }
        if !text.is_empty() {
            out.push(Piece::Text(text));
        }
        out
    }
}

/// The reply a graphics query (`a=q`) expects: OK, under its id. None for
/// other commands, and for a query with no id (the terminal does not reply).
pub fn query_reply(cmd: &[u8]) -> Option<Vec<u8>> {
    let query = is_query(cmd);
    let id = control(cmd)?
        .split(|b| *b == b',')
        .find_map(|p| p.strip_prefix(b"i="));
    let id = id.filter(|id| !id.is_empty() && id.iter().all(u8::is_ascii_digit))?;
    query.then(|| {
        let mut reply = b"\x1b_Gi=".to_vec();
        reply.extend_from_slice(id);
        reply.extend_from_slice(b";OK\x1b\\");
        reply
    })
}

/// A graphics query (`a=q`): answered here, never shown.
pub fn is_query(cmd: &[u8]) -> bool {
    control(cmd).is_some_and(|c| c.split(|b| *b == b',').any(|p| p == b"a=q"))
}

/// The control data of a command: the `key=value` list before `;`.
fn control(cmd: &[u8]) -> Option<&[u8]> {
    let body = cmd.strip_prefix(b"\x1b_G")?;
    body.split(|b| *b == b';' || *b == ESC).next()
}

/// Replies to the terminal reports a graphics tool asks for in its text:
/// device attributes (`CSI c`), the cell size (`CSI 16 t`) and the text
/// area size in pixels (`CSI 14 t`). `cell` is (width, height) in pixels.
pub fn report_replies(text: &[u8], cell: Option<(u16, u16)>, rows: u16, cols: u16) -> Vec<u8> {
    let (w, h) = cell.map_or((0, 0), |(w, h)| (u32::from(w), u32::from(h)));
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let (len, reply) = if rest.starts_with(b"\x1b[c") {
            (3, b"\x1b[?62;22c".to_vec())
        } else if rest.starts_with(b"\x1b[0c") {
            (4, b"\x1b[?62;22c".to_vec())
        } else if cell.is_some() && rest.starts_with(b"\x1b[16t") {
            (5, format!("\x1b[6;{h};{w}t").into_bytes())
        } else if cell.is_some() && rest.starts_with(b"\x1b[14t") {
            let (ph, pw) = (u32::from(rows) * h, u32::from(cols) * w);
            (5, format!("\x1b[4;{ph};{pw}t").into_bytes())
        } else {
            i += 1;
            continue;
        };
        out.extend_from_slice(&reply);
        i += len;
    }
    out
}

/// `cmd` placed at the tool's cursor (`row`, `col`, 0-based) inside a slot
/// whose top-left cell is (`top`, `left`): the cursor is saved, moved there,
/// and restored around it. The command is made quiet (`q=2`): replies from
/// Herdr would arrive in the toolbar's own input, not the tool's.
pub fn placed(cmd: &[u8], top: u16, left: u16, row: u16, col: u16) -> Vec<u8> {
    let r = u32::from(top) + u32::from(row) + 1;
    let c = u32::from(left) + u32::from(col) + 1;
    let mut out = format!("\x1b7\x1b[{r};{c}H").into_bytes();
    out.extend_from_slice(&quiet(cmd));
    out.extend_from_slice(b"\x1b8");
    out
}

/// `cmd` with `q=2` (no replies), replacing any `q=` it had.
fn quiet(cmd: &[u8]) -> Vec<u8> {
    let Some(body) = cmd.strip_prefix(b"\x1b_G") else {
        return cmd.to_vec();
    };
    let end = body
        .iter()
        .position(|b| *b == b';' || *b == ESC)
        .unwrap_or(body.len());
    let mut keys: Vec<&[u8]> = body[..end]
        .split(|b| *b == b',')
        .filter(|p| !p.is_empty() && !p.starts_with(b"q="))
        .collect();
    let has_q = body[..end]
        .split(|b| *b == b',')
        .any(|p| p.starts_with(b"q="));
    let mut out = b"\x1b_G".to_vec();
    if has_q {
        keys.push(b"q=2");
    } else {
        keys.insert(0, b"q=2");
    }
    out.extend_from_slice(&keys.join(&b","[..]));
    out.extend_from_slice(&body[end..]);
    out
}

/// Deletes every image placement in the pane.
pub const DELETE_ALL: &[u8] = b"\x1b_Ga=d,d=A\x1b\\";

#[cfg(test)]
mod tests {
    use super::*;

    fn text(b: &[u8]) -> Piece {
        Piece::Text(b.to_vec())
    }
    fn gfx(b: &[u8]) -> Piece {
        Piece::Graphics(b.to_vec())
    }

    #[test]
    fn text_and_other_escapes_pass_through() {
        let mut s = Splitter::default();
        assert_eq!(s.split(b"ab\x1b[31mc"), vec![text(b"ab\x1b[31mc")]);
        // an APC that is not a graphics command is the screen model's
        assert_eq!(
            s.split(b"\x1b_Xhello\x1b\\"),
            vec![text(b"\x1b_Xhello\x1b\\")]
        );
    }

    #[test]
    fn graphics_commands_are_cut_out() {
        let mut s = Splitter::default();
        assert_eq!(
            s.split(b"ab\x1b_Ga=T,f=100;AAAA\x1b\\cd"),
            vec![text(b"ab"), gfx(b"\x1b_Ga=T,f=100;AAAA\x1b\\"), text(b"cd")]
        );
    }

    #[test]
    fn a_command_cut_by_a_read_waits_for_the_rest() {
        let mut s = Splitter::default();
        assert_eq!(s.split(b"x\x1b_Ga=T;AA"), vec![text(b"x")]);
        assert_eq!(s.split(b"AA\x1b"), vec![]);
        assert_eq!(
            s.split(b"\\y"),
            vec![gfx(b"\x1b_Ga=T;AAAA\x1b\\"), text(b"y")]
        );
        // an escape at the very end may start anything
        assert_eq!(s.split(b"ab\x1b"), vec![text(b"ab")]);
        assert_eq!(s.split(b"[1mz"), vec![text(b"\x1b[1mz")]);
        assert_eq!(s.split(b"ab\x1b_"), vec![text(b"ab")]);
        assert_eq!(s.split(b"Gx\x1b\\"), vec![gfx(b"\x1b_Gx\x1b\\")]);
    }

    #[test]
    fn a_query_is_answered_ok_under_its_id() {
        assert_eq!(
            query_reply(b"\x1b_Gi=31,a=q,t=d,f=24,s=1,v=1;AAAA\x1b\\"),
            Some(b"\x1b_Gi=31;OK\x1b\\".to_vec())
        );
        assert_eq!(query_reply(b"\x1b_Ga=T,i=4;AAAA\x1b\\"), None);
        assert_eq!(
            query_reply(b"\x1b_Ga=q;AAAA\x1b\\"),
            None,
            "no id, no reply"
        );
    }

    #[test]
    fn reports_are_answered_with_the_cell_size() {
        let cell = Some((8, 16));
        assert_eq!(report_replies(b"\x1b[c", cell, 24, 80), b"\x1b[?62;22c");
        assert_eq!(report_replies(b"\x1b[16t", cell, 24, 80), b"\x1b[6;16;8t");
        assert_eq!(
            report_replies(b"x\x1b[14ty", cell, 24, 80),
            b"\x1b[4;384;640t"
        );
        assert!(report_replies(b"plain \x1b[31m text", cell, 24, 80).is_empty());
        // sizes only when the toolbar knows its own cell size
        assert!(report_replies(b"\x1b[16t\x1b[14t", None, 24, 80).is_empty());
        assert_eq!(report_replies(b"\x1b[c", None, 24, 80), b"\x1b[?62;22c");
    }

    #[test]
    fn a_command_is_placed_at_the_tools_cursor_in_the_slot() {
        // quiet (q=2): Herdr's replies would land in the toolbar's input
        assert_eq!(
            placed(b"\x1b_Ga=p,i=4\x1b\\", 1, 2, 3, 4),
            b"\x1b7\x1b[5;7H\x1b_Gq=2,a=p,i=4\x1b\\\x1b8".to_vec()
        );
        assert_eq!(
            placed(b"\x1b_Gm=1,q=1;AAAA\x1b\\", 0, 0, 0, 0),
            b"\x1b7\x1b[1;1H\x1b_Gm=1,q=2;AAAA\x1b\\\x1b8".to_vec()
        );
    }

    #[test]
    fn queries_are_recognised_with_or_without_an_id() {
        assert!(is_query(b"\x1b_Ga=q;AAAA\x1b\\"));
        assert!(is_query(b"\x1b_Gi=3,a=q\x1b\\"));
        assert!(!is_query(b"\x1b_Ga=T;AAAA\x1b\\"));
    }
}
