//! Reconcile: once every pull request of a sent capability has merged, its
//! text is brought in line with what shipped. The tool gathers the spec, the
//! plans, the merged pull requests and the follow-up issues, asks the
//! refinement agent for a rewrite, applies it (keeping the previous version
//! beside the file for undo) and marks the capability unread.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::progress::{Evidence, PrState};
use crate::store::{Intent, State, Store, artifacts};

/// Material the rewrite is based on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Context {
    pub spec: Option<String>,
    pub plans: Vec<String>,
    pub prs: Vec<ShippedPr>,
    pub follow_ups: Vec<FollowUp>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShippedPr {
    pub number: u64,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FollowUp {
    pub number: u64,
    pub title: String,
    pub state: String,
}

/// The merged pull requests, sorted, when every remaining one is merged (a
/// PR closed without merging is ignored); `None` while any is open or unknown.
pub fn merged_set(ev: &Evidence) -> Option<Vec<u64>> {
    let live: Vec<&(u64, PrState)> = ev
        .prs
        .iter()
        .filter(|(_, s)| *s != PrState::Closed)
        .collect();
    if live.is_empty() || live.iter().any(|(_, s)| *s != PrState::Merged) {
        return None;
    }
    let mut set: Vec<u64> = live.iter().map(|(n, _)| *n).collect();
    set.sort_unstable();
    set.dedup();
    Some(set)
}

/// The merged set to reconcile for, when a sent capability's pull requests
/// are all merged and it has not been reconciled for exactly this set.
pub fn needs_reconcile(it: &Intent, ev: &Evidence) -> Option<Vec<u64>> {
    if it.fm.state != State::Sent {
        return None;
    }
    let set = merged_set(ev)?;
    (set != it.fm.reconciled_for).then_some(set)
}

const SPEC_LIMIT: usize = 12_000;
const PR_BODY_LIMIT: usize = 3_000;

fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let cut: String = text.chars().take(limit).collect();
    format!("{cut}\n[… cut]")
}

/// The request to the refinement agent; the reply follows the usual contract
/// (a fenced `proposal` block with the whole body, then `questions:`).
pub fn reconcile_prompt(it: &Intent, ctx: &Context) -> String {
    let mut p = String::new();
    p.push_str("Reconcile this capability with the work that shipped. Every pull request below is merged. Rewrite the capability so it describes what was actually built and decided:\n");
    p.push_str("- Rewrite `## Acceptance criteria` to match the shipped behaviour: keep criteria that still hold (keep `[x]` on ticked ones), correct those the work changed, add ones the spec or the pull requests settled, and drop ones that were not built, naming each dropped one under `## Open questions` so nothing disappears silently.\n");
    p.push_str("- When the work spans more than one of the capability's areas, group the criteria under `### <area path>` sub-headings, one per area.\n");
    p.push_str("- Keep `## Description` (update it if the scope changed), `## Preconditions` and every line under `## Artifacts`.\n");
    p.push_str("- Do not turn follow-up issues into criteria: they are separate work.\n");
    p.push_str("- Use only what the material below says; do not invent behaviour.\n\n");
    p.push_str(crate::conversation::FORMAT_REMINDER);
    p.push_str("\n\nThe capability as it stands (title: ");
    p.push_str(&it.fm.title);
    p.push_str("):\n\n");
    p.push_str(it.body.trim_end());
    p.push_str("\n\n");
    if !it.fm.areas.is_empty() {
        p.push_str("Areas it affects besides its own group: ");
        p.push_str(&it.fm.areas.join(", "));
        p.push_str("\n\n");
    }
    if let Some(spec) = &ctx.spec {
        p.push_str("The spec:\n\n");
        p.push_str(&clip(spec, SPEC_LIMIT));
        p.push_str("\n\n");
    }
    if !ctx.plans.is_empty() {
        p.push_str("Plans: ");
        p.push_str(&ctx.plans.join(", "));
        p.push_str("\n\n");
    }
    if !ctx.prs.is_empty() {
        p.push_str("Merged pull requests:\n\n");
        for pr in &ctx.prs {
            p.push_str(&format!(
                "#{} {}\n{}\n\n",
                pr.number,
                pr.title,
                clip(&pr.body, PR_BODY_LIMIT)
            ));
        }
    }
    if !ctx.follow_ups.is_empty() {
        p.push_str("Follow-up issues (separate work, not criteria):\n");
        for f in &ctx.follow_ups {
            p.push_str(&format!("- #{} {} ({})\n", f.number, f.title, f.state));
        }
    }
    p
}

fn gh_json(
    gh: &Path,
    repo: &Path,
    args: &[&str],
    env: &[(String, String)],
) -> Option<serde_json::Value> {
    let out = Command::new(gh)
        .args(args)
        .current_dir(repo)
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

fn str_field(v: &Option<serde_json::Value>, key: &str) -> String {
    v.as_ref()
        .and_then(|v| v.get(key))
        .and_then(|s| s.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Read the spec, and ask `gh` about each merged pull request and each
/// `issue:` line. Runs off the UI thread; anything unreadable is left out.
pub fn gather(
    repo: &Path,
    it: &Intent,
    merged: &[u64],
    gh: &Path,
    env: &[(String, String)],
) -> Context {
    let arts = artifacts(&it.body);
    let spec = arts
        .iter()
        .find(|a| a.kind.eq_ignore_ascii_case("spec"))
        .and_then(|a| crate::send::resolve_in_repo(repo, &a.target))
        .and_then(|p| std::fs::read_to_string(p).ok());
    let plans = arts
        .iter()
        .filter(|a| a.kind.eq_ignore_ascii_case("plan"))
        .filter_map(|a| {
            Path::new(a.target.trim())
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
        })
        .collect();
    let prs = merged
        .iter()
        .map(|n| {
            let v = gh_json(
                gh,
                repo,
                &["pr", "view", &n.to_string(), "--json", "title,body"],
                env,
            );
            ShippedPr {
                number: *n,
                title: str_field(&v, "title"),
                body: str_field(&v, "body"),
            }
        })
        .collect();
    let follow_ups = arts
        .iter()
        .filter(|a| a.kind.eq_ignore_ascii_case("issue"))
        .filter_map(|a| crate::progress::pr_number(&a.target))
        .map(|n| {
            let v = gh_json(
                gh,
                repo,
                &["issue", "view", &n.to_string(), "--json", "title,state"],
                env,
            );
            FollowUp {
                number: n,
                title: str_field(&v, "title"),
                state: str_field(&v, "state"),
            }
        })
        .collect();
    Context {
        spec,
        plans,
        prs,
        follow_ups,
    }
}

/// Where the version before a reconcile is kept: `<stem>.before-reconcile.bak`
/// beside the capability (not `.md`, so the store never lists it).
pub fn backup_path(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    path.with_file_name(format!("{stem}.before-reconcile.bak"))
}

/// Replace the body with the reconciled one: keep the current file as the
/// backup, record the merged set, and mark the capability unread.
pub fn apply(path: &Path, body: &str, merged: &[u64]) -> anyhow::Result<Intent> {
    std::fs::copy(path, backup_path(path))?;
    let mut it = Store::load(path)?;
    it.body = body.to_string();
    it.fm.reconciled_for = merged.to_vec();
    it.fm.unread = true;
    Store::save_force(&mut it)?;
    Ok(it)
}

/// Put the version before the last reconcile back. `false` when there is none.
pub fn undo(path: &Path) -> anyhow::Result<bool> {
    let backup = backup_path(path);
    if !backup.exists() {
        return Ok(false);
    }
    std::fs::copy(&backup, path)?;
    std::fs::remove_file(&backup)?;
    Ok(true)
}
