//! Send an intent to a new Herdr agent: resolve the repo, render the handoff
//! prompt (with the workbrokr preamble where it applies), and drive the Herdr
//! CLI: workspace create → agent start → agent prompt.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{Config, Handoff};
use crate::store::Intent;

/// Handoffs that build code; planning handoffs never get the work-start preamble.
const BUILDING_HANDOFFS: &[&str] = &["issue-report", "enhancement-spec"];

fn artifacts_preamble(intent: &Intent) -> String {
    format!(
        "Whenever you create something for this capability — a spec, a plan, an ADR or decision record, a wayfinder decision ticket, or a GitHub issue — append one line under `## Artifacts` in {} (create the section if missing): `- spec: <path>`, `- plan: <path>`, `- adr: <path>`, `- decision: #<n>`, `- issue: <url or #n>`, `- pr: #<n>` when you open a pull request, and `- demo: <story-id> (criteria: <numbers>)` for each app-demos story you add, naming the acceptance criteria it shows. Paths are relative to the repo root.\n\n",
        intent.path.display()
    )
}

/// Every hand-off: the happy path is settled, edge cases are the agent's to find.
pub const EDGE_CASES_PREAMBLE: &str = "The happy path is settled: every open question below has its answer. Edge cases are now yours to find. List the ones you see, ask about those that change what gets built, and record each decision as you go.\n\n";

const PRECONDITIONS_PREAMBLE: &str = "Before anything else, verify each item under Preconditions against the code and the data: tell me which hold, which do not, and ask about any you cannot settle. Do not build on a precondition you have not confirmed.\n\n";

const WORKBROKR_PREAMBLE: &str = "This repo uses workbrokr. Before touching code, run `work start --area <area> --type <fix|feature> --mode <fullstack|frontend>`; take the area from the intent's tags or group, and ask me if it is unclear.\n\n";

#[derive(Debug, Clone, PartialEq, Eq)]
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

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.step, self.detail)
    }
}

/// The main checkout: parent of `git rev-parse --git-common-dir`, canonical.
/// A worktree resolves to the checkout it belongs to.
pub fn main_checkout(from: &Path) -> anyhow::Result<PathBuf> {
    let out = Command::new("git")
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
    let common = std::fs::canonicalize(&common).unwrap_or(common);
    let top = common.parent().map(Path::to_path_buf).unwrap_or(common);
    Ok(std::fs::canonicalize(&top).unwrap_or(top))
}

/// Every working tree of `repo`, main checkout first (`git worktree list
/// --porcelain`), canonicalised. Just `[repo]` when git cannot answer.
pub fn worktrees(repo: &Path) -> Vec<PathBuf> {
    let out = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo)
        .output();
    let Ok(out) = out else {
        return vec![repo.to_path_buf()];
    };
    if !out.status.success() {
        return vec![repo.to_path_buf()];
    }
    let roots: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(|p| {
            let p = PathBuf::from(p);
            std::fs::canonicalize(&p).unwrap_or(p)
        })
        .collect();
    if roots.is_empty() {
        vec![repo.to_path_buf()]
    } else {
        roots
    }
}

/// A linked target as a file on disk: `~` and absolute paths as written (if
/// they exist), otherwise the first `<worktree>/<target>` that exists, main
/// checkout first. Specs and plans usually live in a worktree until merged.
pub fn resolve_in_repo(repo: &Path, target: &str) -> Option<PathBuf> {
    let p = PathBuf::from(shellexpand::tilde(target.trim()).into_owned());
    if p.is_absolute() {
        return p.exists().then_some(p);
    }
    if !repo.is_dir() {
        return None;
    }
    worktrees(repo)
        .into_iter()
        .map(|root| root.join(&p))
        .find(|candidate| candidate.exists())
}

pub fn uses_workbrokr(repo: &Path, override_: Option<bool>) -> bool {
    match override_ {
        Some(v) => v,
        None => repo.join("helmsmith.config.ts").is_file(),
    }
}

pub fn render_prompt(
    h: &Handoff,
    intent: &Intent,
    attachments: &[(String, u64)],
    workbrokr: bool,
) -> String {
    let atts = if attachments.is_empty() {
        "(none)".to_string()
    } else {
        attachments
            .iter()
            .map(|(n, _)| format!("- {}", intent.assets_dir().join(n).display()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let intent_text = format!("# {}\n\n{}", intent.fm.title, intent.body.trim_end());
    let mut p = h
        .prompt
        .replace("\\n", "\n")
        .replace("{intent}", &intent_text)
        .replace("{attachments}", &atts);
    p = format!("{EDGE_CASES_PREAMBLE}{}{p}", artifacts_preamble(intent));
    let areas: Vec<&str> = intent
        .fm
        .areas
        .iter()
        .map(|a| a.trim().trim_matches('/'))
        .filter(|a| !a.is_empty())
        .collect();
    if !areas.is_empty() {
        p = format!(
            "Besides its own group, this capability also affects these areas: {}. A change that spans areas may need one work area per area.\n\n{p}",
            areas.join(", ")
        );
    }
    if !crate::store::preconditions(&intent.body).is_empty() {
        p = format!("{PRECONDITIONS_PREAMBLE}{p}");
    }
    if workbrokr && BUILDING_HANDOFFS.contains(&h.name.as_str()) {
        p = format!("{WORKBROKR_PREAMBLE}{p}");
    }
    p
}

/// Repo and rendered prompt for the confirm screen. An intent without a repo
/// cannot be sent: guessing would fire an agent at the wrong checkout.
pub fn prepare(
    cfg: &Config,
    h: &Handoff,
    it: &Intent,
    attachments: &[(String, u64)],
) -> Result<(PathBuf, String), String> {
    let repo = it.fm.repo.clone().ok_or_else(|| {
        "capability has no repo: set `repo:` in its front matter to the main checkout to send it"
            .to_string()
    })?;
    if !repo.is_dir() {
        return Err(format!("repo {} does not exist", repo.display()));
    }
    let prompt = render_prompt(h, it, attachments, uses_workbrokr(&repo, cfg.workbrokr));
    Ok((repo, prompt))
}

/// The directory a capture should record as `repo`: Herdr's `workspace_cwd`
/// when the tool runs as a plugin (actions may start in the plugin dir), else
/// the current directory; always resolved to the main checkout.
pub fn capture_repo() -> Option<PathBuf> {
    let from_herdr = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .ok()
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| {
            v.get("workspace_cwd")
                .and_then(|w| w.as_str())
                .map(PathBuf::from)
        });
    let start = from_herdr.or_else(|| std::env::current_dir().ok())?;
    main_checkout(&start).ok()
}

/// `intent-<slug prefix>-<4 random chars>`, valid for Herdr agent names
/// (`[a-z][a-z0-9_-]{0,31}`) and unique per send, so sending the same intent
/// twice never collides.
pub fn agent_name(slug: &str) -> String {
    let cleaned: String = slug
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let mut head = cleaned[..cleaned.len().min(20)]
        .trim_end_matches('-')
        .to_string();
    if head.is_empty() {
        head = "x".to_string();
    }
    let suffix: String = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(4)
        .collect();
    format!("intent-{head}-{suffix}")
}

fn herdr(
    bin: &Path,
    env: &[(String, String)],
    args: &[&str],
    step: &'static str,
) -> Result<serde_json::Value, SendError> {
    let mut cmd = Command::new(bin);
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().map_err(|e| SendError {
        step,
        detail: format!("cannot run {}: {e}", bin.display()),
    })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let detail = serde_json::from_str::<serde_json::Value>(err.trim())
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| err.trim().to_string());
        return Err(SendError {
            step,
            detail: if detail.is_empty() {
                format!("exit {}", out.status)
            } else {
                detail
            },
        });
    }
    Ok(
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim())
            .unwrap_or(serde_json::Value::Null),
    )
}

/// workspace create → agent start → agent prompt. Any failure names its step;
/// the caller leaves the intent untouched on `Err`.
pub fn send(
    herdr_bin: &Path,
    env: &[(String, String)],
    repo: &Path,
    title: &str,
    provider_kind: &str,
    prompt: &str,
) -> Result<SendOutcome, SendError> {
    let cwd = repo.display().to_string();
    let created = herdr(
        herdr_bin,
        env,
        &[
            "workspace",
            "create",
            "--cwd",
            &cwd,
            "--label",
            title,
            "--no-focus",
        ],
        "workspace create",
    )?;
    let workspace_id = created
        .pointer("/result/workspace/workspace_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let pane_id = created
        .pointer("/result/root_pane/pane_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SendError {
            step: "workspace create",
            detail: "no root pane in reply".into(),
        })?
        .to_string();
    let agent = agent_name(&crate::store::slugify(title));
    let orphan = |mut e: SendError| {
        e.detail = format!(
            "{} (workspace {workspace_id} was created and is left open)",
            e.detail
        );
        e
    };
    herdr(
        herdr_bin,
        env,
        &[
            "agent",
            "start",
            &agent,
            "--kind",
            provider_kind,
            "--pane",
            &pane_id,
        ],
        "agent start",
    )
    .map_err(orphan)?;
    herdr(
        herdr_bin,
        env,
        &["agent", "prompt", &agent, prompt],
        "agent prompt",
    )
    .map_err(orphan)?;
    Ok(SendOutcome {
        workspace_id,
        pane_id,
        agent,
    })
}

/// App entry point: finds the `herdr` binary from `HERDR_BIN_PATH` (the env
/// list first, then the process environment), else fails before any call.
pub fn send_intent(
    env: &[(String, String)],
    repo: &Path,
    it: &Intent,
    provider_kind: &str,
    prompt: &str,
) -> Result<SendOutcome, SendError> {
    let bin = env
        .iter()
        .find(|(k, _)| k == "HERDR_BIN_PATH")
        .map(|(_, v)| v.clone())
        .or_else(|| std::env::var("HERDR_BIN_PATH").ok())
        .filter(|b| !b.is_empty())
        .ok_or_else(|| SendError { step: "herdr", detail: "Herdr is required to send: HERDR_BIN_PATH is not set (run inside a Herdr pane or the toolbar)".into() })?;
    send(
        Path::new(&bin),
        env,
        repo,
        &it.fm.title,
        provider_kind,
        prompt,
    )
}
