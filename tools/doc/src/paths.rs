//! Where a workspace's Neovim and toolbar listen, from the environment a
//! Herdr pane gives every program (`HERDR_WORKSPACE_ID`).

use std::path::PathBuf;

fn base(get: &impl Fn(&str) -> Option<String>, xdg: &str, fallback: &str) -> PathBuf {
    get(xdg)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(get("HOME").unwrap_or_default()).join(fallback))
}

/// The workspace's Neovim socket: the Editor's, so one Neovim serves both
/// (`${XDG_CACHE_HOME:-~/.cache}/nvim-editor-<workspace>.sock`).
pub fn nvim_socket(get: impl Fn(&str) -> Option<String>) -> PathBuf {
    let ws = get("HERDR_WORKSPACE_ID")
        .filter(|w| !w.is_empty())
        .unwrap_or_else(|| "local".into());
    base(&get, "XDG_CACHE_HOME", ".cache").join(format!("nvim-editor-{ws}.sock"))
}

/// The workspace's toolbar control socket, which takes `select <tool>`;
/// none outside Herdr.
pub fn toolbar_socket(get: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let ws = get("HERDR_WORKSPACE_ID").filter(|w| !w.is_empty())?;
    Some(
        base(&get, "XDG_STATE_HOME", ".local/state")
            .join("herdr/plugins/jefelabs.toolbar")
            .join(format!("{ws}.sock")),
    )
}
