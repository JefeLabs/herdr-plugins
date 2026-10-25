//! herdr-toolbar: a right-hand activity bar of tools for Herdr.
//!
//! Modes: no arguments runs the TUI inside a Herdr pane; `toggle` opens or
//! closes the workspace's toolbar pane; `select <tool>` switches the running
//! toolbar to a tool by id or 1-based index.

use std::process::ExitCode;

use herdr_toolbar::app::{self, ConfigPaths};
use herdr_toolbar::config::Trust;
use herdr_toolbar::herdr::{self, Control, HerdrContext};
use herdr_toolbar::pty::SpawnEnv;

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Tui,
    Toggle,
    Select(String),
    Trust,
    Focus,
    Usage(String),
}

const USAGE: &str =
    "usage: herdr-toolbar [toggle | focus | select <tool-id|index> | trust | --version]";

fn parse_mode(args: &[String]) -> Mode {
    match args {
        [] => Mode::Tui,
        [a] if a == "toggle" => Mode::Toggle,
        [a] if a == "trust" => Mode::Trust,
        [a] if a == "focus" => Mode::Focus,
        [a, tool] if a == "select" => Mode::Select(tool.clone()),
        [a] if a == "--version" || a == "-V" => {
            Mode::Usage(format!("herdr-toolbar {}", env!("CARGO_PKG_VERSION")))
        }
        _ => Mode::Usage(USAGE.to_string()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match parse_mode(&args) {
        Mode::Tui => run_tui(),
        Mode::Toggle => run_toggle(),
        Mode::Select(tool) => run_select(&tool),
        Mode::Trust => run_trust(),
        Mode::Focus => HerdrContext::from_env().and_then(|ctx| herdr::focus(&ctx)),
        Mode::Usage(text) => {
            let is_version = text.starts_with("herdr-toolbar ");
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
            use std::io::Write;
            // never `eprintln!`: it panics when stderr is a dead pty
            let _ = writeln!(std::io::stderr(), "herdr-toolbar: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run_tui() -> anyhow::Result<()> {
    let ctx = HerdrContext::from_env()?;
    std::fs::create_dir_all(&ctx.state_dir)?;
    std::fs::create_dir_all(&ctx.config_dir)?;
    let global = ctx.config_dir.join("tools.toml");
    if !global.exists() {
        std::fs::write(&global, include_str!("../tools.example.toml"))?;
    }
    if let Some(pane) = &ctx.pane_id {
        std::fs::write(herdr::pane_file(&ctx), format!("{pane}\n"))?;
    }
    let control = Control::bind(&herdr::socket_path(&ctx))?;
    let paths = ConfigPaths {
        global,
        workspace_root: ctx.workspace_cwd.clone(),
        trust: Trust::new(herdr::trust_file(&ctx)),
    };
    let env = SpawnEnv {
        cwd: ctx.workspace_cwd.clone(),
        inherit: ctx.inherit.clone(),
        tool_id: String::new(),
    };
    let pane_file = herdr::pane_file(&ctx);
    let socket = herdr::socket_path(&ctx);
    // `run` exits the process itself, so cleanup goes through `on_exit`.
    app::run(
        paths,
        env,
        Some(Box::new(move || control.poll())),
        Some(Box::new(move || {
            let _ = std::fs::remove_file(pane_file);
            let _ = std::fs::remove_file(socket);
        })),
    )
}

fn run_toggle() -> anyhow::Result<()> {
    herdr::toggle(&HerdrContext::from_env()?)
}

fn run_select(tool: &str) -> anyhow::Result<()> {
    herdr::select(&HerdrContext::from_env()?, tool)
}

/// Allow this workspace's `.herdr-toolbar.toml` (current content) to be merged.
fn run_trust() -> anyhow::Result<()> {
    let ctx = HerdrContext::from_env()?;
    let file = ctx
        .workspace_cwd
        .join(herdr_toolbar::config::WORKSPACE_FILE);
    Trust::new(herdr::trust_file(&ctx)).allow(&file)?;
    println!("trusted {}", file.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mode_from_args() {
        assert_eq!(parse_mode(&[]), Mode::Tui);
        assert_eq!(parse_mode(&["toggle".into()]), Mode::Toggle);
        assert_eq!(
            parse_mode(&["select".into(), "db".into()]),
            Mode::Select("db".into())
        );
        assert_eq!(parse_mode(&["trust".into()]), Mode::Trust);
        assert_eq!(parse_mode(&["focus".into()]), Mode::Focus);
        assert!(matches!(parse_mode(&["bogus".into()]), Mode::Usage(_)));
    }
}
