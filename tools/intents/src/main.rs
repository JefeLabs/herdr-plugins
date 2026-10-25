//! Modes: no arguments runs the TUI; `add [--popup] [title]` captures an intent;
//! `--version` prints the version.

use std::io::Write;
use std::process::ExitCode;

use herdr_intents::config::{Mode, parse_mode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result: anyhow::Result<()> = match parse_mode(&args) {
        Mode::Tui => Ok(()),
        Mode::Add { .. } => Ok(()),
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
