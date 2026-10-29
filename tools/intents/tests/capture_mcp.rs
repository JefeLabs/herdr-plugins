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
        let tmp = tempfile::tempdir().unwrap();
        let evidence = tmp.path().join("with space/evidence");
        fs::create_dir_all(&evidence).unwrap();
        let config = tmp.path().join("mcp.json");
        fs::write(
            &config,
            json!({
                "base_url": BASE,
                "session": "capture-x-1",
                "evidence": evidence,
                "phase": "before",
                "shot": shot,
            })
            .to_string(),
        )
        .unwrap();
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
