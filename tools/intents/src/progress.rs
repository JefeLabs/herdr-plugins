//! Delivery progress of a sent intent: signals in (artifact lines, plan
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

/// Tasks of one linked plan. `readable: false` when the file could not be
/// read; `counted: false` when no ledger says how many are done (the plan
/// executor deletes its ledger once a plan finishes).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanTasks {
    pub done: usize,
    pub total: usize,
    pub readable: bool,
    /// A slice the spec plans for later: a `plan:` line with no path yet.
    pub unplanned: bool,
    pub counted: bool,
    /// Distinct files the plan names in its `- Create/Modify/Test:` lines.
    pub files: usize,
    /// The state of the pull request found by this plan's file name, if any
    /// (`Open` when any is open, else `Merged` when any is merged).
    pub pr: Option<PrState>,
}

impl PlanTasks {
    pub fn size(&self) -> Option<Size> {
        plan_size(self.total, self.files)
    }
}

/// A plan's size, from its tasks and the files it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Size {
    S,
    M,
    L,
    XL,
}

impl Size {
    /// Cells in the bar, and the plan's weight in the building share.
    pub fn cells(self) -> usize {
        match self {
            Size::S => 1,
            Size::M => 2,
            Size::L => 3,
            Size::XL => 4,
        }
    }
}

/// S up to 3 tasks or 8 files, M up to 6 or 20, L up to 10 or 40, XL beyond;
/// the larger of the two scales wins. A plan with neither has no size.
pub fn plan_size(tasks: usize, files: usize) -> Option<Size> {
    if tasks == 0 && files == 0 {
        return None;
    }
    let by_tasks = match tasks {
        0..=3 => Size::S,
        4..=6 => Size::M,
        7..=10 => Size::L,
        _ => Size::XL,
    };
    let by_files = match files {
        0..=8 => Size::S,
        9..=20 => Size::M,
        21..=40 => Size::L,
        _ => Size::XL,
    };
    Some(by_tasks.max(by_files))
}

/// A pull request found by searching for a file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundPr {
    pub number: u64,
    pub state: PrState,
    /// Its head branch, e.g. `feature/org/candidates--meeting-request-org`.
    pub branch: String,
    /// Its title; empty when `gh` gave none.
    pub title: String,
}

/// Pull requests found per spec or plan file name.
pub type FoundByName = Vec<(String, Vec<FoundPr>)>;

/// The work area a workbrokr branch names: `<type>/<area>--<task>` gives
/// `<area>` (e.g. `org/candidates`). `None` for any other branch shape.
pub fn branch_area(branch: &str) -> Option<String> {
    let (_, rest) = branch.split_once('/')?;
    let (area, _) = rest.split_once("--")?;
    let area = area.trim_matches('/');
    (!area.is_empty()).then(|| area.to_string())
}

/// One plan's block in the building section of the bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanBlock {
    pub cells: usize,
    /// 0.0 ..= 1.0 done.
    pub fill: f32,
    /// Its pull request is open: built, waiting for review.
    pub in_review: bool,
}

/// What was gathered outside the intent file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Evidence {
    /// One entry per `plan:` line, in order.
    pub plans: Vec<PlanTasks>,
    /// One entry per `pr:` line whose number could be read.
    pub prs: Vec<(u64, PrState)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    /// 0.0 ..= 10.0, one unit per bar cell (used for sorting).
    pub fill: f32,
    pub label: String,
    pub complete: bool,
    /// How full each stage is, 0.0 ..= 1.0: sent, specced, planned,
    /// building, PR open, merged, verified.
    pub segments: [f32; 7],
    /// The building stage as one block per sized plan; empty when no linked
    /// plan has a size, and the building segment is drawn as three cells.
    pub plans: Vec<PlanBlock>,
}

/// Cells per stage in the bar, in `Progress::segments` order.
const STAGE_CELLS: [usize; 7] = [1, 1, 1, 3, 1, 1, 2];

/// What a bar cell stands for. Each kind has its own glyph (done `█`, in
/// review `▓`, to do `░`) because Herdr draws block glyphs in the default
/// colour, dropping colour and dim; the shades are what reliably shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Done,
    Todo,
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarCell {
    pub ch: char,
    pub kind: CellKind,
}

/// The bar as cells, one block per stage, with no gap cells: the building
/// stage is one block per sized plan. A done block (but the last) ends in a
/// seven-eighths cell (`▉`) whose thin sliver is the border where it meets
/// the next block; elsewhere the change of shade divides blocks.
pub fn stage_cells(p: &Progress) -> Vec<BarCell> {
    const EIGHTHS: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    // a block `cells` wide, `fill` done: full cells, one partial, then to-do cells
    let block = |fill: f32, cells: usize, review: bool| -> Vec<BarCell> {
        if review {
            return vec![
                BarCell {
                    ch: '▓',
                    kind: CellKind::Review,
                };
                cells
            ];
        }
        let scaled = fill.clamp(0.0, 1.0) * cells as f32;
        let mut full = scaled.floor() as usize;
        let mut eighths = ((scaled - full as f32) * 8.0).round() as usize;
        if eighths == 8 {
            full += 1;
            eighths = 0;
        }
        let full = full.min(cells);
        let mut out = vec![
            BarCell {
                ch: '█',
                kind: CellKind::Done,
            };
            full
        ];
        if eighths > 0 && out.len() < cells {
            out.push(BarCell {
                ch: EIGHTHS[eighths - 1],
                kind: CellKind::Done,
            });
        }
        while out.len() < cells {
            out.push(BarCell {
                ch: '░',
                kind: CellKind::Todo,
            });
        }
        out
    };
    let seg = |i: usize| block(p.segments[i], STAGE_CELLS[i], false);
    let mut blocks = vec![seg(0), seg(1), seg(2)];
    if p.plans.is_empty() {
        blocks.push(seg(3));
    } else {
        blocks.extend(p.plans.iter().map(|b| block(b.fill, b.cells, b.in_review)));
    }
    blocks.extend([seg(4), seg(5), seg(6)]);
    let last = blocks.len() - 1;
    let mut out = Vec::new();
    for (i, mut cells) in blocks.into_iter().enumerate() {
        if i < last
            && let Some(end) = cells.last_mut()
            && end.ch == '█'
        {
            end.ch = '▉';
        }
        out.extend(cells);
    }
    out
}

/// The bar's glyphs alone (kinds dropped).
pub fn stage_bar(p: &Progress) -> String {
    stage_cells(p).iter().map(|c| c.ch).collect()
}

/// Stage weights, summing to 10.
const SENT: f32 = 1.0;
const SPECCED: f32 = 2.0;
const PLANNED: f32 = 3.0;
const BUILDING: f32 = 3.0;
const PR_OPEN: f32 = 7.0;
const MERGED: f32 = 8.0;
const VERIFIED: f32 = 2.0;

/// `None` unless the intent has been sent.
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
    // slices the spec plans for later hold the intent at building, even
    // once the planned slices' pull requests are open or merged
    let to_plan = ev.plans.iter().filter(|p| p.unplanned).count();

    let mut building_share = 0.0f32;
    let (base, label) = if to_plan == 0 && !prs.is_empty() && merged == prs.len() {
        let label = if crit.is_empty() {
            "no criteria".to_string()
        } else {
            format!("verifying {ticked}/{}", crit.len())
        };
        (MERGED, label)
    } else if to_plan == 0 && !prs.is_empty() {
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
        let planned = ev.plans.len() - to_plan;
        let readable: Vec<&PlanTasks> = ev
            .plans
            .iter()
            .filter(|p| p.readable && !p.unplanned)
            .collect();
        // a plan without a ledger has an unknown done count: guess nothing
        let uncounted = readable.iter().any(|p| !p.counted && p.total > 0);
        let total: usize = readable.iter().map(|p| p.total).sum();
        let done: usize = readable.iter().map(|p| p.done.min(p.total)).sum();
        building_share = if total == 0 || uncounted {
            0.0
        } else {
            // each slice still to plan counts as one more plan not started
            done as f32 / total as f32 * planned as f32 / (planned + to_plan) as f32
        };
        let label = if readable.len() < planned {
            "building ?".to_string()
        } else if total == 0 || uncounted {
            "building".to_string()
        } else {
            format!("building {done}/{total}")
        };
        let label = if to_plan > 0 {
            format!("{label} · {to_plan} to plan")
        } else {
            label
        };
        (PLANNED + BUILDING * building_share, label)
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

    // one block per sized plan, each filled by its own evidence
    let plans: Vec<PlanBlock> = ev
        .plans
        .iter()
        .filter(|p| p.readable)
        .filter_map(|p| {
            let size = p.size()?;
            let fill = if base >= MERGED || p.pr == Some(PrState::Merged) {
                1.0
            } else if p.counted && p.total > 0 {
                p.done.min(p.total) as f32 / p.total as f32
            } else {
                0.0
            };
            Some(PlanBlock {
                cells: size.cells(),
                fill,
                in_review: fill < 1.0 && p.pr == Some(PrState::Open),
            })
        })
        .collect();
    // a slice still to plan is an empty one-cell block; it weighs like the
    // average planned block, since its size is not known yet
    let mut plans = plans;
    let average = if plans.is_empty() {
        1.0
    } else {
        plans.iter().map(|b| b.cells).sum::<usize>() as f32 / plans.len() as f32
    };
    let sized = plans.len();
    plans.extend((0..to_plan).map(|_| PlanBlock {
        cells: 1,
        fill: 0.0,
        in_review: false,
    }));
    if sized > 0 && base < PR_OPEN {
        // weigh the building share by plan size; a plan in review is built
        let weight: f32 =
            plans[..sized].iter().map(|b| b.cells as f32).sum::<f32>() + average * to_plan as f32;
        let built: f32 = plans
            .iter()
            .map(|b| b.cells as f32 * if b.in_review { 1.0 } else { b.fill })
            .sum();
        building_share = built / weight;
    }
    let reached = |stage: f32| if base >= stage { 1.0 } else { 0.0 };
    let segments = [
        1.0,
        reached(SPECCED),
        reached(PLANNED),
        if base >= PR_OPEN { 1.0 } else { building_share },
        reached(PR_OPEN),
        reached(MERGED),
        if crit.is_empty() {
            0.0
        } else {
            ticked as f32 / crit.len() as f32
        },
    ];
    Some(Progress {
        plans,
        segments,
        fill: (base + verified).min(10.0),
        label: if complete {
            "✓ complete".to_string()
        } else {
            label
        },
        complete,
    })
}

/// Tasks in a superpowers plan: `### Task <n>` headings, done counted from
/// distinct `Task <n>: complete` lines in its ledger. Without a ledger the
/// done count is unknown (`counted: false`); checkbox steps are not tasks and
/// agents do not tick them, so they are not used.
pub fn plan_tasks(plan: &str, ledger: Option<&str>) -> PlanTasks {
    let headings: BTreeSet<u64> = plan
        .lines()
        .filter_map(|l| l.strip_prefix("### Task "))
        .filter_map(leading_number)
        .collect();
    let files = plan_files(plan);
    let Some(ledger) = ledger.filter(|_| !headings.is_empty()) else {
        return PlanTasks {
            done: 0,
            total: headings.len(),
            readable: true,
            unplanned: false,
            counted: false,
            files,
            pr: None,
        };
    };
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
    PlanTasks {
        done: done.len(),
        total: headings.len(),
        readable: true,
        unplanned: false,
        counted: true,
        files,
        pr: None,
    }
}

/// Distinct paths in a plan's `- Create:`, `- Modify:` and `- Test:` lines
/// (the backticked path, without a `:line-range` suffix).
fn plan_files(plan: &str) -> usize {
    let mut seen = BTreeSet::new();
    for line in plan.lines().map(str::trim_start) {
        let Some(rest) = ["- Create:", "- Modify:", "- Test:"]
            .iter()
            .find_map(|p| line.strip_prefix(p))
        else {
            continue;
        };
        let Some((_, after)) = rest.split_once('`') else {
            continue;
        };
        let Some((path, _)) = after.split_once('`') else {
            continue;
        };
        let path = path.split(':').next().unwrap_or(path).trim();
        if !path.is_empty() {
            seen.insert(path.to_string());
        }
    }
    seen.len()
}

fn leading_number(s: &str) -> Option<u64> {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Time since `since`, compactly: `<1m ago`, `45m ago`, `15h ago`, `3d ago`.
pub fn ago(since: chrono::DateTime<chrono::Local>, now: chrono::DateTime<chrono::Local>) -> String {
    let mins = (now - since).num_minutes();
    match mins {
        m if m < 1 => "<1m ago".to_string(),
        m if m < 60 => format!("{m}m ago"),
        m if m < 1440 => format!("{}h ago", m / 60),
        m => format!("{}d ago", m / 1440),
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
/// intent's repo, the repo's main checkout or any of its worktrees (with
/// no repo, beside the intent); its ledger is the
/// `.superpowers/sdd/*/progress.md` (in any worktree) whose first line names it.
pub fn plan_evidence(repo: Option<&Path>, it: &Intent) -> Vec<PlanTasks> {
    let roots = repo.map(crate::send::worktrees).unwrap_or_default();
    artifacts(&it.body)
        .into_iter()
        .filter(|a| a.kind.eq_ignore_ascii_case("plan"))
        .map(|a| {
            // a line that names no file is a slice the spec plans for later
            let t = a.target.trim();
            if !t.contains('/') && !t.ends_with(".md") {
                return PlanTasks {
                    unplanned: true,
                    readable: true,
                    counted: true,
                    ..PlanTasks::default()
                };
            }
            let unreadable = PlanTasks::default();
            let path = match repo {
                Some(r) => crate::send::resolve_in_repo(r, &a.target),
                None => {
                    // as the Artifacts reader does: a relative path is read
                    // beside the intent
                    let p = PathBuf::from(shellexpand::tilde(a.target.trim()).into_owned());
                    let p = if p.is_absolute() {
                        p
                    } else {
                        it.path.parent().map(|d| d.join(&p)).unwrap_or(p)
                    };
                    p.exists().then_some(p)
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
/// is a ledger header `# SDD ledger — plan: <path>` naming `plan` (either path
/// a suffix of the other, compared by whole components). A header that names
/// no file matches nothing.
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
                .filter(|l| l.starts_with("# SDD ledger"))
                .and_then(|l| l.split_once("plan:"))
                .map(|(_, p)| p.trim().to_string())
            else {
                continue;
            };
            let (named, plan) = (Path::new(&named), Path::new(plan));
            if named.file_name().is_none() {
                continue;
            }
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
    pr_info(gh, repo, target, env).0
}

/// A pull request's state and title from `gh pr view`; `Unknown` and no
/// title when `gh` cannot say.
pub fn pr_info(
    gh: &Path,
    repo: &Path,
    target: &str,
    env: &[(String, String)],
) -> (PrState, Option<String>) {
    let t = target.trim();
    let args: Vec<String> = if let Some(n) = t.strip_prefix('#') {
        vec![n.trim().to_string()]
    } else if t.contains("/pull/") {
        vec![t.to_string()]
    } else if let Some((slug, n)) = t.split_once('#') {
        vec![n.trim().to_string(), "-R".into(), slug.trim().to_string()]
    } else {
        return (PrState::Unknown, None);
    };
    let view = ["pr".to_string(), "view".to_string(), args[0].clone()];
    let rest: Vec<String> = ["--json".to_string(), "state,title".to_string()]
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
        return (PrState::Unknown, None);
    };
    if !out.status.success() {
        return (PrState::Unknown, None);
    }
    let v = serde_json::from_slice::<serde_json::Value>(&out.stdout).ok();
    let field = |k: &str| {
        v.as_ref()
            .and_then(|v| v.get(k))
            .and_then(|s| s.as_str())
            .map(str::to_string)
    };
    let state = match field("state").as_deref() {
        Some("OPEN") => PrState::Open,
        Some("MERGED") => PrState::Merged,
        Some("CLOSED") => PrState::Closed,
        _ => PrState::Unknown,
    };
    (state, field("title").filter(|t| !t.trim().is_empty()))
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

/// Pull requests in `repo` whose title or description mentions `query` (a
/// spec or plan file name): `gh pr list --state all --search "<query>"`.
/// Empty when gh cannot answer.
pub fn prs_mentioning(
    gh: &Path,
    repo: &Path,
    query: &str,
    env: &[(String, String)],
) -> Vec<FoundPr> {
    let out = Command::new(gh)
        .args([
            "pr",
            "list",
            "--state",
            "all",
            "--search",
            &format!("\"{query}\""),
            "--json",
            "number,state,headRefName,title",
            "--limit",
            "30",
        ])
        .current_dir(repo)
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return Vec::new();
    };
    v.as_array()
        .map(|prs| {
            prs.iter()
                .filter_map(|p| {
                    let number = p.get("number")?.as_u64()?;
                    let state = match p.get("state")?.as_str()? {
                        "OPEN" => PrState::Open,
                        "MERGED" => PrState::Merged,
                        "CLOSED" => PrState::Closed,
                        _ => PrState::Unknown,
                    };
                    let branch = p
                        .get("headRefName")
                        .and_then(|b| b.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let title = p
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or_default()
                        .to_string();
                    Some(FoundPr {
                        number,
                        state,
                        branch,
                        title,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
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
        title: Option<String>,
    },
    /// Plan tasks for the intent at `path` (reading plans runs git).
    Plans {
        path: PathBuf,
        plans: Vec<PlanTasks>,
    },
    /// Everything a reconcile needs, gathered: the prompt for the agent.
    Reconcile {
        path: PathBuf,
        merged: Vec<u64>,
        prompt: String,
    },
    /// Pull requests found mentioning each of the intent's spec or plan
    /// file names, per name.
    FoundPrs {
        path: PathBuf,
        by_name: FoundByName,
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
            let (state, title) = tool_bin(&env, "GH_BIN_PATH", Some("gh"))
                .map(|gh| pr_info(&gh, &repo, &target, &env))
                .unwrap_or((PrState::Unknown, None));
            let _ = tx.send((
                key,
                Update::Pr {
                    repo,
                    target,
                    state,
                    title,
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

    /// Search for pull requests mentioning each of `queries` (file names).
    pub fn request_found_prs(&mut self, path: PathBuf, repo: PathBuf, queries: Vec<String>) {
        let key = format!("found:{}", path.display());
        if !self.in_flight.insert(key.clone()) {
            return;
        }
        let env = self.env.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let by_name: FoundByName = match tool_bin(&env, "GH_BIN_PATH", Some("gh")) {
                Some(gh) => queries
                    .iter()
                    .map(|q| (q.clone(), prs_mentioning(&gh, &repo, q, &env)))
                    .collect(),
                None => Vec::new(),
            };
            let _ = tx.send((key, Update::FoundPrs { path, by_name }));
        });
    }

    /// Gather the spec, merged PRs and follow-ups for a reconcile, off the UI thread.
    pub fn request_reconcile(&mut self, intent: Intent, merged: Vec<u64>) {
        let key = format!("reconcile:{}", intent.path.display());
        let Some(repo) = intent.fm.repo.clone() else {
            return;
        };
        if !self.in_flight.insert(key.clone()) {
            return;
        }
        let env = self.env.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let gh = tool_bin(&env, "GH_BIN_PATH", Some("gh")).unwrap_or_else(|| "gh".into());
            let ctx = crate::reconcile::gather(&repo, &intent, &merged, &gh, &env);
            let prompt = crate::reconcile::reconcile_prompt(&intent, &ctx);
            let _ = tx.send((
                key,
                Update::Reconcile {
                    path: intent.path,
                    merged,
                    prompt,
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
