//! Streaming conversation turns: spawn a provider CLI in print mode, stream its
//! events over a channel, cancel by killing its process group.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use std::os::unix::process::CommandExt;

use anyhow::Context;

use crate::config::ProviderSpec;
use crate::provider::{Event, Reply, parse_line, parse_reply, render_argv};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnMsg {
    Delta(String),
    ConversationId(String),
    Finished(Reply),
    Failed(String),
}

pub struct Turn {
    child: Arc<Mutex<Child>>,
    rx: Receiver<TurnMsg>,
    tx: Sender<TurnMsg>,
    pid: u32,
    cancelled: bool,
    /// Set by `cancel` so the reader thread can tell a user cancel from a crash.
    cancel_flag: Arc<std::sync::atomic::AtomicBool>,
}

/// End a provider that is done but still running: SIGTERM to its process
/// group, SIGKILL after half a second (agy ignores SIGTERM), then reap it.
fn end_group(child: &Arc<Mutex<Child>>) {
    let Ok(mut c) = child.lock() else {
        return;
    };
    let pgid = nix::unistd::Pid::from_raw(c.id() as i32);
    if let Ok(Some(_)) = c.try_wait() {
        return;
    }
    let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGTERM);
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = c.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGKILL);
    let _ = c.wait();
}

impl Turn {
    /// Start one turn. `extra_env` is added to the provider's environment
    /// (tests use it for the fake provider). Output is read on threads.
    pub fn start(
        spec: &ProviderSpec,
        prompt: &str,
        conv: Option<&str>,
        cwd: &Path,
        extra_env: &[(String, String)],
    ) -> anyhow::Result<Turn> {
        let outfile = cwd.join(".capabilities-last-message.txt");
        let argv = render_argv(spec, prompt, conv, &outfile);
        let (program, args) = argv.split_first().context("provider argv is empty")?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.process_group(0);
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting provider `{}`", spec.name))?;
        let pid = child.id();
        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;
        let (tx, rx) = channel();

        // stderr: shown as text so a failing CLI explains itself
        {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in lossy_lines(stderr) {
                    if tx.send(TurnMsg::Delta(format!("{line}\n"))).is_err() {
                        break;
                    }
                }
            });
        }

        let child = Arc::new(Mutex::new(child));
        let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let tx = tx.clone();
            let spec = spec.clone();
            let child = Arc::clone(&child);
            let cancel_flag = Arc::clone(&cancel_flag);
            std::thread::spawn(move || {
                let mut text = String::new();
                let mut sent_id: Option<String> = None;
                let mut error: Option<String> = None;
                let mut result_seen = false;
                for line in lossy_lines(stdout) {
                    match parse_line(&spec, &line) {
                        Event::Text(t) => {
                            text.push_str(&t);
                            if tx.send(TurnMsg::Delta(t)).is_err() {
                                return;
                            }
                        }
                        Event::ConversationId(id) => {
                            if sent_id.as_deref() != Some(id.as_str()) {
                                sent_id = Some(id.clone());
                                let _ = tx.send(TurnMsg::ConversationId(id));
                            }
                        }
                        Event::Error(e) => error = Some(e),
                        Event::Done => {
                            // the provider says the turn is over: its reply is
                            // complete, so stop reading. agy can stay running
                            // after its result (and keep stdout open), which
                            // would otherwise hold the turn forever.
                            result_seen = true;
                            break;
                        }
                        Event::Other => {}
                    }
                }
                if result_seen {
                    end_group(&child);
                    let _ = tx.send(TurnMsg::Finished(parse_reply(&text)));
                    return;
                }
                // stdout closed: the process is finishing. Reap it.
                let status = child.lock().ok().and_then(|mut c| c.wait().ok());
                let signal = status.and_then(|s| {
                    if s.code().is_none() {
                        std::os::unix::process::ExitStatusExt::signal(&s)
                    } else {
                        None
                    }
                });
                if let Some(sig) = signal {
                    let msg = if cancel_flag.load(std::sync::atomic::Ordering::Relaxed) {
                        "cancelled".to_string()
                    } else {
                        format!("provider exited by signal {sig} before replying")
                    };
                    let _ = tx.send(TurnMsg::Failed(msg));
                } else if text.trim().is_empty() {
                    let _ = tx.send(TurnMsg::Failed(
                        error.unwrap_or_else(|| "provider produced no reply".to_string()),
                    ));
                } else {
                    let _ = tx.send(TurnMsg::Finished(parse_reply(&text)));
                }
            });
        }

        Ok(Turn {
            child,
            rx,
            tx,
            pid,
            cancelled: false,
            cancel_flag,
        })
    }

    pub fn try_recv(&self) -> Option<TurnMsg> {
        self.rx.try_recv().ok()
    }

    pub fn pid(&self) -> Option<u32> {
        Some(self.pid)
    }

    /// Kill the provider's process group: SIGTERM, then SIGKILL after 500 ms.
    pub fn cancel(&mut self) {
        if self.cancelled {
            return;
        }
        self.cancelled = true;
        self.cancel_flag
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let pgid = nix::unistd::Pid::from_raw(self.pid as i32);
        let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGTERM);
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            if let Ok(mut c) = self.child.try_lock()
                && let Ok(Some(_)) = c.try_wait()
            {
                break;
            }
            if Instant::now() >= deadline {
                let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGKILL);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.tx.send(TurnMsg::Failed("cancelled".to_string()));
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        if !self.cancelled {
            let pgid = nix::unistd::Pid::from_raw(self.pid as i32);
            let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGTERM);
        }
    }
}

/// Lines decoded leniently: one invalid UTF-8 byte must not end the stream
/// (`lines()` would stop there and the child would die of SIGPIPE).
fn lossy_lines<R: Read + Send + 'static>(r: R) -> impl Iterator<Item = String> {
    let mut reader = BufReader::new(r);
    std::iter::from_fn(move || {
        let mut buf = Vec::new();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                while buf.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
                    buf.pop();
                }
                Some(String::from_utf8_lossy(&buf).into_owned())
            }
        }
    })
}

/// The next message of a conversation. With a resumable id the provider
/// remembers context, so only the new message goes. Without one (text
/// providers, a cancelled first turn) the seed and the transcript so far are
/// re-sent, as the spec's degraded mode requires.
/// Restated on every turn, so a conversation that began under an older
/// template still answers in the current structure.
/// What the refinement conversation may ask about: the happy path only.
pub const HAPPY_PATH_RULE: &str = "Open questions are only what you need to know to build the main flow: who, what, where, and what success looks like. Do not ask about edge cases or failure modes; those are handled after hand-off.";

pub const FORMAT_REMINDER: &str = "(Reply format: first line `capability: <who> can <what>`; then a fenced ```proposal block with the complete body as `## Description`, `## Preconditions` (`- [ ]` items, keep `[x]` on confirmed ones), `## Acceptance criteria` (`- [ ]` checks), `## Open questions` (`- ` items, keep answered ones with their `→` answer); then `questions:` with `- ` items or `questions: none`. Open questions are only what you need to know to build the main flow: who, what, where, and what success looks like. Do not ask about edge cases or failure modes; those are handled after hand-off. When the capability lists areas it affects, group `## Acceptance criteria` under `### <area path>` sub-headings, one per area (its own group first), keeping every criterion's wording and checkbox.)";

pub fn follow_up_prompt(
    seed: &str,
    transcript: &[(String, String)],
    message: &str,
    resumable: bool,
) -> String {
    if resumable {
        return format!("{}\n\n{FORMAT_REMINDER}\n", message.trim_end());
    }
    // the seed ends with the reminder too; keep only the last one, after the
    // new message
    let seed = seed.trim_end();
    let seed = seed.strip_suffix(FORMAT_REMINDER).unwrap_or(seed);
    let mut p = String::new();
    p.push_str(seed.trim_end());
    p.push_str("\n\nConversation so far:\n");
    for (role, text) in transcript {
        p.push_str(&format!("\n[{role}]\n{}\n", text.trim_end()));
    }
    p.push_str("\n[user]\n");
    p.push_str(message.trim_end());
    p.push_str("\n\n");
    p.push_str(FORMAT_REMINDER);
    p.push('\n');
    p
}

/// First message of a conversation: the kind's polish template, the intent,
/// and, when the chat was opened on a selection, a focus line quoting it.
pub fn seed_prompt(
    template: &str,
    intent_body: &str,
    selection: Option<&str>,
    kind: &str,
) -> String {
    let mut p = String::new();
    p.push_str(template.trim_end());
    p.push_str("\n\n");
    p.push_str(&format!("The intent (kind: {kind}) as it stands:\n\n"));
    p.push_str(intent_body.trim_end());
    p.push('\n');
    if let Some(sel) = selection.map(str::trim).filter(|s| !s.is_empty()) {
        p.push_str(
            "\nFocus on this passage, but still return the complete intent in the proposal:\n\n",
        );
        for line in sel.lines() {
            p.push_str("> ");
            p.push_str(line);
            p.push('\n');
        }
    }
    p.push('\n');
    p.push_str(FORMAT_REMINDER);
    p.push('\n');
    p
}
