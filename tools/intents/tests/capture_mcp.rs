//! The capture agent's browser tools, spoken to over MCP the way claude does,
//! with a fake agent-browser underneath.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

const BASE: &str = "https://demo-org.skoolscout.com.local:3004";

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
    log: PathBuf,
    evidence: PathBuf,
    _tmp: tempfile::TempDir,
}

impl Server {
    fn start(shot: &str) -> Server {
        Server::spawn(tempfile::tempdir().unwrap(), shot, json!({}))
    }

    /// A completion check's server, with `logs` as `[{path, from}]`.
    fn check(tmp: tempfile::TempDir, logs: Value) -> Server {
        let check_dir = tmp.path().join("with space/checks");
        fs::create_dir_all(&check_dir).unwrap();
        Server::spawn(
            tmp,
            "screenshot",
            json!({"mode": "check", "check_dir": check_dir, "logs": logs}),
        )
    }

    fn check_dir(&self) -> PathBuf {
        self._tmp.path().join("with space/checks")
    }

    fn spawn(tmp: tempfile::TempDir, shot: &str, extra: Value) -> Server {
        let evidence = tmp.path().join("with space/evidence");
        fs::create_dir_all(&evidence).unwrap();
        let config = tmp.path().join("mcp.json");
        let mut cfg = json!({
            "base_url": BASE,
            "session": "capture-x-1",
            "evidence": evidence,
            "phase": "before",
            "shot": shot,
        });
        for (k, v) in extra.as_object().unwrap() {
            cfg[k] = v.clone();
        }
        fs::write(&config, cfg.to_string()).unwrap();
        let log = tmp.path().join("ab.log");
        let mut child = Command::new(env!("CARGO_BIN_EXE_herdr-intents"))
            .args(["capture-mcp", &config.display().to_string()])
            .env(
                "AGENT_BROWSER_BIN_PATH",
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-agent-browser.sh"),
            )
            .env("FAKE_AB_LOG", &log)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Server {
            child,
            stdin,
            stdout,
            next: 1,
            log,
            evidence,
            _tmp: tmp,
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"));
        assert_eq!(v["id"], id, "{v}");
        v
    }

    fn notify(&mut self, method: &str) {
        writeln!(
            self.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "method": method})
        )
        .unwrap();
        self.stdin.flush().unwrap();
    }

    fn open(&mut self) {
        let init = self.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
        );
        assert_eq!(
            init["result"]["serverInfo"]["name"], "intents-capture",
            "{init}"
        );
        assert!(
            init["result"]["capabilities"]["tools"].is_object(),
            "{init}"
        );
        self.notify("notifications/initialized");
    }

    fn tools(&mut self) -> Vec<String> {
        let list = self.request("tools/list", json!({}));
        list["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("{list}"))
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }

    /// (is_error, text)
    fn call(&mut self, tool: &str, arguments: Value) -> (bool, String) {
        let r = self.request("tools/call", json!({"name": tool, "arguments": arguments}));
        let result = &r["result"];
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        (result["isError"].as_bool().unwrap_or(false), text)
    }

    fn browser(&mut self, command: &str, args: &[&str]) -> (bool, String) {
        self.call("browser", json!({"command": command, "args": args}))
    }

    fn log(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn the_browser_tools_run_agent_browser_in_the_captures_own_session() {
    let mut s = Server::start("video");
    s.open();
    assert_eq!(s.tools(), ["browser", "record_start", "record_stop"]);

    let (err, text) = s.browser("open", &["/auth/sign-in"]);
    assert!(!err, "{text}");
    let log = s.log();
    assert_eq!(
        log,
        [
            "[--session][capture-x-1][set][viewport][1280][800]",
            &format!("[--session][capture-x-1][open][{BASE}/auth/sign-in]"),
        ],
        "the window size first, once"
    );

    let out = s.evidence.join("before.mp4").display().to_string();
    let (err, text) = s.call("record_start", json!({}));
    assert!(!err, "{text}");
    assert_eq!(
        s.log().last().unwrap(),
        &format!("[--session][capture-x-1][record][start][{out}]"),
        "the path stays one argument"
    );
    let (err, _) = s.call("record_stop", json!({}));
    assert!(!err);
    let (err, _) = s.browser("click", &["@e3"]);
    assert!(!err);

    // only browser commands are steps: the after repeats them
    let steps = fs::read_to_string(s.evidence.join(".steps-before.txt")).unwrap();
    assert_eq!(steps, "open /auth/sign-in\nclick @e3\n");
}

#[test]
fn refused_commands_never_reach_agent_browser() {
    let mut s = Server::start("video");
    s.open();
    for (cmd, args) in [
        ("eval", vec!["document.cookie"]),
        ("wait", vec!["--fn", "fetch('https://x.example')"]),
        ("open", vec!["https://evil.example/"]),
        ("click", vec!["@e1", "--executable-path", "/bin/sh"]),
        ("snapshot", vec!["--profile", "Default"]),
    ] {
        let (err, text) = s.browser(cmd, &args);
        assert!(err, "{cmd} {args:?} ran: {text}");
    }
    let (err, _) = s.call("screenshot", json!({}));
    assert!(err, "a video capture has no screenshot tool");
    let (err, _) = s.call("bash", json!({"command": "ls"}));
    assert!(err);
    assert_eq!(s.log(), Vec::<String>::new());
}

#[test]
fn a_screenshot_capture_gets_a_screenshot_tool() {
    let mut s = Server::start("screenshot");
    s.open();
    assert_eq!(s.tools(), ["browser", "screenshot"]);
    let (err, text) = s.call("screenshot", json!({}));
    assert!(!err, "{text}");
    let out = s.evidence.join("before.png").display().to_string();
    assert_eq!(
        s.log().last().unwrap(),
        &format!("[--session][capture-x-1][screenshot][{out}]")
    );
}

/// A backend log with one email before the agent started and one after;
/// returns (tmp, log path, the offset at agent start).
fn a_log() -> (tempfile::TempDir, PathBuf, u64) {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("wt/.logs/backend.log");
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, "INFO seed: To: kathia@old.example\nSubject: Seeded\n").unwrap();
    let from = fs::metadata(&log).unwrap().len();
    let mut f = fs::OpenOptions::new().append(true).open(&log).unwrap();
    writeln!(
        f,
        "INFO email sent\nTo: kathia@x.example\nSubject: Meeting request\nBody: please book"
    )
    .unwrap();
    writeln!(f, "INFO unrelated").unwrap();
    (tmp, log, from)
}

#[test]
fn a_check_gets_per_criterion_screenshots_and_logs() {
    let (tmp, log, from) = a_log();
    let mut s = Server::check(tmp, json!([{"path": log, "from": from}]));
    s.open();
    assert_eq!(s.tools(), ["browser", "screenshot", "logs"]);

    let (err, text) = s.call("screenshot", json!({"criterion": 2}));
    assert!(!err, "{text}");
    let shot = s.check_dir().join("2.png").display().to_string();
    assert_eq!(
        s.log().last().unwrap(),
        &format!("[--session][capture-x-1][screenshot][{shot}]")
    );

    let (err, text) = s.call("logs", json!({"criterion": 2, "pattern": "to: KATHIA"}));
    assert!(!err, "{text}");
    assert!(text.contains("To: kathia@x.example"), "{text}");
    assert!(
        text.contains("Subject: Meeting request"),
        "the lines after it: {text}"
    );
    assert!(
        !text.contains("old.example"),
        "only what was logged since the start: {text}"
    );
    let proof = fs::read_to_string(s.check_dir().join("2.log.txt")).unwrap();
    assert!(proof.contains("Subject: Meeting request"), "{proof}");

    let (err, text) = s.call(
        "logs",
        json!({"criterion": 3, "pattern": "nothing like this"}),
    );
    assert!(!err, "{text}");
    assert!(text.contains("no lines"), "{text}");
    assert!(
        !s.check_dir().join("3.log.txt").exists(),
        "no proof without a match"
    );

    let (err, _) = s.call("screenshot", json!({}));
    assert!(err, "a check's screenshot needs its criterion");
}

#[test]
fn a_truncated_log_is_read_from_the_start() {
    let (tmp, log, _) = a_log();
    let mut s = Server::check(tmp, json!([{"path": log, "from": 1_000_000}]));
    s.open();
    let (err, text) = s.call(
        "logs",
        json!({"criterion": 1, "pattern": "Meeting request"}),
    );
    assert!(!err && text.contains("Subject: Meeting request"), "{text}");
}

#[test]
fn logs_only_reads_the_listed_files() {
    let (tmp, log, from) = a_log();
    let mut s = Server::check(tmp, json!([{"path": log, "from": from}]));
    s.open();
    let (_, text) = s.call(
        "logs",
        json!({"criterion": 1, "pattern": "../../etc/passwd"}),
    );
    assert!(
        text.contains("no lines") && !text.contains("root:"),
        "{text}"
    );
}

#[test]
fn a_check_with_no_logs_has_no_logs_tool() {
    let mut s = Server::check(tempfile::tempdir().unwrap(), json!([]));
    s.open();
    assert_eq!(s.tools(), ["browser", "screenshot"]);
    let (err, _) = s.call("logs", json!({"criterion": 1, "pattern": "x"}));
    assert!(err);
}

#[test]
fn a_symlinked_log_is_not_followed() {
    let (tmp, log, _) = a_log();
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, "Subject: secret\n").unwrap();
    let link = tmp.path().join("wt/.logs/link.log");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let _ = log;
    let mut s = Server::check(tmp, json!([{"path": link, "from": 0}]));
    s.open();
    let (_, text) = s.call("logs", json!({"criterion": 1, "pattern": "secret"}));
    assert!(!text.contains("Subject: secret"), "{text}");
}

// ---- a story's take ---------------------------------------------------------

const MARKUP: &str = "say \"hi\" </div><script>alert(1)</script>";

/// A story take's server: shows criteria 1 and 3, holds nothing (tests).
fn take_server() -> Server {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("with space/evidence");
    let mut s = Server::spawn(
        tmp,
        "video",
        json!({"mode": "story", "take": {
            "title": "Mark reinstates Maya's withdrawn application",
            "persona": "Mark, a tenant admin",
            "shows": [[1, "an admin can reinstate it"], [3, MARKUP]],
            "dir": dir,
            "hold_ms": 0
        }}),
    );
    s.open();
    s
}

fn read_captions(s: &Server) -> Vec<u64> {
    serde_json::from_str::<Vec<u64>>(
        &fs::read_to_string(s.evidence.join("captions.json")).unwrap_or_else(|_| "[]".into()),
    )
    .unwrap()
}

fn evals(s: &Server) -> Vec<String> {
    s.log()
        .into_iter()
        .filter(|l| l.contains("[eval]"))
        .collect()
}

#[test]
fn a_take_has_four_tools() {
    let mut s = take_server();
    assert_eq!(s.tools(), ["browser", "take_start", "caption", "take_stop"]);
}

#[test]
fn a_caption_before_the_take_is_refused() {
    let mut s = take_server();
    let (err, t) = s.call("caption", json!({"criterion": 1}));
    assert!(err && t.contains("take_start"), "{t}");
}

#[test]
fn a_caption_for_a_criterion_the_story_does_not_show_is_refused() {
    let mut s = take_server();
    assert!(!s.call("take_start", json!({})).0);
    let (err, t) = s.call("caption", json!({"criterion": 2}));
    assert!(err, "{t}");
    assert!(read_captions(&s).is_empty());
}

#[test]
fn captions_are_recorded_in_order() {
    let mut s = take_server();
    assert!(!s.call("take_start", json!({})).0);
    assert!(!s.call("caption", json!({"criterion": 3})).0);
    assert!(!s.call("caption", json!({"criterion": 1})).0);
    assert!(!s.call("take_stop", json!({})).0);
    assert_eq!(read_captions(&s), vec![3, 1]);
    let log = s.log().join("\n");
    assert!(
        log.contains("[record][start]") && log.contains("take.mp4]"),
        "{log}"
    );
    assert!(
        log.contains("[--cursor]"),
        "the pointer is on the video: {log}"
    );
    assert!(log.contains("[record][stop]"), "{log}");
    assert!(
        s.evidence.join("take.stopped").is_file(),
        "a clean stop is marked"
    );
    let (err, t) = s.call("caption", json!({"criterion": 1}));
    assert!(err, "no caption after the take: {t}");
}

#[test]
fn a_caption_with_markup_is_shown_as_text() {
    let mut s = take_server();
    s.call("take_start", json!({}));
    s.call("caption", json!({"criterion": 3}));
    let shown = evals(&s)
        .into_iter()
        .find(|l| l.contains("alert(1)"))
        .expect("the caption's script");
    assert!(shown.contains("textContent"), "{shown}");
    assert!(!shown.contains("innerHTML"), "{shown}");
    assert!(
        shown.contains(&serde_json::to_string(MARKUP).unwrap()),
        "the text goes in as a string literal: {shown}"
    );
}

#[test]
fn the_cards_name_the_story_and_what_it_showed() {
    let mut s = take_server();
    s.call("take_start", json!({}));
    s.call("caption", json!({"criterion": 1}));
    s.call("take_stop", json!({}));
    let scripts = evals(&s).join("\n");
    assert!(scripts.contains("Mark reinstates Maya"), "{scripts}");
    assert!(scripts.contains("Mark, a tenant admin"), "{scripts}");
    assert!(scripts.contains("an admin can reinstate it"), "{scripts}");
}

/// A take whose intent has an id and version, with the guide's splash.
fn branded_take_server() -> Server {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("with space/evidence");
    let mut s = Server::spawn(
        tmp,
        "video",
        json!({"mode": "story", "take": {
            "title": "Mark reinstates Maya",
            "persona": "Mark",
            "shows": [[1, "an admin can reinstate it"], [3, MARKUP]],
            "dir": dir,
            "hold_ms": 0,
            "intent_title": "Admin can reinstate a withdrawn application",
            "intent_id": "k7q2",
            "version": 3,
            "look": {
                "logo": "data:image/png;base64,Zm9vYmFy",
                "background": "#142b57",
                "accent": "#59b7ff",
                "text": "#fafafa"
            }
        }}),
    );
    s.open();
    s
}

#[test]
fn the_splash_is_the_first_frame() {
    let mut s = branded_take_server();
    assert!(!s.call("take_start", json!({})).0);
    let log = s.log();
    let splash = log
        .iter()
        .position(|l| l.contains("[eval]") && l.contains("This video shows"))
        .expect("a splash");
    let record = log
        .iter()
        .position(|l| l.contains("[record][start]"))
        .expect("a recording");
    assert!(splash < record, "drawn before recording starts: {log:?}");
}

#[test]
fn the_splash_says_what_the_video_covers_and_for_which_version() {
    let mut s = branded_take_server();
    s.call("take_start", json!({}));
    let splash = evals(&s)
        .into_iter()
        .find(|l| l.contains("This video shows"))
        .unwrap();
    for part in [
        "Admin can reinstate a withdrawn application",
        "Mark reinstates Maya",
        "as Mark",
        "an admin can reinstate it",
        "k7q2 · v3 · recorded 20",
    ] {
        assert!(splash.contains(part), "{part:?} in {splash}");
    }
}

#[test]
fn the_splash_carries_the_logo_and_the_brand_colors_as_data() {
    let mut s = branded_take_server();
    s.call("take_start", json!({}));
    let splash = evals(&s)
        .into_iter()
        .find(|l| l.contains("This video shows"))
        .unwrap();
    for part in [
        "data:image/png;base64,Zm9vYmFy",
        "#142b57",
        "#59b7ff",
        "#fafafa",
    ] {
        assert!(splash.contains(part), "{part:?} in {splash}");
    }
    assert!(splash.contains("textContent"), "{splash}");
    assert!(!splash.contains("innerHTML"), "{splash}");
    let markup = serde_json::to_string(MARKUP).unwrap();
    assert!(
        splash.contains(&markup[1..markup.len() - 1]),
        "a criterion goes in as data, never markup: {splash}"
    );
}

#[test]
fn captions_and_the_closing_card_take_the_brand_colors() {
    let mut s = branded_take_server();
    s.call("take_start", json!({}));
    s.call("caption", json!({"criterion": 1}));
    s.call("take_stop", json!({}));
    let scripts = evals(&s);
    let caption = scripts
        .iter()
        .find(|l| {
            l.contains("an admin can reinstate it")
                && !l.contains("This video shows")
                && !l.contains("Shown in this video")
        })
        .unwrap();
    assert!(caption.contains("rgba(20,43,87,0.88)"), "{caption}");
    assert!(caption.contains("#fafafa"), "{caption}");
    let closing = scripts
        .iter()
        .find(|l| l.contains("Shown in this video"))
        .unwrap();
    for part in [
        "✓ 1. an admin can reinstate it",
        "data:image/png;base64,Zm9vYmFy",
        "k7q2 · v3 · recorded 20",
        "#142b57",
    ] {
        assert!(closing.contains(part), "{part:?} in {closing}");
    }
    assert!(
        !closing.contains("✓ 3."),
        "only what was captioned: {closing}"
    );
}

#[test]
fn the_agent_still_cannot_eval() {
    let mut s = take_server();
    let (err, _) = s.browser("eval", &["1"]);
    assert!(err);
}

#[test]
fn only_the_take_is_in_the_steps() {
    let mut s = take_server();
    assert!(!s.browser("open", &["/applications"]).0);
    s.call("take_start", json!({}));
    assert!(!s.browser("click", &["@e1"]).0);
    s.call("take_stop", json!({}));
    let steps = fs::read_to_string(s.evidence.join("steps.txt")).unwrap();
    assert_eq!(steps.lines().collect::<Vec<_>>(), vec!["click @e1"]);
}

#[test]
fn a_tool_error_in_a_take_goes_to_the_run_log() {
    let mut s = take_server();
    let (err, _) = s.call("caption", json!({"criterion": 1}));
    assert!(err);
    let log = fs::read_to_string(s.evidence.join("run.log")).unwrap_or_default();
    assert!(
        log.contains("caption") && log.contains("take_start"),
        "{log}"
    );
}
