//! Which top-level group a Herdr workspace sees.
//!
//! One Herdr workspace per project: a workspace named `skoolscout` sees the
//! `skoolscout` group of the capabilities root as its whole tree.

use std::path::Path;
use std::process::Command;

/// The group this run sees. `CAPABILITIES_SCOPE` when set (empty means the
/// whole tree); else the name of the Herdr workspace the tool runs in, asked
/// fresh through `label_of(workspace id)` and falling back to the plugin
/// context Herdr passed at start; else none: outside Herdr, the whole tree.
pub fn resolve(
    get: impl Fn(&str) -> Option<String>,
    label_of: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    if let Some(set) = get("CAPABILITIES_SCOPE") {
        return group_name(&set);
    }
    let context = get("HERDR_PLUGIN_CONTEXT_JSON")
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok());
    let from_context = |key: &str| {
        context
            .as_ref()
            .and_then(|c| c.get(key))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let id = get("HERDR_WORKSPACE_ID").or_else(|| from_context("workspace_id"));
    let label = id
        .as_deref()
        .and_then(&label_of)
        .or_else(|| from_context("workspace_label"))?;
    group_name(&label)
}

/// A workspace name usable as one folder name: trimmed, not empty, no path
/// separator, not `.`-led. Anything else is no scope rather than a path.
pub fn group_name(name: &str) -> Option<String> {
    let name = name.trim();
    let one_folder =
        !name.is_empty() && !name.starts_with('.') && !name.contains('/') && !name.contains('\\');
    one_folder.then(|| name.to_string())
}

/// `herdr workspace get <id>`: the workspace's current name.
pub fn herdr_label(bin: &Path, id: &str) -> Option<String> {
    let out = Command::new(bin)
        .args(["workspace", "get", id])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let reply: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).ok()?;
    reply
        .pointer("/result/workspace/label")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn inside_herdr_the_workspace_name_is_the_scope() {
        let s = resolve(env(&[("HERDR_WORKSPACE_ID", "w5")]), |id| {
            (id == "w5").then(|| "skoolscout".to_string())
        });
        assert_eq!(s.as_deref(), Some("skoolscout"));
    }

    #[test]
    fn the_plugin_context_names_the_workspace_when_herdr_cannot_be_asked() {
        let ctx = r#"{"workspace_id":"w7","workspace_label":"jefelabs","tab_id":"w7:t1"}"#;
        let s = resolve(env(&[("HERDR_PLUGIN_CONTEXT_JSON", ctx)]), |_| None);
        assert_eq!(s.as_deref(), Some("jefelabs"));
        let asked = resolve(env(&[("HERDR_PLUGIN_CONTEXT_JSON", ctx)]), |id| {
            (id == "w7").then(|| "renamed".to_string())
        });
        assert_eq!(asked.as_deref(), Some("renamed"), "a fresh answer wins");
    }

    #[test]
    fn capabilities_scope_overrides_and_empty_means_everything() {
        let herdr = |_: &str| Some("skoolscout".to_string());
        let set = resolve(
            env(&[
                ("CAPABILITIES_SCOPE", "tourneyseason"),
                ("HERDR_WORKSPACE_ID", "w5"),
            ]),
            herdr,
        );
        assert_eq!(set.as_deref(), Some("tourneyseason"));
        let all = resolve(
            env(&[("CAPABILITIES_SCOPE", ""), ("HERDR_WORKSPACE_ID", "w5")]),
            herdr,
        );
        assert_eq!(all, None);
    }

    #[test]
    fn outside_herdr_there_is_no_scope() {
        assert_eq!(resolve(env(&[]), |_| Some("x".to_string())), None);
    }

    #[test]
    fn a_name_that_is_not_one_folder_is_no_scope() {
        for bad in ["a/b", "a\\b", "..", ".", ".hidden", "   ", ""] {
            assert_eq!(group_name(bad), None, "{bad:?}");
        }
        assert_eq!(group_name("  skoolscout ").as_deref(), Some("skoolscout"));
    }
}
