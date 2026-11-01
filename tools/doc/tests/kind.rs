//! The file-type matrix: how each document is shown, and which ones `v` edits.

use std::path::Path;

use herdr_doc::kind::{Kind, kind};
use herdr_doc::paths::{nvim_socket, toolbar_socket};

#[test]
fn a_documents_kind_comes_from_its_extension_in_any_case() {
    for (file, want) in [
        ("plan.md", Kind::Markdown),
        ("README.MARKDOWN", Kind::Markdown),
        ("data.json", Kind::Json),
        ("tsconfig.jsonc", Kind::Json),
        ("map.geojson", Kind::Json),
        ("people.csv", Kind::Csv),
        ("people.tsv", Kind::Tsv),
        ("shot.PNG", Kind::Image),
        ("a.jpg", Kind::Image),
        ("a.jpeg", Kind::Image),
        ("a.gif", Kind::Image),
        ("a.webp", Kind::Image),
        ("logo.svg", Kind::Svg),
        ("spec.pdf", Kind::Pdf),
        ("notes.txt", Kind::Other),
        ("Makefile", Kind::Other),
    ] {
        assert_eq!(kind(Path::new(file)), want, "{file}");
    }
}

#[test]
fn only_text_documents_with_a_formatted_view_are_editable() {
    for k in [Kind::Markdown, Kind::Json, Kind::Csv, Kind::Tsv] {
        assert!(k.editable(), "{k:?}");
    }
    for k in [Kind::Image, Kind::Svg, Kind::Pdf, Kind::Other] {
        assert!(!k.editable(), "{k:?}");
    }
}

fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |k| {
        vars.iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.to_string())
    }
}

#[test]
fn a_workspace_has_its_own_neovim_and_toolbar_sockets() {
    let home = [("HOME", "/h"), ("HERDR_WORKSPACE_ID", "wD")];
    assert_eq!(
        nvim_socket(env(&home)),
        Path::new("/h/.cache/nvim-editor-wD.sock")
    );
    assert_eq!(
        toolbar_socket(env(&home)).as_deref(),
        Some(Path::new(
            "/h/.local/state/herdr/plugins/jefelabs.toolbar/wD.sock"
        ))
    );
    let xdg = [
        ("HOME", "/h"),
        ("HERDR_WORKSPACE_ID", "w3"),
        ("XDG_CACHE_HOME", "/c"),
        ("XDG_STATE_HOME", "/s"),
    ];
    assert_eq!(nvim_socket(env(&xdg)), Path::new("/c/nvim-editor-w3.sock"));
    assert_eq!(
        toolbar_socket(env(&xdg)).as_deref(),
        Some(Path::new("/s/herdr/plugins/jefelabs.toolbar/w3.sock"))
    );
    // outside Herdr: the Editor's own socket name, and no toolbar
    assert_eq!(
        nvim_socket(env(&[("HOME", "/h")])),
        Path::new("/h/.cache/nvim-editor-local.sock")
    );
    assert_eq!(toolbar_socket(env(&[("HOME", "/h")])), None);
}

#[test]
fn text_reaches_neovim_as_a_quoted_string_never_as_code() {
    use herdr_doc::vim::string;
    assert_eq!(string("/w/plan.md").as_deref(), Some("'/w/plan.md'"));
    assert_eq!(
        string("/w/it's; :!rm|x").as_deref(),
        Some("'/w/it''s; :!rm|x'")
    );
    assert_eq!(string("/w/a\nb"), None, "a control character is refused");
}
