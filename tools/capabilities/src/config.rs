//! Modes, directories, intents.toml, templates, built-in providers and handoffs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

pub const PLUGIN_ID: &str = "jefelabs.capabilities";
pub const KINDS: &[&str] = &["issue", "enhancement", "idea"];

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Tui,
    Add { title: Option<String>, popup: bool },
    Usage(String),
}

const USAGE: &str = "usage: herdr-capabilities [add [--popup] [title] | --version]";

pub fn parse_mode(args: &[String]) -> Mode {
    match args.first().map(String::as_str) {
        None => Mode::Tui,
        Some("--version") | Some("-V") => {
            Mode::Usage(format!("herdr-capabilities {}", env!("CARGO_PKG_VERSION")))
        }
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
            Mode::Add {
                title: if title.is_empty() {
                    None
                } else {
                    Some(title.join(" "))
                },
                popup,
            }
        }
        Some(_) => Mode::Usage(USAGE.to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    pub config: PathBuf,
    pub root: PathBuf,
}

/// Config dir: `CAPABILITIES_CONFIG_DIR`, else this plugin's `HERDR_PLUGIN_CONFIG_DIR`,
/// else `~/.config/herdr/plugins/config/jefelabs.capabilities`. Root from config or `~/capabilities`.
pub fn resolve_dirs(
    get: impl Fn(&str) -> Option<String>,
    home: &Path,
    cfg_root: Option<&str>,
) -> Dirs {
    let config = if let Some(c) = get("CAPABILITIES_CONFIG_DIR") {
        PathBuf::from(c)
    } else if get("HERDR_PLUGIN_ID").as_deref() == Some(PLUGIN_ID)
        && get("HERDR_PLUGIN_CONFIG_DIR").is_some()
    {
        PathBuf::from(get("HERDR_PLUGIN_CONFIG_DIR").unwrap())
    } else {
        home.join(".config/herdr/plugins/config").join(PLUGIN_ID)
    };
    let root = match (get("CAPABILITIES_ROOT"), cfg_root) {
        (Some(env_root), _) => PathBuf::from(expand_home(&env_root, home)),
        (None, Some(r)) => PathBuf::from(expand_home(r, home)),
        (None, None) => home.join("capabilities"),
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

/// `(JSON pointer, expected string value)` guard on an event.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct When {
    pub path: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase", tag = "output")]
pub enum Output {
    /// JSON lines. The event's kind is the string at `type_key` (default `type`).
    /// Text deltas are read from `text_path` on events whose kind is in
    /// `text_types` and that satisfy `text_when` (if set). A conversation id is
    /// read from the first `id_paths` entry that exists on any event. The turn is
    /// done when the kind equals `done_type` (and always on EOF). Events that
    /// repeat the whole reply (a final `result`) must not be in `text_types`.
    Jsonl {
        #[serde(default = "default_type_key")]
        type_key: String,
        text_path: String,
        #[serde(default)]
        text_types: Vec<String>,
        #[serde(default)]
        text_when: Option<When>,
        #[serde(default)]
        id_paths: Vec<String>,
        #[serde(default)]
        done_type: Option<String>,
    },
    /// Plain text: the whole stdout is the reply.
    Text,
}

fn default_type_key() -> String {
    "type".to_string()
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
    /// Options inserted right after the program on every call, e.g.
    /// `["--model", "gemini-4-argon-medium"]`. The only field a user needs to
    /// set to tune a built-in provider.
    #[serde(default)]
    pub extra: Vec<String>,
    /// Models the picker offers for this provider; empty skips the model step.
    #[serde(default)]
    pub models: Vec<String>,
    /// Flag that selects a model, e.g. `--model`.
    #[serde(default = "default_model_flag")]
    pub model_flag: String,
}

fn default_model_flag() -> String {
    "--model".to_string()
}

/// The provider and model used last time; preselected in the picker.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastUsed {
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    /// `tree` or `list`: the layout the browser opens in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    /// The list's sort order (`Updated`, `Progress`, `Captured`, `Title`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    /// The list's filter (`All`, `In progress`, `Completed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
}

fn last_used_path(dirs: &Dirs) -> PathBuf {
    dirs.config.join("last-used.toml")
}

/// Last choice if recorded, else the config's polish provider with no model.
pub fn last_used(dirs: &Dirs, cfg: &Config) -> LastUsed {
    let saved = std::fs::read_to_string(last_used_path(dirs))
        .ok()
        .and_then(|t| toml::from_str::<LastUsed>(&t).ok())
        .unwrap_or_default();
    if cfg.providers.contains_key(&saved.provider) {
        return saved;
    }
    // an unknown (or never chosen) provider falls back, the view settings stay
    LastUsed {
        provider: cfg.polish_provider.clone(),
        model: None,
        ..saved
    }
}

/// Record the browser's layout and sort order without touching the provider
/// and model stored beside them.
pub fn save_view(dirs: &Dirs, layout: &str, sort: &str) -> anyhow::Result<()> {
    save_view_with(dirs, layout, sort, None)
}

/// `save_view`, also recording the list filter when given.
pub fn save_view_with(
    dirs: &Dirs,
    layout: &str,
    sort: &str,
    filter: Option<&str>,
) -> anyhow::Result<()> {
    let mut last = std::fs::read_to_string(last_used_path(dirs))
        .ok()
        .and_then(|t| toml::from_str::<LastUsed>(&t).ok())
        .unwrap_or_default();
    last.layout = Some(layout.to_string());
    last.sort = Some(sort.to_string());
    if let Some(f) = filter {
        last.filter = Some(f.to_string());
    }
    save_last_used(dirs, &last)
}

pub fn save_last_used(dirs: &Dirs, last: &LastUsed) -> anyhow::Result<()> {
    std::fs::create_dir_all(&dirs.config)?;
    std::fs::write(last_used_path(dirs), toml::to_string(last)?)?;
    Ok(())
}

/// A `[providers.<name>]` table that only tunes a built-in: any field absent
/// keeps the built-in value.
#[derive(Debug, Default, Deserialize)]
struct ProviderOverride {
    polish: Option<Vec<String>>,
    resume: Option<Vec<String>>,
    #[serde(flatten)]
    output: Option<Output>,
    send_kind: Option<String>,
    #[serde(default)]
    extra: Vec<String>,
    /// Shorthand for `extra = ["--model", "<model>"]`.
    model: Option<String>,
    models: Option<Vec<String>>,
    model_flag: Option<String>,
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
    providers: BTreeMap<String, ProviderOverride>,
    #[serde(default)]
    handoff: Vec<Handoff>,
}

const DEFAULT_FILES: &[(&str, &str)] = &[
    (
        "capabilities.toml",
        include_str!("../defaults/capabilities.toml"),
    ),
    ("templates/issue.md", include_str!("../defaults/issue.md")),
    (
        "templates/issue.polish.md",
        include_str!("../defaults/issue.polish.md"),
    ),
    (
        "templates/enhancement.md",
        include_str!("../defaults/enhancement.md"),
    ),
    (
        "templates/enhancement.polish.md",
        include_str!("../defaults/enhancement.polish.md"),
    ),
    ("templates/idea.md", include_str!("../defaults/idea.md")),
    (
        "templates/idea.polish.md",
        include_str!("../defaults/idea.polish.md"),
    ),
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
    let path = dirs.config.join("capabilities.toml");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let raw: RawConfig = toml::from_str(&text).with_context(|| path.display().to_string())?;

    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"));
    let root = match &raw.root {
        Some(r) if r != "~/capabilities" => PathBuf::from(expand_home(r, &home)),
        _ => dirs.root.clone(),
    };

    let mut providers = builtin_providers();
    for (name, o) in raw.providers {
        let mut extra = o.extra.clone();
        if let Some(m) = &o.model {
            extra.extend(["--model".to_string(), m.clone()]);
        }
        match providers.get_mut(&name) {
            Some(spec) => {
                if let Some(p) = o.polish {
                    spec.polish = p;
                }
                if let Some(r) = o.resume {
                    spec.resume = r;
                }
                if let Some(out) = o.output {
                    spec.output = out;
                }
                if let Some(k) = o.send_kind {
                    spec.send_kind = k;
                }
                if let Some(m) = o.models {
                    spec.models = m;
                }
                if let Some(f) = o.model_flag {
                    spec.model_flag = f;
                }
                spec.extra = extra;
            }
            None => {
                let Some(polish) = o.polish else {
                    anyhow::bail!("provider `{name}`: a custom provider needs `polish = [...]`");
                };
                providers.insert(
                    name.clone(),
                    ProviderSpec {
                        name: name.clone(),
                        polish,
                        resume: o.resume.unwrap_or_default(),
                        output: o.output.unwrap_or(Output::Text),
                        send_kind: o.send_kind.unwrap_or_else(default_send_kind),
                        extra,
                        models: o.models.unwrap_or_default(),
                        model_flag: o.model_flag.unwrap_or_else(default_model_flag),
                    },
                );
            }
        }
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

/// Where polish turns run: a neutral, stable directory per intent under the
/// root, so the provider has no repository in reach and claude's same-cwd
/// resume keeps working.
pub fn polish_cwd(root: &Path, stem: &str) -> PathBuf {
    root.join(".polish").join(stem)
}

/// Body skeleton (`polish == false`) or polish prompt template for a kind.
pub fn template(dirs: &Dirs, kind: &str, polish: bool) -> anyhow::Result<String> {
    let file = if polish {
        format!("{kind}.polish.md")
    } else {
        format!("{kind}.md")
    };
    let path = dirs.config.join("templates").join(&file);
    std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
}

fn argv(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn jsonl(
    type_key: &str,
    text_path: &str,
    text_types: &[&str],
    text_when: Option<(&str, &str)>,
    id_paths: &[&str],
    done_type: &str,
) -> Output {
    Output::Jsonl {
        type_key: type_key.to_string(),
        text_path: text_path.to_string(),
        text_types: argv(text_types),
        text_when: text_when.map(|(p, v)| When {
            path: p.to_string(),
            value: v.to_string(),
        }),
        id_paths: argv(id_paths),
        done_type: Some(done_type.to_string()),
    }
}

/// The four CLIs present on the owner's machine (2026-10-24). agy and claude
/// event shapes come from real captures; codex and copilot follow their
/// documented shapes and are unverified (codex account 402, copilot denied by
/// policy on the capture machine).
pub fn builtin_providers() -> BTreeMap<String, ProviderSpec> {
    let mut m = BTreeMap::new();
    m.insert(
        "agy".to_string(),
        ProviderSpec {
            name: "agy".into(),
            polish: argv(&[
                "agy",
                "-p={prompt}",
                "--output-format",
                "stream-json",
                "--print-timeout",
                "90s",
                "--disable-slash-commands",
                "--sandbox",
            ]),
            resume: argv(&[
                "agy",
                "--conversation",
                "{conv}",
                "-p={prompt}",
                "--output-format",
                "stream-json",
                "--print-timeout",
                "90s",
                "--disable-slash-commands",
                "--sandbox",
            ]),
            output: jsonl(
                "event",
                "/step_update/text_delta",
                &["step_update"],
                Some(("/step_update/step_type", "agent_response")),
                &["/conversation_id", "/step_update/conversation_id"],
                "result",
            ),
            send_kind: "agy".into(),
            extra: Vec::new(),
            models: argv(&[
                "gemini-4-argon-high",
                "gemini-4-argon-medium",
                "gemini-4-argon-low",
                "claude-opus-5-5-high",
                "claude-opus-5-5-medium",
                "claude-sonnet-5-5-medium",
                "gemini-3.8-flash-high",
            ]),
            model_flag: "--model".into(),
        },
    );
    m.insert(
        "claude".to_string(),
        ProviderSpec {
            name: "claude".into(),
            polish: argv(&[
                "claude",
                "--output-format",
                "stream-json",
                "--include-partial-messages",
                "--session-id",
                "{conv}",
                "--permission-mode",
                "plan",
                "--tools",
                "",
                "--strict-mcp-config",
                "-p",
                "{prompt}",
            ]),
            resume: argv(&[
                "claude",
                "--resume",
                "{conv}",
                "--output-format",
                "stream-json",
                "--include-partial-messages",
                "--permission-mode",
                "plan",
                "--tools",
                "",
                "--strict-mcp-config",
                "-p",
                "{prompt}",
            ]),
            output: jsonl(
                "type",
                "/event/delta/text",
                &["stream_event"],
                Some(("/event/type", "content_block_delta")),
                &["/session_id"],
                "result",
            ),
            send_kind: "claude".into(),
            extra: Vec::new(),
            models: argv(&["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"]),
            model_flag: "--model".into(),
        },
    );
    m.insert(
        "codex".to_string(),
        ProviderSpec {
            name: "codex".into(),
            polish: argv(&[
                "codex",
                "exec",
                "--json",
                "--sandbox",
                "read-only",
                "--",
                "{prompt}",
            ]),
            resume: argv(&[
                "codex",
                "exec",
                "resume",
                "--json",
                "--sandbox",
                "read-only",
                "{conv}",
                "--",
                "{prompt}",
            ]),
            output: jsonl(
                "type",
                "/item/text",
                &["item.completed"],
                Some(("/item/type", "agent_message")),
                &["/thread_id"],
                "turn.completed",
            ),
            send_kind: "codex".into(),
            extra: Vec::new(),
            models: argv(&[]),
            model_flag: "--model".into(),
        },
    );
    m.insert(
        "copilot".to_string(),
        ProviderSpec {
            name: "copilot".into(),
            polish: argv(&[
                "copilot",
                "--output-format",
                "json",
                "--deny-tool",
                "*",
                "-p",
                "{prompt}",
            ]),
            resume: argv(&[
                "copilot",
                "--session-id",
                "{conv}",
                "--output-format",
                "json",
                "--deny-tool",
                "*",
                "-p",
                "{prompt}",
            ]),
            output: jsonl(
                "type",
                "/data/deltaContent",
                &["assistant.message_delta"],
                None,
                &["/data/sessionId"],
                "session.idle",
            ),
            send_kind: "copilot".into(),
            extra: Vec::new(),
            models: argv(&[]),
            model_flag: "--model".into(),
        },
    );
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
        h(
            "issue-report",
            &["issue"],
            "Use the issue-report skill on this finding. Do not file anything until you have confirmed the root cause with me.\n\n{intent}\n\nAttachments:\n{attachments}",
        ),
        h(
            "enhancement-spec",
            &["enhancement"],
            "Use the enhancement-spec skill to write this up as a GitHub issue. Show me the draft before posting.\n\n{intent}",
        ),
        h("grill", &["*"], "/grill-me\n\n{intent}"),
        h("wayfinder", &["*"], "/wayfinder\n\nDestination:\n{intent}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn lookup<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn parse_mode_variants() {
        assert_eq!(parse_mode(&[]), Mode::Tui);
        assert_eq!(
            parse_mode(&["add".into()]),
            Mode::Add {
                title: None,
                popup: false
            }
        );
        assert_eq!(
            parse_mode(&["add".into(), "--popup".into(), "Fix x".into()]),
            Mode::Add {
                title: Some("Fix x".into()),
                popup: true
            }
        );
        assert!(matches!(parse_mode(&["bogus".into()]), Mode::Usage(_)));
        assert!(
            matches!(parse_mode(&["--version".into()]), Mode::Usage(t) if t.starts_with("herdr-capabilities "))
        );
    }

    #[test]
    fn dirs_prefer_env_then_plugin_then_home() {
        let home = Path::new("/home/u");
        let d = resolve_dirs(lookup(&[("CAPABILITIES_CONFIG_DIR", "/c/env")]), home, None);
        assert_eq!(d.config, Path::new("/c/env"));
        assert_eq!(d.root, Path::new("/home/u/capabilities"));

        let d = resolve_dirs(
            lookup(&[
                ("HERDR_PLUGIN_ID", "jefelabs.capabilities"),
                ("HERDR_PLUGIN_CONFIG_DIR", "/c/plugin"),
            ]),
            home,
            None,
        );
        assert_eq!(d.config, Path::new("/c/plugin"));

        let d = resolve_dirs(
            lookup(&[
                ("HERDR_PLUGIN_ID", "jefelabs.toolbar"),
                ("HERDR_PLUGIN_CONFIG_DIR", "/c/toolbar"),
            ]),
            home,
            None,
        );
        assert_eq!(
            d.config,
            Path::new("/home/u/.config/herdr/plugins/config/jefelabs.capabilities"),
            "another plugin's dir is not ours"
        );

        let d = resolve_dirs(lookup(&[]), home, Some("~/notes/capabilities"));
        assert_eq!(d.root, Path::new("/home/u/notes/capabilities"));
    }

    #[test]
    fn first_run_writes_defaults_and_never_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        let cfg = load_or_init(&dirs).unwrap();
        assert!(dirs.config.join("capabilities.toml").is_file());
        for k in ["issue", "enhancement", "idea"] {
            assert!(
                dirs.config
                    .join("templates")
                    .join(format!("{k}.md"))
                    .is_file(),
                "{k}.md"
            );
            assert!(
                dirs.config
                    .join("templates")
                    .join(format!("{k}.polish.md"))
                    .is_file(),
                "{k}.polish.md"
            );
        }
        assert_eq!(cfg.root, dirs.root);
        fs::write(dirs.config.join("templates/issue.md"), "## Mine\n").unwrap();
        load_or_init(&dirs).unwrap();
        assert_eq!(template(&dirs, "issue", false).unwrap(), "## Mine\n");
        assert!(
            template(&dirs, "issue", true)
                .unwrap()
                .contains("```proposal")
        );
    }

    #[test]
    fn builtin_providers_and_handoffs_present() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        let cfg = load_or_init(&dirs).unwrap();
        assert_eq!(cfg.polish_provider, "agy");
        for p in ["agy", "claude", "codex", "copilot"] {
            assert!(cfg.providers.contains_key(p), "provider {p}");
        }
        let agy = &cfg.providers["agy"];
        assert_eq!(
            agy.polish[..4],
            ["agy", "-p={prompt}", "--output-format", "stream-json"]
        );
        assert!(agy.resume.iter().any(|a| a == "--conversation"));
        let claude = &cfg.providers["claude"];
        assert!(
            claude.polish.contains(&"--session-id".to_string())
                && claude.polish.contains(&"{conv}".to_string())
        );
        assert_eq!(
            cfg.providers["codex"].polish[..3],
            ["codex", "exec", "--json"]
        );
        assert!(cfg.providers["copilot"].polish.iter().any(|a| a == "-p"));
        for p in cfg.providers.values() {
            assert!(
                !p.polish
                    .iter()
                    .chain(p.resume.iter())
                    .any(|a| a.contains("allow-all") || a.contains("dangerously")),
                "{} grants tools",
                p.name
            );
        }
        let names: Vec<&str> = cfg.handoffs.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(
            names,
            ["issue-report", "enhancement-spec", "grill", "wayfinder"]
        );
        assert!(cfg.handoffs.iter().all(|h| h.provider == "claude"));
        assert_eq!(cfg.handoffs[0].kinds, ["issue"]);
        assert_eq!(cfg.handoffs[2].kinds, ["*"]);
    }

    #[test]
    fn custom_provider_from_toml_merges() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        load_or_init(&dirs).unwrap();
        fs::write(
            dirs.config.join("capabilities.toml"),
            "polish_provider = \"mine\"\n[providers.mine]\npolish = [\"my-cli\", \"--prompt\", \"{prompt}\"]\noutput = \"text\"\nsend_kind = \"claude\"\n",
        )
        .unwrap();
        let cfg = load_or_init(&dirs).unwrap();
        assert_eq!(cfg.polish_provider, "mine");
        assert_eq!(
            cfg.providers["mine"].polish,
            ["my-cli", "--prompt", "{prompt}"]
        );
        assert!(matches!(cfg.providers["mine"].output, Output::Text));
        assert!(cfg.providers.contains_key("agy"), "built-ins still present");
    }

    #[test]
    fn builtin_providers_restrict_tools_and_repo_access() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        let cfg = load_or_init(&dirs).unwrap();
        let has = |p: &ProviderSpec, flag: &str| {
            p.polish.iter().any(|a| a == flag) && p.resume.iter().any(|a| a == flag)
        };
        assert!(
            has(&cfg.providers["claude"], "--permission-mode")
                && has(&cfg.providers["claude"], "--strict-mcp-config")
        );
        let claude = &cfg.providers["claude"];
        let i = claude.polish.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(claude.polish[i + 1], "", "claude gets an empty tool list");
        assert!(
            has(&cfg.providers["agy"], "--sandbox")
                && !cfg.providers["agy"].polish.iter().any(|a| a == "--mode")
        );
        assert!(
            has(&cfg.providers["codex"], "--sandbox")
                && cfg.providers["codex"]
                    .polish
                    .iter()
                    .any(|a| a == "read-only")
        );
        assert!(has(&cfg.providers["copilot"], "--deny-tool"));
    }

    #[test]
    fn intents_root_env_overrides_the_root() {
        let home = Path::new("/home/u");
        let d = resolve_dirs(
            lookup(&[("CAPABILITIES_ROOT", "/work/repo/intents")]),
            home,
            Some("~/capabilities"),
        );
        assert_eq!(d.root, Path::new("/work/repo/intents"));
    }

    #[test]
    fn polish_cwd_is_a_neutral_dir_under_the_root() {
        let p = polish_cwd(Path::new("/r"), "2026-10-24-slug");
        assert_eq!(p, Path::new("/r/.polish/2026-10-24-slug"));
    }

    #[test]
    fn a_model_override_tunes_a_builtin_without_restating_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        load_or_init(&dirs).unwrap();
        fs::write(
            dirs.config.join("capabilities.toml"),
            "[providers.agy]\nmodel = \"gemini-4-argon-medium\"\n",
        )
        .unwrap();
        let cfg = load_or_init(&dirs).unwrap();
        let agy = &cfg.providers["agy"];
        assert_eq!(agy.extra, ["--model", "gemini-4-argon-medium"]);
        assert!(
            agy.polish.iter().any(|a| a == "--sandbox"),
            "built-in argv kept: {:?}",
            agy.polish
        );
        assert!(agy.resume.iter().any(|a| a == "--conversation"));
    }

    #[test]
    fn builtin_providers_list_models_and_a_model_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        let cfg = load_or_init(&dirs).unwrap();
        let agy = &cfg.providers["agy"];
        assert_eq!(agy.models[0], "gemini-4-argon-high");
        assert!(agy.models.iter().any(|m| m == "gemini-4-argon-medium"));
        assert_eq!(agy.model_flag, "--model");
        assert!(!cfg.providers["claude"].models.is_empty());
    }

    #[test]
    fn last_used_round_trips_and_defaults_to_config() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        let cfg = load_or_init(&dirs).unwrap();
        let first = last_used(&dirs, &cfg);
        assert_eq!(
            first,
            LastUsed {
                provider: "agy".into(),
                model: None,
                ..Default::default()
            },
            "no file yet: the config's polish provider"
        );
        save_last_used(
            &dirs,
            &LastUsed {
                provider: "agy".into(),
                model: Some("gemini-4-argon-high".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            last_used(&dirs, &cfg).model.as_deref(),
            Some("gemini-4-argon-high")
        );
    }

    #[test]
    fn every_kind_template_has_description_criteria_and_questions() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        load_or_init(&dirs).unwrap();
        for k in KINDS {
            let body = template(&dirs, k, false).unwrap();
            for h in [
                "## Description",
                "## Acceptance criteria",
                "## Open questions",
            ] {
                assert!(body.contains(h), "{k}.md lacks {h}:\n{body}");
            }
            let polish = template(&dirs, k, true).unwrap();
            assert!(
                polish.contains("Acceptance criteria") && polish.contains("→"),
                "{k}.polish.md must describe the structure and the answer marker"
            );
        }
    }
}
