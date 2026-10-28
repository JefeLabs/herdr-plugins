use std::fs;
use std::path::{Path, PathBuf};

use herdr_intents::demos::{
    demo_target, discover_demos, find_story, safe_story_id, story_markdown, video_for,
};

const STORIES: &str = r#"
# Meeting requests
stories:
  - id: alex-requests-a-meeting-from-candidates
    title: "Alex requests a meeting from the Candidates header"
    actor: alex
    host: inspiringyoungminds
    dependsOn: [dana-applies-for-her-child]
    tags: [tenant-admin, meeting-request]
    steps:
      - signIn: alex
      - requestMeeting:
          child: Sam Rivera
          caption: Alex picks School-list review
  - id: dana-books-the-requested-meeting
    title: "Dana books the meeting Alex requested"
    actor: dana
    steps:
      - signIn: dana
  - id: casey-opens-the-profile-menu
    title: "Casey opens the profile menu over the sidebar"
    actor: casey
    steps:
      - signIn: casey
"#;

fn repo_with_stories() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(repo.join("app-demos/stories")).unwrap();
    fs::write(repo.join("app-demos/stories/meetings.yaml"), STORIES).unwrap();
    (tmp, repo)
}

#[test]
fn a_demo_target_names_a_story_and_the_criteria_it_shows() {
    assert_eq!(
        demo_target("alex-requests-a-meeting-from-candidates (criteria: 1, 4)"),
        (
            "alex-requests-a-meeting-from-candidates".to_string(),
            vec![1, 4]
        )
    );
    assert_eq!(
        demo_target(" plain-story "),
        ("plain-story".to_string(), vec![])
    );
}

#[test]
fn a_story_is_found_by_id_with_its_steps_and_captions() {
    let (_t, repo) = repo_with_stories();
    let story = find_story(
        std::slice::from_ref(&repo),
        "alex-requests-a-meeting-from-candidates",
    )
    .unwrap();
    assert_eq!(
        story.title,
        "Alex requests a meeting from the Candidates header"
    );
    assert_eq!(story.actor, "alex");
    assert_eq!(story.depends_on, vec!["dana-applies-for-her-child"]);
    assert_eq!(
        story.steps,
        vec![
            "signIn: alex",
            "requestMeeting — Alex picks School-list review"
        ]
    );
    assert!(find_story(&[repo], "nope").is_none());
}

#[test]
fn the_reader_shows_the_story_and_the_criteria_it_demonstrates() {
    let (_t, repo) = repo_with_stories();
    let story = find_story(&[repo], "alex-requests-a-meeting-from-candidates").unwrap();
    let md = story_markdown(
        &story,
        &[(1, "Request Meeting from the candidate header".into())],
    );
    for needle in [
        "# Alex requests a meeting from the Candidates header",
        "alex-requests-a-meeting-from-candidates",
        "inspiringyoungminds",
        "dana-applies-for-her-child",
        "1. Request Meeting from the candidate header",
        "requestMeeting — Alex picks School-list review",
    ] {
        assert!(md.contains(needle), "missing {needle:?}:\n{md}");
    }
}

#[test]
fn the_newest_recorded_video_is_played_when_there_is_one() {
    let (_t, repo) = repo_with_stories();
    assert!(
        video_for(
            std::slice::from_ref(&repo),
            "alex-requests-a-meeting-from-candidates"
        )
        .is_none()
    );
    let dir = repo.join("app-demos/videos/alex-requests-a-meeting-from-candidates");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("take-1.webm"), b"old").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(dir.join("take-2.mp4"), b"new").unwrap();
    fs::write(dir.join("take-2.json"), b"{}").unwrap();
    assert_eq!(
        video_for(&[repo], "alex-requests-a-meeting-from-candidates").unwrap(),
        dir.join("take-2.mp4")
    );
}

#[test]
fn stories_matching_the_title_are_found_unless_linked() {
    let (_t, repo) = repo_with_stories();
    let found = discover_demos(&repo, "Advisors request a meeting from Candidates", &[]);
    let ids: Vec<&str> = found.iter().map(|a| a.target.as_str()).collect();
    assert!(found.iter().all(|a| a.kind == "demo"));
    assert_eq!(
        ids,
        vec![
            "alex-requests-a-meeting-from-candidates",
            "dana-books-the-requested-meeting"
        ],
        "both meeting-request stories, not the profile menu"
    );
    let linked = discover_demos(
        &repo,
        "Advisors request a meeting from Candidates",
        &["alex-requests-a-meeting-from-candidates (criteria: 1)".to_string()],
    );
    assert!(
        !linked
            .iter()
            .any(|a| a.target == "alex-requests-a-meeting-from-candidates"),
        "already linked: {linked:?}"
    );
    assert!(discover_demos(Path::new("/nonexistent"), "anything at all", &[]).is_empty());
}

#[test]
fn only_plain_video_files_inside_the_story_folder_are_played() {
    let (t, repo) = repo_with_stories();
    let dir = repo.join("app-demos/videos/alex-requests-a-meeting-from-candidates");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("take-1.webm"), b"real").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    // a newer "video" that is really a link to something else must never be opened
    let outside = t.path().join("payload.command");
    fs::write(&outside, b"#!/bin/sh\necho pwned\n").unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("take-2.mp4")).unwrap();
    assert_eq!(
        video_for(
            std::slice::from_ref(&repo),
            "alex-requests-a-meeting-from-candidates"
        )
        .unwrap(),
        dir.join("take-1.webm"),
        "the symlink is skipped"
    );
    // a story id that climbs out of the videos folder is refused
    let escape = repo.join("app-demos/videos/../x");
    fs::create_dir_all(&escape).unwrap();
    fs::write(escape.join("a.mp4"), b"x").unwrap();
    assert!(video_for(std::slice::from_ref(&repo), "../x").is_none());
}

#[test]
fn story_ids_are_plain_names() {
    assert!(safe_story_id("alex-requests-a-meeting-from-candidates"));
    assert!(safe_story_id("story_2.v1"));
    for bad in ["", "../x", "a/b", ".hidden", "-flag", "a b", "a\\b"] {
        assert!(!safe_story_id(bad), "{bad:?}");
    }
}

#[test]
fn a_story_must_share_a_word_with_its_id_not_only_its_title() {
    let (_t, repo) = repo_with_stories();
    fs::write(
        repo.join("app-demos/stories/more.yaml"),
        "stories:\n  - id: walkthrough-one\n    title: \"Advisors request a meeting from Candidates\"\n    actor: alex\n    steps:\n      - signIn: alex\n",
    )
    .unwrap();
    let found = discover_demos(&repo, "Advisors request a meeting from Candidates", &[]);
    assert!(
        !found.iter().any(|a| a.target == "walkthrough-one"),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|a| a.target == "alex-requests-a-meeting-from-candidates"),
        "{found:?}"
    );
}
