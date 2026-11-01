//! `herdr-doc render`: each kind through its renderer; a missing renderer
//! prints nothing (so `less` shows the file as it is).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Bin {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    log: PathBuf,
}

/// A folder of stand-in renderers, each logging how it was called.
fn bin(tools: &[&str]) -> Bin {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("bin");
    fs::create_dir_all(&dir).unwrap();
    let log = tmp.path().join("calls.log");
    for t in tools {
        let body = match *t {
            // resvg IN OUT: writes OUT
            "resvg" => format!(
                "#!/bin/sh\necho \"resvg $*\" >> '{}'\necho png > \"$2\"\n",
                log.display()
            ),
            // viu: says what it would show, and whether the file is there
            "viu" => format!(
                "#!/bin/sh\necho \"viu $*\" >> '{}'\nfor a; do last=$a; done\n[ -f \"$last\" ] && echo \"IMAGE $last\"\n",
                log.display()
            ),
            _ => format!(
                "#!/bin/sh\necho \"{t} $*\" >> '{}'\necho \"RENDERED by {t}\"\n",
                log.display()
            ),
        };
        let path = dir.join(t);
        fs::write(&path, body).unwrap();
        let mut perm = fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
        fs::set_permissions(&path, perm).unwrap();
    }
    Bin {
        _tmp: tmp,
        dir,
        log,
    }
}

fn render(b: &Bin, file: &Path) -> (String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_herdr-doc"))
        .args(["render", &file.display().to_string()])
        .env("PATH", format!("{}:/usr/bin:/bin", b.dir.display()))
        .env("HERDR_DOC_COLUMNS", "80")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let calls = fs::read_to_string(&b.log).unwrap_or_default();
    (String::from_utf8_lossy(&out.stdout).into_owned(), calls)
}

fn doc(b: &Bin, name: &str, text: &str) -> PathBuf {
    let p = b.dir.parent().unwrap().join(name);
    fs::write(&p, text).unwrap();
    p
}

#[test]
fn each_text_kind_goes_through_its_renderer_at_the_panes_width() {
    let b = bin(&["mdcat", "jaq", "tv"]);
    for (name, want_call) in [
        ("plan.md", "mdcat --ansi --columns 80 "),
        ("data.json", "jaq -C . "),
        ("people.csv", "tv -a "),
        ("people.tsv", "tv -a -s \t "),
    ] {
        let file = doc(&b, name, "x");
        let (out, calls) = render(&b, &file);
        assert!(out.contains("RENDERED by"), "{name}: {out}");
        assert!(
            calls.lines().last().unwrap_or("").starts_with(want_call)
                && calls
                    .lines()
                    .last()
                    .unwrap()
                    .ends_with(&file.display().to_string()),
            "{name}: {calls:?}"
        );
    }
}

#[test]
fn a_missing_renderer_prints_nothing_so_less_shows_the_file() {
    let b = bin(&[]);
    for name in ["plan.md", "data.json", "notes.txt", "shot.png"] {
        let (out, _) = render(&b, &doc(&b, name, "x"));
        assert_eq!(out, "", "{name}");
    }
}

#[test]
fn an_svg_is_drawn_to_a_picture_that_is_gone_afterwards() {
    let b = bin(&["resvg", "viu"]);
    let file = doc(&b, "logo.svg", "<svg/>");
    let (out, calls) = render(&b, &file);
    let png = out
        .trim()
        .strip_prefix("IMAGE ")
        .expect("viu saw the picture")
        .to_string();
    assert!(png.ends_with(".png"), "{png}");
    assert!(
        !Path::new(&png).exists(),
        "the temporary picture is removed"
    );
    assert!(
        calls.contains(&format!("resvg {} {png}", file.display())),
        "{calls}"
    );
    assert!(calls.contains(&format!("viu -b -w 80 {png}")), "{calls}");
}

#[test]
fn images_and_pdfs_are_shown_by_their_tools() {
    let b = bin(&["viu", "pdftotext"]);
    let png = doc(&b, "shot.PNG", "x");
    let (out, calls) = render(&b, &png);
    assert!(out.contains("IMAGE"), "{out}");
    assert!(
        calls.contains(&format!("viu -b -w 80 {}", png.display())),
        "{calls}"
    );
    let pdf = doc(&b, "spec.pdf", "x");
    let (_, calls) = render(&b, &pdf);
    assert!(
        calls.contains(&format!("pdftotext -layout {} -", pdf.display())),
        "{calls}"
    );
}
