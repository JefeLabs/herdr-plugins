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
    /// `unzip <zip> <member> <folder>`, for the installer.
    Unzip(String, String, String),
    /// `attach <relay> [<control> <tool> <prefix>]`: a pop-out's pane.
    Attach(String, Option<herdr_toolbar::relay::DockTo>),
    /// `wrap <control> <tool> <prefix> -- <command…>`: the pane of a
    /// pop-out with a `popout` command of its own.
    Wrap(herdr_toolbar::relay::DockTo, Vec<String>),
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
        // a pop-out's pane; not in the usage line
        [a, socket] if a == "attach" => Mode::Attach(socket.clone(), None),
        // ...that leaves with `Ctrl-] c` or `Ctrl-] d` (the toolbar's prefix, then c or d)
        [a, socket, control, tool, prefix] if a == "attach" => Mode::Attach(
            socket.clone(),
            Some(herdr_toolbar::relay::DockTo {
                control: control.into(),
                tool: tool.clone(),
                prefix: prefix.clone(),
            }),
        ),
        // a pop-out running its own command, behind the same leave keys
        [a, control, tool, prefix, dashes, command @ ..]
            if a == "wrap" && dashes == "--" && !command.is_empty() =>
        {
            Mode::Wrap(
                herdr_toolbar::relay::DockTo {
                    control: control.into(),
                    tool: tool.clone(),
                    prefix: prefix.clone(),
                },
                command.to_vec(),
            )
        }
        // the installer's fallback for .zip tools; not in the usage line
        [a, zip, member, folder] if a == "unzip" => {
            Mode::Unzip(zip.clone(), member.clone(), folder.clone())
        }
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
        Mode::Attach(socket, dock) => {
            herdr_toolbar::relay::attach(std::path::Path::new(&socket), dock)
        }
        Mode::Wrap(dock, command) => match herdr_toolbar::wrap::wrap(&dock, &command) {
            Ok(code) => return ExitCode::from(code),
            Err(e) => Err(e),
        },
        Mode::Trust => run_trust(),
        Mode::Focus => HerdrContext::from_env().and_then(|ctx| herdr::focus(&ctx)),
        Mode::Unzip(zip, member, folder) => herdr_toolbar::unzip::extract_member(
            std::path::Path::new(&zip),
            &member,
            std::path::Path::new(&folder),
        )
        .map(|_| ()),
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
        cell: None,
    };
    let pane_file = herdr::pane_file(&ctx);
    let socket = herdr::socket_path(&ctx);
    // `run` exits the process itself, so cleanup goes through `on_exit`.
    app::run(
        paths,
        env,
        Some(ctx),
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
        assert_eq!(
            parse_mode(&["unzip".into(), "a.zip".into(), "dir/x".into(), "out".into()]),
            Mode::Unzip("a.zip".into(), "dir/x".into(), "out".into())
        );
        assert_eq!(parse_mode(&["focus".into()]), Mode::Focus);
        assert_eq!(
            parse_mode(&["attach".into(), "r.sock".into()]),
            Mode::Attach("r.sock".into(), None)
        );
        assert_eq!(
            parse_mode(&[
                "attach".into(),
                "r.sock".into(),
                "c.sock".into(),
                "files".into(),
                "ctrl-]".into()
            ]),
            Mode::Attach(
                "r.sock".into(),
                Some(herdr_toolbar::relay::DockTo {
                    control: "c.sock".into(),
                    tool: "files".into(),
                    prefix: "ctrl-]".into(),
                })
            )
        );
        assert_eq!(
            parse_mode(
                &[
                    "wrap",
                    "c.sock",
                    "web",
                    "ctrl-]",
                    "--",
                    "terminal-browser",
                    "open"
                ]
                .map(String::from)
            ),
            Mode::Wrap(
                herdr_toolbar::relay::DockTo {
                    control: "c.sock".into(),
                    tool: "web".into(),
                    prefix: "ctrl-]".into(),
                },
                vec!["terminal-browser".into(), "open".into()]
            )
        );
        assert!(
            matches!(
                parse_mode(&["wrap", "c.sock", "web", "ctrl-]", "--"].map(String::from)),
                Mode::Usage(_)
            ),
            "a wrap with nothing to run"
        );
        assert!(matches!(parse_mode(&["bogus".into()]), Mode::Usage(_)));
    }
}
