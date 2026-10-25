use std::path::Path;

use herdr_capabilities::config::builtin_providers;
use herdr_capabilities::provider::{Event, parse_line, parse_reply, render_argv};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/events/{name}.jsonl")),
    )
    .unwrap()
}

fn drive(name: &str) -> (String, Option<String>, bool) {
    let spec = &builtin_providers()[name];
    let mut text = String::new();
    let mut id = None;
    let mut done = false;
    for line in fixture(name).lines() {
        match parse_line(spec, line) {
            Event::Text(t) => text.push_str(&t),
            Event::ConversationId(i) => id = Some(i),
            Event::Done => done = true,
            Event::Error(e) => panic!("error event: {e}"),
            Event::Other => {}
        }
    }
    (text, id, done)
}

#[test]
fn render_argv_substitutes_placeholders_for_each_builtin() {
    let specs = builtin_providers();
    let out = Path::new("/tmp/out.txt");
    for (name, spec) in &specs {
        let fresh = render_argv(spec, "hello world", None, out);
        assert!(
            fresh.iter().any(|a| a.contains("hello world")),
            "{name}: prompt missing in {fresh:?}"
        );
        assert!(
            !fresh.iter().any(|a| a.contains("{prompt}")),
            "{name}: raw placeholder left"
        );
        let resumed = render_argv(spec, "again", Some("conv-123"), out);
        assert!(
            resumed.iter().any(|a| a == "conv-123"),
            "{name}: conv id missing in {resumed:?}"
        );
        assert!(
            !resumed.iter().any(|a| a.contains("{conv}")),
            "{name}: raw conv placeholder left"
        );
    }
    // claude's fresh form carries a generated uuid in place of {conv}
    let fresh = render_argv(&specs["claude"], "x", None, out);
    let i = fresh.iter().position(|a| a == "--session-id").unwrap();
    assert_eq!(
        fresh[i + 1].len(),
        36,
        "uuid expected, got {}",
        fresh[i + 1]
    );
}

#[test]
fn parse_agy_stream_yields_text_id_and_done() {
    let (text, id, done) = drive("agy");
    assert!(text.starts_with("Here is a tighter version."), "{text:?}");
    assert!(text.contains("```proposal"));
    assert_eq!(id.as_deref(), Some("dde502fd-bcf1-4bfb-a275-4e6de5ad9a7d"));
    assert!(done);
}

#[test]
fn parse_claude_stream_yields_text_id_and_done() {
    let (text, id, done) = drive("claude");
    assert!(text.starts_with("Here is a tighter version."), "{text:?}");
    assert_eq!(id.as_deref(), Some("d9f0ccda-7d43-4480-88eb-bd90426c0c1a"));
    assert!(done);
}

#[test]
fn parse_codex_stream_yields_text_id_and_done() {
    let (text, id, done) = drive("codex");
    assert!(text.starts_with("Here is a tighter version."), "{text:?}");
    assert_eq!(id.as_deref(), Some("019a0c1e-aaaa-7000-8000-000000000001"));
    assert!(done);
}

#[test]
fn parse_copilot_stream_yields_text_id_and_done() {
    let (text, id, done) = drive("copilot");
    assert!(text.starts_with("Here is a tighter version."), "{text:?}");
    assert_eq!(id.as_deref(), Some("cp-sess-1"));
    assert!(done);
}

#[test]
fn deltas_are_not_double_counted_with_the_final_message() {
    // codex/copilot/agy all repeat the full text in a final event; text must appear once
    for name in ["agy", "claude", "codex", "copilot"] {
        let (text, _, _) = drive(name);
        assert_eq!(
            text.matches("Here is a tighter version.").count(),
            1,
            "{name}: {text:?}"
        );
    }
}

#[test]
fn non_json_line_is_text_not_error() {
    let spec = &builtin_providers()["agy"];
    assert!(
        matches!(parse_line(spec, "warning: something odd"), Event::Text(t) if t == "warning: something odd\n")
    );
}

#[test]
fn parse_reply_extracts_proposal_and_questions() {
    let r = parse_reply(
        "Here is a tighter version.\n\n```proposal\n## Observed\nx\n```\nquestions:\n- Which org?\n- Since when?\n",
    );
    assert_eq!(r.prose.trim(), "Here is a tighter version.");
    assert_eq!(r.proposal.as_deref(), Some("## Observed\nx\n"));
    assert_eq!(r.questions, ["Which org?", "Since when?"]);
}

#[test]
fn parse_reply_without_block_has_no_proposal() {
    let r = parse_reply("Just chatting, no block here.");
    assert!(r.proposal.is_none());
    assert!(r.questions.is_empty());
    assert_eq!(r.prose, "Just chatting, no block here.");
}

#[test]
fn empty_proposal_block_is_none_and_questions_none_is_empty() {
    let r = parse_reply("```proposal\n\n```\nquestions: none\n");
    assert!(r.proposal.is_none());
    assert!(r.questions.is_empty());
}

// I3: a code block inside the proposal must not close it
#[test]
fn proposal_block_survives_nested_code_fences() {
    let text = "Intro\n\n````proposal\n## Steps\n```bash\nmake test\n```\n## Evidence\nlog\n````\nquestions: none\n";
    let r = parse_reply(text);
    assert_eq!(
        r.proposal.as_deref(),
        Some("## Steps\n```bash\nmake test\n```\n## Evidence\nlog\n")
    );
    // CommonMark: a bare fence shorter than the opener does not close it
    let text = "`````proposal\nA\n````inner\nx\n````\nB\n`````\n";
    let r = parse_reply(text);
    assert_eq!(r.proposal.as_deref(), Some("A\n````inner\nx\n````\nB\n"));
    // a plain three-backtick block with no nesting still works
    let r = parse_reply("```proposal\nonly\n```\n");
    assert_eq!(r.proposal.as_deref(), Some("only\n"));
}

#[test]
fn questions_header_variants_are_recognised() {
    let r = parse_reply("```proposal\nx\n```\n**Questions:**\n- One?\n");
    assert_eq!(r.questions, ["One?"]);
    let r = parse_reply("```proposal\nx\n```\nquestions: None\n");
    assert!(r.questions.is_empty(), "{:?}", r.questions);
}

// security: a conversation id that looks like an option must never reach argv
#[test]
fn hostile_conversation_id_falls_back_to_a_fresh_conversation() {
    let spec = &builtin_providers()["codex"];
    let out = Path::new("/tmp/o");
    for bad in ["--sandbox=danger-full-access", "-x", "a b", "id;rm", ""] {
        let argv = render_argv(spec, "p", Some(bad), out);
        assert!(
            !argv.iter().any(|a| a == bad),
            "{bad:?} leaked into {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a == "resume"),
            "{bad:?} must not resume: {argv:?}"
        );
    }
    let good = render_argv(spec, "p", Some("019a0c1e-aaaa-7000-8000-000000000001"), out);
    assert!(good.iter().any(|a| a == "resume"));
}

#[test]
fn extra_options_are_inserted_after_the_program() {
    let mut spec = builtin_providers()["codex"].clone();
    spec.extra = vec!["--model".into(), "m".into()];
    let argv = render_argv(&spec, "p", None, Path::new("/tmp/o"));
    assert_eq!(&argv[..3], ["codex", "--model", "m"]);
    let dd = argv.iter().position(|a| a == "--").unwrap();
    assert!(
        dd > 2 && argv[dd + 1] == "p",
        "prompt stays after `--`: {argv:?}"
    );
}

#[test]
fn reply_may_carry_a_capability_phrase_as_the_new_title() {
    let r = parse_reply(
        "capability: Families can skip a folder without seeing Pending\n\n```proposal\n## Description\nx\n```\nquestions: none\n",
    );
    assert_eq!(
        r.title.as_deref(),
        Some("Families can skip a folder without seeing Pending")
    );
    assert!(
        !r.prose.contains("capability:"),
        "the title line is not prose: {:?}",
        r.prose
    );
    let r = parse_reply("```proposal\nx\n```\n");
    assert!(r.title.is_none());
}
