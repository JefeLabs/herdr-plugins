//! Modes, directories, intents.toml, templates, built-in providers and handoffs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

pub const PLUGIN_ID: &str = "jefelabs.intents";
pub const KINDS: &[&str] = &["issue", "enhancement", "idea"];

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Tui,
    Add { title: Option<String>, popup: bool },
    Usage(String),
}

const USAGE: &str = "usage: herdr-intents [add [--popup] [title] | --version]";

pub fn parse_mode(args: &[String]) -> Mode {
    match args.first().map(String::as_str) {
        None => Mode::Tui,
        Some("--version") | Some("-V") => Mode::Usage(format!("herdr-intents {}", env!("CARGO_PKG_VERSION"))),
        Some("add") => {
            let mut popup = false;
            let mut title = Vec::new();
            for a in &args[1..] {
                if a == "--popup" {
                    popup = true;
                } else {
                    title.push(a.as_str());
                }
            }
            Mode::Add { title: if title.is_empty() { None } else { Some(title.join(" ")) }, popup }
        }
        Some(_) => Mode::Usage(USAGE.to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    pub config: PathBuf,
    pub root: PathBuf,
}

/// Config dir: `INTENTS_CONFIG_DIR`, else this plugin's `HERDR_PLUGIN_CONFIG_DIR`,
/// else `~/.config/herdr/plugins/config/jefelabs.intents`. Root from config or `~/intents`.
pub fn resolve_dirs(get: impl Fn(&str) -> Option<String>, home: &Path, cfg_root: Option<&str>) -> Dirs {
    let config = if let Some(c) = get("INTENTS_CONFIG_DIR") {
        PathBuf::from(c)
    } else if get("HERDR_PLUGIN_ID").as_deref() == Some(PLUGIN_ID) && get("HERDR_PLUGIN_CONFIG_DIR").is_some() {
        PathBuf::from(get("HERDR_PLUGIN_CONFIG_DIR").unwrap())
    } else {
        home.join(".config/herdr/plugins/config").join(PLUGIN_ID)
    };
    let root = match cfg_root {
        Some(r) => PathBuf::from(expand_home(r, home)),
        None => home.join("intents"),
    };
    Dirs { config, root }
}

fn expand_home(p: &str, home: &Path) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => home.join(rest).display().to_string(),
        None if p == "~" => home.display().to_string(),
        None => p.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase", tag = "output")]
pub enum Output {
    /// JSON lines: `text_path` is read from events whose `type` is in `text_types`;
    /// `id_path` from any event; the turn is done when `type == done_type` (and on EOF).
    Jsonl {
        text_path: String,
        #[serde(default)]
        text_types: Vec<String>,
        #[serde(default)]
        id_path: Option<String>,
        #[serde(default)]
        done_type: Option<String>,
    },
    /// Plain text: the whole stdout is the reply.
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProviderSpec {
    #[serde(default)]
    pub name: String,
    pub polish: Vec<String>,
    #[serde(default)]
    pub resume: Vec<String>,
    #[serde(flatten)]
    pub output: Output,
    #[serde(default = "default_send_kind")]
    pub send_kind: String,
}

fn default_send_kind() -> String {
    "claude".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Handoff {
    pub name: String,
    pub kinds: Vec<String>,
    #[serde(default)]
    pub provider: String,
    pub prompt: String,
}

impl Handoff {
    pub fn applies_to(&self, kind: &str) -> bool {
        self.kinds.iter().any(|k| k == "*" || k == kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub root: PathBuf,
    pub editor: String,
    pub send_provider: String,
    pub polish_provider: String,
    pub workbrokr: Option<bool>,
    pub providers: BTreeMap<String, ProviderSpec>,
    pub handoffs: Vec<Handoff>,
}

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    root: Option<String>,
    editor: Option<String>,
    send_provider: Option<String>,
    polish_provider: Option<String>,
    workbrokr: Option<bool>,
    #[serde(default)]
    providers: BTreeMap<String, ProviderSpec>,
    #[serde(default)]
    handoff: Vec<Handoff>,
}

const DEFAULT_FILES: &[(&str, &str)] = &[
    ("intents.toml", include_str!("../defaults/intents.toml")),
    ("templates/issue.md", include_str!("../defaults/issue.md")),
    ("templates/issue.polish.md", include_str!("../defaults/issue.polish.md")),
    ("templates/enhancement.md", include_str!("../defaults/enhancement.md")),
    ("templates/enhancement.polish.md", include_str!("../defaults/enhancement.polish.md")),
    ("templates/idea.md", include_str!("../defaults/idea.md")),
    ("templates/idea.polish.md", include_str!("../defaults/idea.polish.md")),
];

/// Write the default files that are missing (never overwrite), then load.
pub fn load_or_init(dirs: &Dirs) -> anyhow::Result<Config> {
    for (rel, text) in DEFAULT_FILES {
        let path = dirs.config.join(rel);
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        }
    }
    let path = dirs.config.join("intents.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let raw: RawConfig = toml::from_str(&text).with_context(|| path.display().to_string())?;

    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"));
    let root = match &raw.root {
        Some(r) if r != "~/intents" => PathBuf::from(expand_home(r, &home)),
        _ => dirs.root.clone(),
    };

    let mut providers = builtin_providers();
    for (name, mut spec) in raw.providers {
        spec.name = name.clone();
        providers.insert(name, spec);
    }
    let mut handoffs = builtin_handoffs();
    for h in raw.handoff {
        match handoffs.iter_mut().find(|x| x.name == h.name) {
            Some(slot) => *slot = h,
            None => handoffs.push(h),
        }
    }
    let send_provider = raw.send_provider.unwrap_or_else(|| "claude".to_string());
    for h in &mut handoffs {
        if h.provider.is_empty() {
            h.provider = send_provider.clone();
        }
    }

    Ok(Config {
        root,
        editor: raw.editor.unwrap_or_else(|| "$EDITOR".to_string()),
        send_provider,
        polish_provider: raw.polish_provider.unwrap_or_else(|| "agy".to_string()),
        workbrokr: raw.workbrokr,
        providers,
        handoffs,
    })
}

/// Body skeleton (`polish == false`) or polish prompt template for a kind.
pub fn template(dirs: &Dirs, kind: &str, polish: bool) -> anyhow::Result<String> {
    let file = if polish { format!("{kind}.polish.md") } else { format!("{kind}.md") };
    let path = dirs.config.join("templates").join(&file);
    std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
}

fn argv(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn jsonl(text_path: &str, text_types: &[&str], id_path: Option<&str>, done_type: Option<&str>) -> Output {
    Output::Jsonl {
        text_path: text_path.to_string(),
        text_types: text_types.iter().map(|s| s.to_string()).collect(),
        id_path: id_path.map(str::to_string),
        done_type: done_type.map(str::to_string),
    }
}

/// The four CLIs present on the owner's machine (2026-10-24). Event paths are
/// the builder's best reading of each stream and are corrected against a real
/// capture in the provider tests.
pub fn builtin_providers() -> BTreeMap<String, ProviderSpec> {
    let mut m = BTreeMap::new();
    m.insert("agy".to_string(), ProviderSpec {
        name: "agy".into(),
        polish: argv(&["agy", "-p={prompt}", "--output-format", "stream-json", "--print-timeout", "90s", "--disable-slash-commands"]),
        resume: argv(&["agy", "--conversation", "{conv}", "-p={prompt}", "--output-format", "stream-json", "--print-timeout", "90s", "--disable-slash-commands"]),
        output: jsonl("/content", &["text"], Some("/conversation_id"), Some("result")),
        send_kind: "agy".into(),
    });
    m.insert("claude".to_string(), ProviderSpec {
        name: "claude".into(),
        polish: argv(&["claude", "-p", "{prompt}", "--output-format", "stream-json", "--include-partial-messages", "--session-id", "{conv}"]),
        resume: argv(&["claude", "--resume", "{conv}", "-p", "{prompt}", "--output-format", "stream-json", "--include-partial-messages"]),
        output: jsonl("/event/delta/text", &["stream_event"], Some("/session_id"), Some("result")),
        send_kind: "claude".into(),
    });
    m.insert("codex".to_string(), ProviderSpec {
        name: "codex".into(),
        polish: argv(&["codex", "exec", "--json", "{prompt}"]),
        resume: argv(&["codex", "exec", "resume", "{conv}", "{prompt}", "--json"]),
        output: jsonl("/item/text", &["item.completed"], Some("/thread_id"), Some("turn.completed")),
        send_kind: "codex".into(),
    });
    m.insert("copilot".to_string(), ProviderSpec {
        name: "copilot".into(),
        polish: argv(&["copilot", "-p", "{prompt}", "--output-format", "json"]),
        resume: argv(&["copilot", "--session-id", "{conv}", "-p", "{prompt}", "--output-format", "json"]),
        output: jsonl("/content", &["assistant.message", "message"], Some("/session_id"), Some("result")),
        send_kind: "copilot".into(),
    });
    m
}

pub fn builtin_handoffs() -> Vec<Handoff> {
    let h = |name: &str, kinds: &[&str], prompt: &str| Handoff {
        name: name.into(),
        kinds: argv(kinds),
        provider: "claude".into(),
        prompt: prompt.into(),
    };
    vec![
        h("issue-report", &["issue"], "Use the issue-report skill on this finding. Do not file anything until you have confirmed the root cause with me.\n\n{intent}\n\nAttachments:\n{attachments}"),
        h("enhancement-spec", &["enhancement"], "Use the enhancement-spec skill to write this up as a GitHub issue. Show me the draft before posting.\n\n{intent}"),
        h("grill", &["*"], "/grill-me\n\n{intent}"),
        h("wayfinder", &["*"], "/wayfinder\n\nDestination:\n{intent}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn lookup<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn parse_mode_variants() {
        assert_eq!(parse_mode(&[]), Mode::Tui);
        assert_eq!(parse_mode(&["add".into()]), Mode::Add { title: None, popup: false });
        assert_eq!(
            parse_mode(&["add".into(), "--popup".into(), "Fix x".into()]),
            Mode::Add { title: Some("Fix x".into()), popup: true }
        );
        assert!(matches!(parse_mode(&["bogus".into()]), Mode::Usage(_)));
        assert!(matches!(parse_mode(&["--version".into()]), Mode::Usage(t) if t.starts_with("herdr-intents ")));
    }

    #[test]
    fn dirs_prefer_env_then_plugin_then_home() {
        let home = Path::new("/home/u");
        let d = resolve_dirs(lookup(&[("INTENTS_CONFIG_DIR", "/c/env")]), home, None);
        assert_eq!(d.config, Path::new("/c/env"));
        assert_eq!(d.root, Path::new("/home/u/intents"));

        let d = resolve_dirs(lookup(&[("HERDR_PLUGIN_ID", "jefelabs.intents"), ("HERDR_PLUGIN_CONFIG_DIR", "/c/plugin")]), home, None);
        assert_eq!(d.config, Path::new("/c/plugin"));

        let d = resolve_dirs(lookup(&[("HERDR_PLUGIN_ID", "jefelabs.toolbar"), ("HERDR_PLUGIN_CONFIG_DIR", "/c/toolbar")]), home, None);
        assert_eq!(d.config, Path::new("/home/u/.config/herdr/plugins/config/jefelabs.intents"), "another plugin's dir is not ours");

        let d = resolve_dirs(lookup(&[]), home, Some("~/notes/intents"));
        assert_eq!(d.root, Path::new("/home/u/notes/intents"));
    }

    #[test]
    fn first_run_writes_defaults_and_never_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs { config: tmp.path().join("cfg"), root: tmp.path().join("root") };
        let cfg = load_or_init(&dirs).unwrap();
        assert!(dirs.config.join("intents.toml").is_file());
        for k in ["issue", "enhancement", "idea"] {
            assert!(dirs.config.join("templates").join(format!("{k}.md")).is_file(), "{k}.md");
            assert!(dirs.config.join("templates").join(format!("{k}.polish.md")).is_file(), "{k}.polish.md");
        }
        assert_eq!(cfg.root, dirs.root);
        fs::write(dirs.config.join("templates/issue.md"), "## Mine\n").unwrap();
        load_or_init(&dirs).unwrap();
        assert_eq!(template(&dirs, "issue", false).unwrap(), "## Mine\n");
        assert!(template(&dirs, "issue", true).unwrap().contains("```proposal"));
    }

    #[test]
    fn builtin_providers_and_handoffs_present() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs { config: tmp.path().join("cfg"), root: tmp.path().join("root") };
        let cfg = load_or_init(&dirs).unwrap();
        assert_eq!(cfg.polish_provider, "agy");
        for p in ["agy", "claude", "codex", "copilot"] {
            assert!(cfg.providers.contains_key(p), "provider {p}");
        }
        let agy = &cfg.providers["agy"];
        assert_eq!(agy.polish, ["agy", "-p={prompt}", "--output-format", "stream-json", "--print-timeout", "90s", "--disable-slash-commands"]);
        assert!(agy.resume.iter().any(|a| a == "--conversation"));
        let claude = &cfg.providers["claude"];
        assert!(claude.polish.contains(&"--session-id".to_string()) && claude.polish.contains(&"{conv}".to_string()));
        assert_eq!(cfg.providers["codex"].polish[..3], ["codex", "exec", "--json"]);
        assert_eq!(cfg.providers["copilot"].polish[..2], ["copilot", "-p"]);
        for p in cfg.providers.values() {
            assert!(!p.polish.iter().chain(p.resume.iter()).any(|a| a.contains("allow-all") || a.contains("dangerously")), "{} grants tools", p.name);
        }
        let names: Vec<&str> = cfg.handoffs.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["issue-report", "enhancement-spec", "grill", "wayfinder"]);
        assert!(cfg.handoffs.iter().all(|h| h.provider == "claude"));
        assert_eq!(cfg.handoffs[0].kinds, ["issue"]);
        assert_eq!(cfg.handoffs[2].kinds, ["*"]);
    }

    #[test]
    fn custom_provider_from_toml_merges() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs { config: tmp.path().join("cfg"), root: tmp.path().join("root") };
        load_or_init(&dirs).unwrap();
        fs::write(
            dirs.config.join("intents.toml"),
            "polish_provider = \"mine\"\n[providers.mine]\npolish = [\"my-cli\", \"--prompt\", \"{prompt}\"]\noutput = \"text\"\nsend_kind = \"claude\"\n",
        )
        .unwrap();
        let cfg = load_or_init(&dirs).unwrap();
        assert_eq!(cfg.polish_provider, "mine");
        assert_eq!(cfg.providers["mine"].polish, ["my-cli", "--prompt", "{prompt}"]);
        assert!(matches!(cfg.providers["mine"].output, Output::Text));
        assert!(cfg.providers.contains_key("agy"), "built-ins still present");
    }
}
