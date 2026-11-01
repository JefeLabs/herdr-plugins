//! Modes: no arguments runs the TUI; `add [--popup] [title]` captures an intent;
//! `--version` prints the version.

use std::io::Write;
use std::process::ExitCode;

use herdr_intents::app;
use herdr_intents::config::{Mode, load_or_init, parse_mode, resolve_dirs};
use herdr_intents::scope;

/// Resolve dirs (env, then config's root), load config, run.
fn with_config(
    f: impl FnOnce(herdr_intents::config::Config, herdr_intents::config::Dirs) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let home = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/"));
    let dirs = resolve_dirs(|k| std::env::var(k).ok(), &home, None);
    // the plugin used to be jefelabs.capabilities: bring its config along once
    if let Err(e) = herdr_intents::config::migrate_old_config(&dirs.config) {
        let _ = writeln!(
            std::io::stderr(),
            "herdr-intents: could not carry over the old capabilities config: {e:#}"
        );
    }
    let mut cfg = load_or_init(&dirs)?;
    // intents moved from ~/intents to ~/.helmsmith/intents/spaces: once
    let settled = herdr_intents::storage::settle_root(&home, &cfg.root);
    cfg.root = settled.root;
    let dirs = herdr_intents::config::Dirs {
        config: dirs.config,
        root: cfg.root.clone(),
    };
    // a space is named after its meta repo: one still under another name
    // moves there once, unless a run is going in it
    let moved = herdr_intents::space_move::settle_spaces(&dirs, |space, meta| {
        herdr_intents::capture::space_busy(&dirs.root, space, meta)
    });
    let mut notices: Vec<String> = settled.notice.into_iter().chain(moved).collect();
    // one Herdr workspace sees one top-level group: its meta repo's space,
    // else the one named like it
    let herdr = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into());
    let mut unnamed = None;
    cfg.scope = scope::resolve(
        |k| std::env::var(k).ok(),
        |id| scope::herdr_label(std::path::Path::new(&herdr), id),
        || {
            let repo = herdr_intents::send::capture_repo()?;
            match herdr_intents::space::repo_space(&dirs, &repo)? {
                Ok(space) => Some(space),
                Err(why) => {
                    unnamed = Some(why);
                    None
                }
            }
        },
    )
    // a name the space has left (a workspace, INTENTS_SCOPE) still finds it
    .map(|s| herdr_intents::space::current_name(&dirs, &s));
    notices.extend(unnamed);
    cfg.notice = (!notices.is_empty()).then(|| notices.join("; "));
    // a space whose storage is not built yet shows no intents and takes none
    cfg.storage_block = cfg
        .scope
        .as_deref()
        .and_then(|s| herdr_intents::space::storage_block(&dirs, s));
    f(cfg, dirs)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result: anyhow::Result<()> = match parse_mode(&args) {
        Mode::Tui => with_config(app::run),
        Mode::Add { title, popup } => {
            let piped =
                (!popup && !std::io::IsTerminal::is_terminal(&std::io::stdin())).then(|| {
                    let mut input = String::new();
                    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
                    input
                });
            with_config(|cfg, dirs| app::add(cfg, dirs, title, popup, piped))
        }
        Mode::CaptureMcp(config) => herdr_intents::capture_mcp::serve(&config),
        Mode::OpenSpace { first } => {
            // Herdr's workspace.created hook, or (--first) its startup hook;
            // the line lands in Herdr's plugin log
            use herdr_intents::open_space;
            let get = |k: &str| std::env::var(k).ok();
            let bin = get("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
            let herdr = open_space::cli(bin.into());
            let id = if first {
                herdr(&["workspace", "list"])
                    .ok()
                    .and_then(|list| open_space::first_workspace(&list))
            } else {
                let event = get("HERDR_PLUGIN_EVENT_JSON");
                open_space::event_workspace(event.as_deref(), get("HERDR_WORKSPACE_ID").as_deref())
            };
            let home = get("HOME").map_or_else(|| "/".into(), std::path::PathBuf::from);
            let dirs = resolve_dirs(get, &home, None);
            // a workspace named with a name the space has left is the space's
            let also_called = |meta: &std::path::Path, space: &str| {
                herdr_intents::space::other_names(&dirs, meta, space)
            };
            let line = match id {
                Some(id) => open_space::describe(&id, &open_space::open(&id, also_called, herdr)),
                None if first => "open-space: the session has no workspace".to_string(),
                None => "open-space: the event names no workspace".to_string(),
            };
            let _ = writeln!(std::io::stdout(), "{line}");
            Ok(())
        }
        Mode::Context => {
            // the toolbar asks: which space, its meta repo, and the database
            // of the agent whose chat is in the main pane (TOOLBAR_AGENT_CWD)
            use herdr_intents::{context, space};
            let get = |k: &str| std::env::var(k).ok();
            let home = get("HOME").map_or_else(|| "/".into(), std::path::PathBuf::from);
            let dirs = resolve_dirs(get, &home, None);
            let herdr = get("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
            let here = herdr_intents::send::capture_repo();
            let found = scope::resolve(
                get,
                |id| scope::herdr_label(std::path::Path::new(&herdr), id),
                || space::repo_space(&dirs, here.as_deref()?)?.ok(),
            )
            .map(|s| space::current_name(&dirs, &s));
            let agent = get("TOOLBAR_AGENT_CWD")
                .filter(|a| !a.trim().is_empty())
                .map(std::path::PathBuf::from);
            let ctx = context::gather(
                &dirs,
                found.as_deref(),
                here.as_deref(),
                agent.as_deref(),
                context::DB_TIMEOUT,
            );
            let _ = writeln!(std::io::stdout(), "{}", serde_json::Value::Object(ctx));
            Ok(())
        }
        Mode::Trust => {
            // the owner says the meta repo's db: command may run on its own
            let get = |k: &str| std::env::var(k).ok();
            let home = get("HOME").map_or_else(|| "/".into(), std::path::PathBuf::from);
            let dirs = resolve_dirs(get, &home, None);
            match herdr_intents::send::capture_repo() {
                Some(repo) if herdr_intents::space::is_meta(&repo) => {
                    match herdr_intents::context::trust_db(&dirs, &repo) {
                        Ok(cmd) => {
                            let _ = writeln!(
                                std::io::stdout(),
                                "trusted the db: command of {}:\n  {cmd}",
                                repo.display()
                            );
                            Ok(())
                        }
                        Err(why) => Err(anyhow::anyhow!(why)),
                    }
                }
                Some(repo) => Err(anyhow::anyhow!("{} is not a meta repo", repo.display())),
                None => Err(anyhow::anyhow!("run it inside the meta repo")),
            }
        }
        Mode::CaptureRun(order) => herdr_intents::capture::run_order(&order),
        Mode::Usage(text) => {
            let is_version = text.starts_with("herdr-intents ");
            println!("{text}");
            return if is_version {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            };
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // never eprintln!: it panics when stderr is a dead pty
            let _ = writeln!(std::io::stderr(), "herdr-intents: {e:#}");
            ExitCode::FAILURE
        }
    }
}
