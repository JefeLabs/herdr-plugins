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
pub fn render_argv(
    spec: &ProviderSpec,
    prompt: &str,
    conv: Option<&str>,
    outfile: &Path,
) -> Vec<String> {
    let (template, conv_value) = match conv {
        Some(id) if !spec.resume.is_empty() => (&spec.resume, id.to_string()),
        _ => (&spec.polish, uuid::Uuid::new_v4().to_string()),
    };
    template
        .iter()
        .map(|a| {
            a.replace("{prompt}", prompt)
                .replace("{conv}", &conv_value)
                .replace("{outfile}", &outfile.display().to_string())
        })
        .collect()
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
}

/// Find the fenced ```proposal block and the `questions:` list.
pub fn parse_reply(text: &str) -> Reply {
    let mut reply = Reply::default();
    let mut prose = String::new();
    let mut proposal = String::new();
    let mut in_block = false;
    let mut in_questions = false;
    let mut saw_block = false;
    for line in text.lines() {
        let t = line.trim_end();
        if !in_block && t.trim_start().starts_with("```proposal") {
            in_block = true;
            saw_block = true;
            in_questions = false;
            continue;
        }
        if in_block {
            if t.trim_start() == "```" {
                in_block = false;
            } else {
                proposal.push_str(line);
                proposal.push('\n');
            }
            continue;
        }
        let lower = t.trim().to_ascii_lowercase();
        if lower.starts_with("questions:") {
            in_questions = true;
            let rest = t.trim()[10..].trim();
            if !rest.is_empty() && rest != "none" {
                reply.questions.push(rest.to_string());
            }
            continue;
        }
        if in_questions {
            if let Some(q) = t.trim_start().strip_prefix("- ") {
                reply.questions.push(q.trim().to_string());
                continue;
            }
            if t.trim().is_empty() {
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
