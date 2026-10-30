//! Applying a completion check's results to an intent: ticks, vetoes and
//! missing items.

use std::fs;
use std::path::{Path, PathBuf};

use herdr_intents::check::{CheckRecord, CriterionResult, Verdict, apply, checks_dir};
use herdr_intents::store::{self, Intent, MissingLink, State, Store, criterion_key};

const C1: &str = "the primary contact gets an email";
const C2: &str = "the email names the meeting";
const C3: &str = "admins get nothing";

fn store() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let s = Store::open(&tmp.path().join("root")).unwrap();
    (tmp, s)
}

/// A sent intent with three criteria (the second ticked by its owner when `owner_ticked`).
fn sent(s: &Store, owner_ticked: bool) -> Intent {
    let body = format!(
        "## Description\nNotify the primary contact.\n\n## Acceptance criteria\n- [ ] {C1}\n- [{}] {C2}\n- [ ] {C3}\n",
        if owner_ticked { "x" } else { " " }
    );
    let it = s
        .create(
            Path::new("skoolscout/org"),
            "Notify primary contact",
            "idea",
            &body,
            None,
        )
        .unwrap();
    Store::update_fields(&it.path, |fm| fm.state = State::Sent).unwrap()
}

/// A check's result for criterion `n` (1..=3); `proof` writes the file too.
fn result(it: &Intent, n: usize, verdict: Verdict, proof: bool) -> CriterionResult {
    let text = [C1, C2, C3][n - 1];
    let file = format!("{n}.png");
    if proof {
        let dir = checks_dir(it);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(&file),
            format!("png {n} {:?}", std::time::Instant::now()),
        )
        .unwrap();
    }
    CriterionResult {
        number: n,
        key: criterion_key(text),
        text: text.into(),
        verdict,
        proof: proof.then_some(file),
    }
}

fn record(results: Vec<CriterionResult>, merged: Vec<u64>) -> CheckRecord {
    CheckRecord {
        at: chrono::Local::now(),
        merged,
        results,
        warning: None,
    }
}

fn ticks(path: &Path) -> Vec<bool> {
    store::criteria(&Store::load(path).unwrap().body)
        .iter()
        .map(|c| c.done)
        .collect()
}

/// Every intent file in the group, but the original.
fn items(s: &Store, original: &Intent) -> Vec<Intent> {
    let dir = s.root().join("skoolscout/org");
    let mut out: Vec<Intent> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md") && *p != original.path)
        .map(|p| Store::load(&p).unwrap())
        .collect();
    out.sort_by_key(|i| i.path.clone());
    out
}

#[test]
fn a_confirmed_criterion_is_ticked_and_recorded() {
    let (_t, s) = store();
    let it = sent(&s, false);
    let rec = record(
        vec![
            result(&it, 1, Verdict::Confirmed("only the contact".into()), true),
            result(&it, 3, Verdict::Confirmed("no proof".into()), false),
        ],
        vec![],
    );
    let applied = apply(&s, &it.path, &rec).unwrap();
    assert_eq!(applied.ticked, 1);
    assert_eq!(
        ticks(&it.path),
        [true, false, false],
        "no tick without proof"
    );
    assert_eq!(
        Store::load(&it.path).unwrap().fm.check_ticks,
        [criterion_key(C1)]
    );
}

#[test]
fn an_owners_tick_is_never_unticked() {
    let (_t, s) = store();
    let it = sent(&s, true);
    let applied = apply(
        &s,
        &it.path,
        &record(
            vec![result(&it, 2, Verdict::Fails("no name".into()), true)],
            vec![],
        ),
    )
    .unwrap();
    assert!(ticks(&it.path)[1], "the owner's tick stays");
    assert_eq!(applied.created.len(), 1, "the gap is still filed");
}

#[test]
fn a_check_tick_is_unticked_when_it_later_fails() {
    let (_t, s) = store();
    let it = sent(&s, false);
    apply(
        &s,
        &it.path,
        &record(
            vec![result(&it, 1, Verdict::Confirmed("ok".into()), true)],
            vec![],
        ),
    )
    .unwrap();
    assert!(ticks(&it.path)[0]);
    let applied = apply(
        &s,
        &it.path,
        &record(
            vec![result(&it, 1, Verdict::Fails("no email now".into()), true)],
            vec![],
        ),
    )
    .unwrap();
    assert_eq!(applied.unticked, 1);
    assert!(!ticks(&it.path)[0]);
    assert!(Store::load(&it.path).unwrap().fm.check_ticks.is_empty());
}

#[test]
fn an_owner_untick_is_never_ticked_again() {
    let (_t, s) = store();
    let it = sent(&s, false);
    Store::update_fields(&it.path, |fm| fm.owner_unticked = vec![criterion_key(C1)]).unwrap();
    apply(
        &s,
        &it.path,
        &record(
            vec![result(&it, 1, Verdict::Confirmed("ok".into()), true)],
            vec![],
        ),
    )
    .unwrap();
    assert!(!ticks(&it.path)[0]);
}

#[test]
fn a_failing_criterion_files_a_linked_missing_item() {
    let (_t, s) = store();
    let it = sent(&s, false);
    let applied = apply(
        &s,
        &it.path,
        &record(
            vec![result(
                &it,
                2,
                Verdict::Fails("no meeting name".into()),
                true,
            )],
            vec![],
        ),
    )
    .unwrap();
    let made = items(&s, &it);
    assert_eq!(made.len(), 1);
    let item = &made[0];
    assert_eq!(applied.created, vec![item.path.clone()]);
    assert_eq!(item.fm.kind, "issue");
    assert_eq!(item.fm.state, State::Refining);
    let link = item.fm.missing_for.clone().unwrap();
    assert_eq!(
        (link.stem.as_str(), link.key.as_str()),
        (it.stem().as_str(), criterion_key(C2).as_str())
    );
    let crit = store::criteria(&item.body);
    assert_eq!(crit.len(), 1);
    assert_eq!((crit[0].text.as_str(), crit[0].done), (C2, false));
    assert!(item.body.contains("no meeting name"), "{}", item.body);
    assert!(
        item.body.contains("Notify primary contact"),
        "names the original: {}",
        item.body
    );
    assert_eq!(
        Store::load(&it.path).unwrap().fm.missing,
        [MissingLink {
            stem: item.stem(),
            key: criterion_key(C2),
            rechecked: false
        }]
    );
    let proof: Vec<PathBuf> = fs::read_dir(item.assets_dir())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert!(
        proof.iter().any(|p| p.to_string_lossy().ends_with("2.png")),
        "{proof:?}"
    );
}

#[test]
fn a_missing_criterion_files_a_feature() {
    let (_t, s) = store();
    let it = sent(&s, false);
    apply(
        &s,
        &it.path,
        &record(
            vec![result(
                &it,
                3,
                Verdict::Missing("no such screen".into()),
                false,
            )],
            vec![],
        ),
    )
    .unwrap();
    assert_eq!(items(&s, &it)[0].fm.kind, "enhancement");
}

#[test]
fn an_open_item_gets_the_new_proof_not_a_twin() {
    let (_t, s) = store();
    let it = sent(&s, false);
    apply(
        &s,
        &it.path,
        &record(
            vec![result(&it, 2, Verdict::Fails("a".into()), true)],
            vec![],
        ),
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let applied = apply(
        &s,
        &it.path,
        &record(
            vec![result(&it, 2, Verdict::Fails("b".into()), true)],
            vec![],
        ),
    )
    .unwrap();
    let made = items(&s, &it);
    assert_eq!(made.len(), 1, "no twin");
    assert_eq!(applied.updated, vec![made[0].path.clone()]);
    assert_eq!(
        fs::read_dir(made[0].assets_dir()).unwrap().count(),
        2,
        "both proofs"
    );
    assert_eq!(Store::load(&it.path).unwrap().fm.missing.len(), 1);
}

#[test]
fn a_completed_item_does_not_stop_a_new_one() {
    let (_t, s) = store();
    let it = sent(&s, false);
    apply(
        &s,
        &it.path,
        &record(
            vec![result(&it, 2, Verdict::Fails("a".into()), true)],
            vec![],
        ),
    )
    .unwrap();
    let mut first = items(&s, &it).remove(0);
    first.body = store::set_criteria_ticks(&first.body, |_| true);
    Store::save_force(&mut first).unwrap();
    apply(
        &s,
        &it.path,
        &record(
            vec![result(&it, 2, Verdict::Fails("again".into()), true)],
            vec![],
        ),
    )
    .unwrap();
    assert_eq!(
        items(&s, &it).len(),
        2,
        "a new item, the done one untouched"
    );
    assert_eq!(Store::load(&it.path).unwrap().fm.missing.len(), 2);
}

#[test]
fn a_dismissed_gap_files_nothing() {
    let (_t, s) = store();
    let it = sent(&s, false);
    Store::update_fields(&it.path, |fm| fm.dismissed = vec![criterion_key(C2)]).unwrap();
    let applied = apply(
        &s,
        &it.path,
        &record(
            vec![result(&it, 2, Verdict::Fails("a".into()), true)],
            vec![],
        ),
    )
    .unwrap();
    assert!(applied.created.is_empty() && items(&s, &it).is_empty());
    assert!(Store::load(&it.path).unwrap().fm.missing.is_empty());
}

#[test]
fn results_for_a_reworded_criterion_tick_nothing() {
    let (_t, s) = store();
    let it = sent(&s, false);
    let mut stale = result(&it, 1, Verdict::Confirmed("ok".into()), true);
    stale.key = criterion_key("the primary contact gets a text");
    stale.text = "the primary contact gets a text".into();
    let mut gone = result(&it, 3, Verdict::Fails("x".into()), true);
    gone.key = criterion_key("admins get a digest");
    let applied = apply(&s, &it.path, &record(vec![stale, gone], vec![])).unwrap();
    assert_eq!(applied.ticked, 0);
    assert!(applied.created.is_empty());
    assert_eq!(ticks(&it.path), [false, false, false]);
}

#[test]
fn applying_keeps_an_edit_made_during_the_check() {
    let (_t, s) = store();
    let it = sent(&s, false);
    let rec = record(
        vec![result(&it, 1, Verdict::Confirmed("ok".into()), true)],
        vec![],
    );
    // the owner edits while the check runs
    let mut edited = Store::load(&it.path).unwrap();
    edited.body = edited.body.replace(
        "Notify the primary contact.",
        "Notify the primary contact.\nAn edit made while the check ran.",
    );
    Store::save_force(&mut edited).unwrap();
    apply(&s, &it.path, &rec).unwrap();
    let now = Store::load(&it.path).unwrap();
    assert!(
        now.body.contains("An edit made while the check ran."),
        "{}",
        now.body
    );
    assert!(ticks(&it.path)[0]);
}

#[test]
fn checked_for_records_the_merged_set() {
    let (_t, s) = store();
    let it = sent(&s, false);
    apply(&s, &it.path, &record(vec![], vec![42])).unwrap();
    assert_eq!(Store::load(&it.path).unwrap().fm.checked_for, [42]);
    apply(&s, &it.path, &record(vec![], vec![])).unwrap();
    assert_eq!(
        Store::load(&it.path).unwrap().fm.checked_for,
        [42],
        "a check by hand leaves it"
    );
}
