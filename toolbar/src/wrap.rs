//! The pane end of a pop-out whose tool has a `popout` command of its own,
//! such as a browser that draws in the pane itself: `herdr-toolbar wrap
//! <control> <tool> <prefix> -- <command…>`. The command runs on a terminal
//! of its own, at the pane's size in cells and pixels (images are drawn to
//! fit). What it prints reaches the pane byte for byte, with no screen in
//! between; what is typed reaches it too, except the toolbar's leave keys
//! (`Ctrl-] c`, `Ctrl-] d`), kept the same way as by `attach`.

use std::io::{Read, Write};
use std::time::Duration;

use anyhow::Context;
use crossterm::terminal;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::relay::{DockKeys, DockTo};

/// This terminal's size, in cells and, when it says, pixels.
fn pane_size() -> PtySize {
    match terminal::window_size() {
        Ok(w) => PtySize {
            rows: w.rows,
            cols: w.columns,
            pixel_width: w.width,
            pixel_height: w.height,
        },
        Err(_) => {
            let (cols, rows) = terminal::size().unwrap_or((80, 24));
            PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            }
        }
    }
}

/// Runs `command` behind the leave keys, until it ends; its exit code is
/// the wrapper's, so the pane's shell sees how it went.
pub fn wrap(dock: &DockTo, command: &[String]) -> anyhow::Result<u8> {
    let (program, args) = command.split_first().context("nothing to run")?;
    let mut keys = DockKeys::new(&crate::keys::KeyChord::parse(&dock.prefix)?);
    let mut size = pane_size();
    let pty = native_pty_system().openpty(size)?;
    let mut cmd = CommandBuilder::new(program);
    cmd.args(args);
    cmd.cwd(std::env::current_dir()?);
    let mut child = pty
        .slave
        .spawn_command(cmd)
        .with_context(|| format!("cannot start {program}"))?;
    drop(pty.slave);
    let mut output = pty.master.try_clone_reader()?;
    let mut input = pty.master.take_writer()?;
    let mut killer = child.clone_killer();
    terminal::enable_raw_mode()?;

    // keys, as typed, to the command; a leave key goes to the toolbar instead
    let dock = dock.clone();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = [0u8; 4096];
        while let Ok(n) = stdin.read(&mut buf) {
            if n == 0 {
                break;
            }
            let (typed, leave) = keys.feed(&buf[..n]);
            if !typed.is_empty()
                && input
                    .write_all(&typed)
                    .and_then(|()| input.flush())
                    .is_err()
            {
                break;
            }
            if let Some(how) = leave {
                // the toolbar closes this pane; with no toolbar to tell, the
                // command ends here instead, and the pane is a shell's again
                if crate::relay::tell(&dock, how).is_err() {
                    let _ = killer.kill();
                }
                break;
            }
        }
    });
    // this terminal's size, whenever it changes
    let master = pty.master;
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(200));
            let now = pane_size();
            if now != size {
                size = now;
                if master.resize(now).is_err() {
                    break;
                }
            }
        }
    });
    // the command, until it ends
    let mut out = std::io::stdout().lock();
    let mut buf = [0u8; 65536];
    while let Ok(n) = output.read(&mut buf) {
        if n == 0 || out.write_all(&buf[..n]).and_then(|()| out.flush()).is_err() {
            break;
        }
    }
    let status = child.wait()?;
    let _ = out.write_all(crate::relay::SHELL_AGAIN);
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
    Ok(u8::try_from(status.exit_code()).unwrap_or(1))
}
