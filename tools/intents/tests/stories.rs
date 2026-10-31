//! An intent's `## Stories` section: the stories its demo videos show.

use herdr_intents::reconcile::remap_shows;
use herdr_intents::store::{
    CRITERIA_HEADING, IntentStory, add_checklist_item, replace_stories, shown_criteria, stories,
    story_folder, unshown_criteria,
};

const BODY: &str = "## Description\nx\n\n## Acceptance criteria\n- [ ] an admin can reinstate a withdrawn application\n- [ ] the family is told\n- [ ] it shows under Submitted again\n\n## Stories\n### Mark reinstates Maya's withdrawn application\n- persona: Mark, a tenant admin\n- shows: 1, 3\n- starting data: Maya has a submitted application, then withdrawn\n1. Open Maya's application from Applications\n2. Choose Reinstate and confirm\n3. See the application back under Submitted\n";

#[test]
fn a_story_reads_its_four_parts() {
    let all = stories(BODY);
    assert_eq!(all.len(), 1);
    let s = &all[0];
    assert_eq!(
        (s.number, s.name.as_str()),
        (1, "Mark reinstates Maya's withdrawn application")
    );
    assert_eq!(s.persona.as_deref(), Some("Mark, a tenant admin"));
    assert_eq!(s.shows, vec![1, 3]);
    assert_eq!(
        s.starting_data.as_deref(),
        Some("Maya has a submitted application, then withdrawn")
    );
    assert_eq!(
        s.steps,
        vec![
            "Open Maya's application from Applications",
            "Choose Reinstate and confirm",
            "See the application back under Submitted"
        ]
    );
    assert!(!s.waits);
}

#[test]
fn a_story_waiting_for_answers_has_no_steps() {
    let b = "## Stories\n### X\n- persona: Mark\n- shows: 2\ndrafts wait for the open questions\n";
    let s = &stories(b)[0];
    assert!(s.waits);
    assert!(s.steps.is_empty());
}

#[test]
fn shows_beyond_the_criteria_are_dropped() {
    let b = "## Acceptance criteria\n- [ ] c one\n- [ ] c two\n- [ ] c three\n\n## Stories\n### A\n- shows: 1, 7\n";
    let s = &stories(b)[0];
    assert_eq!(
        shown_criteria(b, s),
        (vec![(1, "c one".to_string())], vec![7])
    );
}

#[test]
fn two_stories_with_one_name_get_their_own_folders() {
    let b = "## Stories\n### Mark reinstates Maya's withdrawn application\n- shows: 1\n### Mark reinstates Maya's withdrawn application\n- shows: 2\n";
    let all: Vec<IntentStory> = stories(b);
    assert_eq!(
        story_folder(&all[0]),
        "1-mark-reinstates-maya-s-withdrawn-application"
    );
    assert_ne!(story_folder(&all[0]), story_folder(&all[1]));
}

#[test]
fn criteria_no_story_shows_are_listed() {
    let b = "## Acceptance criteria\n- [ ] a\n- [ ] b\n- [ ] c\n- [ ] d\n\n## Stories\n### A\n- shows: 1\n### B\n- shows: 3\n";
    assert_eq!(unshown_criteria(b), vec![2, 4]);
}

#[test]
fn a_reconcile_that_renumbers_criteria_renumbers_shows() {
    let stories_text = "## Stories\n### A\n- shows: 1, 3\n";
    let before = format!("## Acceptance criteria\n- [ ] a\n- [ ] b\n- [ ] c\n\n{stories_text}");
    let after =
        format!("## Acceptance criteria\n- [ ] new\n- [ ] a\n- [ ] b\n- [ ] c\n\n{stories_text}");
    assert!(
        remap_shows(&before, &after).contains("- shows: 2, 4"),
        "{}",
        remap_shows(&before, &after)
    );
}

#[test]
fn a_shows_line_the_agent_changed_stands() {
    let before = "## Acceptance criteria\n- [ ] a\n- [ ] b\n\n## Stories\n### A\n- shows: 1\n";
    let after =
        "## Acceptance criteria\n- [ ] new\n- [ ] a\n- [ ] b\n\n## Stories\n### A\n- shows: 2, 3\n";
    assert!(remap_shows(before, after).contains("- shows: 2, 3"));
}

#[test]
fn adding_a_criterion_keeps_stories_after_the_criteria() {
    let b = "## Description\nx\n\n## Stories\n### A\n- shows: 1\n";
    let out = add_checklist_item(b, CRITERIA_HEADING, "x works");
    let criteria = out.find("## Acceptance criteria").unwrap();
    let stories_at = out.find("## Stories").unwrap();
    assert!(criteria < stories_at, "{out}");
}

#[test]
fn a_story_the_agent_renumbered_keeps_its_numbers() {
    let before = "## Acceptance criteria\n- [ ] a\n- [ ] b\n\n## Stories\n### A\n- shows: 1\n### B\n- shows: 2\n";
    let after = "## Acceptance criteria\n- [ ] new\n- [ ] a\n- [ ] b\n\n## Stories\n### A\n- shows: 2\n### B\n- shows: 3\n";
    let out = remap_shows(before, after);
    assert!(
        out.contains("### A\n- shows: 2\n### B\n- shows: 3"),
        "{out}"
    );
}

const FULL: &str = "## Description\nx\n\n## Acceptance criteria\n- [ ] one\n\n## Stories\n### Old\n- shows: 1\n\n## Open questions\n- q?\n  → a\n\n## Decisions\n- D1\n\n## Artifacts\n- pr: #5\n";

#[test]
fn new_stories_replace_only_the_stories_section() {
    let out = replace_stories(FULL, "## Stories\n### New\n- persona: Mark\n- shows: 1\n");
    assert!(out.contains("### New") && !out.contains("### Old"), "{out}");
    for kept in [
        "## Description\nx",
        "- [ ] one",
        "## Decisions\n- D1",
        "- pr: #5",
        "→ a",
    ] {
        assert!(out.contains(kept), "{kept:?} kept:\n{out}");
    }
    let at = |h: &str| out.find(h).unwrap();
    assert!(at("## Acceptance criteria") < at("## Stories"));
    assert!(at("## Stories") < at("## Open questions"));
}

#[test]
fn stories_are_added_after_the_criteria_when_missing() {
    let body = "## Description\nx\n\n## Acceptance criteria\n- [ ] one\n\n## Decisions\n- D1\n";
    let out = replace_stories(body, "### New\n- shows: 1\n");
    assert!(out.contains("## Stories\n### New"), "{out}");
    let at = |h: &str| out.find(h).unwrap();
    assert!(at("## Acceptance criteria") < at("## Stories"));
    assert!(at("## Stories") < at("## Decisions"), "{out}");
}

#[test]
fn only_the_stories_section_of_a_proposal_is_taken() {
    let out = replace_stories(
        FULL,
        "## Description\nchanged\n\n## Stories\n### New\n- shows: 1\n\n## Acceptance criteria\n- [ ] reworded\n",
    );
    assert!(out.contains("### New"), "{out}");
    assert!(
        !out.contains("changed") && !out.contains("reworded"),
        "{out}"
    );
}
