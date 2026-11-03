//! The pop-out: the Editor's Neovim shown in a Herdr pane of its own.
//!
//! The pane (`herdr-doc popout`) claims a marker beside the Neovim socket
//! naming its process; while a live process holds it, the toolbar's slot
//! lets go of Neovim (a session shown in two places is only as big as the
//! smaller one). A pane closed by Herdr dies without cleaning up, so a
//! marker naming a dead process counts as no pop-out.

use std::path::{Path, PathBuf};

/// The marker beside `socket`: `<socket>.popout`.
pub fn marker(socket: &Path) -> PathBuf {
    let mut name = socket.as_os_str().to_owned();
    name.push(".popout");
    PathBuf::from(name)
}

/// The process named by `socket`'s marker, if any.
pub fn holder(socket: &Path) -> Option<i32> {
    std::fs::read_to_string(marker(socket))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Whether a pop-out holds `socket`'s Neovim: a marker naming a process
/// that `alive` says is running.
pub fn popped_out(socket: &Path, alive: impl Fn(i32) -> bool) -> bool {
    holder(socket).is_some_and(alive)
}

/// Marks `socket`'s Neovim as shown by process `pid`. Written whole, then
/// renamed into place, so a reader never sees half a number.
pub fn claim(socket: &Path, pid: i32) -> std::io::Result<()> {
    let to = marker(socket);
    let mut tmp = to.as_os_str().to_owned();
    tmp.push(format!(".{pid}"));
    std::fs::write(&tmp, format!("{pid}\n"))?;
    std::fs::rename(&tmp, &to)
}

/// Removes the marker, only while it still names `pid`: a newer pop-out's
/// marker stays.
pub fn release(socket: &Path, pid: i32) {
    if holder(socket) == Some(pid) {
        let _ = std::fs::remove_file(marker(socket));
    }
}

/// Whether process `pid` is running (it exists, ours or not).
pub fn alive(pid: i32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    pid > 0 && !matches!(kill(Pid::from_raw(pid), None), Err(Errno::ESRCH))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sock(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("nvim-editor-w7.sock")
    }

    #[test]
    fn no_marker_is_no_popout() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!popped_out(&sock(&tmp), |_| true));
    }

    #[test]
    fn a_claim_by_a_live_process_is_a_popout() {
        let tmp = tempfile::tempdir().unwrap();
        claim(&sock(&tmp), 4242).unwrap();
        assert_eq!(holder(&sock(&tmp)), Some(4242));
        assert!(popped_out(&sock(&tmp), |pid| pid == 4242));
    }

    #[test]
    fn a_claim_by_a_dead_process_is_no_popout() {
        let tmp = tempfile::tempdir().unwrap();
        claim(&sock(&tmp), 4242).unwrap();
        assert!(!popped_out(&sock(&tmp), |_| false));
    }

    #[test]
    fn release_keeps_a_newer_claim() {
        let tmp = tempfile::tempdir().unwrap();
        claim(&sock(&tmp), 1).unwrap();
        claim(&sock(&tmp), 2).unwrap();
        release(&sock(&tmp), 1);
        assert_eq!(holder(&sock(&tmp)), Some(2));
        release(&sock(&tmp), 2);
        assert_eq!(holder(&sock(&tmp)), None);
    }

    #[test]
    fn this_process_is_alive_and_a_reaped_one_is_not() {
        assert!(alive(std::process::id() as i32));
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        assert!(!alive(pid));
    }
}
