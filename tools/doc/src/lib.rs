//! herdr-doc: a document shown formatted (Markdown, JSON, CSV/TSV, images,
//! SVG, PDF) in one Neovim session, toggled to Neovim's own buffer to edit
//! it. An agent opens a file with `herdr-doc open <file>`.

pub mod kind;
pub mod nvim;
pub mod paths;
pub mod render;
pub mod vim;
