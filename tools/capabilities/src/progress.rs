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
        // a state gh could not give is not shown as one
        let unknown = prs.iter().any(|(_, s)| *s == PrState::Unknown);
        let label = match (prs.len(), unknown) {
            (1, true) => format!("PR #{}", prs[0].0),
            (1, false) => format!("PR #{} open", prs[0].0),
            (n, true) => format!("{n} PRs"),
            (n, false) => format!("PRs {merged}/{n} merged"),
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

/// How long since `since`, compactly: `<1m`, `45m`, `2h 14m`, `3d 4h`.
pub fn running_for(
    since: chrono::DateTime<chrono::Local>,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    let mins = (now - since).num_minutes();
    if mins < 1 {
        return "<1m".to_string();
    }
    let (d, h, m) = (mins / 1440, (mins % 1440) / 60, mins % 60);
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}

/// `#5830`, `owner/repo#12`, or a GitHub pull request URL.
pub fn pr_number(target: &str) -> Option<u64> {
    let t = target.trim();
    if let Some(n) = t.strip_prefix('#') {
        return n.trim().parse().ok();
    }
    if let Some((_, after)) = t.split_once("/pull/") {
        return leading_number(after);
    }
    // owner/repo#12
    let (_, n) = t.split_once('#')?;
    n.trim().parse().ok()
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

/// `gh pr view … --json state`, run in `repo`, for a `pr:` target as linked:
/// `#n` asks about this repo's PR, a URL is passed as is (it names its own
/// repo), and `owner/repo#n` adds `-R owner/repo`. Anything but a clear
/// answer is `Unknown`.
pub fn pr_state(gh: &Path, repo: &Path, target: &str, env: &[(String, String)]) -> PrState {
    let t = target.trim();
    let args: Vec<String> = if let Some(n) = t.strip_prefix('#') {
        vec![n.trim().to_string()]
    } else if t.contains("/pull/") {
        vec![t.to_string()]
    } else if let Some((slug, n)) = t.split_once('#') {
        vec![n.trim().to_string(), "-R".into(), slug.trim().to_string()]
    } else {
        return PrState::Unknown;
    };
    let view = ["pr".to_string(), "view".to_string(), args[0].clone()];
    let rest: Vec<String> = ["--json".to_string(), "state".to_string()]
        .into_iter()
        .chain(args[1..].iter().cloned())
        .collect();
    let out = Command::new(gh)
        .args(view.iter().chain(rest.iter()))
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

/// One agent as `herdr agent list` reports it, plus where it is working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInfo {
    /// `working`, `idle`, `blocked`, `done`, `unknown`.
    pub status: String,
    /// The worktree it is in: the area under `.claude/worktrees/` (e.g.
    /// `org/team`), else the worktree folder's name.
    pub worktree: Option<String>,
    /// That worktree's folder (the one holding `.slot`).
    pub worktree_path: Option<PathBuf>,
    /// The workbrokr slot from that worktree's `.slot` file.
    pub slot: Option<u32>,
}

/// The nearest folder at or above `cwd` holding a `.slot` file: its area name,
/// slot number and path. All `None` when there is none.
pub fn worktree_of(cwd: &Path) -> (Option<String>, Option<u32>, Option<PathBuf>) {
    for dir in cwd.ancestors().take(16) {
        let Ok(text) = std::fs::read_to_string(dir.join(".slot")) else {
            continue;
        };
        let slot = text.trim().parse().ok();
        let parts: Vec<String> = dir
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        let area = parts
            .windows(2)
            .position(|w| w[0] == ".claude" && w[1] == "worktrees")
            .map(|i| parts[i + 2..].join("/"))
            .filter(|a| !a.is_empty())
            .or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()));
        return (area, slot, Some(dir.to_path_buf()));
    }
    (None, None, None)
}

/// `herdr agent list` as `name → AgentInfo`. Agents without a name (not
/// started by this tool) are skipped. `None` when herdr cannot answer.
pub fn agent_statuses(
    herdr: &Path,
    env: &[(String, String)],
) -> Option<HashMap<String, AgentInfo>> {
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
                let cwd = a
                    .get("foreground_cwd")
                    .or_else(|| a.get("cwd"))
                    .and_then(|c| c.as_str());
                let (worktree, slot, worktree_path) =
                    cwd.map(|c| worktree_of(Path::new(c))).unwrap_or_default();
                Some((
                    name.to_string(),
                    AgentInfo {
                        status: status.to_string(),
                        worktree,
                        worktree_path,
                        slot,
                    },
                ))
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
    Agents(Option<HashMap<String, AgentInfo>>),
    Pr {
        repo: PathBuf,
        /// The `pr:` target as linked.
        target: String,
        state: PrState,
    },
    /// Plan tasks for the capability at `path` (reading plans runs git).
    Plans {
        path: PathBuf,
        plans: Vec<PlanTasks>,
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

    pub fn request_pr(&mut self, repo: PathBuf, target: String) {
        let key = format!("pr:{}:{target}", repo.display());
        if !self.in_flight.insert(key.clone()) {
            return;
        }
        let env = self.env.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let state = tool_bin(&env, "GH_BIN_PATH", Some("gh"))
                .map(|gh| pr_state(&gh, &repo, &target, &env))
                .unwrap_or(PrState::Unknown);
            let _ = tx.send((
                key,
                Update::Pr {
                    repo,
                    target,
                    state,
                },
            ));
        });
    }

    /// Read the plans of `intent` (git and file reads) off the UI thread.
    pub fn request_plans(&mut self, intent: Intent) {
        let key = format!("plans:{}", intent.path.display());
        if !self.in_flight.insert(key.clone()) {
            return;
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let plans = plan_evidence(intent.fm.repo.as_deref(), &intent);
            let _ = tx.send((
                key,
                Update::Plans {
                    path: intent.path,
                    plans,
                },
            ));
        });
    }

    /// Requests started and not yet drained.
    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
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
