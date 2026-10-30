//! App-demo stories (skoolscout's `app-demos/stories/*.yaml`) linked to a
//! intent as `- demo: <story-id> (criteria: 1, 4)`: recorded, captioned
//! videos that show its acceptance criteria working.

use std::path::{Path, PathBuf};

use crate::store::{Artifact, significant_words, word_matches};

/// One story, as much of it as the reader shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Story {
    pub id: String,
    pub title: String,
    pub actor: String,
    pub host: String,
    pub depends_on: Vec<String>,
    pub tags: Vec<String>,
    /// Each step as `action` or `action: value`, then ` — caption` when it has one.
    pub steps: Vec<String>,
    pub file: PathBuf,
}

/// `story-id (criteria: 1, 4)` → the story id and the (1-based) criteria it shows.
pub fn demo_target(target: &str) -> (String, Vec<usize>) {
    let t = target.trim();
    let (id, rest) = match t.split_once(char::is_whitespace) {
        Some((id, rest)) => (id, rest),
        None => (t, ""),
    };
    let criteria = rest
        .split_once("criteria:")
        .map(|(_, list)| {
            list.trim_end_matches(')')
                .split(',')
                .filter_map(|n| n.trim().parse::<usize>().ok())
                .collect()
        })
        .unwrap_or_default();
    (id.to_string(), criteria)
}

fn text(v: &serde_yaml_ng::Value, key: &str) -> String {
    v.get(key)
        .and_then(|s| s.as_str())
        .unwrap_or_default()
        .to_string()
}

fn list(v: &serde_yaml_ng::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|l| l.as_sequence())
        .map(|l| {
            l.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn step_text(step: &serde_yaml_ng::Value) -> Option<String> {
    let map = step.as_mapping()?;
    let (action, value) = map.iter().next()?;
    let action = action.as_str()?;
    let mut out = match value.as_str() {
        Some(s) => format!("{action}: {s}"),
        None => action.to_string(),
    };
    if let Some(caption) = value.get("caption").and_then(|c| c.as_str()) {
        out.push_str(" — ");
        out.push_str(caption);
    }
    Some(out)
}

/// Every story in `<root>/app-demos/stories/*.yaml`, over the given roots
/// (the main checkout and its worktrees); unreadable files are skipped.
pub fn all_stories(roots: &[PathBuf]) -> Vec<Story> {
    let mut out = Vec::new();
    for root in roots {
        let Ok(dir) = std::fs::read_dir(root.join("app-demos/stories")) else {
            continue;
        };
        let mut files: Vec<PathBuf> = dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("yaml" | "yml")))
            .collect();
        files.sort();
        for file in files {
            let Some(doc) = std::fs::read_to_string(&file)
                .ok()
                .and_then(|t| serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&t).ok())
            else {
                continue;
            };
            let Some(stories) = doc.get("stories").and_then(|s| s.as_sequence()) else {
                continue;
            };
            for s in stories {
                let id = text(s, "id");
                if id.is_empty() || out.iter().any(|o: &Story| o.id == id) {
                    continue;
                }
                out.push(Story {
                    title: text(s, "title"),
                    actor: text(s, "actor"),
                    host: text(s, "host"),
                    depends_on: list(s, "dependsOn"),
                    tags: list(s, "tags"),
                    steps: s
                        .get("steps")
                        .and_then(|l| l.as_sequence())
                        .map(|l| l.iter().filter_map(step_text).collect())
                        .unwrap_or_default(),
                    file: file.clone(),
                    id,
                });
            }
        }
    }
    out
}

pub fn find_story(roots: &[PathBuf], id: &str) -> Option<Story> {
    all_stories(roots).into_iter().find(|s| s.id == id)
}

/// The story for the in-tool reader, with the criteria it demonstrates.
pub fn story_markdown(story: &Story, criteria: &[(usize, String)]) -> String {
    let mut md = format!("# {}\n\n", story.title);
    md.push_str(&format!("story `{}`", story.id));
    if !story.actor.is_empty() {
        md.push_str(&format!(" · actor **{}**", story.actor));
    }
    if !story.host.is_empty() {
        md.push_str(&format!(" · host `{}`", story.host));
    }
    md.push_str("\n\n");
    if !criteria.is_empty() {
        md.push_str("## Shows these acceptance criteria\n\n");
        for (n, c) in criteria {
            md.push_str(&format!("{n}. {c}\n"));
        }
        md.push('\n');
    }
    if !story.depends_on.is_empty() {
        md.push_str(&format!("Runs after: {}\n\n", story.depends_on.join(", ")));
    }
    if !story.tags.is_empty() {
        md.push_str(&format!("Tags: {}\n\n", story.tags.join(", ")));
    }
    if !story.steps.is_empty() {
        md.push_str("## Steps\n\n");
        for s in &story.steps {
            md.push_str(&format!("- {s}\n"));
        }
        md.push('\n');
    }
    md.push_str("`o` plays the recorded video, or opens the demos site at this story (`make demos-open` in app-demos).\n");
    md
}

/// A story id that can safely name a folder and a URL fragment: letters,
/// digits, `.`, `_` and `-`, not starting with `.` or `-`.
pub fn safe_story_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// The newest recorded take (`.webm` / `.mp4`) under `app-demos/videos/<id>/`.
/// Only plain files count: a symlink is never returned (the result is handed
/// to the system opener), and the file must sit inside the story's folder.
pub fn video_for(roots: &[PathBuf], id: &str) -> Option<PathBuf> {
    if !safe_story_id(id) {
        return None;
    }
    let mut takes: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for root in roots {
        let dir = root.join("app-demos/videos").join(id);
        let Ok(canon_dir) = std::fs::canonicalize(&dir) else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if !matches!(p.extension().and_then(|x| x.to_str()), Some("webm" | "mp4")) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if !meta.file_type().is_file() {
                continue;
            }
            let inside = std::fs::canonicalize(&p).is_ok_and(|c| c.starts_with(&canon_dir));
            if let (true, Ok(modified)) = (inside, meta.modified()) {
                takes.push((modified, p));
            }
        }
    }
    takes.into_iter().max_by_key(|(t, _)| *t).map(|(_, p)| p)
}

/// Whether the demos page (`make demos-open`, which listens on 127.0.0.1)
/// answers on `port`.
pub fn site_running(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(200)).is_ok()
}

/// The demos site (`make demos-open`, on `port`), opened at one story.
pub fn site_url(port: u16, id: &str) -> String {
    format!("http://localhost:{port}/#{id}")
}

/// Stories in `repo` whose title shares at least two significant words with
/// the intent's title, minus those already linked (`already` holds the
/// targets of the intent's `demo:` lines).
pub fn discover_demos(repo: &Path, title: &str, already: &[String]) -> Vec<Artifact> {
    let wanted = significant_words(title);
    if wanted.is_empty() {
        return Vec::new();
    }
    let needed = wanted.len().min(2);
    let linked: Vec<String> = already.iter().map(|a| demo_target(a).0).collect();
    all_stories(&[repo.to_path_buf()])
        .into_iter()
        .filter(|s| !linked.contains(&s.id))
        .filter(|s| {
            let id = significant_words(&s.id.replace('-', " "));
            let words = significant_words(&format!("{} {}", s.title, s.id.replace('-', " ")));
            let hits: Vec<&String> = wanted
                .iter()
                .filter(|w| words.iter().any(|h| word_matches(w, h)))
                .collect();
            // the id must share a word too, as a file name must
            hits.len() >= needed && hits.iter().any(|w| id.iter().any(|i| word_matches(w, i)))
        })
        .map(|s| Artifact {
            kind: "demo".into(),
            target: s.id,
        })
        .collect()
}
