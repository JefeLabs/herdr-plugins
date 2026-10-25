//! Delivery progress of a sent capability: signals in (artifact lines, plan
//! tasks, pull request states, ticked criteria), a ten-cell bar and a label out.
//!
//! Stages Sent → Specced → Planned → Building → PR open → Merged are
//! cumulative: the furthest one with evidence sets the base, and earlier ones
//! count as reached. Ticked acceptance criteria fill only the Verified share
//! and never count as evidence for any earlier stage.

use std::collections::BTreeSet;

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
