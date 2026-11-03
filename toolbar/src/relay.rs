//! The pop-out of a tool running in the toolbar: a Herdr pane running
//! `herdr-toolbar attach <socket>` shows the tool, at the pane's size, and
//! types into it, while the slot says where it went. The toolbar end
//! (`serve`) hands the tool's pty to whoever connects; the pane end
//! (`attach`) relays its terminal. On the wire, the toolbar sends the
//! tool's output as it is; the pane sends frames: its size, and its keys.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::pty::{PtyHandle, Size};

/// What the pane says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// Bytes typed (keys, mouse, paste), for the tool as they are.
    Input(Vec<u8>),
    /// The pane's terminal size, first and whenever it changes.
    Size(Size),
}

/// A frame on the wire: a kind byte (`i`, `s`), the payload's length as
/// four bytes, big-endian, then the payload.
pub fn write_frame(to: &mut impl Write, frame: &Frame) -> std::io::Result<()> {
    let (kind, payload) = match frame {
        Frame::Input(bytes) => (b'i', bytes.clone()),
        Frame::Size(s) => (b's', [s.rows.to_be_bytes(), s.cols.to_be_bytes()].concat()),
    };
    let mut wire = Vec::with_capacity(5 + payload.len());
    wire.push(kind);
    wire.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    wire.extend_from_slice(&payload);
    to.write_all(&wire)?;
    to.flush()
}

/// No frame is bigger: a paste of a megabyte comes in pieces.
const MAX_FRAME: usize = 1 << 20;

/// The next frame; `None` at the end.
pub fn read_frame(from: &mut impl Read) -> std::io::Result<Option<Frame>> {
    use std::io::{Error, ErrorKind};
    let mut kind = [0u8; 1];
    if let Err(e) = from.read_exact(&mut kind) {
        return match e.kind() {
            ErrorKind::UnexpectedEof => Ok(None),
            _ => Err(e),
        };
    }
    let mut len = [0u8; 4];
    from.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(Error::new(ErrorKind::InvalidData, "frame too big"));
    }
    let mut payload = vec![0u8; len];
    from.read_exact(&mut payload)?;
    match (kind[0], &payload[..]) {
        (b'i', _) => Ok(Some(Frame::Input(payload))),
        (b's', [r0, r1, c0, c1]) => Ok(Some(Frame::Size(Size {
            rows: u16::from_be_bytes([*r0, *r1]),
            cols: u16::from_be_bytes([*c0, *c1]),
        }))),
        _ => Err(Error::new(ErrorKind::InvalidData, "unknown frame")),
    }
}

/// How a pop-out's pane asks to go: either way the pane closes and its
/// tool is back in its slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leave {
    /// `Ctrl-] c`: only the pane closes; the toolbar keeps what it shows.
    Close,
    /// `Ctrl-] d` (or `p`): the toolbar shows the tool and takes focus.
    Dock,
}

/// Watches what the pop-out's pane types for the toolbar's prefix then a
/// leave key (`Ctrl-] c` closes the pane, `Ctrl-] d` docks the tool in the
/// toolbar, `Ctrl-] p` too). The prefix twice types it once; the prefix
/// then any other key drops the prefix and lets that key through.
pub struct DockKeys {
    /// The prefix as a terminal usually sends it (`Ctrl-]` is `0x1d`).
    legacy: Option<Vec<u8>>,
    /// The prefix as the kitty keyboard protocol names it, for a tool that
    /// turned that on: its key code and modifier bits.
    kitty: Option<(u32, u32)>,
    /// The prefix came last: the next key decides.
    armed: bool,
}

/// The keys after the prefix that leave, as they are typed and as kitty
/// key codes.
const LEAVE: [(u8, Leave); 3] = [
    (b'c', Leave::Close),
    (b'd', Leave::Dock),
    (b'p', Leave::Dock),
];

/// Kitty's Caps Lock and Num Lock modifier bits: they say how the keyboard
/// is set, not what was held, so they never make a key another key.
const LOCKS: u32 = 64 | 128;

/// A key as the kitty keyboard protocol sends it:
/// `ESC [ code[:alternates] [; modifiers[:event] [; text]] u`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KittyKey {
    code: u32,
    /// The modifier bits held (shift 1, alt 2, ctrl 4, …), locks removed.
    held: u32,
    /// 1 a press, 2 a repeat, 3 a release.
    event: u32,
}

impl KittyKey {
    /// A key that decides what follows the prefix: pressed, and not a
    /// modifier key on its own (Shift, Ctrl, Alt, … are 57441 to 57454).
    fn decides(&self) -> bool {
        self.event == 1 && !(57441..=57454).contains(&self.code)
    }
}

/// The kitty key at the start of `bytes`, and how many bytes it takes.
fn kitty_key(bytes: &[u8]) -> Option<(usize, KittyKey)> {
    let body = bytes.strip_prefix(b"\x1b[")?;
    let end = body
        .iter()
        .position(|b| !(b.is_ascii_digit() || *b == b';' || *b == b':'))?;
    if body[end] != b'u' {
        return None;
    }
    let fields = std::str::from_utf8(&body[..end]).ok()?;
    let mut fields = fields.split(';');
    let code = fields.next()?.split(':').next()?.parse().ok()?;
    let (mods, event) = match fields.next().filter(|f| !f.is_empty()) {
        None => (1, 1),
        Some(f) => {
            let mut parts = f.split(':');
            let mods: u32 = parts.next()?.parse().ok()?;
            let event = match parts.next() {
                Some(e) => e.parse().ok()?,
                None => 1,
            };
            (mods, event)
        }
    };
    let key = KittyKey {
        code,
        held: mods.saturating_sub(1) & !LOCKS,
        event,
    };
    Some((2 + end + 1, key))
}

impl DockKeys {
    pub fn new(prefix: &crate::keys::KeyChord) -> Self {
        let chord = crossterm::event::KeyEvent::new(prefix.code, prefix.modifiers);
        let kitty = match prefix.code {
            crossterm::event::KeyCode::Char(c) => {
                use crossterm::event::KeyModifiers as M;
                let m = &prefix.modifiers;
                let bits = u32::from(m.contains(M::SHIFT))
                    + u32::from(m.contains(M::ALT)) * 2
                    + u32::from(m.contains(M::CONTROL)) * 4;
                Some((c as u32, bits))
            }
            _ => None,
        };
        Self {
            legacy: crate::keys::key_to_bytes(&chord, false),
            kitty,
            armed: false,
        }
    }

    /// Whether `key` is the prefix's key, however it was held.
    fn is_prefix_key(&self, key: &KittyKey) -> bool {
        self.kitty.is_some_and(|(code, _)| key.code == code)
    }

    /// How long the prefix is at the start of `bytes`, if it is pressed there.
    fn prefix_at(&self, bytes: &[u8], key: Option<(usize, KittyKey)>) -> Option<usize> {
        if let Some(legacy) = self.legacy.as_ref().filter(|l| bytes.starts_with(l)) {
            return Some(legacy.len());
        }
        let (n, key) = key?;
        let (code, bits) = self.kitty?;
        (key.event == 1 && key.code == code && key.held == bits).then_some(n)
    }

    /// The leave key at the start of `bytes`, typed or as an unmodified
    /// kitty press.
    fn leave_at(bytes: &[u8], key: Option<(usize, KittyKey)>) -> Option<Leave> {
        let pressed = match key {
            Some((_, k)) if k.held == 0 => u8::try_from(k.code).ok()?,
            Some(_) => return None,
            None => *bytes.first()?,
        };
        LEAVE
            .iter()
            .find(|(k, _)| *k == pressed)
            .map(|(_, how)| *how)
    }

    /// What of `input` goes on to the tool, and whether it asked to leave.
    /// Typing after the leave key is dropped: the pane is about to close.
    pub fn feed(&mut self, input: &[u8]) -> (Vec<u8>, Option<Leave>) {
        let mut out = Vec::with_capacity(input.len());
        let mut i = 0;
        while i < input.len() {
            let rest = &input[i..];
            let key = kitty_key(rest);
            if self.armed {
                // a release, a repeat or a lone modifier waits for a real key;
                // the prefix's own release is dropped, as its press was
                if let Some((n, k)) = key.filter(|(_, k)| !k.decides()) {
                    if !self.is_prefix_key(&k) {
                        out.extend_from_slice(&rest[..n]);
                    }
                    i += n;
                    continue;
                }
                self.armed = false;
                if let Some(how) = Self::leave_at(rest, key) {
                    return (out, Some(how));
                }
                if let Some(n) = self.prefix_at(rest, key) {
                    out.extend_from_slice(&rest[..n]);
                    i += n;
                }
                // any other key: the prefix is dropped and the key read as usual
                continue;
            }
            if let Some(n) = self.prefix_at(rest, key) {
                self.armed = true;
                i += n;
                continue;
            }
            // a kitty key goes on whole
            let n = key.map_or(1, |(n, _)| n);
            out.extend_from_slice(&rest[..n]);
            i += n;
        }
        (out, None)
    }
}

/// The toolbar end: each client in turn gets the tool (at its size) until
/// it leaves, then the tool goes back to the slot. Ends when `stop` is set.
pub fn serve(listener: UnixListener, tool: PtyHandle, stop: Arc<AtomicBool>, wake: Sender<()>) {
    if listener.set_nonblocking(true).is_err() {
        return;
    }
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((client, _)) => {
                show(client, &tool);
                tool.detach();
                let _ = wake.send(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => return,
        }
    }
}

/// One client: its size first, then the tool is its until it leaves (or
/// the tool is given back, which shuts its connection).
fn show(mut client: UnixStream, tool: &PtyHandle) {
    if client.set_nonblocking(false).is_err() {
        return;
    }
    // a client that never says its size is not waited for
    let _ = client.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(Some(Frame::Size(size))) = read_frame(&mut client) else {
        return;
    };
    let _ = client.set_read_timeout(None);
    let Ok(out) = client.try_clone() else {
        return;
    };
    if tool.attach(out, size).is_err() {
        return;
    }
    while let Ok(Some(frame)) = read_frame(&mut client) {
        match frame {
            Frame::Input(bytes) => {
                if tool.input(&bytes).is_err() {
                    return;
                }
            }
            Frame::Size(size) => tool.resize(size),
        }
    }
}

/// Where a pop-out's pane sends its leave keys (`Ctrl-] c`, `Ctrl-] d`): the toolbar's
/// control socket, the tool it shows, and the toolbar's prefix as
/// `tools.toml` writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockTo {
    pub control: std::path::PathBuf,
    pub tool: String,
    pub prefix: String,
}

/// The pane end (`herdr-toolbar attach <socket> [<control> <tool> <prefix>]`):
/// this terminal shows the tool and types into it, until the toolbar lets go.
pub fn attach(socket: &Path, dock: Option<DockTo>) -> anyhow::Result<()> {
    use anyhow::Context;
    use crossterm::terminal;

    let mut toolbar = UnixStream::connect(socket).with_context(|| {
        format!(
            "the toolbar is not showing a pop-out at {}",
            socket.display()
        )
    })?;
    let mut dock_keys = match &dock {
        Some(d) => Some(DockKeys::new(&crate::keys::KeyChord::parse(&d.prefix)?)),
        None => None,
    };
    let size_now = || {
        terminal::size()
            .map(|(cols, rows)| Size { rows, cols })
            .unwrap_or(Size { rows: 24, cols: 80 })
    };
    let mut size = size_now();
    write_frame(&mut toolbar, &Frame::Size(size))?;
    terminal::enable_raw_mode()?;

    // keys, as typed, to the tool; the dock key goes to the toolbar instead
    let mut keys = toolbar.try_clone()?;
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = [0u8; 4096];
        while let Ok(n) = stdin.read(&mut buf) {
            if n == 0 {
                break;
            }
            let (typed, leave) = match dock_keys.as_mut() {
                Some(k) => k.feed(&buf[..n]),
                None => (buf[..n].to_vec(), None),
            };
            if !typed.is_empty() && write_frame(&mut keys, &Frame::Input(typed)).is_err() {
                break;
            }
            if let Some(how) = leave {
                // the toolbar closes this pane; one that cannot be told is
                // let go of here, which gives the tool back all the same
                let told = dock.as_ref().is_some_and(|d| tell(d, how).is_ok());
                if !told {
                    let _ = keys.shutdown(std::net::Shutdown::Both);
                }
                break;
            }
        }
    });
    // this terminal's size, whenever it changes
    let mut sizes = toolbar.try_clone()?;
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(200));
            let now = size_now();
            if now != size {
                size = now;
                if write_frame(&mut sizes, &Frame::Size(now)).is_err() {
                    break;
                }
            }
        }
    });
    // the tool, until the toolbar lets go
    let mut out = std::io::stdout().lock();
    let mut buf = [0u8; 65536];
    while let Ok(n) = toolbar.read(&mut buf) {
        if n == 0 || out.write_all(&buf[..n]).and_then(|()| out.flush()).is_err() {
            break;
        }
    }
    let _ = out.write_all(SHELL_AGAIN);
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
    Ok(())
}

/// Whatever modes a tool left on, a pane's terminal is a shell's again:
/// colours, the main screen, the cursor, no mouse, no bracketed paste,
/// normal cursor and keypad keys.
pub(crate) const SHELL_AGAIN: &[u8] =
    b"\x1b[0m\x1b[?1049l\x1b[?25h\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[?1l\x1b>";

/// Tells the toolbar a pop-out's pane asked to leave (`Ctrl-] c` / `d`).
pub(crate) fn tell(dock: &DockTo, how: Leave) -> anyhow::Result<()> {
    match how {
        Leave::Close => crate::herdr::close_popout(&dock.control, &dock.tool),
        Leave::Dock => crate::herdr::dock(&dock.control, &dock.tool),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::KeyChord;

    fn ctrl_bracket() -> DockKeys {
        DockKeys::new(&KeyChord::parse("ctrl-]").unwrap())
    }

    #[test]
    fn the_prefix_then_d_docks_and_neither_reaches_the_tool() {
        let mut keys = ctrl_bracket();
        assert_eq!(keys.feed(b"ab\x1ddcd"), (b"ab".to_vec(), Some(Leave::Dock)));
    }

    #[test]
    fn the_prefix_then_c_closes_and_neither_reaches_the_tool() {
        let mut keys = ctrl_bracket();
        assert_eq!(
            keys.feed(b"ab\x1dcxy"),
            (b"ab".to_vec(), Some(Leave::Close))
        );
    }

    #[test]
    fn c_and_d_without_the_prefix_are_typed() {
        let mut keys = ctrl_bracket();
        assert_eq!(keys.feed(b"cd"), (b"cd".to_vec(), None));
    }

    #[test]
    fn the_prefix_then_p_still_docks_and_neither_reaches_the_tool() {
        let mut keys = ctrl_bracket();
        assert_eq!(keys.feed(b"ab\x1dpcd"), (b"ab".to_vec(), Some(Leave::Dock)));
    }

    #[test]
    fn the_dock_key_may_arrive_in_two_reads() {
        let mut keys = ctrl_bracket();
        assert_eq!(keys.feed(b"a\x1d"), (b"a".to_vec(), None));
        assert_eq!(keys.feed(b"p"), (Vec::new(), Some(Leave::Dock)));
    }

    #[test]
    fn the_prefix_twice_types_it_once() {
        let mut keys = ctrl_bracket();
        assert_eq!(keys.feed(b"\x1d\x1dp"), (b"\x1dp".to_vec(), None));
    }

    #[test]
    fn the_prefix_then_another_key_lets_that_key_through() {
        let mut keys = ctrl_bracket();
        // an arrow key after the prefix arrives whole
        assert_eq!(keys.feed(b"\x1d\x1b[Ax"), (b"\x1b[Ax".to_vec(), None));
    }

    #[test]
    fn typing_without_the_prefix_goes_through_as_it_is() {
        let mut keys = ctrl_bracket();
        assert_eq!(keys.feed(b"plain p"), (b"plain p".to_vec(), None));
    }

    #[test]
    fn a_tool_using_the_kitty_keyboard_protocol_gets_its_prefix_recognised() {
        // Ctrl-] as kitty encodes it, then a leave key as text or as its own escape
        for (k, how) in [
            (&b"c"[..], Leave::Close),
            (b"\x1b[99u", Leave::Close),
            (b"d", Leave::Dock),
            (b"\x1b[100u", Leave::Dock),
            (b"p", Leave::Dock),
            (b"\x1b[112u", Leave::Dock),
        ] {
            let mut keys = ctrl_bracket();
            let typed = [&b"x\x1b[93;5u"[..], k].concat();
            assert_eq!(keys.feed(&typed), (b"x".to_vec(), Some(how)), "{k:?}");
        }
    }

    /// Ctrl-] as kitty sends it: key 93 (`]`), modifiers 1 + ctrl (4).
    const KITTY_PREFIX: &[u8] = b"\x1b[93;5u";

    #[test]
    fn a_kitty_leave_key_that_carries_its_text_still_leaves() {
        // "report all keys as escapes" + "associated text": d is ESC[100;;100u
        // (Herdr sent exactly this to a probe tool on 2026-11-02)
        for (k, how) in [
            (&b"\x1b[99;;99u"[..], Leave::Close),
            (b"\x1b[100;;100u", Leave::Dock),
            (b"\x1b[112;;112u", Leave::Dock),
        ] {
            let mut keys = ctrl_bracket();
            let typed = [&b"x"[..], KITTY_PREFIX, k].concat();
            assert_eq!(keys.feed(&typed), (b"x".to_vec(), Some(how)), "{k:?}");
        }
    }

    #[test]
    fn kitty_keys_reported_as_presses_are_read_as_keys() {
        // "report event types": a press may say so (`:1`), the prefix too
        let mut keys = ctrl_bracket();
        assert_eq!(
            keys.feed(b"\x1b[93;5:1u\x1b[100;1:1u"),
            (Vec::new(), Some(Leave::Dock))
        );
    }

    #[test]
    fn releases_after_the_prefix_wait_for_the_next_key() {
        // Ctrl-] pressed, then ] and Ctrl let go, then d: the prefix's own
        // release is dropped (its press never reached the tool), Ctrl's goes on
        let mut keys = ctrl_bracket();
        let typed = [
            KITTY_PREFIX,
            b"\x1b[93;1:3u",
            b"\x1b[57442;1:3u",
            b"\x1b[100u",
        ]
        .concat();
        assert_eq!(
            keys.feed(&typed),
            (b"\x1b[57442;1:3u".to_vec(), Some(Leave::Dock))
        );
    }

    #[test]
    fn a_modifier_key_after_the_prefix_waits_for_the_next_key() {
        // Shift pressed and let go on its own: neither decides, both go on
        let mut keys = ctrl_bracket();
        let typed = [KITTY_PREFIX, b"\x1b[57441;2u\x1b[57441;1:3u", b"\x1b[99u"].concat();
        assert_eq!(
            keys.feed(&typed),
            (b"\x1b[57441;2u\x1b[57441;1:3u".to_vec(), Some(Leave::Close))
        );
    }

    #[test]
    fn caps_lock_or_num_lock_does_not_hide_a_kitty_key() {
        // lock modifiers (64, 128) ride along on the prefix and the leave key
        let mut keys = ctrl_bracket();
        assert_eq!(
            keys.feed(b"\x1b[93;69u\x1b[100;129;100u"),
            (Vec::new(), Some(Leave::Dock))
        );
    }

    #[test]
    fn a_kitty_leave_key_held_with_a_modifier_is_typed_instead() {
        // Shift-d after the prefix is another key: the prefix is dropped
        let mut keys = ctrl_bracket();
        let typed = [KITTY_PREFIX, b"\x1b[100;2;68u"].concat();
        assert_eq!(keys.feed(&typed), (b"\x1b[100;2;68u".to_vec(), None));
    }

    #[test]
    fn a_kitty_prefix_twice_types_it_once() {
        // press, release, press: one press reaches the tool, with its release
        let mut keys = ctrl_bracket();
        let typed = [KITTY_PREFIX, b"\x1b[93;5:3u", KITTY_PREFIX, b"\x1b[93;5:3u"].concat();
        assert_eq!(
            keys.feed(&typed),
            (b"\x1b[93;5u\x1b[93;5:3u".to_vec(), None)
        );
    }

    #[test]
    fn the_prefix_let_go_on_its_own_does_not_arm() {
        let mut keys = ctrl_bracket();
        assert_eq!(
            keys.feed(b"\x1b[93;5:3ud"),
            (b"\x1b[93;5:3ud".to_vec(), None)
        );
    }

    #[test]
    fn the_configured_prefix_is_the_one_watched() {
        let mut keys = DockKeys::new(&KeyChord::parse("ctrl-a").unwrap());
        assert_eq!(keys.feed(b"\x1dp"), (b"\x1dp".to_vec(), None));
        assert_eq!(keys.feed(b"\x01p"), (Vec::new(), Some(Leave::Dock)));
    }
}
