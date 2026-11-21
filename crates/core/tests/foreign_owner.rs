//! G1: git refuses a repository owned by another user unless `safe.directory` trusts it, and the
//! scanner never reads the global or system config where that trust would live. The refusal
//! must come out as `unknown` facts and an `ask`, never as a clean repo.
//!
//! Needs a repository owned by another user, so it only runs when `DC_TEST_FOREIGN_ROOT` names a
//! directory holding such a repository at `theirs` (CI creates it with sudo on Linux). Without the
//! variable the test is a no-op and says so; it is not counted as verification. Before the G1 fix
//! this failed on the CI runner: the inherited configuration trusted the directory and the repo
//! was inspected and reported clean (CI prints where that `safe.directory` entry comes from).

mod common;

use common::*;
use disk_cleaner_core::{Action, Tri};
use std::path::PathBuf;

#[test]
fn repository_owned_by_another_user_is_unknown_not_clean() {
    let root = match std::env::var_os("DC_TEST_FOREIGN_ROOT") {
        Some(m) => PathBuf::from(m),
        None => {
            eprintln!(
                "DC_TEST_FOREIGN_ROOT not set: foreign-owner test skipped (not verified here)"
            );
            return;
        }
    };
    let theirs = root.join("theirs");
    assert!(theirs.join(".git").is_dir(), "fixture missing");
    let res = scan_with(&root, network_everywhere());
    let f = facts_of(&res, &theirs);
    assert!(
        f.entries().iter().all(|(_, t)| *t == Tri::Unknown),
        "git must refuse a repository owned by another user: {f:?}"
    );
    assert_eq!(action_for(&res, &theirs), Action::Ask);
}
