//! The formatted view of a document, for `less` (`LESSOPEN="|herdr-doc
//! render %s"`): each kind through its renderer. A renderer that isn't
//! installed prints nothing, and `less` then shows the file as it is.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::kind::{Kind, kind};

/// The program `name` on `PATH`, if it is there.
fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(name))
            .find(|p| {
                std::fs::metadata(p).is_ok_and(|m| {
                    m.is_file()
                        && std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o111 != 0
                })
            })
    })
}

/// Runs `program args`, its output into `out`; nothing when it isn't there.
fn run(program: &str, args: &[&str], out: &mut dyn Write) -> anyhow::Result<()> {
    let Some(path) = which(program) else {
        return Ok(());
    };
    let done = Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    out.write_all(&done.stdout)?;
    Ok(())
}

/// Writes the formatted view of `path`, `cols` wide, into `out`.
pub fn render(path: &Path, cols: u16, out: &mut dyn Write) -> anyhow::Result<()> {
    let file = path.to_string_lossy();
    let cols = cols.to_string();
    match kind(path) {
        Kind::Markdown => run("mdcat", &["--ansi", "--columns", &cols, &file], out),
        Kind::Json => run("jaq", &["-C", ".", &file], out),
        Kind::Csv => run("tv", &["-a", &file], out),
        Kind::Tsv => run("tv", &["-a", "-s", "\t", &file], out),
        Kind::Image => run("viu", &["-b", "-w", &cols, &file], out),
        Kind::Svg => {
            if which("resvg").is_none() || which("viu").is_none() {
                return Ok(());
            }
            // drawn to a picture that is removed when this ends, whatever happens
            let picture = tempfile::Builder::new().suffix(".png").tempfile()?;
            let png = picture.path().to_string_lossy().into_owned();
            let drawn = Command::new("resvg")
                .args([&*file, &png])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;
            if drawn.success() {
                run("viu", &["-b", "-w", &cols, &png], out)?;
            }
            Ok(())
        }
        Kind::Pdf => run("pdftotext", &["-layout", &file, "-"], out),
        Kind::Other => Ok(()),
    }
}

/// How wide the view is: `HERDR_DOC_COLUMNS`, else the terminal's width
/// (`less` runs the renderer with its output piped, so it is asked of
/// `/dev/tty`), else `COLUMNS`, else 100.
pub fn columns(get: impl Fn(&str) -> Option<String>) -> u16 {
    let number = |v: Option<String>| {
        v.and_then(|c| c.trim().parse::<u16>().ok())
            .filter(|&c| c > 0)
    };
    number(get("HERDR_DOC_COLUMNS"))
        .or_else(tty_columns)
        .or_else(|| number(get("COLUMNS")))
        .unwrap_or(100)
}

/// The terminal's width, from `stty size` on `/dev/tty`.
fn tty_columns() -> Option<u16> {
    let tty = std::fs::File::open("/dev/tty").ok()?;
    let out = Command::new("stty")
        .arg("size")
        .stdin(tty)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace()
        .nth(1)?
        .parse()
        .ok()
        .filter(|&c: &u16| c > 0)
}
