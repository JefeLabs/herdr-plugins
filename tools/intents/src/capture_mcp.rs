//! The capture agent's only tools: an MCP server (`herdr-intents capture-mcp
//! <config>`) whose `browser`, `record_start`, `record_stop` and `screenshot`
//! tools check every command against a fixed list before agent-browser runs
//! it, in the capture's own session. The agent gets no shell.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::capture::{Phase, Shot};

/// The server's name: its tools reach claude as `mcp__intents-capture__<tool>`.
pub const SERVER: &str = "intents-capture";

/// What the runner tells the server, in a file it writes before claude starts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfig {
    /// The app on the capture's stack, e.g. `https://demo-org…:3004`.
    pub base_url: String,
    /// The agent-browser session, one per capture.
    pub session: String,
    /// Where the recording or screenshot and the steps go.
    pub evidence: PathBuf,
    pub phase: Phase,
    pub shot: Shot,
}

/// The tools the agent gets for this kind of capture.
pub fn tool_names(shot: Shot) -> Vec<&'static str> {
    match shot {
        Shot::Video => vec!["browser", "record_start", "record_stop"],
        Shot::Screenshot => vec!["browser", "screenshot"],
    }
}

/// Where the server keeps the browser commands it ran, one per line.
pub fn steps_path(evidence: &Path, phase: Phase) -> PathBuf {
    evidence.join(match phase {
        Phase::Before => ".steps-before.txt",
        Phase::After => ".steps-after.txt",
    })
}

/// The recording or screenshot a capture writes, in its evidence folder.
pub fn out_name(phase: Phase, shot: Shot) -> String {
    format!(
        "{}.{}",
        match phase {
            Phase::Before => "before",
            Phase::After => "after",
        },
        match shot {
            Shot::Video => "mp4",
            Shot::Screenshot => "png",
        }
    )
}

/// How one `browser` command may be called: its positional arguments (the
/// first one, when `first` is set, from that list) and the flags it accepts,
/// each with whether it takes a value. Anything else is refused, so global
/// flags (`--session`, `--executable-path`, `--profile`, `--auto-connect`…),
/// `wait --fn` and every command off the list never reach agent-browser.
struct Shape {
    min: usize,
    max: usize,
    first: &'static [&'static str],
    flags: &'static [(&'static str, bool)],
}

fn shape(command: &str) -> Option<Shape> {
    let s = |min, max, first, flags| {
        Some(Shape {
            min,
            max,
            first,
            flags,
        })
    };
    match command {
        "click" | "dblclick" | "hover" | "focus" | "check" | "uncheck" | "scrollintoview" => {
            s(1, 1, &[], &[])
        }
        "type" | "fill" | "drag" => s(2, 2, &[], &[]),
        "press" => s(1, 1, &[], &[]),
        "select" => s(2, 10, &[], &[]),
        "keyboard" => s(2, 2, &["type", "inserttext"], &[]),
        "scroll" => s(1, 2, &["up", "down", "left", "right"], &[]),
        "wait" => s(
            0,
            1,
            &[],
            &[
                ("--text", true),
                ("--url", true),
                ("--load", true),
                ("--state", true),
            ],
        ),
        "snapshot" => s(
            0,
            0,
            &[],
            &[
                ("-i", false),
                ("--interactive", false),
                ("-u", false),
                ("--urls", false),
                ("-c", false),
                ("--compact", false),
                ("-d", true),
                ("--depth", true),
                ("-s", true),
                ("--selector", true),
                ("--delta", false),
                ("--full", false),
            ],
        ),
        "get" => s(
            1,
            3,
            &[
                "text", "html", "value", "attr", "title", "url", "count", "box", "styles",
            ],
            &[],
        ),
        "is" => s(2, 2, &["visible", "enabled", "checked"], &[]),
        "find" => s(
            2,
            4,
            &[
                "role",
                "text",
                "label",
                "placeholder",
                "alt",
                "title",
                "testid",
                "first",
                "last",
                "nth",
            ],
            &[("--name", true), ("--exact", false)],
        ),
        "mouse" => s(1, 3, &["move", "down", "up", "wheel"], &[]),
        "back" | "forward" | "reload" => s(0, 0, &[], &[]),
        _ => None,
    }
}

/// A plain number, the only argument that may start with `-` (`-300`, `-2.5`).
fn is_number(arg: &str) -> bool {
    let digits = arg.strip_prefix('-').unwrap_or(arg);
    let mut parts = digits.splitn(2, '.');
    let whole = parts.next().unwrap_or("");
    let frac = parts.next();
    !whole.is_empty()
        && whole.chars().all(|c| c.is_ascii_digit())
        && frac.is_none_or(|f| !f.is_empty() && f.chars().all(|c| c.is_ascii_digit()))
}

/// A flag-looking argument: starts with `-` and is not a plain number.
fn is_flag(arg: &str) -> bool {
    arg.starts_with('-') && !is_number(arg)
}

/// The address `open` goes to: a path on the app, or an address on it.
fn open_target(target: &str, base: &str) -> Result<String, String> {
    if target.starts_with('/') && !target.starts_with("//") {
        return Ok(format!("{base}{target}"));
    }
    let on_base = target == base
        || ["/", "?", "#"]
            .iter()
            .any(|sep| target.starts_with(&format!("{base}{sep}")));
    if on_base {
        Ok(target.to_string())
    } else {
        Err(format!(
            "open goes only to the app ({base}): give a path such as /auth/sign-in"
        ))
    }
}

/// The agent-browser arguments for one `browser` call, or why it is refused.
pub fn check_browser(command: &str, args: &[String], base: &str) -> Result<Vec<String>, String> {
    if command == "open" {
        if args.len() != 1 {
            return Err("open takes one address".into());
        }
        return Ok(vec!["open".into(), open_target(&args[0], base)?]);
    }
    let Some(shape) = shape(command) else {
        return Err(format!(
            "`{command}` is not one of the browser commands this capture may run"
        ));
    };
    let mut positional = 0;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if is_flag(a) {
            let Some((_, takes_value)) = shape.flags.iter().find(|(f, _)| f == a) else {
                return Err(format!("`{command}` does not take `{a}`"));
            };
            if *takes_value {
                // a value that looks like a flag could be read as one by agent-browser
                match args.get(i + 1) {
                    None => return Err(format!("`{a}` needs a value")),
                    Some(v) if is_flag(v) => {
                        return Err(format!("the value of `{a}` cannot start with `-`"));
                    }
                    Some(_) => {}
                }
                i += 1;
            }
        } else {
            if positional == 0 && !shape.first.is_empty() && !shape.first.contains(&a.as_str()) {
                return Err(format!(
                    "`{command}` starts with one of: {}",
                    shape.first.join(", ")
                ));
            }
            positional += 1;
        }
        i += 1;
    }
    if positional < shape.min || positional > shape.max || (command == "wait" && args.is_empty()) {
        return Err(format!("wrong number of arguments for `{command}`"));
    }
    let mut out = vec![command.to_string()];
    out.extend(args.iter().cloned());
    Ok(out)
}

/// One step as the "after" reads it: `open` by path, arguments with spaces quoted.
fn step_line(command: &str, args: &[String], base: &str) -> String {
    let shown = args.iter().map(|a| {
        let a = if command == "open" {
            a.strip_prefix(base)
                .filter(|p| !p.is_empty())
                .unwrap_or(a.as_str())
                .to_string()
        } else {
            a.clone()
        };
        if a.chars().any(char::is_whitespace) || a.is_empty() {
            format!("\"{}\"", a.replace('"', "\\\""))
        } else {
            a
        }
    });
    std::iter::once(command.to_string())
        .chain(shown)
        .collect::<Vec<_>>()
        .join(" ")
}

const BROWSER_HELP: &str = "Drive the browser on the app being checked. `command` is one agent-browser command and `args` its arguments, one string each: open (a path on the app, e.g. /auth/sign-in), snapshot (-i lists the interactive elements as @e refs), click, dblclick, fill, type, press (e.g. Enter, Meta+u), select, check, uncheck, hover, focus, scroll, scrollintoview, drag, keyboard, mouse, wait (a selector, milliseconds, --text, --url, --load or --state), get, is, find, back, forward, reload.";

fn tool_list(shot: Shot) -> serde_json::Value {
    let empty = serde_json::json!({"type": "object", "properties": {}});
    let tools: Vec<serde_json::Value> = tool_names(shot)
        .into_iter()
        .map(|name| match name {
            "browser" => serde_json::json!({
                "name": "browser",
                "description": BROWSER_HELP,
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string"},
                        "args": {"type": "array", "items": {"type": "string"}}
                    },
                    "required": ["command"]
                }
            }),
            "record_start" => serde_json::json!({"name": name, "description": "Start recording the browser to this capture's video.", "inputSchema": empty}),
            "record_stop" => serde_json::json!({"name": name, "description": "Stop recording and save the video.", "inputSchema": empty}),
            _ => serde_json::json!({"name": name, "description": "Save a screenshot of the page as this capture's picture.", "inputSchema": empty}),
        })
        .collect();
    serde_json::json!({ "tools": tools })
}

/// The server's state: the config and whether the window has its size yet.
struct Server {
    cfg: McpConfig,
    agent_browser: PathBuf,
    sized: bool,
}

impl Server {
    /// Runs agent-browser in the capture's session; (ok, its output).
    fn run(&mut self, args: &[String]) -> (bool, String) {
        if !self.sized {
            self.sized = true;
            let (w, h) = crate::capture::VIEWPORT;
            let _ = self.exec(&[
                "set".into(),
                "viewport".into(),
                w.to_string(),
                h.to_string(),
            ]);
        }
        self.exec(args)
    }

    fn exec(&self, args: &[String]) -> (bool, String) {
        let out = std::process::Command::new(&self.agent_browser)
            .arg("--session")
            .arg(&self.cfg.session)
            .args(args)
            .current_dir(&self.cfg.evidence)
            .stdin(std::process::Stdio::null())
            .output();
        match out {
            Ok(o) => {
                let mut text = String::from_utf8_lossy(&o.stdout).to_string();
                let err = String::from_utf8_lossy(&o.stderr);
                if !err.trim().is_empty() {
                    text.push_str(&err);
                }
                if text.len() > 60_000 {
                    let mut cut = 60_000;
                    while !text.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    text.truncate(cut);
                    text.push_str("\n[… cut]");
                }
                (o.status.success(), text)
            }
            Err(e) => (false, format!("agent-browser: {e}")),
        }
    }

    /// One `tools/call`: (is_error, text).
    fn call(&mut self, name: &str, arguments: &serde_json::Value) -> (bool, String) {
        if !tool_names(self.cfg.shot).contains(&name) {
            return (true, format!("no tool `{name}` here"));
        }
        let out = self
            .cfg
            .evidence
            .join(out_name(self.cfg.phase, self.cfg.shot));
        let out = out.display().to_string();
        let (ok, text) = match name {
            "record_start" => self.run(&["record".into(), "start".into(), out]),
            "record_stop" => self.run(&["record".into(), "stop".into()]),
            "screenshot" => self.run(&["screenshot".into(), out]),
            _ => {
                let command = arguments
                    .get("command")
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
                let args: Vec<String> = arguments
                    .get("args")
                    .and_then(|a| a.as_array())
                    .map(|a| {
                        a.iter()
                            .map(|x| {
                                x.as_str()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| x.to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let checked = match check_browser(command, &args, &self.cfg.base_url) {
                    Ok(c) => c,
                    Err(why) => return (true, format!("refused: {why}")),
                };
                let (ok, text) = self.run(&checked);
                if ok {
                    use std::io::Write;
                    let line = step_line(command, &checked[1..], &self.cfg.base_url);
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(steps_path(&self.cfg.evidence, self.cfg.phase))
                    {
                        let _ = writeln!(f, "{line}");
                    }
                }
                (ok, text)
            }
        };
        (!ok, text)
    }

    /// The reply to one JSON-RPC message; `None` for a notification.
    fn handle(&mut self, msg: &serde_json::Value) -> Option<serde_json::Value> {
        let id = msg.get("id")?.clone();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let result = match method {
            "initialize" => serde_json::json!({
                "protocolVersion": params.get("protocolVersion").cloned().unwrap_or_else(|| "2025-06-18".into()),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": SERVER, "version": env!("CARGO_PKG_VERSION")}
            }),
            "ping" => serde_json::json!({}),
            "tools/list" => tool_list(self.cfg.shot),
            "tools/call" => {
                let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let arguments = params.get("arguments").cloned().unwrap_or_default();
                let (is_error, text) = self.call(name, &arguments);
                serde_json::json!({"content": [{"type": "text", "text": text}], "isError": is_error})
            }
            _ => {
                return Some(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32601, "message": format!("no method {method}")}
                }));
            }
        };
        Some(serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }
}

/// Serves the tools over stdin/stdout (one JSON-RPC message per line) until
/// stdin closes.
pub fn serve(config: &Path) -> anyhow::Result<()> {
    use std::io::{BufRead, Write};
    let cfg: McpConfig = serde_json::from_str(&std::fs::read_to_string(config)?)?;
    let agent_browser =
        crate::progress::tool_bin(&[], "AGENT_BROWSER_BIN_PATH", Some("agent-browser"))
            .unwrap_or_else(|| "agent-browser".into());
    let mut server = Server {
        cfg,
        agent_browser,
        sized: false,
    };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(msg) => server.handle(&msg),
            Err(e) => Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32700, "message": format!("parse error: {e}")}
            })),
        };
        if let Some(r) = reply {
            writeln!(stdout, "{r}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://demo-org.skoolscout.com.local:3004";

    fn check(command: &str, args: &[&str]) -> Result<Vec<String>, String> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        check_browser(command, &args, BASE)
    }

    fn ok(command: &str, args: &[&str]) -> Vec<String> {
        check(command, args).unwrap_or_else(|e| panic!("{command} {args:?} refused: {e}"))
    }

    #[test]
    fn open_takes_a_path_or_an_address_on_the_stack() {
        assert_eq!(
            ok("open", &["/auth/sign-in"]),
            ["open", &format!("{BASE}/auth/sign-in")]
        );
        assert_eq!(
            ok("open", &[&format!("{BASE}/x?y=1")]),
            ["open", &format!("{BASE}/x?y=1")]
        );
        assert_eq!(ok("open", &[BASE]), ["open", BASE]);
        for bad in [
            "https://evil.example/",
            "https://demo-org.skoolscout.com.local:3004.evil.com/",
            "https://demo-org.skoolscout.com.local:3004@evil.com/",
            "//evil.com/x",
            "javascript:alert(1)",
            "file:///etc/passwd",
        ] {
            assert!(check("open", &[bad]).is_err(), "{bad} opened");
        }
        assert!(check("open", &[]).is_err());
        assert!(check("open", &["/a", "/b"]).is_err());
    }

    #[test]
    fn commands_off_the_list_are_refused() {
        for (cmd, args) in [
            ("eval", vec!["1"]),
            ("read", vec!["https://x"]),
            ("close", vec![]),
            ("set", vec!["viewport", "1", "1"]),
            ("tab", vec!["new"]),
            ("connect", vec!["9222"]),
            ("upload", vec!["@e1", "/etc/passwd"]),
            ("download", vec!["@e1", "/tmp/x"]),
            ("record", vec!["start", "/tmp/x.mp4"]),
            ("screenshot", vec!["/tmp/x.png"]),
            ("get", vec!["cdp-url"]),
        ] {
            assert!(check(cmd, &args).is_err(), "{cmd} {args:?} allowed");
        }
    }

    #[test]
    fn only_listed_flags_pass() {
        assert!(check("wait", &["--fn", "true"]).is_err(), "page JavaScript");
        assert!(check("wait", &["--download"]).is_err());
        assert!(check("click", &["@e1", "--executable-path", "/bin/sh"]).is_err());
        assert!(check("click", &["@e1", "--session", "other"]).is_err());
        assert!(check("open", &["/x", "--auto-connect"]).is_err());
        assert!(check("snapshot", &["--profile", "Default"]).is_err());
        assert!(check("click", &["--", "@e1"]).is_err());
        assert_eq!(
            ok("wait", &["--text", "Saved"]),
            ["wait", "--text", "Saved"]
        );
        assert_eq!(ok("wait", &["@e3"]), ["wait", "@e3"]);
        assert_eq!(ok("snapshot", &["-i", "-c"]), ["snapshot", "-i", "-c"]);
        assert_eq!(
            ok("find", &["role", "button", "click", "--name", "Save"]),
            ["find", "role", "button", "click", "--name", "Save"]
        );
        assert_eq!(ok("mouse", &["wheel", "-300"]), ["mouse", "wheel", "-300"]);
        assert_eq!(ok("get", &["text", "@e1"]), ["get", "text", "@e1"]);
        assert_eq!(ok("fill", &["@e3", "a b"]), ["fill", "@e3", "a b"]);
        assert_eq!(ok("press", &["Meta+u"]), ["press", "Meta+u"]);
        assert_eq!(ok("back", &[]), ["back"]);
    }

    #[test]
    fn a_flag_value_is_never_another_flag() {
        // agent-browser could read these values as its own global flags
        assert!(check("wait", &["--text", "--auto-connect"]).is_err());
        assert!(check("snapshot", &["-s", "--session", "-d", "x"]).is_err());
        assert!(check("find", &["role", "button", "--name", "--profile"]).is_err());
        assert!(check("wait", &["--url", "-x"]).is_err());
        // only plain numbers pass as numbers
        assert!(check("mouse", &["wheel", "-inf"]).is_err());
        assert!(check("mouse", &["wheel", "-1e5"]).is_err());
        assert!(check("mouse", &["wheel", "-NaN"]).is_err());
        assert_eq!(
            ok("mouse", &["wheel", "-300.5"]),
            ["mouse", "wheel", "-300.5"]
        );
        assert_eq!(ok("snapshot", &["-d", "3"]), ["snapshot", "-d", "3"]);
    }

    #[test]
    fn video_gets_recording_and_screenshot_gets_a_shot() {
        assert_eq!(
            tool_names(Shot::Video),
            ["browser", "record_start", "record_stop"]
        );
        assert_eq!(tool_names(Shot::Screenshot), ["browser", "screenshot"]);
    }
}
