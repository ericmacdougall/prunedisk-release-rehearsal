//! Follow-up to the ninth merge-gate review of PR #2 (R3): a repository that declares
//! `extensions.preciousObjects = true` says other repositories borrow its objects, so it is a
//! dependency signal. Coverage (R1, R2) is tested end to end in the CLI tests and, for mount
//! points, in `mount.rs`.

mod common;

use common::*;
use disk_cleaner_core::{plan, Action, PlanOptions, Tri};
use std::path::Path;
use std::process::Command;

/// `git repack -a -d` refuses to delete packs in a precious-objects repository: proof that git
/// itself honours the setting in this fixture.
fn repack_refused(repo: &Path) -> bool {
    let out = Command::new("git")
        .args(["repack", "-a", "-d", "-q"])
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git");
    !out.status.success() && String::from_utf8_lossy(&out.stderr).contains("precious")
}

#[test]
fn r3_a_repository_declaring_precious_objects_makes_it_ask() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    git(&a, &["config", "extensions.preciousObjects", "true"]);
    assert!(repack_refused(&a), "git must honour preciousObjects here");

    let res = scan_with(&root, network_everywhere());
    let info = node(&res, &a).repo.as_ref().expect("repo info");
    assert_eq!(info.facts.no_dependents, Tri::No, "{:?}", info.notes);
    assert!(
        info.notes.iter().any(|n| n.contains("preciousObjects")),
        "a note says why: {:?}",
        info.notes
    );
    let p = plan(&res, &PlanOptions::new(7, far_future()));
    let it = p
        .items
        .iter()
        .find(|i| i.path == a.to_string_lossy())
        .expect("plan item");
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    assert!(it.reason.contains("preciousObjects"), "{}", it.reason);
}

#[test]
fn r3_control_precious_objects_set_to_false_changes_nothing() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    git(&a, &["config", "extensions.preciousObjects", "false"]);
    assert!(!repack_refused(&a));

    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &a).no_dependents, Tri::Yes);
    assert_eq!(action_for(&res, &a), Action::BackupThenRemove);
}
