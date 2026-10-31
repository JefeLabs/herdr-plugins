//! The capture runner against fake `work`, `make` and `claude`.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use herdr_intents::capture::{
    CaptureJob, CaptureOrder, CaptureRecord, CaptureRequest, CaptureUpdate, Job, Outcome, Phase,
    Shot, cancel_running, is_running, launch, read_records,
};
use herdr_intents::check::{CheckRecord, Verdict, read_check};

// ---- the run guide: run.md or capture.md, work or commands mode ------------------

fn guide_in(files: &[(&str, &str)]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join(".helmsmith")).unwrap();
    for (name, text) in files {
        fs::write(tmp.path().join(".helmsmith").join(name), text).unwrap();
    }
    tmp
}

#[test]
fn a_guide_names_its_story_personas() {
    use herdr_intents::capture::{guide_stories, read_guide};
    let run = "---\nstart: ./serve\nurl: http://127.0.0.1:{port}\n---\nSign in at /auth.\n\n## Stories\n### Personas\n- Mark: tenant admin, signs in as mark@example.test\n- Pat: a parent, signs in as pat@example.test\n### Setup may create\n- applications and notes, never people\n\n## Avoid\n- billing\n";
    let tmp = guide_in(&[("run.md", run)]);
    let s = guide_stories(&read_guide(tmp.path()).unwrap()).unwrap();
    assert_eq!(s.personas, vec!["Mark", "Pat"]);
    assert!(s.text.starts_with("## Stories"), "{}", s.text);
    assert!(s.text.contains("never people"));
    assert!(
        !s.text.contains("billing"),
        "only its own section: {}",
        s.text
    );
}

#[test]
fn a_guide_without_a_stories_section_has_no_personas() {
    use herdr_intents::capture::{guide_stories, read_guide};
    let run = "---\nstart: ./serve\nurl: http://127.0.0.1:{port}\n---\nSign in at /auth.\n";
    let tmp = guide_in(&[("run.md", run)]);
    assert_eq!(guide_stories(&read_guide(tmp.path()).unwrap()), None);
}

// ---- the splash: the guide's logo and colors for a story video's cards ----------

const SPLASH_GUIDE: &str = "---\nstart: ./serve\nurl: http://127.0.0.1:{port}\nsplash:\n  logo: brand/logo.png\n  background: \"#142b57\"\n  accent: \"#59b7ff\"\n  text: \"#fafafa\"\n---\nbody\n";

#[test]
fn a_guide_reads_its_splash_block() {
    use herdr_intents::capture::read_guide;
    let tmp = guide_in(&[("run.md", SPLASH_GUIDE)]);
    let g = read_guide(tmp.path()).unwrap();
    assert_eq!(g.splash.logo.as_deref(), Some("brand/logo.png"));
    assert_eq!(g.splash.background.as_deref(), Some("#142b57"));
    assert_eq!(g.splash.accent.as_deref(), Some("#59b7ff"));
    assert_eq!(g.splash.text.as_deref(), Some("#fafafa"));
}

#[test]
fn the_cards_take_the_guides_colors_and_its_logo_as_data() {
    use herdr_intents::capture::{card_look, read_guide};
    let tmp = guide_in(&[("run.md", SPLASH_GUIDE)]);
    fs::create_dir_all(tmp.path().join("brand")).unwrap();
    fs::write(tmp.path().join("brand/logo.png"), "foobar").unwrap();
    let (look, warnings) = card_look(&read_guide(tmp.path()).unwrap().splash, tmp.path());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(look.logo.as_deref(), Some("data:image/png;base64,Zm9vYmFy"));
    assert_eq!(look.background, "#142b57");
    assert_eq!(look.accent, "#59b7ff");
    assert_eq!(look.text, "#fafafa");
}

#[test]
fn an_svg_logo_is_data_too() {
    use herdr_intents::capture::{GuideSplash, card_look};
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("logo.svg"), "fo").unwrap();
    let splash = GuideSplash {
        logo: Some("logo.svg".into()),
        ..Default::default()
    };
    let (look, _) = card_look(&splash, tmp.path());
    assert_eq!(look.logo.as_deref(), Some("data:image/svg+xml;base64,Zm8="));
}

#[test]
fn without_a_splash_block_the_cards_keep_their_own_look() {
    use herdr_intents::capture::{CardLook, GuideSplash, card_look};
    let tmp = tempfile::tempdir().unwrap();
    let (look, warnings) = card_look(&GuideSplash::default(), tmp.path());
    assert_eq!(look, CardLook::default());
    assert!(look.logo.is_none());
    assert!(warnings.is_empty());
}

#[test]
fn a_color_or_logo_that_could_inject_or_escape_is_skipped_and_said() {
    use herdr_intents::capture::{CardLook, GuideSplash, card_look};
    let tmp = tempfile::tempdir().unwrap();
    let inside = tmp.path().join("repo");
    fs::create_dir_all(&inside).unwrap();
    fs::write(tmp.path().join("outside.png"), "x").unwrap();
    fs::write(inside.join("anim.gif"), "x").unwrap();
    fs::write(inside.join("huge.png"), vec![0u8; 300 * 1024]).unwrap();
    std::os::unix::fs::symlink(tmp.path().join("outside.png"), inside.join("link.png")).unwrap();
    for (logo, why) in [
        ("../outside.png", "outside"),
        ("link.png", "outside"),
        ("/etc/hosts", "outside"),
        ("anim.gif", "png, svg or jpeg"),
        ("huge.png", "256 KB"),
        ("gone.png", "gone.png"),
    ] {
        let splash = GuideSplash {
            logo: Some(logo.into()),
            background: Some("red;position:static".into()),
            ..Default::default()
        };
        let (look, warnings) = card_look(&splash, &inside);
        assert!(look.logo.is_none(), "{logo}");
        assert_eq!(look.background, CardLook::default().background, "{logo}");
        let said = warnings.join(" | ");
        assert!(said.contains(why), "{logo}: {said}");
        assert!(said.contains("red;position:static"), "{said}");
    }
}

#[test]
fn run_md_wins_and_capture_md_still_works() {
    use herdr_intents::capture::{GuideMode, read_guide};
    let old =
        "---\ndefault_area: org/social\nseed: [kathia-signup]\nurl: https://old:{port}\n---\nold\n";
    let tmp = guide_in(&[("capture.md", old)]);
    let g = read_guide(tmp.path()).unwrap();
    assert_eq!(
        g.mode,
        GuideMode::Work {
            default_area: "org/social".into()
        }
    );
    assert_eq!(g.url, "https://old:{port}");
    assert_eq!(g.seed, ["kathia-signup"]);
    let tmp = guide_in(&[
        ("capture.md", old),
        (
            "run.md",
            "---\nstart: npm run dev\nurl: http://localhost:{port}\n---\nnew\n",
        ),
    ]);
    let g = read_guide(tmp.path()).unwrap();
    assert_eq!(g.url, "http://localhost:{port}", "run.md wins");
    assert!(matches!(g.mode, GuideMode::Commands { .. }), "{:?}", g.mode);
}

#[test]
fn a_commands_guide_is_read() {
    use herdr_intents::capture::{GuideMode, read_guide};
    let tmp = guide_in(&[(
        "run.md",
        "---\nstart: docker compose up -d && npm run dev\nurl: http://localhost:{port}\nready: /health\nready_timeout: 30\nseed: [\"npm run seed\"]\nstop: docker compose down\nlogs: [../api/app.log]\n---\nSign in as demo@example.com.\n",
    )]);
    let g = read_guide(tmp.path()).unwrap();
    assert_eq!(
        g.mode,
        GuideMode::Commands {
            start: "docker compose up -d && npm run dev".into(),
            ready: Some("/health".into()),
            ready_timeout: 30,
            stop: Some("docker compose down".into()),
            stop_timeout: 60,
        }
    );
    assert_eq!(g.seed, ["npm run seed"]);
    assert_eq!(g.logs, ["../api/app.log"]);
    assert_eq!(g.body, "Sign in as demo@example.com.\n");
    let tmp = guide_in(&[(
        "run.md",
        "---\nstart: make up\nurl: http://localhost:8080\n---\n",
    )]);
    match read_guide(tmp.path()).unwrap().mode {
        GuideMode::Commands {
            ready_timeout,
            ready,
            stop,
            ..
        } => {
            assert_eq!(ready_timeout, 600, "ten minutes unless the guide says");
            assert_eq!((ready, stop), (None, None));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_guide_with_neither_start_nor_default_area_is_refused() {
    use herdr_intents::capture::read_guide;
    let tmp = guide_in(&[("run.md", "---\nurl: http://localhost:8080\n---\n")]);
    let e = read_guide(tmp.path()).unwrap_err().to_string();
    assert!(e.contains("start") && e.contains("default_area"), "{e}");
}

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
            job: Job::Capture,
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

// ---- commands mode: any project's app, from its run guide ------------------------

/// "The app": answers every connection with 200 until the test ends; its port.
fn fake_app() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            use std::io::{Read, Write};
            let mut stream = stream;
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    port
}

fn has_python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// A `start` whose own app answers: python's http.server on `port` (a number
/// or `$PORT`), its pid kept in {pid}.
fn served(port: &str) -> String {
    format!(
        "echo $$ > {{pid}}; exec python3 -m http.server --bind 127.0.0.1 {port} >/dev/null 2>&1"
    )
}

/// When a process started, as `ps` says (what a reused pid would not match).
fn started_at(pid: u32) -> String {
    let out = std::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A port nothing listens on.
fn unused_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn alive(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

impl Fx {
    /// A meta repo (`repo/`) whose run.md has `front` ({log} and {pid}: files
    /// the test reads; {port} is left for the tool).
    fn commands(front: &str) -> Fx {
        let fx = Fx::new();
        let log = fx.tmp.path().join("app-events.log");
        let pid = fx.tmp.path().join("app.pid");
        let text = front
            .replace("{log}", &log.display().to_string())
            .replace("{pid}", &pid.display().to_string());
        fs::write(
            fx.tmp.path().join("repo/.helmsmith/run.md"),
            format!("---\n{text}---\nSign in as demo.\n"),
        )
        .unwrap();
        fx
    }

    fn app_pid(&self) -> Option<i32> {
        fs::read_to_string(self.tmp.path().join("app.pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    fn wait_done(&self, job: &CaptureJob) -> CaptureRecord {
        let start = Instant::now();
        loop {
            while let Some(u) = job.try_recv() {
                if let CaptureUpdate::Done(rec) = u {
                    return rec;
                }
            }
            assert!(start.elapsed() < Duration::from_secs(15), "never finished");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn a_commands_guide_starts_seeds_runs_and_stops() {
    if !has_python() {
        return;
    }
    let port = unused_port();
    let fx = Fx::commands(&format!(
        "start: echo start >> {{log}}; {}\nurl: http://127.0.0.1:{port}\nseed: [\"echo seed1 >> {{log}}\", \"echo seed2 >> {{log}}\"]\nstop: echo stop >> {{log}}\n",
        served(&port.to_string())
    ));
    let rec = fx.run(false);
    assert!(
        !matches!(rec.outcome, Outcome::Failed(_)),
        "{:?}",
        rec.outcome
    );
    let events: Vec<String> = fx
        .log("app-events.log")
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(events, ["start", "seed1", "seed2", "stop"]);
    assert_eq!(fx.log("work.log"), "", "no work slots in commands mode");
    assert!(
        fx.log("claude.log")
            .contains(&format!("http://127.0.0.1:{port}")),
        "the agent gets the app's address"
    );
    assert!(
        !alive(fx.app_pid().unwrap()),
        "the app's processes are gone"
    );
}

#[test]
fn the_port_placeholder_gets_a_free_port_as_port() {
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let fx = Fx::commands(
        "start: echo \"start $PORT\" >> {log}; echo $$ > {pid}; exec python3 -m http.server --bind 127.0.0.1 $PORT\nurl: http://127.0.0.1:{port}\n",
    );
    let rec = fx.run(false);
    assert!(
        !matches!(rec.outcome, Outcome::Failed(_)),
        "{:?}",
        rec.outcome
    );
    let events = fx.log("app-events.log");
    let port: u16 = events
        .trim()
        .strip_prefix("start ")
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        fx.log("claude.log")
            .contains(&format!("http://127.0.0.1:{port}")),
        "{events}"
    );
}

#[test]
fn a_start_that_exits_fails_fast_with_its_output() {
    let dead = unused_port();
    let fx = Fx::commands(&format!(
        "start: echo oops-no-such-script; exit 3\nurl: http://127.0.0.1:{dead}\nready_timeout: 30\n"
    ));
    let t = Instant::now();
    let rec = fx.run(false);
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    match rec.outcome {
        Outcome::Failed(why) => assert!(
            why.contains("start exited") && why.contains("oops-no-such-script"),
            "{why}"
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_app_that_never_answers_times_out() {
    let dead = unused_port();
    let fx = Fx::commands(&format!(
        "start: echo $$ > {{pid}}; exec sleep 60\nurl: http://127.0.0.1:{dead}\nready_timeout: 2\n"
    ));
    let rec = fx.run(false);
    match rec.outcome {
        Outcome::Failed(why) => assert!(why.contains("did not answer"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(!alive(fx.app_pid().unwrap()), "stopped after the timeout");
}

#[test]
fn a_failing_seed_ends_the_run_and_stops_the_app() {
    if !has_python() {
        return;
    }
    let port = unused_port();
    let fx = Fx::commands(&format!(
        "start: {}\nurl: http://127.0.0.1:{port}\nseed: [\"exit 4\"]\nstop: echo stop >> {{log}}\n",
        served(&port.to_string())
    ));
    let rec = fx.run(false);
    match rec.outcome {
        Outcome::Failed(why) => assert!(why.contains("exit 4"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(fx.log("app-events.log").trim(), "stop");
    assert!(!alive(fx.app_pid().unwrap()));
}

#[test]
fn a_cancelled_run_stops_the_apps_process_group() {
    if !has_python() {
        return;
    }
    let port = unused_port();
    let mut fx = Fx::commands(&format!(
        "start: {}\nurl: http://127.0.0.1:{port}\nstop: echo stop >> {{log}}\n",
        served(&port.to_string())
    ));
    fx.set("FAKE_CLAUDE_HANG", "1");
    let mut job = CaptureJob::spawn(fx.request(), fx.env.clone(), false);
    let start = Instant::now();
    while fx.log("claude.log").is_empty() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the agent never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    job.cancel();
    let rec = fx.wait_done(&job);
    assert!(
        matches!(rec.outcome, Outcome::Failed(_)),
        "{:?}",
        rec.outcome
    );
    assert!(fx.log("app-events.log").contains("stop"));
    assert!(!alive(fx.app_pid().unwrap()), "cancelled: the app is gone");
}

#[test]
fn a_second_run_without_a_port_placeholder_waits_its_turn() {
    let port = unused_port();
    let fx = Fx::commands(&format!(
        "start: echo $$ > {{pid}}; exec sleep 60\nurl: http://127.0.0.1:{port}\n"
    ));
    let lock = herdr_intents::capture::space_lock_path(&fx.tmp.path().join("repo"));
    fs::write(
        &lock,
        format!("{} http://127.0.0.1:{port}\n", std::process::id()),
    )
    .unwrap();
    let rec = fx.run(false);
    let _ = fs::remove_file(&lock);
    match rec.outcome {
        Outcome::Failed(why) => assert!(
            why.contains(&format!("another run is using http://127.0.0.1:{port}")),
            "{why}"
        ),
        other => panic!("{other:?}"),
    }
    assert!(fx.app_pid().is_none(), "the app never started");
}

#[test]
fn a_missing_sibling_repo_is_named_with_its_clone_command() {
    let port = unused_port();
    let fx = Fx::commands(&format!(
        "start: echo start >> {{log}}; exec sleep 60\nurl: http://127.0.0.1:{port}\n"
    ));
    fs::write(
        fx.tmp.path().join("repo/.helmsmith/space.toml"),
        "repos = [\n  { path = \".\", git = \"git@example.com:acme/meta.git\" },\n  { path = \"../web\", git = \"git@example.com:acme/web.git\" },\n]\n",
    )
    .unwrap();
    let rec = fx.run(false);
    match rec.outcome {
        Outcome::Failed(why) => assert!(
            why.contains("../web") && why.contains("git clone git@example.com:acme/web.git ../web"),
            "{why}"
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(fx.log("app-events.log"), "", "nothing started");
}

#[test]
fn a_crashed_runs_app_is_ended_by_the_next_run() {
    use std::os::unix::process::CommandExt;
    if !has_python() {
        return;
    }
    let port = unused_port();
    let fx = Fx::commands(&format!(
        "start: {}\nurl: http://127.0.0.1:{port}\n",
        served(&port.to_string())
    ));
    let mut left = std::process::Command::new("sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap();
    let evidence = fx.tmp.path().join("evidence");
    fs::create_dir_all(&evidence).unwrap();
    fs::write(
        evidence.join(".app.pgid"),
        format!("{}\n{}\n", left.id(), started_at(left.id())),
    )
    .unwrap();
    let _ = fx.run(false);
    let start = Instant::now();
    while left.try_wait().unwrap().is_none() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the crashed run's app is still running"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_reused_process_group_number_is_left_alone() {
    use std::os::unix::process::CommandExt;
    if !has_python() {
        return;
    }
    let port = unused_port();
    let fx = Fx::commands(&format!(
        "start: {}\nurl: http://127.0.0.1:{port}\n",
        served(&port.to_string())
    ));
    let mut other = std::process::Command::new("sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap();
    let evidence = fx.tmp.path().join("evidence");
    fs::create_dir_all(&evidence).unwrap();
    // the number a crashed run's app had, now someone else's
    fs::write(
        evidence.join(".app.pgid"),
        format!("{}\nMon Jan  1 00:00:00 2001\n", other.id()),
    )
    .unwrap();
    let _ = fx.run(false);
    let still = other.try_wait().unwrap().is_none();
    let _ = other.kill();
    let _ = other.wait();
    assert!(
        still,
        "a group that is not the crashed run's app is left alone"
    );
}

#[test]
fn a_fixed_url_already_in_use_is_refused() {
    let port = fake_app();
    let fx = Fx::commands(&format!(
        "start: echo start >> {{log}}; exec sleep 60\nurl: http://127.0.0.1:{port}\nseed: [\"echo seed >> {{log}}\"]\nstop: echo stop >> {{log}}\n"
    ));
    let rec = fx.run(false);
    match rec.outcome {
        Outcome::Failed(why) => assert!(
            why.contains(&format!(
                "something already answers at http://127.0.0.1:{port}"
            )),
            "{why}"
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fx.log("app-events.log"),
        "",
        "someone else's app: nothing started, seeded or stopped"
    );
}

#[test]
fn localhost_is_ready_when_the_app_listens_on_ipv4_only() {
    if !has_python() {
        return;
    }
    let fx = Fx::commands(&format!(
        "start: {}\nurl: http://localhost:{{port}}\nready_timeout: 5\n",
        served("$PORT")
    ));
    let rec = fx.run(false);
    assert!(
        !matches!(rec.outcome, Outcome::Failed(_)),
        "{:?}",
        rec.outcome
    );
}

#[test]
fn a_start_that_exits_0_with_the_app_running_keeps_waiting() {
    if !has_python() {
        return;
    }
    let fx = Fx::commands(
        "start: python3 -m http.server --bind 127.0.0.1 $PORT >/dev/null 2>&1 & echo $! > {pid}; exit 0\nurl: http://127.0.0.1:{port}\n",
    );
    let rec = fx.run(false);
    assert!(
        !matches!(rec.outcome, Outcome::Failed(_)),
        "{:?}",
        rec.outcome
    );
    assert!(
        !alive(fx.app_pid().unwrap()),
        "the app it left running is ended"
    );
}

#[test]
fn a_cancel_stops_a_running_seed() {
    if !has_python() {
        return;
    }
    let port = unused_port();
    let fx = Fx::commands(&format!(
        "start: {}\nurl: http://127.0.0.1:{port}\nseed: [\"echo seed $$ >> {{log}}; sleep 6\"]\n",
        served(&port.to_string())
    ));
    let mut job = CaptureJob::spawn(fx.request(), fx.env.clone(), false);
    let start = Instant::now();
    let seed_pid: i32 = loop {
        if let Some(pid) = fx
            .log("app-events.log")
            .lines()
            .find_map(|l| l.strip_prefix("seed ").and_then(|p| p.trim().parse().ok()))
        {
            break pid;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the seed never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let t = Instant::now();
    job.cancel();
    let rec = fx.wait_done(&job);
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    assert!(
        matches!(rec.outcome, Outcome::Failed(_)),
        "{:?}",
        rec.outcome
    );
    assert!(!alive(seed_pid), "the seed is stopped too");
}

#[test]
fn a_hung_stop_is_cut_off_and_the_app_still_stops() {
    if !has_python() {
        return;
    }
    let port = unused_port();
    let fx = Fx::commands(&format!(
        "start: {}\nurl: http://127.0.0.1:{port}\nstop: sleep 30\nstop_timeout: 1\n",
        served(&port.to_string())
    ));
    let t = Instant::now();
    let rec = fx.run(false);
    assert!(t.elapsed() < Duration::from_secs(8), "{:?}", t.elapsed());
    let warning = rec.warning.unwrap_or_default();
    assert!(warning.contains("did not finish within 1s"), "{warning}");
    assert!(!alive(fx.app_pid().unwrap()));
}

#[test]
fn a_seed_that_hangs_is_cut_off() {
    if !has_python() {
        return;
    }
    let port = unused_port();
    let fx = Fx::commands(&format!(
        "start: {}\nurl: http://127.0.0.1:{port}\nready_timeout: 2\nseed: [\"sleep 30\"]\n",
        served(&port.to_string())
    ));
    let rec = fx.run(false);
    match rec.outcome {
        Outcome::Failed(why) => assert!(why.contains("did not finish within 2s"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_check_that_cannot_start_its_app_records_why() {
    let fx = Fx::checking("1: confirmed: ok");
    let dead = unused_port();
    fs::write(
        fx.tmp.path().join("repo/.helmsmith/run.md"),
        format!("---\nstart: echo oops-no-app; exit 2\nurl: http://127.0.0.1:{dead}\n---\nhi\n"),
    )
    .unwrap();
    let order = CaptureOrder {
        req: fx.check_request(None),
        env: fx.env.clone(),
        managed_mcp: false,
    };
    launch(&order).unwrap();
    let checks = fx.tmp.path().join("checks");
    fx.wait("the check to end", |_| !is_running(&checks));
    let why = herdr_intents::check::read_failure(&checks).expect("the reason is kept");
    assert!(
        why.contains("start exited") && why.contains("oops-no-app"),
        "{why}"
    );
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
    assert_eq!(config["session"], session());
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

/// The fixture capture's browser session (intent `new-ab12cd34`, capture 1).
fn session() -> String {
    herdr_intents::capture::browser_session("new-ab12cd34", 1)
}

#[test]
fn the_browser_session_is_closed_after_the_agent() {
    let fx = Fx::new();
    fx.run(false);
    let ab = fx.log("ab.log");
    assert!(
        ab.lines()
            .any(|l| l == format!("[--session][{}][close]", session())),
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
    // a cancel that lands before the agent's group is known leaves the stop
    // (and the browser's close) to the job's own thread, a moment later
    let start = Instant::now();
    while !fx
        .log("ab.log")
        .contains(&format!("[--session][{}][close]", session()))
    {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the browser goes too"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
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

// ---- a completion check ------------------------------------------------------

const INTENT: &str = "---\ntitle: Notify primary contact\nkind: idea\nstate: sent\n---\n## Description\nx\n\n## Acceptance criteria\n- [ ] the primary contact gets an email\n- [ ] the email names the meeting\n";

impl Fx {
    /// A check of a two-criterion intent; the guide lists the backend log.
    fn checking(result: &str) -> Fx {
        let mut fx = Fx::new();
        let t = fx.tmp.path().to_path_buf();
        fs::write(
            t.join("repo/.helmsmith/capture.md"),
            "---\ndefault_area: org/social\nseed: [kathia-signup]\nurl: https://demo-org.skoolscout.com.local:{port}\nlogs: [.logs/backend.log]\n---\nSign in with Cmd+U.\n",
        )
        .unwrap();
        fs::create_dir_all(t.join("worktree/.logs")).unwrap();
        fs::write(t.join("worktree/.logs/backend.log"), "seeded: an email\n").unwrap();
        fs::create_dir_all(t.join("root/skoolscout/org")).unwrap();
        fs::write(t.join("root/skoolscout/org/notify.md"), INTENT).unwrap();
        fs::create_dir_all(t.join("checks")).unwrap();
        let events = t.join("check-events.jsonl");
        fs::write(
            &events,
            format!(
                "{{\"type\":\"result\",\"result\":{}}}\n",
                serde_json::Value::String(result.into())
            ),
        )
        .unwrap();
        fx.set("FAKE_CLAUDE_EVENTS", &events.display().to_string());
        fx.set("FAKE_CHECK_DIR", &t.join("checks").display().to_string());
        fx.env
            .retain(|(k, _)| k != "FAKE_CLAUDE_WRITES" && k != "FAKE_CLAUDE_STEPS");
        fx
    }

    fn check_request(&self, keys: Option<Vec<String>>) -> CaptureRequest {
        let t = self.tmp.path();
        CaptureRequest {
            phase: Phase::Before,
            shot: Shot::Screenshot,
            evidence: t.join("checks"),
            job: Job::Check {
                keys,
                check_dir: t.join("checks"),
                intent_path: t.join("root/skoolscout/org/notify.md"),
                root: t.join("root"),
                fold: false,
            },
            ..self.request()
        }
    }

    /// Runs `req` in this process; (the check's results, the run's record).
    fn run_check(&self, req: CaptureRequest) -> (Option<CheckRecord>, CaptureRecord) {
        let job = CaptureJob::spawn(req, self.env.clone(), false);
        let start = Instant::now();
        let mut checked = None;
        loop {
            while let Some(u) = job.try_recv() {
                match u {
                    CaptureUpdate::Checked(c) => checked = Some(c),
                    CaptureUpdate::Done(rec) => return (checked, rec),
                    _ => {}
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "the check never finished"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn a_check_runs_on_the_same_stack_and_records_each_criterion() {
    let mut fx = Fx::checking("Done.\n1: confirmed: only the contact\n2: fails: no meeting name");
    fx.set("FAKE_CLAUDE_PROOF", "1");
    let (checked, _) = fx.run_check(fx.check_request(None));
    let work: Vec<String> = fx.log("work.log").lines().map(str::to_string).collect();
    assert!(work[0].contains("start --area org/team"), "{work:?}");
    assert!(
        work.last().unwrap().contains("finish --area org/team"),
        "{work:?}"
    );
    let claude = fx.log("claude.log");
    let allowed = arg_after(&claude, "--allowedTools").unwrap();
    assert!(
        allowed.contains("mcp__intents-capture__screenshot")
            && allowed.contains("mcp__intents-capture__logs"),
        "{allowed}"
    );
    assert!(
        claude.contains("1. the primary contact gets an email"),
        "{claude}"
    );
    let checks = fx.tmp.path().join("checks");
    let cfg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(checks.join(".capture-mcp.json")).unwrap())
            .unwrap();
    assert_eq!(cfg["mode"], "check");
    assert_eq!(
        cfg["logs"][0]["from"],
        "seeded: an email\n".len() as u64,
        "the seed's lines are not the agent's: {cfg}"
    );
    let checked = checked.expect("a check reports its results");
    assert_eq!(
        checked.results[0].verdict,
        Verdict::Confirmed("only the contact".into())
    );
    assert_eq!(checked.results[0].proof.as_deref(), Some("1.png"));
    assert_eq!(
        checked.results[1].verdict,
        Verdict::Fails("no meeting name".into())
    );
    assert_eq!(read_check(&checks), Some(checked), "kept as latest.json");
}

#[test]
fn a_bugs_check_also_records_the_after() {
    let mut fx = Fx::checking("fixed: only the contact now\n1: confirmed: ok\n2: confirmed: ok");
    fx.set("FAKE_CLAUDE_PROOF", "1,2");
    let evidence = fx.tmp.path().join("evidence");
    fs::create_dir_all(&evidence).unwrap();
    fx.set(
        "FAKE_CLAUDE_WRITES",
        &evidence.join("after.mp4").display().to_string(),
    );
    let before = CaptureRecord {
        phase: Phase::Before,
        shot: Shot::Video,
        outcome: Outcome::Reproduced("every admin gets it".into()),
        file: Some("before.mp4".into()),
        steps: vec!["open /x".into()],
        viewport: (1280, 800),
        at: chrono::Local::now(),
        merged: Vec::new(),
        warning: None,
    };
    let req = CaptureRequest {
        phase: Phase::After,
        shot: Shot::Video,
        evidence: evidence.clone(),
        before: Some(before),
        merged: vec![42],
        ..fx.check_request(None)
    };
    let (checked, rec) = fx.run_check(req);
    assert_eq!(rec.outcome, Outcome::Fixed("only the contact now".into()));
    assert_eq!(rec.file.as_deref(), Some("after.mp4"));
    let allowed = arg_after(&fx.log("claude.log"), "--allowedTools").unwrap();
    assert!(allowed.contains("record_start"), "{allowed}");
    let checked = checked.unwrap();
    assert_eq!(checked.merged, vec![42]);
    assert!(
        checked
            .results
            .iter()
            .all(|r| matches!(r.verdict, Verdict::Confirmed(_)))
    );
}

#[test]
fn a_recheck_checks_only_its_criteria() {
    let mut fx = Fx::checking("2: confirmed: names it now");
    fx.set("FAKE_CLAUDE_PROOF", "2");
    let checks = fx.tmp.path().join("checks");
    let key = |t: &str| herdr_intents::store::criterion_key(t);
    let earlier = CheckRecord {
        at: chrono::Local::now(),
        merged: vec![42],
        results: vec![
            herdr_intents::check::CriterionResult {
                number: 1,
                key: key("the primary contact gets an email"),
                text: "the primary contact gets an email".into(),
                verdict: Verdict::Confirmed("earlier".into()),
                proof: Some("1.png".into()),
            },
            herdr_intents::check::CriterionResult {
                number: 2,
                key: key("the email names the meeting"),
                text: "the email names the meeting".into(),
                verdict: Verdict::Fails("no name".into()),
                proof: None,
            },
        ],
        warning: None,
    };
    herdr_intents::check::write_check(&checks, &earlier).unwrap();
    fx.run_check(fx.check_request(Some(vec![key("the email names the meeting")])));
    let claude = fx.log("claude.log");
    assert!(
        claude.contains("2. the email names the meeting"),
        "{claude}"
    );
    assert!(
        !claude.contains("1. the primary contact"),
        "only its criterion: {claude}"
    );
    let latest = read_check(&checks).unwrap();
    assert_eq!(
        latest.results[0].verdict,
        Verdict::Confirmed("earlier".into())
    );
    assert_eq!(
        latest.results[1].verdict,
        Verdict::Confirmed("names it now".into())
    );
}

#[test]
fn a_launched_check_applies_its_results() {
    let mut fx = Fx::checking("1: confirmed: ok\n2: fails: no meeting name");
    fx.set("FAKE_CLAUDE_PROOF", "1");
    let order = CaptureOrder {
        req: fx.check_request(None),
        env: fx.env.clone(),
        managed_mcp: false,
    };
    launch(&order).unwrap();
    let checks = fx.tmp.path().join("checks");
    fx.wait("the check to end", |_| !is_running(&checks));
    let intent = fx.tmp.path().join("root/skoolscout/org/notify.md");
    let it = herdr_intents::store::Store::load(&intent).unwrap();
    let crit = herdr_intents::store::criteria(&it.body);
    assert!(crit[0].done && !crit[1].done, "{}", it.body);
    assert_eq!(it.fm.missing.len(), 1, "the failing one has its item");
}

#[test]
fn a_failure_after_a_fold_files_nothing() {
    let mut fx = Fx::checking("1: confirmed: ok\n2: fails: no meeting name");
    fx.set("FAKE_CLAUDE_PROOF", "1");
    let mut req = fx.check_request(None);
    if let Job::Check { fold, .. } = &mut req.job {
        *fold = true;
    }
    let order = CaptureOrder {
        req,
        env: fx.env.clone(),
        managed_mcp: false,
    };
    launch(&order).unwrap();
    let checks = fx.tmp.path().join("checks");
    fx.wait("the check to end", |_| !is_running(&checks));
    let latest = read_check(&checks).expect("the results are kept");
    assert_eq!(
        latest.results[1].verdict,
        Verdict::CantCheck("the fold may describe this wrongly: no meeting name".into())
    );
    let intent = fx.tmp.path().join("root/skoolscout/org/notify.md");
    let it = herdr_intents::store::Store::load(&intent).unwrap();
    let crit = herdr_intents::store::criteria(&it.body);
    assert!(crit[0].done && !crit[1].done, "{}", it.body);
    assert!(it.fm.missing.is_empty(), "a fold's failure files nothing");
}

#[test]
fn a_log_that_leads_out_of_the_worktree_is_refused() {
    let fx = Fx::checking("1: confirmed: ok");
    let t = fx.tmp.path();
    fs::write(t.join("outside.txt"), "token: secret\n").unwrap();
    let log = t.join("worktree/.logs/backend.log");
    fs::remove_file(&log).unwrap();
    std::os::unix::fs::symlink(t.join("outside.txt"), &log).unwrap();
    let (_, rec) = fx.run_check(fx.check_request(None));
    match rec.outcome {
        Outcome::Failed(why) => assert!(why.contains("log"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fx.log("claude.log"),
        "",
        "no agent with a log it may not read"
    );
    assert!(
        fx.log("work.log")
            .lines()
            .last()
            .unwrap()
            .contains("finish --area")
    );
}

#[test]
fn a_check_with_no_answers_spends_nothing() {
    let fx = Fx::checking("I ran out of time before I could look.");
    let (checked, rec) = fx.run_check(fx.check_request(None));
    assert!(
        checked.is_none(),
        "nothing to apply: the merged set is not spent"
    );
    match rec.outcome {
        Outcome::Failed(why) => assert!(why.contains("no answers"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(
        read_check(&fx.tmp.path().join("checks")).is_none(),
        "no ? results written"
    );
}

// ---- recording the stories ---------------------------------------------------

const STORY_INTENT: &str = "---\ntitle: Reinstate a withdrawn application\nkind: enhancement\nstate: sent\n---\n## Description\nx\n\n## Acceptance criteria\n- [ ] one\n- [ ] two\n- [ ] three\n\n## Stories\n### Mark reinstates Maya\n- persona: Mark\n- shows: 1, 3\n- starting data: Maya's application, withdrawn\n1. Open it\n2. Reinstate it\n### Pat sees it back\n- persona: Pat\n- shows: 2\n- starting data: a reinstated application\n1. Open applications\n";
const STORY_GUIDE: &str = "---\ndefault_area: org/social\nseed: [kathia-signup]\nurl: https://demo-org.skoolscout.com.local:{port}\nlogs: [.logs/backend.log]\n---\nSign in with Cmd+U.\n\n## Stories\n### Personas\n- Mark: tenant admin, signs in as mark@example.test\n- Pat: a parent, signs in as pat@example.test\n### Setup may create\n- applications, never people\n";
const ALL_CONFIRMED: &str = "1: confirmed: a\n2: confirmed: b\n3: confirmed: c";

impl Fx {
    /// A check of a three-criterion intent with two stories, whose guide
    /// names their personas; every story agent leaves captions 1, 3 and 2.
    fn stories(result: &str) -> Fx {
        let mut fx = Fx::checking(result);
        let t = fx.tmp.path().to_path_buf();
        fs::write(t.join("repo/.helmsmith/capture.md"), STORY_GUIDE).unwrap();
        fs::write(t.join("root/skoolscout/org/notify.md"), STORY_INTENT).unwrap();
        fx.set("FAKE_CLAUDE_PROOF", "1,2,3");
        fx.set("FAKE_CLAUDE_CAPTIONS", "1,3,2");
        fx
    }

    fn stories_dir(&self) -> std::path::PathBuf {
        self.tmp
            .path()
            .join("root/skoolscout/org/notify.assets/stories")
    }

    fn story(&self, n: usize) -> std::path::PathBuf {
        self.stories_dir().join(match n {
            1 => "1-mark-reinstates-maya",
            _ => "2-pat-sees-it-back",
        })
    }

    fn story_request(&self, numbers: Vec<usize>) -> CaptureRequest {
        let t = self.tmp.path();
        CaptureRequest {
            phase: Phase::Before,
            shot: Shot::Video,
            evidence: self.stories_dir(),
            job: Job::Story {
                numbers,
                intent_path: t.join("root/skoolscout/org/notify.md"),
                root: t.join("root"),
                stories_dir: self.stories_dir(),
            },
            ..self.request()
        }
    }
}

#[test]
fn a_passing_check_records_each_story_in_the_same_run() {
    use herdr_intents::capture::{StoryOutcome, read_story};
    let fx = Fx::stories(ALL_CONFIRMED);
    fx.run_check(fx.check_request(None));
    assert_eq!(
        read_story(&fx.story(1)).unwrap().outcome,
        StoryOutcome::Kept
    );
    assert_eq!(
        read_story(&fx.story(2)).unwrap().outcome,
        StoryOutcome::Kept
    );
    assert!(fx.story(1).join("video.mp4").is_file());
    let work = fx.log("work.log");
    assert_eq!(
        work.lines().filter(|l| l.contains("start --area")).count(),
        1,
        "one app for the check and the takes: {work}"
    );
    let claude = fx.log("claude.log");
    assert!(
        claude.contains("mcp__intents-capture__take_start"),
        "{claude}"
    );
}

#[test]
fn a_story_whose_criterion_did_not_show_keeps_no_video() {
    use herdr_intents::capture::{StoryOutcome, read_story};
    let mut fx = Fx::stories(ALL_CONFIRMED);
    fx.set("FAKE_CLAUDE_CAPTIONS", "1");
    fx.run_check(fx.check_request(None));
    assert_eq!(
        read_story(&fx.story(1)).unwrap().outcome,
        StoryOutcome::NotShown(vec![3])
    );
    assert!(!fx.story(1).join("video.mp4").exists());
}

#[test]
fn a_failing_check_records_no_story() {
    let fx = Fx::stories("1: confirmed: a\n2: fails: no\n3: confirmed: c");
    fx.run_check(fx.check_request(None));
    assert!(
        !fx.stories_dir().exists() || fs::read_dir(fx.stories_dir()).unwrap().next().is_none(),
        "nothing recorded"
    );
}

#[test]
fn a_re_check_records_again_only_the_stories_it_touched() {
    use herdr_intents::capture::read_story;
    let mut fx = Fx::stories(ALL_CONFIRMED);
    fx.run_check(fx.check_request(None));
    let first = (
        read_story(&fx.story(1)).unwrap().at,
        read_story(&fx.story(2)).unwrap().at,
    );
    // criterion 3 is checked again alone: story 1 shows it, story 2 does not
    fx.set("FAKE_CLAUDE_PROOF", "3");
    let events = fx.tmp.path().join("check-events.jsonl");
    fs::write(
        &events,
        "{\"type\":\"result\",\"result\":\"3: confirmed: c\"}\n",
    )
    .unwrap();
    fx.run_check(fx.check_request(Some(vec![herdr_intents::store::criterion_key("three")])));
    assert!(
        read_story(&fx.story(1)).unwrap().at > first.0,
        "recorded again"
    );
    assert_eq!(read_story(&fx.story(2)).unwrap().at, first.1, "left alone");
}

#[test]
fn a_guide_without_personas_records_nothing_and_says_why() {
    use herdr_intents::capture::{StoryOutcome, read_story};
    let fx = Fx::stories(ALL_CONFIRMED);
    fs::write(
        fx.tmp.path().join("repo/.helmsmith/capture.md"),
        STORY_GUIDE.split("\n## Stories").next().unwrap(),
    )
    .unwrap();
    fx.run_check(fx.check_request(None));
    assert_eq!(
        read_story(&fx.story(1)).unwrap().outcome,
        StoryOutcome::Failed("the run guide lists no personas for stories".into())
    );
    assert!(!fx.log("claude.log").contains("take_start"), "no take ran");
}

#[test]
fn a_persona_the_guide_does_not_list_fails_that_story() {
    use herdr_intents::capture::{StoryOutcome, read_story};
    let fx = Fx::stories(ALL_CONFIRMED);
    fs::write(
        fx.tmp.path().join("root/skoolscout/org/notify.md"),
        STORY_INTENT.replace("- persona: Pat", "- persona: Zed"),
    )
    .unwrap();
    fx.run_check(fx.check_request(None));
    assert_eq!(
        read_story(&fx.story(1)).unwrap().outcome,
        StoryOutcome::Kept
    );
    assert!(
        matches!(read_story(&fx.story(2)).unwrap().outcome, StoryOutcome::Failed(w) if w.contains("Zed"))
    );
}

#[test]
fn a_cancelled_take_keeps_no_video() {
    use herdr_intents::capture::{StoryOutcome, read_story};
    let mut fx = Fx::stories(ALL_CONFIRMED);
    fx.set("FAKE_CLAUDE_HANG_TAKE", "1");
    let mut job = CaptureJob::spawn(fx.story_request(vec![1]), fx.env.clone(), false);
    let start = Instant::now();
    while !fx.story(1).join("take.mp4").exists() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the take never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    job.cancel();
    fx.wait_done(&job);
    assert!(read_story(&fx.story(1)).is_none_or(|r| r.outcome != StoryOutcome::Kept));
    assert!(!fx.story(1).join("video.mp4").exists());
    assert!(
        !fx.story(1).join("take.mp4").exists(),
        "the partial take is gone"
    );
}

#[test]
fn a_story_job_records_the_chosen_story() {
    use herdr_intents::capture::read_story;
    let fx = Fx::stories(ALL_CONFIRMED);
    let job = CaptureJob::spawn(fx.story_request(vec![2]), fx.env.clone(), false);
    fx.wait_done(&job);
    assert!(read_story(&fx.story(2)).is_some());
    assert!(read_story(&fx.story(1)).is_none());
}

#[test]
fn a_story_run_records_the_stories_queued_while_it_runs() {
    use herdr_intents::capture::{queue_story, queued_stories, read_story};
    let fx = Fx::stories(ALL_CONFIRMED);
    queue_story(&fx.stories_dir(), 1).unwrap();
    let job = CaptureJob::spawn(fx.story_request(vec![2]), fx.env.clone(), false);
    fx.wait_done(&job);
    assert!(read_story(&fx.story(2)).is_some(), "the chosen story");
    assert!(read_story(&fx.story(1)).is_some(), "the queued story");
    assert_eq!(queued_stories(&fx.stories_dir()), Vec::<usize>::new());
    let work = fx.log("work.log");
    assert_eq!(
        work.lines().filter(|l| l.contains("start --area")).count(),
        1,
        "one app for both takes: {work}"
    );
}

#[test]
fn a_failing_check_still_records_the_stories_queued_by_hand() {
    use herdr_intents::capture::{queue_story, queued_stories, read_story};
    let fx = Fx::stories("1: confirmed: a\n2: fails: no\n3: confirmed: c");
    queue_story(&fx.stories_dir(), 2).unwrap();
    fx.run_check(fx.check_request(None));
    assert!(read_story(&fx.story(2)).is_some(), "asked for by hand");
    assert!(read_story(&fx.story(1)).is_none(), "not passing: no others");
    assert_eq!(queued_stories(&fx.stories_dir()), Vec::<usize>::new());
}

#[test]
fn queued_stories_say_why_when_the_app_cannot_start() {
    use herdr_intents::capture::{StoryOutcome, queue_story, queued_stories, read_story};
    let mut fx = Fx::stories(ALL_CONFIRMED);
    fx.set("FAKE_WORK_REFUSE", "org/team:HEALTH_TIMEOUT");
    queue_story(&fx.stories_dir(), 1).unwrap();
    let job = CaptureJob::spawn(fx.story_request(vec![2]), fx.env.clone(), false);
    fx.wait_done(&job);
    for n in [1, 2] {
        match read_story(&fx.story(n)).map(|r| r.outcome) {
            Some(StoryOutcome::Failed(why)) => {
                assert!(why.contains("HEALTH_TIMEOUT"), "{n}: {why}")
            }
            other => panic!("story {n}: {other:?}"),
        }
    }
    assert_eq!(queued_stories(&fx.stories_dir()), Vec::<usize>::new());
}

#[test]
fn a_story_take_carries_the_intent_and_the_guides_look() {
    let fx = Fx::stories(ALL_CONFIRMED);
    let t = fx.tmp.path();
    let guide = STORY_GUIDE.replace(
        "logs: [.logs/backend.log]\n",
        "logs: [.logs/backend.log]\nsplash:\n  logo: brand/logo.png\n  background: \"#142b57\"\n  accent: \"url(x)\"\n",
    );
    fs::write(t.join("repo/.helmsmith/capture.md"), guide).unwrap();
    fs::create_dir_all(t.join("repo/brand")).unwrap();
    fs::write(t.join("repo/brand/logo.png"), "foobar").unwrap();
    let intent = t.join("root/skoolscout/org/notify.md");
    let text = fs::read_to_string(&intent).unwrap();
    fs::write(
        &intent,
        text.replacen("---\n", "---\nid: k7q2\nversion: 3\n", 1),
    )
    .unwrap();
    let job = CaptureJob::spawn(fx.story_request(vec![1]), fx.env.clone(), false);
    fx.wait_done(&job);
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(fx.story(1).join(".capture-mcp.json")).unwrap())
            .unwrap();
    let take = &config["take"];
    assert_eq!(take["intent_title"], "Reinstate a withdrawn application");
    assert_eq!(take["intent_id"], "k7q2");
    assert_eq!(take["version"], 3);
    assert_eq!(take["look"]["background"], "#142b57");
    assert_eq!(take["look"]["logo"], "data:image/png;base64,Zm9vYmFy");
    assert_eq!(
        take["look"]["accent"], "#7aa2ff",
        "a bad color keeps the default"
    );
    let log = fs::read_to_string(fx.story(1).join("run.log")).unwrap();
    assert!(log.contains("the splash's accent `url(x)`"), "{log}");
}

#[test]
fn the_story_prompt_orders_setup_rehearsal_and_take() {
    use herdr_intents::capture::{guide_stories, read_guide, story_prompt};
    let fx = Fx::stories(ALL_CONFIRMED);
    let guide = read_guide(&fx.tmp.path().join("repo")).unwrap();
    let story = &herdr_intents::store::stories(STORY_INTENT)[0];
    let shows = vec![(1, "one".to_string()), (3, "three".to_string())];
    let text = guide_stories(&guide).unwrap().text;
    let p = story_prompt(
        &guide,
        story,
        &shows,
        "Reinstate",
        "http://127.0.0.1:1",
        &text,
    );
    let at = |s: &str| p.find(s).unwrap_or_else(|| panic!("no {s:?} in {p}"));
    assert!(at("sign in as Mark") < at("rehearse"));
    assert!(at("rehearse") < at("set up fresh"));
    assert!(at("set up fresh") < at("take_start"));
    assert!(p.contains("never sign in, type a password or a code after take_start"));
    assert!(
        p.contains("fresh records with new names"),
        "leftovers from an earlier run"
    );
    assert!(
        p.contains("1. one") && p.contains("3. three"),
        "the criteria to caption"
    );
    assert!(p.contains("never people"), "the guide's rules");
}

#[test]
fn a_take_that_never_stopped_is_not_kept() {
    use herdr_intents::capture::{StoryOutcome, read_story};
    let mut fx = Fx::stories(ALL_CONFIRMED);
    fx.set("FAKE_CLAUDE_NO_STOP", "1");
    fx.run_check(fx.check_request(None));
    assert!(
        matches!(read_story(&fx.story(1)).unwrap().outcome, StoryOutcome::Failed(w) if w.contains("take_stop")),
        "{:?}",
        read_story(&fx.story(1))
    );
    assert!(!fx.story(1).join("video.mp4").exists());
    assert!(!fx.story(1).join("take.mp4").exists());
}

#[test]
fn a_re_take_that_fails_keeps_the_earlier_video() {
    use herdr_intents::capture::{StoryOutcome, read_story};
    let mut fx = Fx::stories(ALL_CONFIRMED);
    fx.run_check(fx.check_request(None));
    assert!(fx.story(1).join("video.mp4").is_file());
    fx.set("FAKE_CLAUDE_CAPTIONS", "1");
    let job = CaptureJob::spawn(fx.story_request(vec![1]), fx.env.clone(), false);
    fx.wait_done(&job);
    assert_eq!(
        read_story(&fx.story(1)).unwrap().outcome,
        StoryOutcome::NotShown(vec![3])
    );
    assert_eq!(
        fs::read_to_string(fx.story(1).join("video.mp4")).unwrap(),
        "video",
        "the earlier video stays"
    );
}

#[test]
fn the_card_names_the_persona_only() {
    let fx = Fx::stories(ALL_CONFIRMED);
    fs::write(
        fx.tmp.path().join("root/skoolscout/org/notify.md"),
        STORY_INTENT.replace(
            "- persona: Mark\n",
            "- persona: Mark: tenant admin, signs in as mark@example.test with code 123456\n",
        ),
    )
    .unwrap();
    fx.run_check(fx.check_request(None));
    let cfg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(fx.story(1).join(".capture-mcp.json")).unwrap())
            .unwrap();
    assert_eq!(cfg["take"]["persona"], "Mark", "{cfg}");
}

#[test]
fn a_take_keeps_its_run_log() {
    let fx = Fx::stories(ALL_CONFIRMED);
    fx.run_check(fx.check_request(None));
    let log = fs::read_to_string(fx.story(1).join("run.log")).unwrap_or_default();
    assert!(log.contains("1: confirmed: a"), "the agent's reply: {log}");
}

#[test]
fn a_check_writes_its_stage_while_it_runs() {
    let mut fx = Fx::checking("1: confirmed: a\n2: confirmed: b");
    fx.set("FAKE_CLAUDE_HANG", "1");
    let mut job = CaptureJob::spawn(fx.check_request(None), fx.env.clone(), false);
    let checks = fx.tmp.path().join("checks");
    let start = Instant::now();
    loop {
        if herdr_intents::capture::read_stage(&checks).as_deref()
            == Some("the agent is checking the criteria")
        {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "no stage: {:?}",
            herdr_intents::capture::read_stage(&checks)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    job.cancel();
    fx.wait_done(&job);
}

// ---- programs installed under nvm (Herdr's PATH does not load nvm) ----------

#[test]
fn nvms_default_node_is_found() {
    use herdr_intents::capture::nvm_bin;
    let tmp = tempfile::tempdir().unwrap();
    let nvm = tmp.path();
    for v in ["v22.1.0", "v24.2.0", "v24.18.0"] {
        fs::create_dir_all(nvm.join(format!("versions/node/{v}/bin"))).unwrap();
    }
    fs::create_dir_all(nvm.join("alias/lts")).unwrap();
    let set = |a: &str| fs::write(nvm.join("alias/default"), a).unwrap();
    let bin = |v: &str| Some(nvm.join(format!("versions/node/{v}/bin")));
    set("24\n");
    assert_eq!(nvm_bin(nvm), bin("v24.18.0"), "the newest 24");
    set("v22.1.0");
    assert_eq!(nvm_bin(nvm), bin("v22.1.0"));
    set("node");
    assert_eq!(nvm_bin(nvm), bin("v24.18.0"), "the newest installed");
    set("lts/*");
    fs::write(nvm.join("alias/lts/*"), "lts/jod").unwrap();
    fs::write(nvm.join("alias/lts/jod"), "v22.1.0").unwrap();
    assert_eq!(nvm_bin(nvm), bin("v22.1.0"), "an alias of an alias");
    set("19");
    assert_eq!(nvm_bin(nvm), None, "not installed");
    fs::remove_file(nvm.join("alias/default")).unwrap();
    assert_eq!(nvm_bin(nvm), None, "no default");
}

#[test]
fn a_check_finds_work_in_nvms_default_node() {
    let mut fx = Fx::checking("1: confirmed: a\n2: confirmed: b");
    fx.env.retain(|(k, _)| k != "WORK_BIN_PATH");
    let nvm = fx.tmp.path().join("nvm");
    let bin = nvm.join("versions/node/v24.0.0/bin");
    fs::create_dir_all(&bin).unwrap();
    fs::copy(fixture("fake-work.sh"), bin.join("work")).unwrap();
    fs::create_dir_all(nvm.join("alias")).unwrap();
    fs::write(nvm.join("alias/default"), "24").unwrap();
    fx.set("NVM_DIR", &nvm.display().to_string());
    fx.run_check(fx.check_request(None));
    let work = fx.log("work.log");
    assert!(work.contains("start --area"), "{work}");
}

#[test]
fn a_missing_work_says_how_to_get_it() {
    let mut fx = Fx::checking("1: confirmed: a\n2: confirmed: b");
    fx.set("WORK_BIN_PATH", "/nonexistent/work");
    let (_, rec) = fx.run_check(fx.check_request(None));
    match rec.outcome {
        Outcome::Failed(why) => {
            assert!(why.contains("npm i -g @helmsmith/workbrokr"), "{why}");
            assert!(why.contains("nvm"), "{why}");
        }
        other => panic!("{other:?}"),
    }
}
