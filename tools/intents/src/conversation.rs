//! Streaming conversation turns: spawn a provider CLI in print mode, stream its
//! events over a channel, cancel by killing its process group.

use std::io::{BufRead, BufReader};
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
        let outfile = cwd.join(".intents-last-message.txt");
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
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if tx.send(TurnMsg::Delta(format!("{line}\n"))).is_err() {
                        break;
                    }
                }
            });
        }

        let child = Arc::new(Mutex::new(child));
        {
            let tx = tx.clone();
            let spec = spec.clone();
            let child = Arc::clone(&child);
            std::thread::spawn(move || {
                let mut text = String::new();
                let mut sent_id: Option<String> = None;
                let mut error: Option<String> = None;
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
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
                        Event::Done | Event::Other => {}
                    }
                }
                // stdout closed: the process is finishing. Reap it.
                let status = child.lock().ok().and_then(|mut c| c.wait().ok());
                let killed = status
                    .map(|s| !s.success() && s.code().is_none())
                    .unwrap_or(false);
                if killed {
                    let _ = tx.send(TurnMsg::Failed("cancelled".to_string()));
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
    p
}
