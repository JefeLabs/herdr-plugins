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
    cfg.notice = settled.notice;
    // one Herdr workspace sees one top-level group: its meta repo's space,
    // else the one named like it
    let herdr = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into());
    let mut unreadable = None;
    cfg.scope = scope::resolve(
        |k| std::env::var(k).ok(),
        |id| scope::herdr_label(std::path::Path::new(&herdr), id),
        || {
            let repo = herdr_intents::send::capture_repo()?;
            match herdr_intents::space::intents_space(&repo)? {
                Ok(space) => Some(space),
                Err(why) => {
                    unreadable = Some(why);
                    None
                }
            }
        },
    );
    if cfg.notice.is_none() {
        cfg.notice = unreadable;
    }
    // a space whose storage is not built yet shows no intents and takes none
    cfg.storage_block = cfg
        .scope
        .as_deref()
        .and_then(|s| herdr_intents::space::storage_block(&dirs, s));
    let dirs = herdr_intents::config::Dirs {
        config: dirs.config,
        root: cfg.root.clone(),
    };
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
            let line = match id {
                Some(id) => open_space::describe(&id, &open_space::open(&id, herdr)),
                None if first => "open-space: the session has no workspace".to_string(),
                None => "open-space: the event names no workspace".to_string(),
            };
            let _ = writeln!(std::io::stdout(), "{line}");
            Ok(())
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
