//! What a toolbar in a space needs to know, for `herdr-intents context`: the
//! space, its meta repo, and the database of the active agent (the one whose
//! chat is in the main pane). The space's run guide says how to find that
//! database: `db:`, a command run at the top of the agent's worktree that
//! prints its address. The toolbar never learns how a project runs.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::config::Dirs;

/// How long the run guide's `db:` command may take.
pub const DB_TIMEOUT: Duration = Duration::from_secs(5);

/// The top of the git worktree `dir` is in; `dir` itself outside git.
fn worktree_top(dir: &Path) -> PathBuf {
    std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .unwrap_or_else(|| dir.to_path_buf())
}

/// Runs the run guide's `db:` command `cmd` (through `sh -c`, at the top of
/// the agent's worktree `agent`, with `INTENTS_META` set to the meta repo)
/// and returns the first line it prints; `Err` says why there is none.
pub fn agent_db(cmd: &str, agent: &Path, meta: &Path, timeout: Duration) -> Result<String, String> {
    use std::os::unix::process::CommandExt;
    let mut child = std::process::Command::new("sh")
        .args(["-c", cmd])
        .current_dir(worktree_top(agent))
        .env("INTENTS_META", meta)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // its own group, so giving up ends whatever it started too
        .process_group(0)
        .spawn()
        .map_err(|e| format!("cannot run the run guide's db command: {e}"))?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_string(&mut text);
            }
            text
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let group = nix::unistd::Pid::from_raw(-(child.id() as i32));
                let _ = nix::sys::signal::kill(group, nix::sys::signal::Signal::SIGKILL);
                let _ = child.wait();
                return Err(format!(
                    "the run guide's db command took longer than {timeout:?}"
                ));
            }
        }
    };
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    if !status.success() {
        let why = err.lines().rev().map(str::trim).find(|l| !l.is_empty());
        return Err(match why {
            Some(line) => line.to_string(),
            None => format!("the run guide's db command failed ({status})"),
        });
    }
    out.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "the run guide's db command printed nothing".to_string())
}

/// The context of a workspace in space `space` whose folder is `here`, with
/// active agent `agent`: `space`, `meta`, `agent` (the top of its worktree)
/// and `db`, each present only when known; `meta_error` and `db_error` say
/// why one is not.
pub fn gather(
    dirs: &Dirs,
    space: Option<&str>,
    here: Option<&Path>,
    agent: Option<&Path>,
    timeout: Duration,
) -> Map<String, Value> {
    let mut out = Map::new();
    let mut put = |k: &str, v: String| {
        out.insert(k.to_string(), Value::String(v));
    };
    let agent = agent.map(worktree_top);
    if let Some(a) = &agent {
        put("agent", a.display().to_string());
    }
    let meta = match space {
        None => Err("this workspace is not a space".to_string()),
        Some(space) => {
            put("space", space.to_string());
            // the workspace's own folder, when it is this space's meta repo
            let own = here.filter(|h| {
                crate::space::repo_space(dirs, h)
                    .and_then(Result::ok)
                    .is_some_and(|s| s.eq_ignore_ascii_case(space))
            });
            own.map(Path::to_path_buf)
                .or_else(|| crate::space::meta_for(dirs, space))
                .ok_or_else(|| {
                    format!("no meta repo known for space {space}: open a workspace on it once")
                })
        }
    };
    let db = match (&meta, &agent) {
        (Err(_), _) => Err("no meta repo, so no run guide".to_string()),
        (Ok(_), None) => Err("no active agent".to_string()),
        (Ok(meta), Some(agent)) => match crate::capture::read_guide(meta) {
            Err(e) => Err(format!("{e:#}")),
            Ok(g) => match g.db {
                None => Err(format!(
                    "the run guide in {} has no db: command",
                    meta.join(".helmsmith").display()
                )),
                // a repo's own command runs only once its owner trusts it
                Some(cmd) if !is_trusted(dirs, meta, &cmd) => Err(format!(
                    "the run guide's db: command is not trusted yet: run `herdr-intents trust` in {}",
                    meta.display()
                )),
                Some(cmd) => agent_db(&cmd, agent, meta, timeout),
            },
        },
    };
    match meta {
        Ok(m) => put("meta", m.display().to_string()),
        Err(e) => put("meta_error", e),
    }
    match db {
        Ok(d) => put("db", d),
        Err(e) => put("db_error", e),
    }
    out
}

/// Where this person keeps the `db:` commands they trust: the exact
/// command, by meta repo.
fn trust_file(dirs: &Dirs) -> PathBuf {
    dirs.config.join("trusted-db.json")
}

fn trusted(dirs: &Dirs) -> std::collections::BTreeMap<String, String> {
    std::fs::read(trust_file(dirs))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn trust_key(meta: &Path) -> String {
    std::fs::canonicalize(meta)
        .unwrap_or_else(|_| crate::space::plain(meta))
        .display()
        .to_string()
}

/// Whether `cmd` is the `db:` command trusted for meta repo `meta`.
fn is_trusted(dirs: &Dirs, meta: &Path, cmd: &str) -> bool {
    trusted(dirs).get(&trust_key(meta)).map(String::as_str) == Some(cmd)
}

/// Trusts the `db:` command of the run guide in meta repo `meta` as it is
/// now (`herdr-intents trust`), and returns it. A repo's run guide is its
/// author's: the toolbar's context runs a `db:` command on its own, so it
/// runs one only once its owner has trusted that exact command; changing
/// it needs trusting again.
pub fn trust_db(dirs: &Dirs, meta: &Path) -> Result<String, String> {
    let guide = crate::capture::read_guide(meta).map_err(|e| format!("{e:#}"))?;
    let cmd = guide.db.ok_or_else(|| {
        format!(
            "the run guide in {} has no db: command",
            meta.join(".helmsmith").display()
        )
    })?;
    let mut all = trusted(dirs);
    all.insert(trust_key(meta), cmd.clone());
    let file = trust_file(dirs);
    let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dirs.config)
        .and_then(|()| std::fs::write(&file, text + "\n"))
        .map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(cmd)
}
