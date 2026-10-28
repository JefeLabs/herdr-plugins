//! Adding an intent by conversation: the owner picks a type and says it in
//! their own words; the agent restates it, asks up to five questions one at a
//! time, then writes the title and the intent up. This module holds the pure
//! part: what each turn's prompt says and how a reply is read. The screen that
//! drives it lives in `app.rs`.

use crate::config::kind_label;
use crate::conversation::{HAPPY_PATH_RULE, POLISH_RULE};

pub const MAX_QUESTIONS: usize = 5;

/// Appears only in write-up prompts; tests look for it in the provider log.
pub const WRITE_UP_MARKER: &str = "Write the intent up now.";

/// What an agent turn is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Restate,
    Ask,
    WriteUp,
}

/// An ask turn's reply: one question, or "I know enough".
#[derive(Debug, PartialEq, Eq)]
pub enum Asked {
    Question(String),
    Enough,
}

/// A question the agent asked and the owner's answer (`None` = skipped).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Qa {
    pub question: String,
    pub answer: Option<String>,
}

pub struct Interview {
    /// Stored form: issue | refactor | enhancement | idea.
    pub kind: String,
    /// The group path under the root the intent is filed in, when known.
    pub home: Option<String>,
    /// Existing group paths in the workspace's tree.
    pub groups: Vec<String>,
    /// The kind's interview template.
    pub guide: String,
    pub words: String,
    pub corrections: Vec<String>,
    pub qas: Vec<Qa>,
    /// What the owner asked to change at the write-up, in order.
    pub refinements: Vec<String>,
}

impl Interview {
    pub fn new(kind: &str, home: Option<String>, groups: Vec<String>, guide: String) -> Self {
        Interview {
            kind: kind.to_string(),
            home,
            groups,
            guide,
            words: String::new(),
            corrections: Vec::new(),
            qas: Vec::new(),
            refinements: Vec::new(),
        }
    }

    /// Questions the agent may still ask.
    pub fn budget_left(&self) -> usize {
        MAX_QUESTIONS.saturating_sub(self.qas.len())
    }

    fn home_line(&self) -> String {
        format!("Home group: {}", self.home.as_deref().unwrap_or("unknown"))
    }

    fn groups_block(&self) -> String {
        let mut b = String::from("Existing groups:\n");
        for g in &self.groups {
            b.push_str(&format!("- {g}\n"));
        }
        b
    }

    fn step_rule(&self, step: Step) -> String {
        match step {
            Step::Restate => "Do not ask any questions yet. Reply with one line `summary: <the intent in one sentence>`, then one short paragraph restating what the author said, polished: fix spelling, grammar and unclear wording, keep every fact and decision, add nothing. End with: Is that right?".to_string(),
            Step::Ask => format!(
                "This is question {n} of at most {MAX_QUESTIONS}. Ask the single most useful next question, in one or two sentences, on one line that starts `question: `. Ask one question only. When you already know enough to write the intent up, reply with the single word `enough` instead.\n{HAPPY_PATH_RULE}\nWhen the home group is unknown, or you cannot tell which of the existing groups the work touches, one question may ask that, offering the likely groups from the list.",
                n = self.qas.len() + 1
            ),
            Step::WriteUp => {
                let keep = match &self.home {
                    Some(h) => format!(" It is filed in `{h}`; keep that unless the author asked to move it."),
                    None => String::new(),
                };
                format!(
                    "{WRITE_UP_MARKER} Start with one line that says what this is, judged from the conversation: `bug: <what goes wrong> when <condition>`, `refactor: <what changes> in <where>`, `feature: <who> can <what>` or `idea: <the idea>`. Then one line `group: <path>` naming the group this intent belongs in, one of the existing groups.{keep} Then a fenced ```proposal block with the complete intent body: `## Description`, `## Preconditions` (`- [ ]` items), `## Acceptance criteria` (`- [ ]` checks: the criteria for its own group first, ungrouped, then one `### <group path>` sub-heading per other existing group the work touches, each with its own criteria), and `## Open questions` (each answered question with its `→ answer` on an indented line below it; each skipped question left open, without an answer). Then `questions:` with `- ` items for anything still unknown, or `questions: none`.\n{POLISH_RULE}"
                )
            }
        }
    }

    /// Everything so far, for a provider that cannot resume (or a first turn).
    fn context(&self, step: Step) -> String {
        let mut p = String::new();
        p.push_str(self.guide.trim_end());
        p.push_str("\n\n");
        p.push_str(&format!("Kind: {}\n", kind_label(&self.kind)));
        p.push_str(&self.home_line());
        p.push('\n');
        if step != Step::Restate {
            p.push_str(&self.groups_block());
        }
        p.push_str("\nThe author's words:\n");
        p.push_str(self.words.trim_end());
        p.push('\n');
        if !self.corrections.is_empty() {
            p.push_str("\nCorrections from the author:\n");
            for c in &self.corrections {
                p.push_str(&format!("- {}\n", c.trim()));
            }
        }
        if !self.refinements.is_empty() {
            p.push_str("\nChanges the author asked for at the write-up:\n");
            for r in &self.refinements {
                p.push_str(&format!("- {}\n", r.trim()));
            }
        }
        if !self.qas.is_empty() {
            p.push_str("\nQuestions so far:\n");
            for qa in &self.qas {
                p.push_str(&format!(
                    "- {}\n  → {}\n",
                    qa.question.trim(),
                    qa.answer.as_deref().map(str::trim).unwrap_or("(skipped)")
                ));
            }
        }
        p
    }

    /// The prompt for one turn. `message` is the owner's latest input (the
    /// words, a correction, an answer, `(skipped)`, a refinement, or empty);
    /// it must already be recorded in `self`: a resumed prompt sends only
    /// `message`, a full one only what `self` holds.
    pub fn prompt(&self, step: Step, resumable: bool, message: &str) -> String {
        let first = step == Step::Restate && self.corrections.is_empty();
        // everything the owner said is recorded in `self`, so a full prompt
        // never needs `message` on top
        let mut p = if !resumable || first {
            self.context(step)
        } else {
            let mut p = String::new();
            if !message.trim().is_empty() {
                p.push_str(message.trim());
                p.push_str("\n\n");
            }
            if step != Step::Restate {
                p.push_str(&self.home_line());
                p.push('\n');
                p.push_str(&self.groups_block());
            }
            p
        };
        p.push('\n');
        p.push_str(&self.step_rule(step));
        p.push('\n');
        p
    }

    /// A title for a raw save: the words' first line, cut at a word.
    pub fn raw_title(&self) -> String {
        const MAX: usize = 80;
        let line = self
            .words
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("");
        if line.chars().count() <= MAX {
            return line.to_string();
        }
        let head: String = line.chars().take(MAX).collect();
        let cut = match head.rfind(char::is_whitespace) {
            Some(i) if i > 0 => &head[..i],
            _ => head.as_str(),
        };
        format!("{}…", cut.trim_end())
    }

    /// A body for a raw save: everything typed, answers under Open questions.
    pub fn raw_body(&self) -> String {
        let mut b = String::from("## Description\n\n");
        b.push_str(self.words.trim());
        b.push('\n');
        for c in self.corrections.iter().chain(&self.refinements) {
            b.push('\n');
            b.push_str(c.trim());
            b.push('\n');
        }
        b.push_str("\n## Open questions\n\n");
        for qa in &self.qas {
            b.push_str(&format!("- {}\n", qa.question.trim()));
            if let Some(a) = qa
                .answer
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
            {
                b.push_str(&format!("  → {a}\n"));
            }
        }
        b
    }

    /// Where to file the intent: the reply's group when it names an existing
    /// one, else the home group.
    pub fn save_group(&self, reply_group: Option<&str>) -> Option<String> {
        let named = reply_group
            .map(|g| {
                g.trim()
                    .trim_matches(['`', '"', '\''])
                    .trim_matches('/')
                    .to_string()
            })
            .filter(|g| self.groups.iter().any(|x| x == g));
        named.or_else(|| self.home.clone())
    }
}

/// Read an ask turn's reply: the first `question:` line, or `enough`.
pub fn parse_ask(text: &str) -> Asked {
    for line in text.lines() {
        let t = line.trim().trim_matches(['*', '_']).trim();
        let lower = t.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("question:") {
            let q = t[t.len() - rest.len()..].trim();
            if !q.is_empty() {
                return Asked::Question(q.to_string());
            }
        }
        if lower.trim_end_matches(['.', '!']) == "enough" {
            return Asked::Enough;
        }
    }
    Asked::Question(text.trim().to_string())
}

/// What the add screen is waiting for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Awaiting {
    /// The owner's own words.
    Words,
    /// The agent's turn is running.
    Turn,
    /// "Is that right?" after a restatement.
    Confirm,
    /// An answer to this question.
    Answer(String),
    /// The write-up is on screen.
    Review,
    /// The last turn failed, for this reason.
    Retry(String),
}

impl Awaiting {
    pub fn name(&self) -> &'static str {
        match self {
            Awaiting::Words => "words",
            Awaiting::Turn => "turn",
            Awaiting::Confirm => "confirm",
            Awaiting::Answer(_) => "answer",
            Awaiting::Review => "review",
            Awaiting::Retry(_) => "retry",
        }
    }
}

/// The add screen's state: the interview plus the turn machinery around it.
pub struct InterviewScreen {
    pub iv: Interview,
    pub awaiting: Awaiting,
    /// The running or last turn's step.
    pub step: Step,
    /// The input that turn was sent with (a retry re-sends it).
    pub message: String,
    pub turn: Option<crate::conversation::Turn>,
    pub transcript: Vec<(String, String)>,
    pub input: String,
    pub provider: String,
    pub model: Option<String>,
    pub conv_id: Option<String>,
    /// `<root>/.polish/new-xxxxxxxx`, where the agent runs.
    pub folder: std::path::PathBuf,
    /// Where `a` would add today: the group a save falls back to.
    pub fallback: std::path::PathBuf,
    pub write_up: Option<crate::provider::Reply>,
    pub save_kind: String,
    pub scroll: u16,
}

/// A fresh folder name under `<root>/.polish/` for one conversation.
pub fn polish_folder_name() -> String {
    let id: String = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect();
    format!("new-{id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GROUPS: &[&str] = &[
        "skoolscout",
        "skoolscout/global/notifications",
        "skoolscout/discover-schools/org/team",
    ];

    fn interview(home: Option<&str>) -> Interview {
        let mut iv = Interview::new(
            "issue",
            home.map(str::to_string),
            GROUPS.iter().map(|g| g.to_string()).collect(),
            "GUIDE: find out what happens and what was expected.".into(),
        );
        iv.words = "tenant admins getting notifications when a user accepts a team invite".into();
        iv
    }

    fn qa(q: &str, a: Option<&str>) -> Qa {
        Qa {
            question: q.into(),
            answer: a.map(str::to_string),
        }
    }

    #[test]
    fn the_first_prompt_carries_the_guide_the_words_and_the_restate_rule() {
        let iv = interview(Some("skoolscout"));
        let p = iv.prompt(Step::Restate, true, &iv.words.clone());
        assert!(p.contains("GUIDE: find out"), "{p}");
        assert!(p.contains("tenant admins getting notifications"), "{p}");
        assert!(p.contains("summary:"), "{p}");
        assert!(!p.contains("question:"), "{p}");
        assert!(!p.contains(WRITE_UP_MARKER), "{p}");
    }

    #[test]
    fn ask_prompts_count_down_and_carry_the_groups() {
        let mut iv = interview(Some("skoolscout"));
        iv.qas = vec![qa("Q1?", Some("a1")), qa("Q2?", Some("a2"))];
        let p = iv.prompt(Step::Ask, true, "a2");
        assert!(p.contains("question 3 of at most 5"), "{p}");
        assert!(p.contains("skoolscout/global/notifications"), "{p}");
        assert!(p.contains(HAPPY_PATH_RULE), "{p}");
        assert!(p.contains("enough"), "{p}");
    }

    #[test]
    fn an_unknown_home_group_is_named_as_unknown() {
        let p = interview(None).prompt(Step::Ask, true, "a1");
        assert!(p.contains("Home group: unknown"), "{p}");
        let p =
            interview(Some("skoolscout/discover-schools/org/team")).prompt(Step::Ask, true, "a1");
        assert!(
            p.contains("Home group: skoolscout/discover-schools/org/team"),
            "{p}"
        );
    }

    #[test]
    fn a_non_resumable_prompt_repeats_the_whole_conversation() {
        let mut iv = interview(Some("skoolscout"));
        iv.corrections = vec!["only the inviter should be told".into()];
        iv.qas = vec![qa("Q1?", Some("a1")), qa("Q2?", None)];
        let p = iv.prompt(Step::Ask, false, "");
        assert!(p.contains("tenant admins getting notifications"), "{p}");
        assert!(p.contains("only the inviter should be told"), "{p}");
        assert!(p.contains("Q1?") && p.contains("a1"), "{p}");
        let q2 = p.find("Q2?").expect("Q2 listed");
        assert!(p[q2..].contains("(skipped)"), "{p}");
    }

    #[test]
    fn the_write_up_prompt_asks_for_title_group_and_grouped_criteria() {
        let p = interview(None).prompt(Step::WriteUp, true, "");
        for needle in [
            WRITE_UP_MARKER,
            "group:",
            "```proposal",
            "### <group path>",
            "refactor:",
            POLISH_RULE,
        ] {
            assert!(p.contains(needle), "lacks {needle}: {p}");
        }
    }

    #[test]
    fn parse_ask_takes_one_question_or_enough() {
        assert_eq!(
            parse_ask("question: Which notification?\n"),
            Asked::Question("Which notification?".into())
        );
        assert_eq!(
            parse_ask("question: A?\nquestion: B?"),
            Asked::Question("A?".into())
        );
        assert_eq!(parse_ask("enough"), Asked::Enough);
        assert_eq!(parse_ask("Enough."), Asked::Enough);
        assert_eq!(
            parse_ask("Which one is it?"),
            Asked::Question("Which one is it?".into())
        );
    }

    #[test]
    fn budget_ends_at_five() {
        let mut iv = interview(None);
        iv.qas = (0..5).map(|i| qa(&format!("Q{i}?"), None)).collect();
        assert_eq!(iv.budget_left(), 0);
        iv.qas.push(qa("Q6?", None));
        assert_eq!(iv.budget_left(), 0);
        iv.qas.truncate(2);
        assert_eq!(iv.budget_left(), 3);
    }

    #[test]
    fn raw_title_is_the_first_line_cut_at_a_word() {
        let mut iv = interview(None);
        iv.words = "tenant admins getting notifications\nmore".into();
        assert_eq!(iv.raw_title(), "tenant admins getting notifications");
        iv.words = "word ".repeat(60);
        let t = iv.raw_title();
        assert!(t.chars().count() <= 81, "{t} ({})", t.chars().count());
        assert!(t.ends_with('…'), "{t}");
        assert!(
            t.trim_end_matches('…').ends_with("word"),
            "no half word: {t}"
        );
    }

    #[test]
    fn raw_body_keeps_words_and_answers() {
        let mut iv = interview(None);
        iv.qas = vec![qa("Which notification?", Some("both")), qa("Who?", None)];
        let b = iv.raw_body();
        assert!(b.contains("## Description"), "{b}");
        assert!(b.contains(&iv.words), "{b}");
        assert!(b.contains("## Open questions"), "{b}");
        assert!(b.contains("- Which notification?\n  → both"), "{b}");
        assert!(b.contains("- Who?\n") || b.ends_with("- Who?"), "{b}");
        assert!(
            !b.contains("- Who?\n  →"),
            "a skipped question has no answer: {b}"
        );
    }

    #[test]
    fn save_group_prefers_a_valid_reply_group() {
        let mut iv = interview(Some("skoolscout"));
        assert_eq!(
            iv.save_group(Some("skoolscout/global/notifications/"))
                .as_deref(),
            Some("skoolscout/global/notifications")
        );
        assert_eq!(iv.save_group(Some("nope")).as_deref(), Some("skoolscout"));
        assert_eq!(iv.save_group(None).as_deref(), Some("skoolscout"));
        iv.home = None;
        assert_eq!(iv.save_group(Some("nope")), None);
    }

    #[test]
    fn polish_folder_names_are_new_and_unique() {
        let a = polish_folder_name();
        let b = polish_folder_name();
        assert!(a.starts_with("new-"), "{a}");
        assert_eq!(a.len(), 12, "{a}");
        assert_ne!(a, b);
    }

    #[test]
    fn refinements_are_kept_for_an_agent_that_cannot_resume() {
        let mut iv = interview(Some("skoolscout"));
        iv.qas = vec![qa("Q1?", Some("a5"))];
        iv.refinements = vec![
            "file it under skoolscout/global".into(),
            "shorten it".into(),
        ];
        let p = iv.prompt(Step::WriteUp, false, "shorten it");
        assert!(p.contains("file it under skoolscout/global"), "{p}");
        assert_eq!(p.matches("shorten it").count(), 1, "{p}");
        assert!(
            !p.contains("The author adds"),
            "the last answer is not re-sent: {p}"
        );
        let b = iv.raw_body();
        assert!(b.contains("shorten it"), "{b}");
    }

    #[test]
    fn kind_labels_name_the_type_in_the_prompt() {
        let mut iv = interview(None);
        iv.kind = "enhancement".into();
        assert!(
            iv.prompt(Step::Restate, false, "")
                .contains(&format!("Kind: {}", kind_label("enhancement")))
        );
    }
}
