//! What kind of document a file is: how it is shown, and whether `v` edits it.

use std::path::Path;

/// A document's kind, from its extension (any case).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `.md`, `.markdown`: rendered by mdcat.
    Markdown,
    /// `.json`, `.jsonc`, `.geojson`: indented and colored by jaq.
    Json,
    /// `.csv`: aligned columns by tidy-viewer.
    Csv,
    /// `.tsv`: aligned columns by tidy-viewer.
    Tsv,
    /// `.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`: block characters by viu.
    Image,
    /// `.svg`: drawn by resvg, shown by viu.
    Svg,
    /// `.pdf`: its text by pdftotext.
    Pdf,
    /// Anything else: shown as it is.
    Other,
}

/// The kind of the document at `path`.
pub fn kind(path: &Path) -> Kind {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" => Kind::Markdown,
        "json" | "jsonc" | "geojson" => Kind::Json,
        "csv" => Kind::Csv,
        "tsv" => Kind::Tsv,
        "png" | "jpg" | "jpeg" | "gif" | "webp" => Kind::Image,
        "svg" => Kind::Svg,
        "pdf" => Kind::Pdf,
        _ => Kind::Other,
    }
}

impl Kind {
    /// Whether `v` opens the file in Neovim: the text kinds whose formatted
    /// view is worth coming back to. The single source of truth for editing.
    pub fn editable(self) -> bool {
        matches!(self, Kind::Markdown | Kind::Json | Kind::Csv | Kind::Tsv)
    }
}
