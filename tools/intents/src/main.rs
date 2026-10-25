//! Modes: no arguments runs the TUI; `add [--popup] [title]` captures an intent;
//! `--version` prints the version.

use std::io::Write;
use std::process::ExitCode;

use herdr_intents::app;
use herdr_intents::config::{Mode, load_or_init, parse_mode, resolve_dirs};

/// Resolve dirs (env, then config's root), load config, run.
fn with_config(
    f: impl FnOnce(herdr_intents::config::Config, herdr_intents::config::Dirs) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let home = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/"));
    let dirs = resolve_dirs(|k| std::env::var(k).ok(), &home, None);
    let cfg = load_or_init(&dirs)?;
    let dirs = herdr_intents::config::Dirs {
        config: dirs.config,
        root: cfg.root.clone(),
    };
    f(cfg, dirs)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result: anyhow::Result<()> = match parse_mode(&args) {
        Mode::Tui => with_config(|cfg, dirs| app::run(cfg, dirs)),
        Mode::Add { title, popup } => with_config(|cfg, dirs| app::add(cfg, dirs, title, popup)),
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
