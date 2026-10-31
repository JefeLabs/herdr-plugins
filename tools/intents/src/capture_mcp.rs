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
    /// A capture (the default) or a completion check.
    #[serde(default)]
    pub mode: Mode,
    /// A check's proof folder (`<stem>.assets/checks`).
    #[serde(default)]
    pub check_dir: Option<PathBuf>,
    /// The log files a check may search, each from its length at agent start.
    #[serde(default)]
    pub logs: Vec<LogFile>,
    /// A story take's cards and captions.
    #[serde(default)]
    pub take: Option<TakeConfig>,
}

/// What the agent is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Capture,
    Check,
    /// A story's demo video: one take, with cards and captions.
    Story,
}

/// A story take: what its cards and captions say, and where it goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakeConfig {
    /// The story's name, on the opening card.
    pub title: String,
    pub persona: String,
    /// The criteria the story shows: (number, the intent's own text).
    pub shows: Vec<(usize, String)>,
    /// The story's folder: video.mp4, captions.json, steps.txt.
    pub dir: PathBuf,
    /// How long every card and caption stays (tests: 0); by default 3 s for
    /// a card and 4 s for a caption.
    #[serde(default)]
    pub hold_ms: Option<u64>,
}

/// How long a card stays on screen, unless the take says.
const CARD_MS: u64 = 3000;
/// How long a caption stays on screen, unless the take says.
const CAPTION_MS: u64 = 4000;
/// The overlays' element ids: fixed, so each draw replaces the last.
const CARD_ID: &str = "intents-take-card";
const CAPTION_ID: &str = "intents-take-caption";
const CARD_STYLE: &str = "position:fixed;inset:0;z-index:2147483647;display:flex;align-items:center;justify-content:center;background:rgba(14,14,20,.94);color:#fff;font:600 34px/1.35 system-ui,-apple-system,sans-serif;padding:8vw;white-space:pre-line;text-align:center;box-sizing:border-box";
const CAPTION_STYLE: &str = "position:fixed;left:5vw;right:5vw;bottom:5vh;z-index:2147483647;background:rgba(14,14,20,.88);color:#fff;font:500 24px/1.4 system-ui,-apple-system,sans-serif;padding:16px 22px;border-radius:12px;white-space:pre-line;box-sizing:border-box";

/// The page script that shows `text` in the overlay `id`: the text goes in
/// as a JSON string literal through `textContent`, so it is never markup.
fn overlay_script(id: &str, style: &str, text: &str) -> String {
    let text = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
    format!(
        "(() => {{ let d = document.getElementById('{id}'); if (!d) {{ d = document.createElement('div'); d.id = '{id}'; document.documentElement.appendChild(d); }} d.setAttribute('style', '{style}'); d.textContent = {text}; }})()"
    )
}

/// The page script that takes the overlay `id` away.
fn remove_script(id: &str) -> String {
    format!("(() => {{ const d = document.getElementById('{id}'); if (d) d.remove(); }})()")
}

/// A log file and the byte offset the agent's run started at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogFile {
    pub path: PathBuf,
    pub from: u64,
}

/// The tools the agent gets for this run.
pub fn tool_names(cfg: &McpConfig) -> Vec<&'static str> {
    match (cfg.mode, cfg.shot) {
        (Mode::Capture, Shot::Video) => vec!["browser", "record_start", "record_stop"],
        (Mode::Capture, Shot::Screenshot) => vec!["browser", "screenshot"],
        (Mode::Check, _) => {
            let mut t = vec!["browser", "screenshot"];
            if !cfg.logs.is_empty() {
                t.push("logs");
            }
            // a bug's check also records its after
            if cfg.shot == Shot::Video && cfg.phase == Phase::After {
                t.extend(["record_start", "record_stop"]);
            }
            t
        }
        (Mode::Story, _) => vec!["browser", "take_start", "caption", "take_stop"],
    }
}

/// How many matches one `logs` call returns, the lines kept after each, and
/// the longest line it shows.
const LOG_MATCHES: usize = 40;
const LOG_AFTER: usize = 3;
const LOG_LINE: usize = 400;

/// The lines of `files` logged since each one's offset (all of it when the
/// file is now shorter: it was rotated or truncated) that contain `pattern`,
/// ignoring case, each with the few lines after it.
pub fn search_logs(files: &[LogFile], pattern: &str) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let needle = pattern.to_lowercase();
    let mut out = String::new();
    let mut matches = 0;
    for f in files {
        // never through a symlink: the log must be the file itself
        let opened = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(nix::libc::O_NOFOLLOW)
                .open(&f.path)
        };
        let Ok(mut file) = opened else {
            continue;
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        let from = if f.from <= len { f.from } else { 0 };
        let mut bytes = Vec::new();
        if file.seek(SeekFrom::Start(from)).is_err() || file.read_to_end(&mut bytes).is_err() {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.lines().collect();
        let mut shown_until = 0;
        let mut block = String::new();
        for (i, line) in lines.iter().enumerate() {
            if matches >= LOG_MATCHES {
                break;
            }
            if !line.to_lowercase().contains(&needle) {
                continue;
            }
            matches += 1;
            let end = (i + 1 + LOG_AFTER).min(lines.len());
            for l in &lines[i.max(shown_until)..end] {
                let cut: String = l.chars().take(LOG_LINE).collect();
                block.push_str(&cut);
                block.push('\n');
            }
            shown_until = shown_until.max(end);
        }
        if !block.is_empty() {
            let name = f
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            out.push_str(&format!("== {name} ==\n{block}"));
        }
    }
    out
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

fn tool_list(cfg: &McpConfig) -> serde_json::Value {
    let empty = serde_json::json!({"type": "object", "properties": {}});
    let tools: Vec<serde_json::Value> = tool_names(cfg)
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
            "screenshot" if cfg.mode == Mode::Check => serde_json::json!({
                "name": name,
                "description": "Save a screenshot of the page as the proof for one acceptance criterion.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"criterion": {"type": "integer", "description": "The criterion's number."}},
                    "required": ["criterion"]
                }
            }),
            "take_start" => serde_json::json!({"name": name, "description": "Start the take: recording begins with the story's opening card. Call it only once you are signed in, the starting data is set up and the page is where step 1 begins.", "inputSchema": empty}),
            "caption" => serde_json::json!({
                "name": name,
                "description": "Show one acceptance criterion's caption on the recording, at the moment it is visible on the page. Only the criteria this story shows.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"criterion": {"type": "integer", "description": "The criterion's number."}},
                    "required": ["criterion"]
                }
            }),
            "take_stop" => serde_json::json!({"name": name, "description": "End the take: the closing card, then the recording stops.", "inputSchema": empty}),
            "logs" => serde_json::json!({
                "name": name,
                "description": "Search the app's logs for a word or phrase (ignoring case). Returns only what was logged since you started, each match with the lines after it, and keeps those lines as the proof for the criterion.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "criterion": {"type": "integer", "description": "The criterion's number."},
                        "pattern": {"type": "string"}
                    },
                    "required": ["criterion", "pattern"]
                }
            }),
            _ => serde_json::json!({"name": name, "description": "Save a screenshot of the page as this capture's picture.", "inputSchema": empty}),
        })
        .collect();
    serde_json::json!({ "tools": tools })
}

/// A tool call's `criterion` number (1 to 999).
fn criterion(arguments: &serde_json::Value) -> Option<usize> {
    arguments
        .get("criterion")
        .and_then(|c| c.as_u64())
        .filter(|n| (1..=999).contains(n))
        .map(|n| n as usize)
}

/// The server's state: the config and whether the window has its size yet.
struct Server {
    cfg: McpConfig,
    agent_browser: PathBuf,
    sized: bool,
    /// A story take: started, and the criteria captioned so far.
    taking: bool,
    take_over: bool,
    captioned: Vec<usize>,
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

    /// Draws `text` in the overlay `id`, holds it, then takes it away.
    fn show(&mut self, id: &str, style: &str, text: &str, ms: u64) {
        self.run(&["eval".into(), overlay_script(id, style, text)]);
        std::thread::sleep(std::time::Duration::from_millis(ms));
        self.run(&["eval".into(), remove_script(id)]);
    }

    /// `take_start`, `caption` and `take_stop`: (is_error, text).
    fn take(&mut self, name: &str, arguments: &serde_json::Value) -> (bool, String) {
        let Some(t) = self.cfg.take.clone() else {
            return (true, "no take here".into());
        };
        let hold = |default: u64| t.hold_ms.unwrap_or(default);
        let save_captions = |captioned: &[usize]| {
            let _ = std::fs::write(
                t.dir.join("captions.json"),
                serde_json::to_string(captioned).unwrap_or_else(|_| "[]".into()),
            );
        };
        match name {
            "take_start" => {
                if self.taking || self.take_over {
                    return (true, "the take has already started".into());
                }
                let _ = std::fs::create_dir_all(&t.dir);
                let _ = std::fs::write(t.dir.join("steps.txt"), "");
                save_captions(&[]);
                let video = t.dir.join("video.mp4").display().to_string();
                let (ok, text) = self.run(&["record".into(), "start".into(), video]);
                if !ok {
                    return (true, format!("could not start recording: {text}"));
                }
                self.taking = true;
                self.show(
                    CARD_ID,
                    CARD_STYLE,
                    &format!("{}\n\n{}", t.title, t.persona),
                    hold(CARD_MS),
                );
                (
                    false,
                    "recording: follow the steps, and call caption as each criterion shows".into(),
                )
            }
            "caption" => {
                if !self.taking {
                    return (
                        true,
                        "call take_start first: captions belong to the take".into(),
                    );
                }
                let Some(n) = criterion(arguments) else {
                    return (true, "caption needs the criterion's number".into());
                };
                let Some((_, text)) = t.shows.iter().find(|(c, _)| *c == n) else {
                    return (true, format!("criterion {n} is not one this story shows"));
                };
                self.show(CAPTION_ID, CAPTION_STYLE, text, hold(CAPTION_MS));
                self.captioned.push(n);
                save_captions(&self.captioned);
                (false, format!("captioned criterion {n}"))
            }
            _ => {
                if !self.taking {
                    return (true, "no take is running".into());
                }
                let shown: Vec<String> = t
                    .shows
                    .iter()
                    .filter(|(c, _)| self.captioned.contains(c))
                    .map(|(c, text)| format!("{c}. {text}"))
                    .collect();
                self.show(
                    CARD_ID,
                    CARD_STYLE,
                    &format!("Shown\n\n{}", shown.join("\n")),
                    hold(CARD_MS),
                );
                let (ok, text) = self.run(&["record".into(), "stop".into()]);
                self.taking = false;
                self.take_over = true;
                if !ok {
                    return (true, format!("could not stop recording: {text}"));
                }
                (false, "the take is over".into())
            }
        }
    }

    /// One `tools/call`: (is_error, text).
    fn call(&mut self, name: &str, arguments: &serde_json::Value) -> (bool, String) {
        if !tool_names(&self.cfg).contains(&name) {
            return (true, format!("no tool `{name}` here"));
        }
        let out = self
            .cfg
            .evidence
            .join(out_name(self.cfg.phase, self.cfg.shot));
        let out = out.display().to_string();
        let (ok, text) = match name {
            "take_start" | "caption" | "take_stop" => return self.take(name, arguments),
            "record_start" => self.run(&["record".into(), "start".into(), out]),
            "record_stop" => self.run(&["record".into(), "stop".into()]),
            "screenshot" if self.cfg.mode == Mode::Check => {
                let Some(n) = criterion(arguments) else {
                    return (true, "screenshot needs the criterion's number".into());
                };
                let Some(dir) = self.cfg.check_dir.clone() else {
                    return (true, "no folder for proof".into());
                };
                let shot = dir.join(format!("{n}.png")).display().to_string();
                self.run(&["screenshot".into(), shot])
            }
            "screenshot" => self.run(&["screenshot".into(), out]),
            "logs" => {
                let (Some(n), Some(pattern)) = (
                    criterion(arguments),
                    arguments
                        .get("pattern")
                        .and_then(|p| p.as_str())
                        .filter(|p| !p.trim().is_empty()),
                ) else {
                    return (
                        true,
                        "logs needs the criterion's number and a pattern".into(),
                    );
                };
                let found = search_logs(&self.cfg.logs, pattern);
                if found.is_empty() {
                    return (
                        false,
                        format!("no lines contain `{pattern}` since the check started"),
                    );
                }
                if let Some(dir) = &self.cfg.check_dir {
                    use std::io::Write;
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(dir.join(format!("{n}.log.txt")))
                    {
                        let _ = write!(f, "{found}");
                    }
                }
                (true, found)
            }
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
                // a take keeps only its own steps; setup and rehearsal are not kept
                let steps = match (&self.cfg.take, self.cfg.mode) {
                    (Some(t), Mode::Story) => self.taking.then(|| t.dir.join("steps.txt")),
                    (_, Mode::Story) => None,
                    _ => Some(steps_path(&self.cfg.evidence, self.cfg.phase)),
                };
                if ok && let Some(steps) = steps {
                    use std::io::Write;
                    let line = step_line(command, &checked[1..], &self.cfg.base_url);
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(steps)
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
            "tools/list" => tool_list(&self.cfg),
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
        taking: false,
        take_over: false,
        captioned: Vec::new(),
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

    fn cfg(mode: Mode, phase: Phase, shot: Shot, logs: bool) -> McpConfig {
        McpConfig {
            base_url: BASE.into(),
            session: "s".into(),
            evidence: PathBuf::from("/e"),
            phase,
            shot,
            mode,
            check_dir: Some(PathBuf::from("/c")),
            logs: if logs {
                vec![LogFile {
                    path: PathBuf::from("/w/.logs/backend.log"),
                    from: 0,
                }]
            } else {
                Vec::new()
            },
            take: None,
        }
    }

    #[test]
    fn video_gets_recording_and_screenshot_gets_a_shot() {
        let capture = |shot| cfg(Mode::Capture, Phase::Before, shot, false);
        assert_eq!(
            tool_names(&capture(Shot::Video)),
            ["browser", "record_start", "record_stop"]
        );
        assert_eq!(
            tool_names(&capture(Shot::Screenshot)),
            ["browser", "screenshot"]
        );
    }

    #[test]
    fn a_check_gets_a_screenshot_logs_and_a_bugs_after_recording() {
        assert_eq!(
            tool_names(&cfg(Mode::Check, Phase::Before, Shot::Screenshot, true)),
            ["browser", "screenshot", "logs"]
        );
        assert_eq!(
            tool_names(&cfg(Mode::Check, Phase::Before, Shot::Screenshot, false)),
            ["browser", "screenshot"]
        );
        assert_eq!(
            tool_names(&cfg(Mode::Check, Phase::After, Shot::Video, true)),
            [
                "browser",
                "screenshot",
                "logs",
                "record_start",
                "record_stop"
            ]
        );
    }
}
