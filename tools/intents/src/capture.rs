//! Evidence for bugs: an agent that may only drive a browser reproduces the
//! bug on a local stack and records it; after the fix merges, the same steps
//! run again. This part is pure: the repo's capture guide, which area to start
//! a stack in, the prompt, and reading the agent's outcome and steps.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What a capture records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shot {
    Screenshot,
    Video,
}

/// Before the fix (reproducing the bug) or after it merged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Before,
    After,
}

/// Where a capture stands, with the agent's one line or the reason it failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "text", rename_all = "snake_case")]
pub enum Outcome {
    Running(String),
    Reproduced(String),
    NotReproduced(String),
    Fixed(String),
    StillBroken(String),
    Failed(String),
}

/// `.helmsmith/capture.md` in a repo: front matter for the tool, the body for
/// the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureGuide {
    /// The area to start a stack in when the intent's own area cannot.
    pub default_area: String,
    /// e2e stories the tool runs on the new slot before the agent starts.
    pub seed: Vec<String>,
    /// The app's address, with `{port}` for the slot's frontend port.
    pub url: String,
    /// Instructions for the agent: signing in, what to avoid.
    pub body: String,
}

/// One capture, as written beside the evidence (`<phase>.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRecord {
    pub phase: Phase,
    pub shot: Shot,
    pub outcome: Outcome,
    /// The screenshot or video, relative to the evidence folder.
    #[serde(default)]
    pub file: Option<String>,
    /// The agent-browser commands the agent ran, in order.
    #[serde(default)]
    pub steps: Vec<String>,
    pub viewport: (u16, u16),
    pub at: chrono::DateTime<chrono::Local>,
    /// For an "after": the merged pull requests it checked, so it runs once per set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merged: Vec<u64>,
}

/// Every capture uses the same window, so a before and its after compare.
pub const VIEWPORT: (u16, u16) = (1280, 800);

/// The guide's front matter, as written.
#[derive(Deserialize)]
struct GuideFront {
    default_area: String,
    #[serde(default)]
    seed: Vec<String>,
    url: String,
}

/// Reads `<repo>/.helmsmith/capture.md`: YAML front matter, then the body.
pub fn read_guide(repo: &Path) -> anyhow::Result<CaptureGuide> {
    use anyhow::Context;
    let path = repo.join(".helmsmith").join("capture.md");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let rest = text
        .strip_prefix("---\n")
        .with_context(|| format!("{}: no front matter", path.display()))?;
    let end = rest
        .find("\n---\n")
        .with_context(|| format!("{}: front matter never closes", path.display()))?;
    let front: GuideFront = serde_yaml_ng::from_str(&rest[..end])
        .with_context(|| format!("{}: front matter", path.display()))?;
    Ok(CaptureGuide {
        default_area: front.default_area,
        seed: front.seed,
        url: front.url,
        body: rest[end + "\n---\n".len()..].to_string(),
    })
}

/// The areas to try for a stack, in order: the one the intent's group path
/// ends with (its last two segments), then the guide's default. `work start`
/// decides which one can run.
pub fn area_candidates(group_rel: Option<&str>, guide: &CaptureGuide) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(group) = group_rel {
        let parts: Vec<&str> = group.split('/').filter(|p| !p.is_empty()).collect();
        if parts.len() >= 2 {
            out.push(parts[parts.len() - 2..].join("/"));
        }
    }
    if !out.contains(&guide.default_area) {
        out.push(guide.default_area.clone());
    }
    out
}

/// GitHub issue numbers named by the intent's `issue:` artifact lines
/// (`#123` or an `…/issues/123` URL); a local `.issues/…` path has none.
pub fn issue_numbers(body: &str) -> Vec<u64> {
    crate::store::artifacts(body)
        .into_iter()
        .filter(|a| a.kind.eq_ignore_ascii_case("issue"))
        .filter_map(|a| {
            let t = a.target.trim();
            let digits = match t.strip_prefix('#') {
                Some(n) => n,
                None => &t[t.rfind("/issues/")? + "/issues/".len()..],
            };
            digits
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|d| d.parse().ok())
        })
        .collect()
}

pub fn evidence_dir(polish_folder: &Path) -> PathBuf {
    polish_folder.join("evidence")
}

fn record_path(dir: &Path, phase: Phase) -> PathBuf {
    dir.join(match phase {
        Phase::Before => "before.json",
        Phase::After => "after.json",
    })
}

/// Writes `<dir>/<phase>.json`, replacing any earlier one for that phase.
pub fn write_record(dir: &Path, rec: &CaptureRecord) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        record_path(dir, rec.phase),
        serde_json::to_string_pretty(rec)?,
    )?;
    Ok(())
}

/// The before, then the after, as far as they exist.
pub fn read_records(dir: &Path) -> Vec<CaptureRecord> {
    [Phase::Before, Phase::After]
        .into_iter()
        .filter_map(|ph| std::fs::read_to_string(record_path(dir, ph)).ok())
        .filter_map(|t| serde_json::from_str(&t).ok())
        .collect()
}

pub fn capture_prompt(
    guide: &CaptureGuide,
    phase: Phase,
    shot: Shot,
    intent: &str,
    base_url: &str,
    out_file: &Path,
    before: Option<&CaptureRecord>,
) -> String {
    let out = out_file.display();
    let (w, h) = VIEWPORT;
    let mut p = String::new();
    p.push_str(guide.body.trim_end());
    p.push_str(&format!(
        "\n\nThe app: {base_url}\n\nThe bug, in the reporter's words:\n\n{}\n\n",
        intent.trim()
    ));
    p.push_str(&format!(
        "You may only run agent-browser commands. First run `agent-browser set viewport {w} {h}`, and keep that size.\n"
    ));
    if let (Phase::After, Some(b)) = (phase, before) {
        p.push_str("The bug was reproduced earlier with these steps. Repeat them exactly, in order, on the same form:\n");
        for step in &b.steps {
            p.push_str(&format!("- `{step}`\n"));
        }
        let saw = match &b.outcome {
            Outcome::Reproduced(t) | Outcome::NotReproduced(t) => t.as_str(),
            _ => "",
        };
        p.push_str(&format!("Back then: {saw}\n"));
    }
    p.push_str(&match shot {
        Shot::Video => format!(
            "Record it: run `agent-browser record start {out}` just before the steps that show the bug, and `agent-browser record stop` once the result is on screen.\n"
        ),
        Shot::Screenshot => format!(
            "When the result is on screen, run `agent-browser screenshot {out}`.\n"
        ),
    });
    p.push_str(&match phase {
        Phase::Before => "End your reply with one line: `reproduced: <what you saw, in one line>` or `couldn't reproduce: <what you tried>`.\n".to_string(),
        Phase::After => "End your reply with one line: `fixed: <what you see now, in one line>` or `still broken: <what you still see>`.\n".to_string(),
    });
    p
}

/// The agent's last outcome line for the phase; none is a failure.
pub fn parse_outcome(phase: Phase, text: &str) -> Outcome {
    let mut found: Option<Outcome> = None;
    for line in text.lines() {
        let t = line
            .trim()
            .trim_matches(['*', '`', '-', ' '])
            .replace('’', "'");
        let lower = t.to_ascii_lowercase();
        let after = |prefix: &str| -> Option<String> {
            lower
                .starts_with(prefix)
                .then(|| t[prefix.len()..].trim().to_string())
        };
        let hit = match phase {
            Phase::Before => after("reproduced:")
                .map(Outcome::Reproduced)
                .or_else(|| after("couldn't reproduce:").map(Outcome::NotReproduced))
                .or_else(|| after("could not reproduce:").map(Outcome::NotReproduced)),
            Phase::After => after("fixed:")
                .map(Outcome::Fixed)
                .or_else(|| after("still broken:").map(Outcome::StillBroken)),
        };
        if hit.is_some() {
            found = hit;
        }
    }
    found.unwrap_or_else(|| Outcome::Failed("no outcome in the agent's reply".into()))
}

/// The agent-browser command in one line of claude's stream-json, if any.
pub fn steps_from_stream(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("type")?.as_str()? != "assistant" {
        return None;
    }
    v.pointer("/message/content")?
        .as_array()?
        .iter()
        .filter(|c| c.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
        .filter(|c| c.get("name").and_then(|n| n.as_str()) == Some("Bash"))
        .filter_map(|c| c.pointer("/input/command").and_then(|x| x.as_str()))
        .find(|cmd| cmd.trim_start().starts_with("agent-browser"))
        .map(|cmd| cmd.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUIDE: &str = "---\ndefault_area: org/social\nseed: [kathia-signup]\nurl: https://demo-org.skoolscout.com.local:{port}\n---\nSign in with Cmd+U.\n";

    fn guide() -> CaptureGuide {
        CaptureGuide {
            default_area: "org/social".into(),
            seed: vec!["kathia-signup".into()],
            url: "https://demo-org.skoolscout.com.local:{port}".into(),
            body: "Sign in with Cmd+U.\n".into(),
        }
    }

    fn record(phase: Phase) -> CaptureRecord {
        CaptureRecord {
            phase,
            shot: Shot::Video,
            outcome: Outcome::Reproduced("the email goes to every admin".into()),
            file: Some("before.mp4".into()),
            steps: vec![
                "agent-browser open https://x".into(),
                "agent-browser click @e3".into(),
            ],
            viewport: VIEWPORT,
            at: chrono::Local::now(),
            merged: Vec::new(),
        }
    }

    #[test]
    fn the_guide_is_read_from_helmsmith_capture_md() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".helmsmith")).unwrap();
        std::fs::write(tmp.path().join(".helmsmith/capture.md"), GUIDE).unwrap();
        assert_eq!(read_guide(tmp.path()).unwrap(), guide());
        let e = read_guide(&tmp.path().join("nowhere")).unwrap_err();
        assert!(format!("{e:#}").contains("capture.md"), "{e:#}");
    }

    #[test]
    fn area_candidates_try_the_intents_area_then_the_default() {
        let g = guide();
        assert_eq!(
            area_candidates(Some("skoolscout/discover-schools/org/team"), &g),
            ["org/team", "org/social"]
        );
        assert_eq!(area_candidates(Some("skoolscout"), &g), ["org/social"]);
        assert_eq!(area_candidates(None, &g), ["org/social"]);
        assert_eq!(
            area_candidates(Some("skoolscout/discover-schools/org/social"), &g),
            ["org/social"]
        );
    }

    #[test]
    fn issue_numbers_come_from_issue_lines() {
        let body = "## Description\nx\n\n## Artifacts\n\n- issue: #123\n- issue: https://github.com/o/r/issues/456\n- issue: .issues/x.md\n- pr: #9\n";
        assert_eq!(issue_numbers(body), [123, 456]);
    }

    #[test]
    fn outcome_lines_are_read_per_phase() {
        assert_eq!(
            parse_outcome(
                Phase::Before,
                "I opened it.\nreproduced: the email goes to every admin"
            ),
            Outcome::Reproduced("the email goes to every admin".into())
        );
        assert_eq!(
            parse_outcome(Phase::Before, "Couldn't reproduce: no invite button"),
            Outcome::NotReproduced("no invite button".into())
        );
        assert_eq!(
            parse_outcome(Phase::After, "still broken: admins still get it"),
            Outcome::StillBroken("admins still get it".into())
        );
        assert_eq!(
            parse_outcome(Phase::After, "Fixed: only the inviter is emailed"),
            Outcome::Fixed("only the inviter is emailed".into())
        );
    }

    #[test]
    fn a_reply_without_an_outcome_is_a_failure() {
        assert!(matches!(
            parse_outcome(Phase::Before, "I looked around and saw a page."),
            Outcome::Failed(_)
        ));
    }

    #[test]
    fn steps_are_the_agent_browser_commands_it_ran() {
        let tool = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"agent-browser open https://x"}}]}}"#;
        assert_eq!(
            steps_from_stream(tool).as_deref(),
            Some("agent-browser open https://x")
        );
        let other = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#;
        assert_eq!(steps_from_stream(other), None);
        let text = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"text":"hi"}}}"#;
        assert_eq!(steps_from_stream(text), None);
    }

    #[test]
    fn records_round_trip_before_then_after() {
        let tmp = tempfile::tempdir().unwrap();
        let mut after = record(Phase::After);
        after.outcome = Outcome::StillBroken("still".into());
        after.merged = vec![12];
        write_record(tmp.path(), &after).unwrap();
        write_record(tmp.path(), &record(Phase::Before)).unwrap();
        let recs = read_records(tmp.path());
        assert_eq!(recs.len(), 2);
        assert_eq!(
            (recs[0].phase, recs[1].phase),
            (Phase::Before, Phase::After)
        );
        assert_eq!(recs[1].merged, [12]);
    }

    #[test]
    fn the_prompt_carries_viewport_output_and_contract() {
        let out = Path::new("/scratch/evidence/before.mp4");
        let p = capture_prompt(
            &guide(),
            Phase::Before,
            Shot::Video,
            "Admins are emailed",
            "https://demo-org.skoolscout.com.local:3004",
            out,
            None,
        );
        for needle in [
            "set viewport 1280 800",
            "record start /scratch/evidence/before.mp4",
            "reproduced:",
            "Sign in with Cmd+U.",
            "https://demo-org.skoolscout.com.local:3004",
            "Admins are emailed",
        ] {
            assert!(p.contains(needle), "lacks {needle}: {p}");
        }
        let before = record(Phase::Before);
        let p = capture_prompt(
            &guide(),
            Phase::After,
            Shot::Video,
            "Admins are emailed",
            "https://x",
            Path::new("/e/after.mp4"),
            Some(&before),
        );
        for needle in [
            "agent-browser open https://x",
            "agent-browser click @e3",
            "fixed:",
            "still broken:",
        ] {
            assert!(p.contains(needle), "lacks {needle}: {p}");
        }
    }
}
