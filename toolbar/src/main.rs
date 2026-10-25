//! herdr-toolbar: a right-hand activity bar of tools for Herdr.
//!
//! Modes: no arguments runs the TUI inside a Herdr pane; `toggle` opens or
//! closes the workspace's toolbar pane; `select <tool>` switches the running
//! toolbar to a tool by id or 1-based index.

use std::process::ExitCode;

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Tui,
    Toggle,
    Select(String),
    Usage(String),
}

const USAGE: &str = "usage: herdr-toolbar [toggle | select <tool-id|index> | --version]";

fn parse_mode(args: &[String]) -> Mode {
    match args {
        [] => Mode::Tui,
        [a] if a == "toggle" => Mode::Toggle,
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
        Mode::Usage(text) => {
            let is_version = text.starts_with("herdr-toolbar ");
            println!("{text}");
            return if is_version { ExitCode::SUCCESS } else { ExitCode::from(2) };
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("herdr-toolbar: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run_tui() -> anyhow::Result<()> {
    Ok(())
}

fn run_toggle() -> anyhow::Result<()> {
    Ok(())
}

fn run_select(_tool: &str) -> anyhow::Result<()> {
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
        assert!(matches!(parse_mode(&["bogus".into()]), Mode::Usage(_)));
    }
}
