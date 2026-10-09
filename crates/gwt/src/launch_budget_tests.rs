//! Artificial admission tests: callbacks never start a process or a model.
use crate::launch_budget::{BudgetScope, BudgetStore};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn fixture() -> (tempfile::TempDir, BudgetStore, BudgetScope) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let scope = BudgetScope {
        trial_id: "bounded-trial-1".to_string(),
        project_root: dunce::canonicalize(root).unwrap(),
        issue_number: 6,
        max_starts: 1,
    };
    let store = BudgetStore::new(dir.path().join("state/launch-budget.json"));
    (dir, store, scope)
}

#[test]
fn launch_budget_unarmed_does_not_arm_or_limit_existing_launches() {
    let (_dir, store, scope) = fixture();
    assert!(store.status().unwrap().is_none());
    assert_eq!(
        store
            .run(&scope, "session-1", || Ok::<_, std::io::Error>(7))
            .unwrap(),
        7
    );
    assert!(store.status().unwrap().is_none());
}

#[test]
fn launch_budget_one_start_is_saved_before_callback_and_second_is_refused() {
    let (_dir, store, scope) = fixture();
    store.arm(&scope).unwrap();
    store
        .run(&scope, "session-1", || {
            assert_eq!(store.status().unwrap().unwrap().used_starts, 1);
            Ok::<_, std::io::Error>(())
        })
        .unwrap();
    let calls = AtomicUsize::new(0);
    assert!(store
        .run(&scope, "session-2", || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn launch_budget_restart_and_same_scope_arm_never_refill_a_spent_slot() {
    let (_dir, store, scope) = fixture();
    store.arm(&scope).unwrap();
    store
        .run(&scope, "session-1", || Ok::<_, std::io::Error>(()))
        .unwrap();
    let reopened = BudgetStore::new(store.path().to_path_buf());
    assert_eq!(reopened.arm(&scope).unwrap().used_starts, 1);
    assert!(reopened.run(&scope, "session-2", || Ok(())).is_err());
}

#[test]
fn launch_budget_failed_or_unknown_callback_cannot_return_the_allowance() {
    let (_dir, store, scope) = fixture();
    store.arm(&scope).unwrap();
    assert!(store
        .run::<()>(&scope, "session-1", || Err(std::io::Error::other(
            "unknown launch outcome"
        )))
        .is_err());
    assert_eq!(store.status().unwrap().unwrap().used_starts, 1);
    assert!(store.run(&scope, "session-2", || Ok(())).is_err());
}

#[test]
fn launch_budget_wrong_issue_project_trial_and_limit_never_start() {
    let (_dir, store, scope) = fixture();
    store.arm(&scope).unwrap();
    let mut cases = Vec::new();
    let mut other = scope.clone();
    other.issue_number += 1;
    cases.push(other);
    let mut other = scope.clone();
    other.trial_id.push_str("-other");
    cases.push(other);
    let mut other = scope.clone();
    other.project_root = scope.project_root.parent().unwrap().to_path_buf();
    cases.push(other);
    let mut other = scope.clone();
    other.max_starts = 2;
    cases.push(other);
    for other in cases {
        assert!(store
            .run::<()>(&other, "session-1", || panic!(
                "wrong target must never start"
            ))
            .is_err());
        assert!(store.arm(&other).is_err());
    }
    assert_eq!(store.status().unwrap().unwrap().used_starts, 0);
}

#[test]
fn launch_budget_rejects_invalid_scope_before_creating_a_budget() {
    let (_dir, store, scope) = fixture();
    for (trial_id, issue_number, max_starts) in [
        ("", 6, 1),
        ("trial", 0, 1),
        ("trial", 6, 0),
        ("trial", 6, 2),
    ] {
        let mut other = scope.clone();
        other.trial_id = trial_id.to_string();
        other.issue_number = issue_number;
        other.max_starts = max_starts;
        assert!(store.arm(&other).is_err());
    }
    assert!(store.status().unwrap().is_none());
}

#[test]
fn launch_budget_concurrent_launches_share_exactly_one_allowance() {
    let (_dir, store, scope) = fixture();
    store.arm(&scope).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let workers: Vec<_> = (0..8)
        .map(|n| {
            let store = BudgetStore::new(store.path().to_path_buf());
            let scope = scope.clone();
            let calls = calls.clone();
            std::thread::spawn(move || {
                store
                    .run(&scope, &format!("session-{n}"), || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .is_ok()
            })
        })
        .collect();
    assert_eq!(
        workers
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|ok| *ok)
            .count(),
        1
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn launch_budget_partial_spent_record_and_missing_scope_fail_closed() {
    let (_dir, store, scope) = fixture();
    store.arm(&scope).unwrap();
    std::fs::write(store.spent_path(), b"").unwrap();
    assert!(store.status().is_err());
    assert!(store
        .run::<()>(&scope, "session-1", || panic!("partial save must block"))
        .is_err());
    std::fs::remove_file(store.path()).unwrap();
    assert!(store.status().is_err());
    assert!(store.arm(&scope).is_err());
}

#[test]
fn launch_budget_corrupt_or_duplicate_scope_is_not_an_unarmed_budget() {
    let (_dir, store, scope) = fixture();
    store.arm(&scope).unwrap();
    for data in ["{", "{\"schema_version\":1,\"schema_version\":1}"] {
        std::fs::write(store.path(), data).unwrap();
        assert!(store.status().is_err());
        assert!(store
            .run::<()>(&scope, "session-1", || panic!("invalid scope must block"))
            .is_err());
    }
}

#[test]
fn launch_budget_save_error_prevents_callback_and_retries() {
    let (_dir, store, scope) = fixture();
    store.arm(&scope).unwrap();
    std::fs::create_dir(store.spent_path()).unwrap();
    assert!(store
        .run::<()>(&scope, "session-1", || panic!("save failure must block"))
        .is_err());
    assert!(store
        .run::<()>(&scope, "session-2", || panic!(
            "save failure must block retry"
        ))
        .is_err());
}

fn saved_session(
    root: &std::path::Path,
    sessions: &std::path::Path,
    issue: Option<u64>,
) -> gwt_agent::Session {
    let mut session = gwt_agent::Session::new(root, "trial", gwt_agent::AgentId::Codex);
    session.project_state_root = Some(root.to_path_buf());
    session.linked_issue_number = issue;
    session.launch_budget_trial_id = Some("bounded-trial-1".to_string());
    session.save(sessions).unwrap();
    session
}

#[test]
fn launch_budget_saved_review_and_retry_share_the_same_total_start() {
    let (dir, store, scope) = fixture();
    store.arm(&scope).unwrap();
    let sessions = dir.path().join("sessions");
    let first = saved_session(&scope.project_root, &sessions, Some(6));
    crate::launch_budget::admit_saved_agent_at(&store, &scope.project_root, &sessions, &first.id)
        .unwrap();
    let reviewer = saved_session(&scope.project_root, &sessions, Some(6));
    assert!(crate::launch_budget::admit_saved_agent_at(
        &store,
        &scope.project_root,
        &sessions,
        &reviewer.id
    )
    .is_err());
    assert!(crate::launch_budget::admit_saved_agent_at(
        &store,
        &scope.project_root,
        &sessions,
        &first.id
    )
    .is_err());
}

#[test]
fn launch_budget_saved_session_requires_exact_issue_project_and_identity() {
    let (dir, store, scope) = fixture();
    store.arm(&scope).unwrap();
    let sessions = dir.path().join("sessions");
    for issue in [None, Some(7)] {
        let session = saved_session(&scope.project_root, &sessions, issue);
        assert!(crate::launch_budget::admit_saved_agent_at(
            &store,
            &scope.project_root,
            &sessions,
            &session.id
        )
        .is_err());
    }
    let session = saved_session(dir.path(), &sessions, Some(6));
    assert!(crate::launch_budget::admit_saved_agent_at(
        &store,
        &scope.project_root,
        &sessions,
        &session.id
    )
    .is_err());
    assert!(crate::launch_budget::admit_saved_agent_at(
        &store,
        &scope.project_root,
        &sessions,
        "../foreign"
    )
    .is_err());
    assert!(crate::launch_budget::admit_saved_agent_at(
        &store,
        &scope.project_root,
        &sessions,
        "missing"
    )
    .is_err());
    assert_eq!(store.status().unwrap().unwrap().used_starts, 0);
}

#[test]
fn launch_budget_panic_after_saved_intent_cannot_refund() {
    let (_dir, store, scope) = fixture();
    store.arm(&scope).unwrap();
    assert!(std::panic::catch_unwind(
        || store.run::<()>(&scope, "session-1", || panic!("artificial interruption"))
    )
    .is_err());
    assert!(store.run(&scope, "session-2", || Ok(())).is_err());
}

#[test]
fn launch_budget_old_or_missing_saved_trial_is_never_relabelled() {
    let (dir, store, scope) = fixture();
    store.arm(&scope).unwrap();
    let sessions = dir.path().join("sessions");
    for trial_id in [None, Some("previous-trial")] {
        let mut session = saved_session(&scope.project_root, &sessions, Some(6));
        session.launch_budget_trial_id = trial_id.map(str::to_owned);
        session.save(&sessions).unwrap();
        assert!(crate::launch_budget::admit_saved_agent_at(
            &store,
            &scope.project_root,
            &sessions,
            &session.id
        )
        .is_err());
        assert_eq!(store.status().unwrap().unwrap().used_starts, 0);
    }
}

#[test]
fn launch_budget_physical_admission_orders_arm_after_an_unarmed_start() {
    let (dir, store, scope) = fixture();
    let sessions = dir.path().join("sessions");
    let session = saved_session(&scope.project_root, &sessions, Some(6));
    let admission = crate::launch_budget::admit_saved_agent_at(
        &store,
        &scope.project_root,
        &sessions,
        &session.id,
    )
    .unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.path().with_extension("lock"))
        .unwrap();
    assert!(fs2::FileExt::try_lock_exclusive(&lock).is_err());
    assert!(store.status().unwrap().is_none());
    drop(admission);
    fs2::FileExt::try_lock_exclusive(&lock).unwrap();
    drop(lock);
    store.arm(&scope).unwrap();
    let admission = crate::launch_budget::admit_saved_agent_at(
        &store,
        &scope.project_root,
        &sessions,
        &session.id,
    )
    .unwrap();
    assert_eq!(store.status().unwrap().unwrap().used_starts, 1);
    drop(admission);
}

#[test]
fn launch_budget_store_identity_is_the_canonical_location_not_git_origin() {
    let (dir, _store, scope) = fixture();
    let _home = gwt_core::test_support::ScopedGwtHome::set(dir.path());
    let before = BudgetStore::for_project(&scope.project_root)
        .path()
        .to_path_buf();
    let git_dir = scope.project_root.join(".git");
    std::fs::create_dir(&git_dir).unwrap();
    let mut previous_origin_hash = None;
    for origin in [
        "https://example.invalid/first.git",
        "https://example.invalid/second.git",
    ] {
        std::fs::write(
            git_dir.join("config"),
            format!("[remote \"origin\"]\nurl = {origin}\n"),
        )
        .unwrap();
        let origin_hash = gwt_core::paths::project_scope_hash(&scope.project_root);
        assert_ne!(
            origin_hash,
            gwt_core::repo_hash::compute_path_hash(&scope.project_root)
        );
        if let Some(previous) = previous_origin_hash {
            assert_ne!(origin_hash, previous);
        }
        previous_origin_hash = Some(origin_hash);
        assert_eq!(BudgetStore::for_project(&scope.project_root).path(), before);
    }
    assert_eq!(
        BudgetStore::for_project(&scope.project_root.join(".")).path(),
        before
    );
}
