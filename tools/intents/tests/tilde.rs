//! Its own test binary: it points HOME at a temporary folder, which only this
//! process sees.

use std::fs;
use std::path::Path;

use herdr_intents::progress::{plan_evidence, plan_tasks};
use herdr_intents::send::resolve_in_repo;
use herdr_intents::store::{State, Store};

#[test]
fn a_tilde_plan_path_is_read_from_home() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(home.join("plans")).unwrap();
    let plan = "### Task 1: a\n### Task 2: b\n### Task 3: c\n";
    fs::write(home.join("plans/p.md"), plan).unwrap();
    // SAFETY: this binary holds this one test, so nothing else reads HOME
    unsafe { std::env::set_var("HOME", &home) };

    let store = Store::open(&tmp.path().join("root")).unwrap();
    let it = store
        .create(
            Path::new("org"),
            "Notify the family",
            "idea",
            "## Artifacts\n- plan: ~/plans/p.md\n",
            None,
        )
        .unwrap();
    let it = Store::update_fields(&it.path, |fm| fm.state = State::Sent).unwrap();
    // without a repo, and with one that does not hold it
    assert_eq!(plan_evidence(None, &it), vec![plan_tasks(plan, None)]);
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    assert_eq!(
        plan_evidence(Some(&repo), &it),
        vec![plan_tasks(plan, None)]
    );
    assert_eq!(
        resolve_in_repo(&repo, "~/plans/p.md"),
        Some(home.join("plans/p.md"))
    );
}
