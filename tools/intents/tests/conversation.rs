use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use herdr_intents::config::{Output, ProviderSpec, When};
use herdr_intents::conversation::{
    FORMAT_REMINDER, HAPPY_PATH_RULE, POLISH_RULE, Turn, TurnMsg, follow_up_prompt, seed_prompt,
};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// A fake provider that replays the agy fixture.
fn spec() -> ProviderSpec {
    ProviderSpec {
        name: "fake".into(),
        polish: vec![
            fixtures().join("fake-provider.sh").display().to_string(),
            "--prompt={prompt}".into(),
        ],
        resume: vec![
            fixtures().join("fake-provider.sh").display().to_string(),
            "--resume={conv}".into(),
            "--prompt={prompt}".into(),
        ],
        output: Output::Jsonl {
            type_key: "event".into(),
            text_path: "/step_update/text_delta".into(),
            text_types: vec!["step_update".into()],
            text_when: Some(When {
                path: "/step_update/step_type".into(),
                value: "agent_response".into(),
            }),
            id_paths: vec![
                "/conversation_id".into(),
                "/step_update/conversation_id".into(),
            ],
            done_type: Some("result".into()),
        },
        send_kind: "claude".into(),
        extra: Vec::new(),
        models: Vec::new(),
        model_flag: "--model".into(),
    }
}

fn collect(turn: &Turn, limit: Duration) -> Vec<TurnMsg> {
    let start = Instant::now();
    let mut out = Vec::new();
    while start.elapsed() < limit {
        if let Some(m) = turn.try_recv() {
            let finished = matches!(m, TurnMsg::Finished(_) | TurnMsg::Failed(_));
            out.push(m);
            if finished {
                break;
            }
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    out
}

fn env(events: Option<&str>, mode: Option<&str>) -> Vec<(String, String)> {
    let mut v = Vec::new();
    if let Some(e) = events {
        v.push((
            "FAKE_EVENTS".into(),
            fixtures().join("events").join(e).display().to_string(),
        ));
    }
    if let Some(m) = mode {
        v.push(("FAKE_MODE".into(), m.into()));
    }
    v
}

#[test]
fn turn_streams_deltas_then_finished_with_reply() {
    let tmp = tempfile::tempdir().unwrap();
    let turn = Turn::start(
        &spec(),
        "polish this",
        None,
        tmp.path(),
        &env(Some("agy.jsonl"), None),
    )
    .unwrap();
    let msgs = collect(&turn, Duration::from_secs(5));
    let deltas: String = msgs
        .iter()
        .filter_map(|m| {
            if let TurnMsg::Delta(d) = m {
                Some(d.as_str())
            } else {
                None
            }
        })
        .collect();
    assert!(
        deltas.starts_with("Here is a tighter version."),
        "{deltas:?}"
    );
    let Some(TurnMsg::Finished(reply)) = msgs.last() else {
        panic!("last message: {:?}", msgs.last())
    };
    assert!(reply.proposal.as_deref().unwrap().contains("## Observed"));
    assert_eq!(reply.questions.len(), 2);
}

#[test]
fn turn_reports_conversation_id_once() {
    let tmp = tempfile::tempdir().unwrap();
    let turn = Turn::start(
        &spec(),
        "x",
        None,
        tmp.path(),
        &env(Some("agy.jsonl"), None),
    )
    .unwrap();
    let msgs = collect(&turn, Duration::from_secs(5));
    let ids: Vec<&String> = msgs
        .iter()
        .filter_map(|m| {
            if let TurnMsg::ConversationId(i) = m {
                Some(i)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        ids,
        [&"dde502fd-bcf1-4bfb-a275-4e6de5ad9a7d".to_string()],
        "exactly one id message"
    );
}

#[test]
fn resume_uses_the_resume_argv() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("argv.log");
    let mut e = env(Some("agy.jsonl"), None);
    e.push(("FAKE_PROVIDER_LOG".into(), log.display().to_string()));
    let turn = Turn::start(&spec(), "again", Some("conv-9"), tmp.path(), &e).unwrap();
    let _ = collect(&turn, Duration::from_secs(5));
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(logged.contains("--resume=conv-9"), "{logged}");
}

#[test]
fn a_provider_that_lingers_after_its_result_still_finishes_the_turn() {
    // agy can print its result and then never exit; the result ends the turn
    let tmp = tempfile::tempdir().unwrap();
    let turn = Turn::start(
        &spec(),
        "x",
        None,
        tmp.path(),
        &env(Some("agy.jsonl"), Some("hang")),
    )
    .unwrap();
    let pid = turn.pid().unwrap();
    let msgs = collect(&turn, Duration::from_secs(3));
    assert!(
        matches!(msgs.last(), Some(TurnMsg::Finished(r)) if r.proposal.is_some()),
        "{:?}",
        msgs.last()
    );
    // and the lingering process is ended
    let start = Instant::now();
    while nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok() {
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "provider still alive"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn cancel_kills_a_hanging_provider_within_a_second() {
    let tmp = tempfile::tempdir().unwrap();
    let mut turn = Turn::start(
        &spec(),
        "x",
        None,
        tmp.path(),
        &env(Some("agy.jsonl"), Some("midturn")),
    )
    .unwrap();
    let _ = collect(&turn, Duration::from_millis(400)); // deltas arrive, no Finished (mid-turn)
    let pid = turn.pid().unwrap();
    let t0 = Instant::now();
    turn.cancel();
    assert!(t0.elapsed() < Duration::from_secs(1));
    let start = Instant::now();
    while nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok() {
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "provider still alive"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let msgs = collect(&turn, Duration::from_millis(500));
    assert!(
        matches!(msgs.last(), Some(TurnMsg::Failed(m)) if m.contains("cancel")),
        "{:?}",
        msgs.last()
    );
}

#[test]
fn garbage_lines_become_text_not_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let turn = Turn::start(
        &spec(),
        "x",
        None,
        tmp.path(),
        &env(Some("agy.jsonl"), Some("garbage")),
    )
    .unwrap();
    let msgs = collect(&turn, Duration::from_secs(5));
    assert!(
        msgs.iter()
            .any(|m| matches!(m, TurnMsg::Delta(d) if d.contains("warming up")))
    );
    assert!(matches!(msgs.last(), Some(TurnMsg::Finished(_))));
}

#[test]
fn seed_prompt_includes_body_and_focus_line_only_with_selection() {
    let t = "ROLE\n\nKind: issue\n\n{contract}";
    let p = seed_prompt(t, "## Observed\nx\n", None, "issue");
    assert!(p.contains("ROLE") && p.contains("## Observed\nx"));
    assert!(!p.to_lowercase().contains("focus on"));
    let p = seed_prompt(t, "## Observed\nx\n", Some("x"), "issue");
    assert!(p.to_lowercase().contains("focus on"), "{p}");
    assert!(p.contains("\nx\n") || p.contains("> x"));
}

// I9: one invalid UTF-8 line must not end the turn
#[test]
fn invalid_utf8_line_does_not_kill_the_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let events = tmp.path().join("bad.jsonl");
    let mut bytes = std::fs::read(fixtures().join("events/agy.jsonl")).unwrap();
    let mut with_bad = b"warming up \xff\xfe not utf8\n".to_vec();
    with_bad.append(&mut bytes);
    std::fs::write(&events, with_bad).unwrap();
    let turn = Turn::start(
        &spec(),
        "x",
        None,
        tmp.path(),
        &[("FAKE_EVENTS".to_string(), events.display().to_string())],
    )
    .unwrap();
    let msgs = collect(&turn, Duration::from_secs(5));
    assert!(
        matches!(msgs.last(), Some(TurnMsg::Finished(r)) if r.proposal.is_some()),
        "{:?}",
        msgs.last()
    );
}

// I8: with no conversation id (text providers, cancelled first turn) the
// follow-up must carry the seed and the transcript, not just the new message
#[test]
fn follow_up_prompt_without_id_carries_seed_and_transcript() {
    let p = follow_up_prompt(
        "SEED",
        &[
            ("user".to_string(), "polish".to_string()),
            ("agent".to_string(), "reply one".to_string()),
        ],
        "shorter please",
        false,
    );
    assert!(
        p.contains("SEED")
            && p.contains("reply one")
            && p.contains(
                "shorter please
"
            ),
        "{p}"
    );
    let p = follow_up_prompt(
        "SEED",
        &[("user".to_string(), "polish".to_string())],
        "shorter please",
        true,
    );
    assert!(
        p.starts_with("shorter please") && !p.contains("SEED"),
        "with a resumable conversation the seed and transcript are not re-sent: {p}"
    );
}

// a provider that dies by signal on its own is not "cancelled"
#[test]
fn provider_killed_by_signal_is_reported_as_such_not_as_cancelled() {
    let tmp = tempfile::tempdir().unwrap();
    let turn = Turn::start(&spec(), "x", None, tmp.path(), &env(None, Some("selfkill"))).unwrap();
    let msgs = collect(&turn, Duration::from_secs(5));
    match msgs.last() {
        Some(TurnMsg::Failed(m)) => assert!(m.contains("signal") && !m.contains("cancel"), "{m}"),
        other => panic!("{other:?}"),
    }
}

// every turn restates the reply format, so a conversation that began under an
// older template still comes back with Preconditions and Acceptance criteria
#[test]
fn follow_up_prompts_always_carry_the_format_reminder() {
    let resumed = follow_up_prompt("SEED", &[], "shorter please", true);
    assert!(resumed.starts_with("shorter please"), "{resumed}");
    for h in [
        "bug:",
        "refactor:",
        "feature:",
        "idea:",
        "## Preconditions",
        "## Acceptance criteria",
        "## Open questions",
        "```proposal",
    ] {
        assert!(resumed.contains(h), "resumed turn lacks {h}: {resumed}");
    }
    let fresh = follow_up_prompt("SEED", &[("user".into(), "polish".into())], "again", false);
    assert!(
        fresh.contains("SEED") && fresh.contains("## Preconditions"),
        "{fresh}"
    );
}

#[test]
fn every_turn_carries_the_happy_path_rule() {
    assert_eq!(
        HAPPY_PATH_RULE,
        "Open questions are only what you need to know to build the main flow: who, what, where, and what success looks like. Do not ask about edge cases or failure modes; those are handled after hand-off."
    );
    assert!(FORMAT_REMINDER.contains(HAPPY_PATH_RULE));
    assert!(seed_prompt("template", "body", None, "idea").contains(HAPPY_PATH_RULE));
    for t in [
        include_str!("../defaults/idea.polish.md"),
        include_str!("../defaults/issue.polish.md"),
        include_str!("../defaults/enhancement.polish.md"),
    ] {
        assert!(t.contains(HAPPY_PATH_RULE), "bundled polish template: {t}");
    }
}

#[test]
fn every_turn_asks_for_criteria_grouped_by_area() {
    let rule = "group `## Acceptance criteria` under `### <area path>` sub-headings, one per area (its own group first), keeping every criterion's wording and checkbox";
    assert!(FORMAT_REMINDER.contains(rule), "{FORMAT_REMINDER}");
    assert!(seed_prompt("template", "body", None, "idea").contains(rule));
    assert!(
        follow_up_prompt("SEED", &[("user".into(), "polish".into())], "again", true).contains(rule)
    );
}

#[test]
fn a_follow_up_carries_the_format_reminder_once() {
    let seed = seed_prompt("template", "## Description\nbody\n", None, "idea");
    assert_eq!(seed.matches(FORMAT_REMINDER).count(), 1);
    let transcript = [
        ("user".to_string(), "polish".to_string()),
        ("agent".to_string(), "a reply".to_string()),
    ];
    let p = follow_up_prompt(&seed, &transcript, "again", false);
    assert_eq!(p.matches(FORMAT_REMINDER).count(), 1, "{p}");
    assert!(
        p.find(FORMAT_REMINDER) > p.rfind("again"),
        "the reminder comes after the new message"
    );
    assert!(
        p.trim_end().ends_with(POLISH_RULE),
        "then the polish rule, last"
    );
    let resumed = follow_up_prompt(&seed, &transcript, "again", true);
    assert_eq!(resumed.matches(FORMAT_REMINDER).count(), 1);
}

#[test]
fn every_turn_asks_to_polish_what_the_author_wrote_answers_included() {
    assert_eq!(
        POLISH_RULE,
        "Polish everything the author wrote, answers included: fix spelling, grammar and unclear wording in the description, preconditions, acceptance criteria, each open question and each `→ answer`, keeping its meaning and every decision in it. Keep every answered question with its polished `→ answer`, and do not re-ask it."
    );
    // polish turns carry it; the shared format reminder does not, so a
    // reconcile (which must copy criteria word for word) never polishes
    assert!(!FORMAT_REMINDER.contains(POLISH_RULE));
    let seed = seed_prompt("template", "body", None, "idea");
    assert_eq!(seed.matches(POLISH_RULE).count(), 1);
    let transcript = [("user".to_string(), "polish".to_string())];
    for resumable in [true, false] {
        let p = follow_up_prompt(&seed, &transcript, "again", resumable);
        assert_eq!(p.matches(POLISH_RULE).count(), 1, "{p}");
        assert_eq!(p.matches(FORMAT_REMINDER).count(), 1, "{p}");
    }
    for t in [
        include_str!("../defaults/idea.polish.md"),
        include_str!("../defaults/issue.polish.md"),
        include_str!("../defaults/enhancement.polish.md"),
    ] {
        assert!(t.contains(POLISH_RULE), "bundled polish template: {t}");
        assert!(
            !t.contains("with its answer, and do not re-ask it"),
            "the old keep-as-is rule is gone: {t}"
        );
    }
}

// every turn lets the agent say what the intent is, and ask when it cannot tell
#[test]
fn every_turn_offers_bug_feature_or_idea_and_asks_when_unsure() {
    let resumed = follow_up_prompt("SEED", &[], "again", true);
    for h in ["`bug:", "`feature:", "`idea:", "Is this a bug"] {
        assert!(resumed.contains(h), "turn lacks {h}: {resumed}");
    }
}
