//! Delivery progress of a sent capability: signals in (artifact lines, plan
//! tasks, pull request states, ticked criteria), a ten-cell bar and a label out.
//!
//! Stages Sent → Specced → Planned → Building → PR open → Merged are
//! cumulative: the furthest one with evidence sets the base, and earlier ones
//! count as reached. Ticked acceptance criteria fill only the Verified share
//! and never count as evidence for any earlier stage.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::store::{Intent, State, artifacts, criteria};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Merged,
    Closed,
    /// `gh` could not say; counts as open.
    Unknown,
}

/// Tasks of one linked plan. `readable: false` when the file could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanTasks {
    pub done: usize,
    pub total: usize,
    pub readable: bool,
}

/// What was gathered outside the capability file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Evidence {
    /// One entry per `plan:` line, in order.
    pub plans: Vec<PlanTasks>,
    /// One entry per `pr:` line whose number could be read.
    pub prs: Vec<(u64, PrState)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    /// 0.0 ..= 10.0, one unit per bar cell.
    pub fill: f32,
    pub label: String,
    pub complete: bool,
}

/// Stage weights, summing to 10.
const SENT: f32 = 1.0;
const SPECCED: f32 = 2.0;
const PLANNED: f32 = 3.0;
const BUILDING: f32 = 3.0;
const PR_OPEN: f32 = 7.0;
const MERGED: f32 = 8.0;
const VERIFIED: f32 = 2.0;

/// `None` unless the capability has been sent.
pub fn progress(it: &Intent, ev: &Evidence) -> Option<Progress> {
    if it.fm.state != State::Sent {
        return None;
    }
    let arts = artifacts(&it.body);
    let count = |kind: &str| {
        arts.iter()
            .filter(|a| a.kind.eq_ignore_ascii_case(kind))
            .count()
    };
    let crit = criteria(&it.body);
    let ticked = crit.iter().filter(|c| c.done).count();
    let complete = !crit.is_empty() && ticked == crit.len();
    let verified = if crit.is_empty() {
        0.0
    } else {
        VERIFIED * ticked as f32 / crit.len() as f32
    };

    let prs: Vec<(u64, PrState)> = ev
        .prs
        .iter()
        .copied()
        .filter(|(_, s)| *s != PrState::Closed)
        .collect();
    let merged = prs.iter().filter(|(_, s)| *s == PrState::Merged).count();

    let (base, label) = if !prs.is_empty() && merged == prs.len() {
        let label = if crit.is_empty() {
            "no criteria".to_string()
        } else {
            format!("verifying {ticked}/{}", crit.len())
        };
        (MERGED, label)
    } else if !prs.is_empty() {
        let label = if prs.len() == 1 {
            format!("PR #{} open", prs[0].0)
        } else {
            format!("PRs {merged}/{} merged", prs.len())
        };
        (PR_OPEN, label)
    } else if count("plan") > 0 {
        let readable: Vec<&PlanTasks> = ev.plans.iter().filter(|p| p.readable).collect();
        let total: usize = readable.iter().map(|p| p.total).sum();
        let done: usize = readable.iter().map(|p| p.done.min(p.total)).sum();
        let share = if total == 0 {
            0.0
        } else {
            BUILDING * done as f32 / total as f32
        };
        let label = if readable.len() < ev.plans.len() {
            "building ?".to_string()
        } else if total == 0 {
            "building".to_string()
        } else {
            format!("building {done}/{total}")
        };
        (PLANNED + share, label)
    } else if count("spec") > 0 || count("issue") > 0 {
        (SPECCED, "planning".to_string())
    } else {
        let decisions = count("decision");
        let label = if decisions > 0 {
            format!(
                "charting · {decisions} decision{}",
                if decisions == 1 { "" } else { "s" }
            )
        } else {
            "speccing".to_string()
        };
        (SENT, label)
    };

    Some(Progress {
        fill: (base + verified).min(10.0),
        label: if complete {
            "✓ complete".to_string()
        } else {
            label
        },
        complete,
    })
}

/// Tasks in a superpowers plan. With a ledger: `### Task <n>` headings against
/// distinct `Task <n>: complete` lines. Without one: ticked checkboxes against
/// all checkboxes in the plan.
pub fn plan_tasks(plan: &str, ledger: Option<&str>) -> PlanTasks {
    let headings: BTreeSet<u64> = plan
        .lines()
        .filter_map(|l| l.strip_prefix("### Task "))
        .filter_map(leading_number)
        .collect();
    if let Some(ledger) = ledger
        && !headings.is_empty()
    {
        let done: BTreeSet<u64> = ledger
            .lines()
            .filter_map(|l| l.strip_prefix("Task "))
            .filter_map(|rest| {
                let n = leading_number(rest)?;
                let after = rest.trim_start_matches(|c: char| c.is_ascii_digit());
                after.starts_with(": complete").then_some(n)
            })
            .filter(|n| headings.contains(n))
            .collect();
        return PlanTasks {
            done: done.len(),
            total: headings.len(),
            readable: true,
        };
    }
    let mut total = 0;
    let mut done = 0;
    for l in plan.lines().map(str::trim_start) {
        if l.starts_with("- [ ]") {
            total += 1;
        } else if l.starts_with("- [x]") || l.starts_with("- [X]") {
            total += 1;
            done += 1;
        }
    }
    PlanTasks {
        done,
        total,
        readable: true,
    }
}

fn leading_number(s: &str) -> Option<u64> {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// `#5830`, or a GitHub pull request URL.
pub fn pr_number(target: &str) -> Option<u64> {
    let t = target.trim();
    if let Some(n) = t.strip_prefix('#') {
        return n.trim().parse().ok();
    }
    let (_, after) = t.split_once("/pull/")?;
    leading_number(after)
}

/// `fill` (0..=10) drawn over `cells` cells: full blocks, one eighth-block
/// partial, then light shade.
pub fn bar(fill: f32, cells: usize) -> String {
    const EIGHTHS: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let scaled = (fill.clamp(0.0, 10.0) / 10.0) * cells as f32;
    let mut full = scaled.floor() as usize;
    let mut eighths = ((scaled - full as f32) * 8.0).round() as usize;
    if eighths == 8 {
        full += 1;
        eighths = 0;
    }
    let full = full.min(cells);
    let mut out = "█".repeat(full);
    let mut used = full;
    if eighths > 0 && used < cells {
        out.push(EIGHTHS[eighths - 1]);
        used += 1;
    }
    out.push_str(&"░".repeat(cells - used));
    out
}

// ---- gathering evidence -------------------------------------------------------

/// Tasks for every `plan:` line of `it`, in order. A plan is read from the
/// repo's main checkout or any of its worktrees; its ledger is the
/// `.superpowers/sdd/*/progress.md` (in any worktree) whose first line names it.
pub fn plan_evidence(repo: Option<&Path>, it: &Intent) -> Vec<PlanTasks> {
    let roots = repo.map(crate::send::worktrees).unwrap_or_default();
    artifacts(&it.body)
        .into_iter()
        .filter(|a| a.kind.eq_ignore_ascii_case("plan"))
        .map(|a| {
            let unreadable = PlanTasks {
                done: 0,
                total: 0,
                readable: false,
            };
            let path = match repo {
                Some(r) => crate::send::resolve_in_repo(r, &a.target),
                None => {
                    let p = PathBuf::from(shellexpand::tilde(a.target.trim()).into_owned());
                    (p.is_absolute() && p.exists()).then_some(p)
                }
            };
            let Some(text) = path.and_then(|p| std::fs::read_to_string(p).ok()) else {
                return unreadable;
            };
            let ledgers = ledgers_for(&roots, a.target.trim());
            if ledgers.is_empty() {
                return plan_tasks(&text, None);
            }
            // several worktrees may each hold a ledger: the furthest along wins
            ledgers
                .iter()
                .map(|l| plan_tasks(&text, Some(l)))
                .max_by_key(|t| t.done)
                .unwrap_or(unreadable)
        })
        .collect()
}

/// Ledger texts under `<root>/.superpowers/sdd/*/progress.md` whose first line
/// `# SDD ledger — plan: <path>` names `plan` (either path a suffix of the other).
fn ledgers_for(roots: &[PathBuf], plan: &str) -> Vec<String> {
    let mut out = Vec::new();
    for root in roots {
        let Ok(dirs) = std::fs::read_dir(root.join(".superpowers/sdd")) else {
            continue;
        };
        for d in dirs.flatten() {
            let Ok(text) = std::fs::read_to_string(d.path().join("progress.md")) else {
                continue;
            };
            let Some(named) = text
                .lines()
                .next()
                .and_then(|l| l.split_once("plan:"))
                .map(|(_, p)| p.trim().to_string())
            else {
                continue;
            };
            let (named, plan) = (Path::new(&named), Path::new(plan));
            if named.ends_with(plan) || plan.ends_with(named) {
                out.push(text);
            }
        }
    }
    out
}

/// `gh pr view <n> --json state`, run in `repo`. Anything but a clear answer
/// is `Unknown`.
pub fn pr_state(gh: &Path, repo: &Path, n: u64, env: &[(String, String)]) -> PrState {
    let out = Command::new(gh)
        .args(["pr", "view", &n.to_string(), "--json", "state"])
        .current_dir(repo)
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else {
        return PrState::Unknown;
    };
    if !out.status.success() {
        return PrState::Unknown;
    }
    let state = serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .ok()
        .and_then(|v| v.get("state").and_then(|s| s.as_str()).map(str::to_string));
    match state.as_deref() {
        Some("OPEN") => PrState::Open,
        Some("MERGED") => PrState::Merged,
        Some("CLOSED") => PrState::Closed,
        _ => PrState::Unknown,
    }
}

/// `herdr agent list` as `name → agent_status`. Agents without a name (not
/// started by this tool) are skipped. `None` when herdr cannot answer.
pub fn agent_statuses(herdr: &Path, env: &[(String, String)]) -> Option<HashMap<String, String>> {
    let out = Command::new(herdr)
        .args(["agent", "list"])
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let agents = v.pointer("/result/agents")?.as_array()?;
    Some(
        agents
            .iter()
            .filter_map(|a| {
                let name = a.get("name")?.as_str()?;
                let status = a.get("agent_status")?.as_str()?;
                Some((name.to_string(), status.to_string()))
            })
            .collect(),
    )
}

/// A tool's binary: `key` from `env` first, then the process environment,
/// else `default` (looked up on PATH). `None` when neither is set and there is
/// no default.
pub fn tool_bin(env: &[(String, String)], key: &str, default: Option<&str>) -> Option<PathBuf> {
    env.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .or_else(|| std::env::var(key).ok())
        .filter(|v| !v.is_empty())
        .or_else(|| default.map(str::to_string))
        .map(PathBuf::from)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    Agents(Option<HashMap<String, String>>),
    Pr {
        repo: PathBuf,
        n: u64,
        state: PrState,
    },
}

/// Runs `herdr` and `gh` on spawned threads and hands results back over a
/// channel, so the UI thread never waits. A request already in flight for the
/// same key is not started again.
pub struct Poller {
    env: Vec<(String, String)>,
    tx: Sender<(String, Update)>,
    rx: Receiver<(String, Update)>,
    in_flight: HashSet<String>,
}

impl Poller {
    pub fn new(env: Vec<(String, String)>) -> Self {
        let (tx, rx) = channel();
        Self {
            env,
            tx,
            rx,
            in_flight: HashSet::new(),
        }
    }

    pub fn request_agents(&mut self) {
        let key = "agents".to_string();
        if !self.in_flight.insert(key.clone()) {
            return;
        }
        let env = self.env.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result =
                tool_bin(&env, "HERDR_BIN_PATH", None).and_then(|bin| agent_statuses(&bin, &env));
            let _ = tx.send((key, Update::Agents(result)));
        });
    }

    pub fn request_pr(&mut self, repo: PathBuf, n: u64) {
        let key = format!("pr:{}:{n}", repo.display());
        if !self.in_flight.insert(key.clone()) {
            return;
        }
        let env = self.env.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let state = tool_bin(&env, "GH_BIN_PATH", Some("gh"))
                .map(|gh| pr_state(&gh, &repo, n, &env))
                .unwrap_or(PrState::Unknown);
            let _ = tx.send((key, Update::Pr { repo, n, state }));
        });
    }

    /// Every result that has arrived since the last call.
    pub fn drain(&mut self) -> Vec<Update> {
        let mut out = Vec::new();
        while let Ok((key, update)) = self.rx.try_recv() {
            self.in_flight.remove(&key);
            out.push(update);
        }
        out
    }
}
