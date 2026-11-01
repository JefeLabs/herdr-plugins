//! Which top-level group a Herdr workspace sees.
//!
//! One Herdr workspace per project: a workspace in skoolscout's meta repo, or
//! one named `skoolscout`, sees the `skoolscout` group of the intents root as
//! its whole tree.

use std::path::Path;
use std::process::Command;

/// The group this run sees. `INTENTS_SCOPE` when set (empty means the
/// whole tree); else the space named by the meta repo the tool runs in
/// (`repo_space`); else the name of the Herdr workspace the tool runs in,
/// asked fresh through `label_of(workspace id)` and falling back to the
/// plugin context Herdr passed at start; else none: outside Herdr, the
/// whole tree.
pub fn resolve(
    get: impl Fn(&str) -> Option<String>,
    label_of: impl Fn(&str) -> Option<String>,
    repo_space: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if let Some(set) = get("INTENTS_SCOPE") {
        return group_name(&set);
    }
    if let Some(space) = repo_space() {
        return Some(space);
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
    let id = current_workspace(&get);
    let label = id
        .as_deref()
        .and_then(&label_of)
        .or_else(|| from_context("workspace_label"))?;
    group_name(&label)
}

/// The id of the Herdr workspace the tool runs in: `HERDR_WORKSPACE_ID`,
/// else the plugin context Herdr passed at start; none outside Herdr (an
/// empty id counts as outside).
pub fn current_workspace(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    let id = match get("HERDR_WORKSPACE_ID") {
        Some(id) => id,
        None => get("HERDR_PLUGIN_CONTEXT_JSON")
            .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
            .and_then(|c| c.get("workspace_id")?.as_str().map(str::to_string))?,
    };
    (!id.trim().is_empty()).then_some(id)
}

/// A workspace name usable as one folder name: trimmed, not empty, no path
/// separator, not `.`-led, not `~`-led (Herdr names a workspace started in
/// the home folder `~`). Anything else is no scope rather than a path.
pub fn group_name(name: &str) -> Option<String> {
    let name = name.trim();
    let one_folder = !name.is_empty()
        && !name.starts_with('.')
        && !name.starts_with('~')
        && !name.contains('/')
        && !name.contains('\\');
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

    #[test]
    fn the_current_workspace_comes_from_herdr_then_the_plugin_context() {
        assert_eq!(
            current_workspace(env(&[("HERDR_WORKSPACE_ID", "w5")])).as_deref(),
            Some("w5")
        );
        let ctx = r#"{"workspace_id":"w7","workspace_label":"jefelabs"}"#;
        assert_eq!(
            current_workspace(env(&[("HERDR_PLUGIN_CONTEXT_JSON", ctx)])).as_deref(),
            Some("w7")
        );
        assert_eq!(current_workspace(env(&[])), None, "outside Herdr");
        assert_eq!(
            current_workspace(env(&[
                ("HERDR_WORKSPACE_ID", ""),
                ("HERDR_PLUGIN_CONTEXT_JSON", ctx)
            ])),
            None,
            "an empty id means outside Herdr"
        );
    }

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn inside_herdr_the_workspace_name_is_the_scope() {
        let s = resolve(
            env(&[("HERDR_WORKSPACE_ID", "w5")]),
            |id| (id == "w5").then(|| "skoolscout".to_string()),
            || None,
        );
        assert_eq!(s.as_deref(), Some("skoolscout"));
    }

    #[test]
    fn the_plugin_context_names_the_workspace_when_herdr_cannot_be_asked() {
        let ctx = r#"{"workspace_id":"w7","workspace_label":"jefelabs","tab_id":"w7:t1"}"#;
        let s = resolve(
            env(&[("HERDR_PLUGIN_CONTEXT_JSON", ctx)]),
            |_| None,
            || None,
        );
        assert_eq!(s.as_deref(), Some("jefelabs"));
        let asked = resolve(
            env(&[("HERDR_PLUGIN_CONTEXT_JSON", ctx)]),
            |id| (id == "w7").then(|| "renamed".to_string()),
            || None,
        );
        assert_eq!(asked.as_deref(), Some("renamed"), "a fresh answer wins");
    }

    #[test]
    fn intents_scope_overrides_and_empty_means_everything() {
        let herdr = |_: &str| Some("skoolscout".to_string());
        let set = resolve(
            env(&[
                ("INTENTS_SCOPE", "tourneyseason"),
                ("HERDR_WORKSPACE_ID", "w5"),
            ]),
            herdr,
            || Some("skoolscout".to_string()),
        );
        assert_eq!(set.as_deref(), Some("tourneyseason"));
        let all = resolve(
            env(&[("INTENTS_SCOPE", ""), ("HERDR_WORKSPACE_ID", "w5")]),
            herdr,
            || Some("skoolscout".to_string()),
        );
        assert_eq!(all, None);
    }

    #[test]
    fn outside_herdr_there_is_no_scope() {
        assert_eq!(resolve(env(&[]), |_| Some("x".to_string()), || None), None);
    }

    #[test]
    fn a_workspace_in_a_meta_repo_sees_that_repos_space() {
        // a clone in a folder called skoolscout: Herdr names the workspace
        // after it, the meta repo's origin names the space skoolscout-com
        let s = resolve(
            env(&[("HERDR_WORKSPACE_ID", "w5")]),
            |_| Some("skoolscout".to_string()),
            || Some("skoolscout-com".to_string()),
        );
        assert_eq!(s.as_deref(), Some("skoolscout-com"));
        let outside = resolve(env(&[]), |_| None, || Some("skoolscout-com".to_string()));
        assert_eq!(
            outside.as_deref(),
            Some("skoolscout-com"),
            "outside Herdr too"
        );
    }

    #[test]
    fn a_workspace_in_the_home_folder_sees_the_whole_tree() {
        // Herdr started in ~ names its workspace `~`: no space is called that
        let s = resolve(
            env(&[("HERDR_WORKSPACE_ID", "wC")]),
            |_| Some("~".to_string()),
            || None,
        );
        assert_eq!(s, None);
    }

    #[test]
    fn a_name_that_is_not_one_folder_is_no_scope() {
        for bad in [
            "a/b", "a\\b", "..", ".", ".hidden", "~", "~edwin", "   ", "",
        ] {
            assert_eq!(group_name(bad), None, "{bad:?}");
        }
        assert_eq!(group_name("  skoolscout ").as_deref(), Some("skoolscout"));
    }
}
