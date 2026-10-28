//! Provider argv templates, JSONL event parsing and the proposal-block parser.

use std::path::Path;

use serde_json::Value;

use crate::config::{Output, ProviderSpec};

/// One parsed line of provider output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Text(String),
    ConversationId(String),
    Done,
    Error(String),
    Other,
}

/// Fill `{prompt}`, `{conv}` and `{outfile}`. With no conversation id the
/// `polish` argv is used and `{conv}` becomes a fresh uuid (claude sets its
/// own session id that way); with one, the `resume` argv is used.
/// A conversation id is safe to place in argv only if it is a plain token:
/// letters, digits, `-`, `_`, `.`, not starting with `-` (an option) and not
/// empty. Anything else falls back to a fresh conversation.
pub fn safe_conversation_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

pub fn render_argv(
    spec: &ProviderSpec,
    prompt: &str,
    conv: Option<&str>,
    outfile: &Path,
) -> Vec<String> {
    let (template, conv_value) = match conv {
        Some(id) if !spec.resume.is_empty() && safe_conversation_id(id) => {
            (&spec.resume, id.to_string())
        }
        _ => (&spec.polish, uuid::Uuid::new_v4().to_string()),
    };
    let mut argv: Vec<String> = template
        .iter()
        .map(|a| {
            a.replace("{prompt}", prompt)
                .replace("{conv}", &conv_value)
                .replace("{outfile}", &outfile.display().to_string())
        })
        .collect();
    // user tuning (`extra`, e.g. a model) goes right after the program, never after a `--`
    if !spec.extra.is_empty() && !argv.is_empty() {
        let tail = argv.split_off(1);
        argv.extend(spec.extra.iter().cloned());
        argv.extend(tail);
    }
    argv
}

/// Classify one line of stdout. Non-JSON lines are shown as text, never an error.
pub fn parse_line(spec: &ProviderSpec, line: &str) -> Event {
    let Output::Jsonl {
        type_key,
        text_path,
        text_types,
        text_when,
        id_paths,
        done_type,
    } = &spec.output
    else {
        return Event::Text(format!("{line}\n"));
    };
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Event::Other;
    }
    let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
        return Event::Text(format!("{line}\n"));
    };
    let kind = v.get(type_key).and_then(Value::as_str).unwrap_or("");

    if kind == "error" {
        let msg = v.get("message").and_then(Value::as_str).unwrap_or(trimmed);
        return Event::Error(msg.to_string());
    }
    if text_types.iter().any(|t| t == kind) {
        let guarded = match text_when {
            Some(w) => v.pointer(&w.path).and_then(Value::as_str) == Some(w.value.as_str()),
            None => true,
        };
        if guarded && let Some(t) = v.pointer(text_path).and_then(Value::as_str) {
            return Event::Text(t.to_string());
        }
    }
    if let Some(id) = id_paths
        .iter()
        .find_map(|p| v.pointer(p).and_then(Value::as_str))
    {
        // Ids repeat on many events; the caller de-duplicates. Prefer the id
        // event only when the line carries nothing else useful.
        if done_type.as_deref() != Some(kind) {
            return Event::ConversationId(id.to_string());
        }
    }
    if done_type.as_deref() == Some(kind) {
        return Event::Done;
    }
    Event::Other
}

/// What a reply contains, per the contract in the polish templates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reply {
    pub prose: String,
    pub proposal: Option<String>,
    pub questions: Vec<String>,
    /// A proposed title, from the reply's first line: `bug: …`,
    /// `capability: <who> can <what>` or `idea: …`.
    pub title: Option<String>,
    /// The kind that line names (`issue`, `refactor`, `enhancement` or `idea`).
    pub kind: Option<String>,
}

/// Find the fenced proposal block and the `questions:` list. Fences follow
/// CommonMark: the opener's backtick run length is remembered and only a bare
/// fence of at least that length closes the block, so code blocks inside the
/// proposal survive (templates ask for a four-backtick opener when needed).
pub fn parse_reply(text: &str) -> Reply {
    let mut reply = Reply::default();
    let mut prose = String::new();
    let mut proposal = String::new();
    let mut fence_len: Option<usize> = None;
    let mut in_questions = false;
    let mut saw_block = false;
    for line in text.lines() {
        let t = line.trim_end();
        let trimmed = t.trim_start();
        if let Some(len) = fence_len {
            let ticks = trimmed.chars().take_while(|c| *c == '`').count();
            if ticks >= len && trimmed[ticks..].trim().is_empty() {
                fence_len = None;
            } else {
                proposal.push_str(line);
                proposal.push('\n');
            }
            continue;
        }
        let ticks = trimmed.chars().take_while(|c| *c == '`').count();
        if ticks >= 3 && trimmed[ticks..].trim().eq_ignore_ascii_case("proposal") {
            if saw_block {
                proposal.clear(); // a later block replaces an earlier one
            }
            fence_len = Some(ticks);
            saw_block = true;
            in_questions = false;
            continue;
        }
        let header = trimmed
            .trim_matches(|c| c == '*' || c == '_')
            .trim()
            .to_ascii_lowercase();
        if let Some((name, _)) = header.split_once(':')
            && let Some(kind) = crate::config::kind_from_heading(name)
        {
            let original = trimmed.trim_matches(|c| c == '*' || c == '_').trim();
            let phrase = original[name.len() + 1..]
                .trim_start_matches(['*', '_'])
                .trim()
                .trim_matches('"')
                .trim();
            if !phrase.is_empty() {
                reply.title = Some(phrase.to_string());
                reply.kind = Some(kind.to_string());
            }
            continue;
        }
        if let Some(after) = header.strip_prefix("questions:") {
            in_questions = true;
            let rest = after.trim().to_string();
            if !rest.is_empty() && rest != "none" {
                let original = trimmed.trim_matches(|c| c == '*' || c == '_').trim();
                let idx = original
                    .to_ascii_lowercase()
                    .find("questions:")
                    .map(|i| i + "questions:".len())
                    .unwrap_or(0);
                reply.questions.push(original[idx..].trim().to_string());
            }
            continue;
        }
        if in_questions {
            if let Some(q) = trimmed.strip_prefix("- ") {
                reply.questions.push(q.trim().to_string());
                continue;
            }
            if trimmed.is_empty() {
                continue;
            }
            in_questions = false;
        }
        prose.push_str(line);
        prose.push('\n');
    }
    reply.prose = prose.trim_end().to_string();
    if saw_block && !proposal.trim().is_empty() {
        reply.proposal = Some(proposal);
    }
    reply
}
