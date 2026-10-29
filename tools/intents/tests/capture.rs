//! The capture runner against fake `work`, `make` and `claude`.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use herdr_intents::capture::{
    CaptureJob, CaptureOrder, CaptureRecord, CaptureRequest, CaptureUpdate, Outcome, Phase, Shot,
    cancel_running, is_running, launch, read_records,
};

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .display()
        .to_string()
}

const GUIDE: &str = "---\ndefault_area: org/social\nseed: [kathia-signup]\nurl: https://demo-org.skoolscout.com.local:{port}\n---\nSign in with Cmd+U.\n";

const EVENTS: &str = r#"{"type":"system","subtype":"init","session_id":"s1"}
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"mcp__intents-capture__browser","input":{"command":"open","args":["/auth/sign-in"]}}]}}
{"type":"result","subtype":"success","result":"Done.\nreproduced: the email goes to every admin","session_id":"s1"}
"#;

struct Fx {
    tmp: tempfile::TempDir,
    env: Vec<(String, String)>,
}

impl Fx {
    fn new() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(repo.join(".helmsmith")).unwrap();
        fs::write(repo.join(".helmsmith/capture.md"), GUIDE).unwrap();
        fs::create_dir_all(tmp.path().join("worktree")).unwrap();
        let events = tmp.path().join("events.jsonl");
        fs::write(&events, EVENTS).unwrap();
        let p = |n: &str| tmp.path().join(n).display().to_string();
        let env = vec![
            ("WORK_BIN_PATH".into(), fixture("fake-work.sh")),
            ("MAKE_BIN_PATH".into(), fixture("fake-make.sh")),
            ("CLAUDE_BIN_PATH".into(), fixture("fake-claude.sh")),
            (
                "AGENT_BROWSER_BIN_PATH".into(),
                fixture("fake-agent-browser.sh"),
            ),
            ("FAKE_AB_LOG".into(), p("ab.log")),
            (
                "INTENTS_BIN_PATH".into(),
                env!("CARGO_BIN_EXE_herdr-intents").into(),
            ),
            (
                "FAKE_CLAUDE_STEPS".into(),
                "open /auth/sign-in\nclick @e3".into(),
            ),
            ("FAKE_WORK_LOG".into(), p("work.log")),
            ("FAKE_MAKE_LOG".into(), p("make.log")),
            ("FAKE_CLAUDE_LOG".into(), p("claude.log")),
            ("FAKE_WORK_TREE".into(), p("worktree")),
            ("FAKE_CLAUDE_EVENTS".into(), events.display().to_string()),
            (
                "FAKE_CLAUDE_WRITES".into(),
                tmp.path().join("evidence/before.mp4").display().to_string(),
            ),
        ];
        Fx { tmp, env }
    }

    fn set(&mut self, k: &str, v: &str) {
        self.env.retain(|(x, _)| x != k);
        self.env.push((k.into(), v.into()));
    }

    fn request(&self) -> CaptureRequest {
        CaptureRequest {
            repo: self.tmp.path().join("repo"),
            group_rel: Some("skoolscout/discover-schools/org/team".into()),
            stem: "new-ab12cd34".into(),
            phase: Phase::Before,
            shot: Shot::Video,
            intent: "Admins are emailed when anyone accepts a team invite".into(),
            evidence: self.tmp.path().join("evidence"),
            before: None,
            n: 1,
            merged: Vec::new(),
        }
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.tmp.path().join(name)).unwrap_or_default()
    }

    fn run(&self, managed_mcp: bool) -> CaptureRecord {
        let job = CaptureJob::spawn(self.request(), self.env.clone(), managed_mcp);
        let start = Instant::now();
        loop {
            while let Some(u) = job.try_recv() {
                if let CaptureUpdate::Done(rec) = u {
                    return rec;
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "capture never finished"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn a_capture_runs_start_env_seed_agent_finish_in_order() {
    let fx = Fx::new();
    let rec = fx.run(false);
    let work: Vec<String> = fx.log("work.log").lines().map(str::to_string).collect();
    assert_eq!(work.len(), 3, "{work:?}");
    assert!(
        work[0].starts_with(
            "capture-new-ab12cd34 start --area org/team --task evidence-1 --type fix --mode fullstack"
        ),
        "{work:?}"
    );
    assert!(
        work[1].starts_with("capture-new-ab12cd34 env --area org/team --task evidence-1"),
        "{work:?}"
    );
    assert!(
        work[2].starts_with("capture-new-ab12cd34 finish --area org/team --task evidence-1"),
        "{work:?}"
    );
    let make = fx.log("make.log");
    assert!(
        make.contains("e2e-story STORY=kathia-signup FAST=1"),
        "{make}"
    );
    assert!(make.contains("app-test-e2e-runner"), "{make}");
    let claude = fx.log("claude.log");
    assert!(
        claude.contains("capture-mcp"),
        "the browser tools' server: {claude}"
    );
    assert!(!claude.contains("dangerously"), "{claude}");
    assert!(
        claude.contains("https://demo-org.skoolscout.com.local:3004"),
        "the slot's port: {claude}"
    );
    assert_eq!(
        rec.outcome,
        Outcome::Reproduced("the email goes to every admin".into())
    );
    assert_eq!(rec.file.as_deref(), Some("before.mp4"));
    assert_eq!(
        rec.steps,
        ["open /auth/sign-in", "click @e3"],
        "from the browser tools"
    );
    assert_eq!(rec.warning, None);
}

/// The argument after `flag` in claude's logged argv.
fn arg_after(claude: &str, flag: &str) -> Option<String> {
    let lines: Vec<&str> = claude.lines().collect();
    lines
        .iter()
        .position(|l| *l == flag)
        .and_then(|i| lines.get(i + 1))
        .map(|l| l.to_string())
}

#[test]
fn the_agent_gets_no_shell_only_the_browser_tools() {
    let fx = Fx::new();
    fx.run(false);
    let claude = fx.log("claude.log");
    assert_eq!(
        arg_after(&claude, "--tools").as_deref(),
        Some(""),
        "no built-in tools: {claude}"
    );
    assert_eq!(
        arg_after(&claude, "--permission-mode").as_deref(),
        Some("dontAsk"),
        "{claude}"
    );
    assert!(
        claude.lines().any(|l| l == "--restricted"),
        "user settings ignored: {claude}"
    );
    assert_eq!(
        arg_after(&claude, "--allowedTools").as_deref(),
        Some(
            "mcp__intents-capture__browser,mcp__intents-capture__record_start,mcp__intents-capture__record_stop"
        ),
        "{claude}"
    );
    assert!(!claude.lines().any(|l| l.starts_with("Bash")), "{claude}");
    let mcp: serde_json::Value =
        serde_json::from_str(&arg_after(&claude, "--mcp-config").unwrap()).unwrap();
    let server = &mcp["mcpServers"]["intents-capture"];
    assert_eq!(server["command"], env!("CARGO_BIN_EXE_herdr-intents"));
    assert_eq!(server["args"][0], "capture-mcp");
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(server["args"][1].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(
        config["base_url"],
        "https://demo-org.skoolscout.com.local:3004"
    );
    assert_eq!(config["session"], "capture-new-ab12cd34-1");
}

#[test]
fn an_older_claude_fails_closed_before_any_stack() {
    let mut fx = Fx::new();
    fx.set("FAKE_CLAUDE_OLD", "1");
    let rec = fx.run(false);
    match rec.outcome {
        Outcome::Failed(why) => assert!(why.contains("update claude"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fx.log("work.log"),
        "",
        "no stack for an agent that cannot be confined"
    );
}

#[test]
fn the_browser_session_is_closed_after_the_agent() {
    let fx = Fx::new();
    fx.run(false);
    let ab = fx.log("ab.log");
    assert!(
        ab.lines()
            .any(|l| l == "[--session][capture-new-ab12cd34-1][close]"),
        "{ab}"
    );
}

#[test]
fn the_seed_runs_on_the_captures_own_slot() {
    let mut fx = Fx::new();
    fx.set("SLOT", "9"); // the shell the tool started in had another slot
    fx.run(false);
    let make = fx.log("make.log");
    assert!(make.starts_with("SLOT=4 "), "{make}");
}

#[test]
fn a_failed_finish_is_on_the_record() {
    let mut fx = Fx::new();
    fx.set("FAKE_WORK_FINISH_FAIL", "DIRTY_WORKTREE");
    let rec = fx.run(false);
    let warning = rec.warning.unwrap_or_default();
    assert!(warning.contains("DIRTY_WORKTREE"), "{warning}");
    assert!(
        warning.contains("work finish --area org/team --task evidence-1"),
        "what to run: {warning}"
    );
}

#[test]
fn a_refused_area_falls_back_to_the_guides_default() {
    let mut fx = Fx::new();
    fx.set("FAKE_WORK_REFUSE", "org/team:SCOPE_CONFLICT");
    let rec = fx.run(false);
    let work = fx.log("work.log");
    let lines: Vec<&str> = work.lines().collect();
    assert!(lines[1].contains("start --area org/social"), "{work}");
    assert!(
        lines.last().unwrap().contains("finish --area org/social"),
        "{work}"
    );
    assert!(
        matches!(rec.outcome, Outcome::Reproduced(_)),
        "{:?}",
        rec.outcome
    );
}

#[test]
fn capture_never_joins_or_merges_into_the_area() {
    for refuse in ["", "org/team:SCOPE_CONFLICT", "org/team:HEALTH_TIMEOUT"] {
        let mut fx = Fx::new();
        if !refuse.is_empty() {
            fx.set("FAKE_WORK_REFUSE", refuse);
        }
        fx.run(false);
        let work = fx.log("work.log");
        assert!(
            !work.contains("--join") && !work.contains("--into-area"),
            "{work}"
        );
    }
}

#[test]
fn a_failed_seed_still_finishes_the_effort() {
    let mut fx = Fx::new();
    fx.set("FAKE_MAKE_FAIL", "1");
    let rec = fx.run(false);
    assert_eq!(
        rec.outcome,
        Outcome::Failed("seed kathia-signup: exit 2".into())
    );
    assert!(
        fx.log("work.log")
            .lines()
            .last()
            .unwrap()
            .contains("finish --area org/team")
    );
    assert_eq!(fx.log("claude.log"), "", "no agent after a failed seed");
}

#[test]
fn a_start_that_fails_otherwise_ends_the_capture() {
    let mut fx = Fx::new();
    fx.set("FAKE_WORK_REFUSE", "org/team:HEALTH_TIMEOUT");
    let rec = fx.run(false);
    match rec.outcome {
        Outcome::Failed(why) => assert!(why.contains("HEALTH_TIMEOUT"), "{why}"),
        other => panic!("{other:?}"),
    }
    let work = fx.log("work.log");
    assert_eq!(
        work.lines().count(),
        1,
        "no second area, nothing to finish: {work}"
    );
}

#[test]
fn strict_mcp_follows_the_managed_config() {
    let fx = Fx::new();
    fx.run(false);
    assert!(
        fx.log("claude.log")
            .lines()
            .any(|l| l == "--strict-mcp-config")
    );
    let fx = Fx::new();
    fx.run(true);
    assert!(
        !fx.log("claude.log")
            .lines()
            .any(|l| l == "--strict-mcp-config")
    );
}

#[test]
fn cancel_stops_the_agent_and_finishes_the_effort() {
    let mut fx = Fx::new();
    fx.set("FAKE_CLAUDE_HANG", "1");
    let mut job = CaptureJob::spawn(fx.request(), fx.env.clone(), false);
    let start = Instant::now();
    while !fx.log("claude.log").contains("---") {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the agent never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    job.cancel();
    let work = fx.log("work.log");
    assert!(
        work.lines()
            .last()
            .unwrap()
            .contains("finish --area org/team"),
        "{work}"
    );
    assert!(
        fx.log("ab.log")
            .contains("[--session][capture-new-ab12cd34-1][close]"),
        "the browser goes too"
    );
}

// ---- a capture in its own process --------------------------------------------

impl Fx {
    fn order(&self) -> CaptureOrder {
        CaptureOrder {
            req: self.request(),
            env: self.env.clone(),
            managed_mcp: false,
        }
    }

    fn evidence(&self) -> std::path::PathBuf {
        self.tmp.path().join("evidence")
    }

    fn wait(&self, what: &str, f: impl Fn(&Fx) -> bool) {
        let start = Instant::now();
        while !f(self) {
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "timed out: {what}; work log: {}",
                self.log("work.log")
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn a_launched_capture_runs_on_its_own_and_finishes() {
    let fx = Fx::new();
    launch(&fx.order()).unwrap();
    assert!(is_running(&fx.evidence()), "running once launched");
    fx.wait("the result", |f| {
        read_records(&f.evidence())
            .first()
            .is_some_and(|r| !matches!(r.outcome, Outcome::Running(_)))
    });
    let rec = read_records(&fx.evidence()).remove(0);
    assert_eq!(
        rec.outcome,
        Outcome::Reproduced("the email goes to every admin".into())
    );
    assert!(
        fx.log("work.log")
            .lines()
            .last()
            .unwrap()
            .contains("finish --area org/team")
    );
    fx.wait("the runner to end", |f| !is_running(&f.evidence()));
}

#[test]
fn a_second_launch_is_refused_while_one_runs() {
    let mut fx = Fx::new();
    fx.set("FAKE_CLAUDE_HANG", "1");
    launch(&fx.order()).unwrap();
    let again = launch(&fx.order());
    assert!(
        again
            .as_ref()
            .is_err_and(|e| e.contains("capturing already")),
        "{again:?}"
    );
    fx.wait("the first capture's stack", |f| {
        f.log("work.log").contains("start --area")
    });
    assert!(cancel_running(&fx.evidence()));
    fx.wait("the runner to end", |f| !is_running(&f.evidence()));
    assert_eq!(fx.log("work.log").matches("start --area").count(), 1);
}

#[test]
fn cancelling_a_launched_capture_finishes_its_effort() {
    let mut fx = Fx::new();
    fx.set("FAKE_CLAUDE_HANG", "1");
    launch(&fx.order()).unwrap();
    fx.wait("the agent to start", |f| {
        f.log("claude.log").contains("---")
    });
    assert!(cancel_running(&fx.evidence()));
    fx.wait("the runner to end", |f| !is_running(&f.evidence()));
    assert!(
        fx.log("work.log")
            .lines()
            .last()
            .unwrap()
            .contains("finish --area org/team")
    );
    let rec = read_records(&fx.evidence()).remove(0);
    assert_eq!(rec.outcome, Outcome::Failed("cancelled".into()));
}

#[test]
fn a_capture_cancelled_while_its_stack_starts_still_finishes_it() {
    let mut fx = Fx::new();
    fx.set("FAKE_WORK_SLOW", "1");
    launch(&fx.order()).unwrap();
    fx.wait("work start", |f| f.log("work.log").contains("start --area"));
    assert!(cancel_running(&fx.evidence()));
    fx.wait("the runner to end", |f| !is_running(&f.evidence()));
    assert!(
        fx.log("work.log")
            .lines()
            .last()
            .unwrap()
            .contains("finish --area org/team"),
        "the stack that was starting is finished: {}",
        fx.log("work.log")
    );
    assert_eq!(fx.log("claude.log"), "", "no agent after a cancel");
}

#[test]
fn a_capture_cancelled_while_seeding_never_starts_the_agent() {
    let mut fx = Fx::new();
    fx.set("FAKE_MAKE_SLOW", "1");
    launch(&fx.order()).unwrap();
    fx.wait("the seed", |f| f.log("make.log").contains("e2e-story"));
    assert!(cancel_running(&fx.evidence()));
    fx.wait("the runner to end", |f| !is_running(&f.evidence()));
    assert_eq!(fx.log("claude.log"), "", "no agent once cancelled");
    assert!(
        fx.log("work.log")
            .lines()
            .last()
            .unwrap()
            .contains("finish --area org/team")
    );
}
