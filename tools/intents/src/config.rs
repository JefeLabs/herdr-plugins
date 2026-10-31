//! Modes, directories, intents.toml, templates, built-in providers and handoffs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use anyhow::Context;
use serde::{Deserialize, Serialize};

pub const PLUGIN_ID: &str = "jefelabs.intents";
pub const KINDS: &[&str] = &["issue", "refactor", "enhancement", "idea"];

/// How a kind is shown: an issue is a bug, an enhancement a feature.
pub fn kind_label(kind: &str) -> &str {
    match kind {
        "issue" => "bug",
        "enhancement" => "feature",
        other => other,
    }
}

/// The kind an agent's first line names: `bug:`, `feature:` or `idea:`
/// (`capability:` from older chats is a feature).
pub fn kind_from_heading(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "bug" => Some("issue"),
        "refactor" => Some("refactor"),
        "feature" | "capability" => Some("enhancement"),
        "idea" => Some("idea"),
        _ => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Tui,
    Add {
        title: Option<String>,
        popup: bool,
    },
    /// The capture agent's browser tools, over MCP on stdin/stdout.
    CaptureMcp(PathBuf),
    /// One capture, detached from the tool that started it.
    CaptureRun(PathBuf),
    /// Herdr's `workspace.created` hook: a workspace opened in a meta repo
    /// becomes its space and loads its intents. `first` (`--first`, the
    /// startup hook): the session's first workspace, which no event names.
    OpenSpace {
        first: bool,
    },
    Usage(String),
}

const USAGE: &str = "usage: herdr-intents [add [--popup] [title] | --version]";

pub fn parse_mode(args: &[String]) -> Mode {
    match args.first().map(String::as_str) {
        None => Mode::Tui,
        Some("--version") | Some("-V") => {
            Mode::Usage(format!("herdr-intents {}", env!("CARGO_PKG_VERSION")))
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
        Some("capture-mcp") if args.len() == 2 => Mode::CaptureMcp(PathBuf::from(&args[1])),
        Some("capture-run") if args.len() == 2 => Mode::CaptureRun(PathBuf::from(&args[1])),
        Some("open-space") if args.len() == 1 => Mode::OpenSpace { first: false },
        Some("open-space") if args.len() == 2 && args[1] == "--first" => {
            Mode::OpenSpace { first: true }
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
/// else `~/.config/herdr/plugins/config/jefelabs.intents`. Root from config or
/// `~/.helmsmith/intents/spaces` (see `storage`).
pub fn resolve_dirs(
    get: impl Fn(&str) -> Option<String>,
    home: &Path,
    cfg_root: Option<&str>,
) -> Dirs {
    let config = if let Some(c) = get("INTENTS_CONFIG_DIR") {
        PathBuf::from(c)
    } else if get("HERDR_PLUGIN_ID").as_deref() == Some(PLUGIN_ID)
        && get("HERDR_PLUGIN_CONFIG_DIR").is_some()
    {
        PathBuf::from(get("HERDR_PLUGIN_CONFIG_DIR").unwrap())
    } else {
        home.join(".config/herdr/plugins/config").join(PLUGIN_ID)
    };
    let root = match (get("INTENTS_ROOT"), cfg_root) {
        (Some(env_root), _) => PathBuf::from(expand_home(&env_root, home)),
        (None, Some(r)) => PathBuf::from(expand_home(r, home)),
        (None, None) => crate::storage::default_root(home),
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
    /// Pass `--strict-mcp-config` (no MCP servers in polish turns). Unset:
    /// yes, unless an organization-managed MCP config forbids it.
    strict_mcp: Option<bool>,
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
    /// The top-level group this run sees (the Herdr workspace's name); `None`
    /// shows the whole tree. Set by the caller from `scope::resolve`.
    pub scope: Option<String>,
    pub editor: String,
    pub send_provider: String,
    pub polish_provider: String,
    pub workbrokr: Option<bool>,
    pub providers: BTreeMap<String, ProviderSpec>,
    pub handoffs: Vec<Handoff>,
    /// Said once at start-up (the intents moved). Set by the caller from
    /// `storage::settle_root`.
    pub notice: Option<String>,
    /// Why this space shows no intents: its `storage` is a placeholder (meta,
    /// remote) or broken. Set by the caller from `space::storage_block`.
    pub storage_block: Option<String>,
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
    ("intents.toml", include_str!("../defaults/intents.toml")),
    ("templates/issue.md", include_str!("../defaults/issue.md")),
    (
        "templates/issue.polish.md",
        include_str!("../defaults/issue.polish.md"),
    ),
    (
        "templates/refactor.md",
        include_str!("../defaults/refactor.md"),
    ),
    (
        "templates/issue.interview.md",
        include_str!("../defaults/issue.interview.md"),
    ),
    (
        "templates/refactor.interview.md",
        include_str!("../defaults/refactor.interview.md"),
    ),
    (
        "templates/enhancement.interview.md",
        include_str!("../defaults/enhancement.interview.md"),
    ),
    (
        "templates/idea.interview.md",
        include_str!("../defaults/idea.interview.md"),
    ),
    (
        "templates/refactor.polish.md",
        include_str!("../defaults/refactor.polish.md"),
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
/// The plugin was called `jefelabs.capabilities`, its config file
/// `capabilities.toml`. On the first run under the new name the old config
/// folder (beside the new one under Herdr's plugins/config) is copied over,
/// `capabilities.toml` becoming `intents.toml`; the old folder is left as it
/// was. Returns whether anything was carried over.
const OLD_PLUGIN_ID: &str = "jefelabs.capabilities";
const OLD_CONFIG_FILE: &str = "capabilities.toml";

pub fn migrate_old_config(config: &Path) -> anyhow::Result<bool> {
    if config.join("intents.toml").exists() {
        return Ok(false);
    }
    let Some(old) = config.parent().map(|p| p.join(OLD_PLUGIN_ID)) else {
        return Ok(false);
    };
    if !old.join(OLD_CONFIG_FILE).is_file() {
        return Ok(false);
    }
    copy_tree(&old, config)?;
    std::fs::rename(config.join(OLD_CONFIG_FILE), config.join("intents.toml"))?;
    Ok(true)
}

/// Copy `from` into `to`, keeping any file already at `to`.
fn copy_tree(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    for entry in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if !target.exists() {
            std::fs::copy(entry.path(), &target)
                .with_context(|| format!("copying {}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// SHA-256 of every earlier default of a kind's polish template. A template
/// that still matches one was never edited, so it is brought up to the
/// current default; an edited one is left alone.
fn old_polish_defaults(kind: &str) -> &'static [&'static str] {
    match kind {
        "issue" => &[
            "20b7106aff84fd4b717831544a8f06854c38833ec267f0f58068174c6c2529a0",
            "cf7ac21cb266d2e0002d368fbe0fe96fa03ff8453c97b3a1652525468e7ce76f",
            "11e7b3af7e8908106dc33dc107b46886f9fcc55773bef2808cfbbad0d19b8b06",
            "2e6d4ad5ebfac608bf4ee1c75078d9a4212be580310d756b17144535599ddf37",
            "4a490a416cb8d007f38192e07e9e5e7d57389aac730daf62fa6691bf6f74bcd7",
            "5f017e2972f22d7b5ceff33a77eeeca0b760d464871422555159e6ccb14ddb46",
            "65b3bb0a548aa6bb6731233215daf5e4964648369fe5582dd83e4ee7d1781f68",
            "749adf96a3fc58f5fdb4c7087c900ca3794d06d25840be9da7af5414152d723f",
            "b49c28cdae786486f6cca3972eaa90bda603b5bf193dfe5db55e74d79b52009a",
            "ffa2e9fa20d251d90f989ce7f6a805f992a31e318e4661d7ed3441d4a86b9588",
        ],
        "enhancement" => &[
            "43b9f706e662b6d766a987e2f7211fc6f118941568b3bd1be70dcf202ac9d913",
            "27ffc89cbea7fcf80c2ae8da67716000803d66a6ff60e77f708536f31e3bab43",
            "17a497a254e4f121cf9dc8b5fec15f026a1199c5cfa9e687190bd87b4f42fad2",
            "54e77f65fe15a51ad42568636dff129f0c2cfd3600640b5996d6ef8a070aff15",
            "5a77b7e1b87fc786d22007bbc880700221ce44283b29f162ac9abd62439fff46",
            "6e002d1d24d299f0666b3b59f16aa998bf398b5773eef5346b0886fcdeb1cf32",
            "8ff8895478caa5cbe3ce263d127b6e9018e960e32abf94f55ec452bed6f1bb9e",
            "9f35ee9e1792b915a5cbee6e91f341af9faa18ea1a908ec206ee11f34f7b9a8c",
            "af2ce3d227385fd1df0311f244bb82bbdebf3825dda96468c5bdf098d4d97166",
            "beafcfa500a95db86e7a7959792ed63af97d7f32ef5dd5fb979d8da058999955",
        ],
        "idea" => &[
            "492bfbb1a517767aad1edfff50760b18337dcedb713ad687baec50740bc6676d",
            "7e002fbb1ec6ff488a4f458db4ccb03b810a5d576613cc80bd7126b093021cec",
            "0189d38bc264b506d1eff5ec5605a1de15ea48619b05206d4df77f9ea1c1e7a3",
            "07659af1a049460d5b8a336d2901a1a6e3d6c7d1ea19741659ad65e49068e4e4",
            "110a9197430b4c4164f14e62d7c90e0c2da6d59dcd8daed93201223b04f4255e",
            "54df2bd92157a8ab51afde79066d98c8a9a3834e1ffe442b16874ae5ef4304a4",
            "7d50b89a465dbf62e4bd7eed77b9167f06d7134261faf664ccd34b536879d820",
            "a36eac4c759d2ce768c1de12a7bb1ff0d8bfabb2c1549758be1e82788ef6fe45",
            "eda55154ef00774c40673a034a50dca17e37af26bfcef01f7ebfa62d5c20c0d3",
            "f93c7c7f0c12b3ba3b5fd080654f548e98c2ca8bf9fcc9dda39862dcc8c798d9",
        ],
        _ => &[],
    }
}

/// Replace `path` with `new` when its content hashes to one of `old`: an
/// untouched earlier default. Returns whether it was replaced.
pub fn upgrade_untouched(path: &Path, new: &str, old: &[&str]) -> anyhow::Result<bool> {
    let Ok(current) = std::fs::read(path) else {
        return Ok(false);
    };
    let hash = format!("{:x}", Sha256::digest(&current));
    if current == new.as_bytes() || !old.contains(&hash.as_str()) {
        return Ok(false);
    }
    std::fs::write(path, new).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

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
    for kind in KINDS {
        let rel = format!("templates/{kind}.polish.md");
        if let Some((_, text)) = DEFAULT_FILES.iter().find(|(r, _)| *r == rel) {
            upgrade_untouched(&dirs.config.join(&rel), text, old_polish_defaults(kind))?;
        }
    }
    let path = dirs.config.join("intents.toml");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let raw: RawConfig = toml::from_str(&text).with_context(|| path.display().to_string())?;

    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"));
    let root = match &raw.root {
        // either default line means the default: the old one follows the move
        Some(r) if r != crate::storage::OLD_ROOT && r != crate::storage::DEFAULT_ROOT => {
            PathBuf::from(expand_home(r, &home))
        }
        _ => dirs.root.clone(),
    };

    let mut providers = builtin_providers();
    let strict_settings: BTreeMap<String, bool> = raw
        .providers
        .iter()
        .filter_map(|(name, o)| o.strict_mcp.map(|b| (name.clone(), b)))
        .collect();
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
    let managed: Vec<&Path> = MANAGED_MCP_CONFIGS.iter().map(Path::new).collect();
    apply_strict_mcp(
        &mut providers,
        &strict_settings,
        managed_mcp_present(&managed),
    );
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
        scope: None,
        notice: None,
        storage_block: None,
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

/// The kind's interview guide (`templates/<kind>.interview.md`): what an
/// add-by-conversation agent asks about.
pub fn interview_guide(dirs: &Dirs, kind: &str) -> anyhow::Result<String> {
    let path = dirs
        .config
        .join("templates")
        .join(format!("{kind}.interview.md"));
    std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
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
/// Where Claude Code looks for an organization-managed MCP config (macOS,
/// then Linux). With one present it refuses `--strict-mcp-config`.
pub const MANAGED_MCP_CONFIGS: &[&str] = &[
    "/Library/Application Support/ClaudeCode/managed-mcp.json",
    "/etc/claude-code/managed-mcp.json",
];

pub fn managed_mcp_present(paths: &[&Path]) -> bool {
    paths.iter().any(|p| p.is_file())
}

/// Drops `--strict-mcp-config` from a provider's command lines where it must
/// not go: `strict_mcp = false` in its `[providers.<name>]` table, or, with
/// no setting, an organization-managed MCP config on this machine (claude
/// refuses the flag then, and only the organization's servers load anyway).
fn apply_strict_mcp(
    providers: &mut BTreeMap<String, ProviderSpec>,
    settings: &BTreeMap<String, bool>,
    managed: bool,
) {
    for (name, spec) in providers.iter_mut() {
        let keep = settings.get(name).copied().unwrap_or(!managed);
        if !keep {
            spec.polish.retain(|a| a != "--strict-mcp-config");
            spec.resume.retain(|a| a != "--strict-mcp-config");
        }
    }
}

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
                "0s",
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
                "0s",
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
                "--verbose",
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
                "--verbose",
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
        h(
            "refactor-spec",
            &["refactor"],
            "Use the enhancement-spec skill to write this up as a refactor GitHub issue with the `refactor` label. Show me the draft before posting.\n\n{intent}",
        ),
        h("grill", &["*"], "/grill-me\n\n{intent}"),
        h("wayfinder", &["*"], "/wayfinder\n\nDestination:\n{intent}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn strict(p: &ProviderSpec) -> (bool, bool) {
        let has = |argv: &[String]| argv.iter().any(|a| a == "--strict-mcp-config");
        (has(&p.polish), has(&p.resume))
    }

    #[test]
    fn claude_streams_with_verbose_so_every_version_accepts_it() {
        // older claude refuses `-p --output-format stream-json` without --verbose
        let claude = &builtin_providers()["claude"];
        assert!(
            claude.polish.iter().any(|a| a == "--verbose"),
            "{:?}",
            claude.polish
        );
        assert!(
            claude.resume.iter().any(|a| a == "--verbose"),
            "{:?}",
            claude.resume
        );
    }

    #[test]
    fn strict_mcp_is_left_off_when_an_enterprise_mcp_config_is_present() {
        let mut providers = builtin_providers();
        apply_strict_mcp(&mut providers, &BTreeMap::new(), true);
        assert_eq!(strict(&providers["claude"]), (false, false));
        let mut providers = builtin_providers();
        apply_strict_mcp(&mut providers, &BTreeMap::new(), false);
        assert_eq!(
            strict(&providers["claude"]),
            (true, true),
            "kept without one"
        );
    }

    #[test]
    fn the_strict_mcp_setting_wins_either_way() {
        let off = BTreeMap::from([("claude".to_string(), false)]);
        let mut providers = builtin_providers();
        apply_strict_mcp(&mut providers, &off, false);
        assert_eq!(strict(&providers["claude"]), (false, false));
        let on = BTreeMap::from([("claude".to_string(), true)]);
        let mut providers = builtin_providers();
        apply_strict_mcp(&mut providers, &on, true);
        assert_eq!(strict(&providers["claude"]), (true, true));
    }

    #[test]
    fn a_managed_mcp_config_is_found_where_claude_looks() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("managed-mcp.json");
        assert!(!managed_mcp_present(&[file.as_path()]));
        fs::write(&file, "{}").unwrap();
        assert!(managed_mcp_present(&[
            tmp.path().join("nope").as_path(),
            file.as_path()
        ]));
        assert!(MANAGED_MCP_CONFIGS.contains(&"/etc/claude-code/managed-mcp.json"));
        assert!(
            MANAGED_MCP_CONFIGS
                .contains(&"/Library/Application Support/ClaudeCode/managed-mcp.json")
        );
    }

    #[test]
    fn strict_mcp_false_in_intents_toml_drops_the_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        load_or_init(&dirs).unwrap();
        let path = dirs.config.join("intents.toml");
        let mut text = fs::read_to_string(&path).unwrap();
        text.push_str("\n[providers.claude]\nstrict_mcp = false\n");
        fs::write(&path, text).unwrap();
        let cfg = load_or_init(&dirs).unwrap();
        assert_eq!(strict(&cfg.providers["claude"]), (false, false));
        assert!(
            cfg.providers["claude"]
                .polish
                .iter()
                .any(|a| a == "--permission-mode"),
            "the rest of the built-in argv stays"
        );
    }

    #[test]
    fn interview_guides_are_seeded_for_every_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        load_or_init(&dirs).unwrap();
        for kind in KINDS {
            let g = interview_guide(&dirs, kind).unwrap();
            assert!(!g.trim().is_empty(), "{kind} has a guide");
        }
        assert!(
            interview_guide(&dirs, "issue")
                .unwrap()
                .contains("expected")
        );
    }

    #[test]
    fn kinds_include_refactor_shown_as_refactor() {
        assert_eq!(KINDS, ["issue", "refactor", "enhancement", "idea"]);
        assert_eq!(kind_label("refactor"), "refactor");
        assert_eq!(kind_from_heading("Refactor"), Some("refactor"));
    }

    #[test]
    fn refactor_templates_are_seeded() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        load_or_init(&dirs).unwrap();
        let t = dirs.config.join("templates");
        assert!(t.join("refactor.md").is_file());
        let polish = fs::read_to_string(t.join("refactor.polish.md")).unwrap();
        assert!(
            polish.contains("refactor: <what changes> in <where>"),
            "{polish}"
        );
    }

    #[test]
    fn refactor_spec_handoff_applies_to_refactors() {
        let all = builtin_handoffs();
        let h = all
            .iter()
            .find(|h| h.name == "refactor-spec")
            .expect("a refactor-spec hand-off");
        assert_eq!(h.kinds, ["refactor"]);
        assert!(h.prompt.contains("enhancement-spec"), "{}", h.prompt);
        assert!(h.prompt.contains("`refactor` label"), "{}", h.prompt);
    }

    #[test]
    fn kinds_are_shown_as_bug_feature_and_idea() {
        assert_eq!(kind_label("issue"), "bug");
        assert_eq!(kind_label("enhancement"), "feature");
        assert_eq!(kind_label("idea"), "idea");
        assert_eq!(
            kind_label("custom"),
            "custom",
            "an unknown kind shows as written"
        );
        assert_eq!(kind_from_heading("bug"), Some("issue"));
        assert_eq!(kind_from_heading("feature"), Some("enhancement"));
        assert_eq!(
            kind_from_heading("Capability"),
            Some("enhancement"),
            "older chats"
        );
        assert_eq!(kind_from_heading("idea"), Some("idea"));
        assert_eq!(kind_from_heading("chore"), None);
    }

    #[test]
    fn an_untouched_old_template_is_upgraded_and_an_edited_one_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let old_text = "Start every reply with one line `capability: <who> can <what>`.\n";
        let old_hash = format!("{:x}", Sha256::digest(old_text.as_bytes()));
        let untouched = tmp.path().join("issue.polish.md");
        fs::write(&untouched, old_text).unwrap();
        assert!(upgrade_untouched(&untouched, "NEW\n", &[old_hash.as_str()]).unwrap());
        assert_eq!(fs::read_to_string(&untouched).unwrap(), "NEW\n");
        let edited = tmp.path().join("idea.polish.md");
        fs::write(&edited, "my own words\n").unwrap();
        assert!(!upgrade_untouched(&edited, "NEW\n", &[old_hash.as_str()]).unwrap());
        assert_eq!(fs::read_to_string(&edited).unwrap(), "my own words\n");
    }

    #[test]
    fn the_shipped_polish_templates_offer_all_three_headings() {
        for kind in KINDS {
            let text = DEFAULT_FILES
                .iter()
                .find(|(rel, _)| *rel == format!("templates/{kind}.polish.md"))
                .map(|(_, t)| *t)
                .unwrap();
            for h in ["`bug:", "`feature:", "`idea:", "Is this a bug"] {
                assert!(text.contains(h), "{kind}.polish.md lacks {h}");
            }
            let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
            assert!(
                !old_polish_defaults(kind).contains(&hash.as_str()),
                "the current {kind} default must not be listed as an old one"
            );
        }
    }

    #[test]
    fn the_old_plugins_config_carries_over_once() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("plugins/config");
        let old = base.join("jefelabs.capabilities");
        fs::create_dir_all(old.join("templates")).unwrap();
        fs::write(old.join("capabilities.toml"), "root = \"~/capabilities\"\n").unwrap();
        fs::write(old.join("last-used.toml"), "provider = \"claude\"\n").unwrap();
        fs::write(old.join("templates/issue.md"), "## Mine\n").unwrap();
        let new = base.join("jefelabs.intents");
        assert!(migrate_old_config(&new).unwrap(), "carried over");
        let read = |p: &str| fs::read_to_string(new.join(p)).unwrap();
        assert_eq!(read("intents.toml"), "root = \"~/capabilities\"\n");
        assert!(
            !new.join("capabilities.toml").exists(),
            "renamed, not copied twice"
        );
        assert_eq!(read("last-used.toml"), "provider = \"claude\"\n");
        assert_eq!(read("templates/issue.md"), "## Mine\n");
        assert!(
            old.join("capabilities.toml").exists(),
            "the old folder is left as it was"
        );
        fs::write(new.join("intents.toml"), "root = \"~/elsewhere\"\n").unwrap();
        assert!(!migrate_old_config(&new).unwrap(), "only the first time");
        assert_eq!(read("intents.toml"), "root = \"~/elsewhere\"\n");
    }

    #[test]
    fn without_an_old_config_nothing_carries_over() {
        let tmp = tempfile::tempdir().unwrap();
        let new = tmp.path().join("plugins/config/jefelabs.intents");
        assert!(!migrate_old_config(&new).unwrap());
        assert!(!new.join("intents.toml").exists());
    }

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
            matches!(parse_mode(&["--version".into()]), Mode::Usage(t) if t.starts_with("herdr-intents "))
        );
        assert_eq!(
            parse_mode(&["open-space".into()]),
            Mode::OpenSpace { first: false }
        );
        assert_eq!(
            parse_mode(&["open-space".into(), "--first".into()]),
            Mode::OpenSpace { first: true }
        );
        assert!(matches!(
            parse_mode(&["open-space".into(), "w3".into()]),
            Mode::Usage(_)
        ));
    }

    #[test]
    fn dirs_prefer_env_then_plugin_then_home() {
        let home = Path::new("/home/u");
        let d = resolve_dirs(lookup(&[("INTENTS_CONFIG_DIR", "/c/env")]), home, None);
        assert_eq!(d.config, Path::new("/c/env"));
        assert_eq!(d.root, Path::new("/home/u/.helmsmith/intents/spaces"));

        let d = resolve_dirs(
            lookup(&[
                ("HERDR_PLUGIN_ID", "jefelabs.intents"),
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
            Path::new("/home/u/.config/herdr/plugins/config/jefelabs.intents"),
            "another plugin's dir is not ours"
        );

        let d = resolve_dirs(lookup(&[]), home, Some("~/notes/intents"));
        assert_eq!(d.root, Path::new("/home/u/notes/intents"));
    }

    #[test]
    fn first_run_writes_defaults_and_never_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            config: tmp.path().join("cfg"),
            root: tmp.path().join("root"),
        };
        let cfg = load_or_init(&dirs).unwrap();
        assert!(dirs.config.join("intents.toml").is_file());
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
    fn agy_waits_for_the_turn_to_finish() {
        // agy hangs instead of exiting when its print timeout fires mid-turn
        let p = builtin_providers();
        for argv in [&p["agy"].polish, &p["agy"].resume] {
            let i = argv.iter().position(|a| a == "--print-timeout").unwrap();
            assert_eq!(argv[i + 1], "0s", "{argv:?}");
        }
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
            [
                "issue-report",
                "enhancement-spec",
                "refactor-spec",
                "grill",
                "wayfinder"
            ]
        );
        assert!(cfg.handoffs.iter().all(|h| h.provider == "claude"));
        assert_eq!(cfg.handoffs[0].kinds, ["issue"]);
        assert_eq!(cfg.handoffs[2].kinds, ["refactor"]);
        assert_eq!(cfg.handoffs[3].kinds, ["*"]);
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
            dirs.config.join("intents.toml"),
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
        assert!(has(&cfg.providers["claude"], "--permission-mode"));
        assert!(
            has(&builtin_providers()["claude"], "--strict-mcp-config"),
            "the built-in claude loads no MCP servers (unless the machine forbids the flag)"
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
    fn both_default_root_lines_mean_the_default() {
        for line in ["~/intents", "~/.helmsmith/intents/spaces"] {
            let tmp = tempfile::tempdir().unwrap();
            let dirs = Dirs {
                config: tmp.path().join("cfg"),
                root: tmp.path().join("default-root"),
            };
            std::fs::create_dir_all(&dirs.config).unwrap();
            std::fs::write(
                dirs.config.join("intents.toml"),
                format!("root = \"{line}\"\n"),
            )
            .unwrap();
            let cfg = load_or_init(&dirs).unwrap();
            assert_eq!(cfg.root, dirs.root, "{line} is the default");
        }
    }

    #[test]
    fn intents_root_env_overrides_the_root() {
        let home = Path::new("/home/u");
        let d = resolve_dirs(
            lookup(&[("INTENTS_ROOT", "/work/repo/intents")]),
            home,
            Some("~/intents"),
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
            dirs.config.join("intents.toml"),
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
