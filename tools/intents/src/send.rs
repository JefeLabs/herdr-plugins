//! Send an intent to a new Herdr agent. Filled in by Task 7; stubs for now.

use std::path::{Path, PathBuf};

use crate::config::{Config, Handoff};
use crate::store::Intent;

pub struct SendOutcome {
    pub workspace_id: String,
    pub pane_id: String,
    pub agent: String,
}

#[derive(Debug)]
pub struct SendError {
    pub step: &'static str,
    pub detail: String,
}

pub fn main_checkout(from: &Path) -> anyhow::Result<PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(from)
        .output()?;
    if !out.status.success() {
        anyhow::bail!("not a git repository: {}", from.display());
    }
    let common = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let common = if common.is_absolute() {
        common
    } else {
        from.join(common)
    };
    let top = common.parent().map(Path::to_path_buf).unwrap_or(common);
    Ok(std::fs::canonicalize(&top).unwrap_or(top))
}

pub fn prepare(_cfg: &Config, h: &Handoff, it: &Intent) -> (PathBuf, String) {
    let repo = it.fm.repo.clone().unwrap_or_else(|| PathBuf::from("."));
    (repo, h.prompt.replace("{intent}", &it.body))
}

pub fn send_intent(
    _env: &[(String, String)],
    _repo: &Path,
    _it: &Intent,
    _kind: &str,
    _prompt: &str,
) -> Result<SendOutcome, SendError> {
    Err(SendError {
        step: "send",
        detail: "not implemented".into(),
    })
}
