//! Regression tests for the click-test of 53c14b6 (P1 to P4): a repository with dependents never
//! recovers then archives, structural layout checks run before any git command, a tag of a tag is
//! not plain, and unknown facts are explained in words.

mod common;

use common::*;
use disk_cleaner_core::{plan, Action, PlanOptions, ScanResult};
use std::fs;
use std::path::Path;

fn item<'a>(p: &'a disk_cleaner_core::Plan, path: &Path) -> &'a disk_cleaner_core::PlanItem {
    p.items
        .iter()
        .find(|i| i.path == path.to_string_lossy())
        .unwrap_or_else(|| panic!("no item for {}", path.display()))
}

fn plan_of(res: &ScanResult) -> disk_cleaner_core::Plan {
    plan(res, &PlanOptions::new(7, far_future()))
}

fn shared_clone(root: &Path, provider: &Path, name: &str) -> std::path::PathBuf {
    let b = root.join(name);
    git(
        root,
        &[
            "clone",
            "-q",
            "--shared",
            provider.to_str().unwrap(),
            b.to_str().unwrap(),
        ],
    );
    b
}

// ---------------------------------------------------------------- P1

#[test]
fn p1_a_repository_other_clones_borrow_from_asks_and_names_them() {
    let (_tmp, root, remotes) = workspace();
    let prov = pushed_repo(&root, &remotes, "prov");
    let b1 = shared_clone(&root, &prov, "b1");
    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &prov);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    assert!(it.reason.contains("deal with it first"), "{}", it.reason);
    assert!(
        it.reason.contains(&b1.to_string_lossy().into_owned()),
        "{}",
        it.reason
    );
    assert_eq!(p.totals.ready_bytes, 0);
}

#[test]
fn p1_the_reason_names_three_dependents_and_counts_the_rest() {
    let (_tmp, root, remotes) = workspace();
    let prov = pushed_repo(&root, &remotes, "prov");
    for n in ["b1", "b2", "b3", "b4"] {
        shared_clone(&root, &prov, n);
    }
    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &prov);
    assert_eq!(it.action, Action::Ask);
    assert!(it.reason.contains("4 other checkouts"), "{}", it.reason);
    assert!(it.reason.contains("and 1 more"), "{}", it.reason);
}

#[cfg(unix)]
#[test]
fn p1_the_target_of_a_git_symlink_asks() {
    use std::os::unix::fs::symlink;
    let (_tmp, root, remotes) = workspace();
    let store = pushed_repo(&root, &remotes, "store");
    let v1 = root.join("v1");
    write(&v1.join("work.txt"), "edits\n");
    symlink(store.join(".git"), v1.join(".git")).unwrap();
    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    assert_eq!(item(&p, &store).action, Action::Ask);
}

// ---------------------------------------------------------------- P2

#[cfg(unix)]
#[test]
fn p2_a_symlinked_head_is_reported_by_name_and_no_git_command_runs() {
    use std::os::unix::fs::symlink;
    let (tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    fs::rename(repo.join(".git/HEAD"), repo.join(".git/HEAD.real")).unwrap();
    symlink("HEAD.real", repo.join(".git/HEAD")).unwrap();
    let log = tmp.path().join("git-calls.log");
    let wrapper = script(
        tmp.path(),
        "git-logging",
        &format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nexec git \"$@\"\n",
            log.display()
        ),
    );
    let cfg = network_everywhere().with_git(disk_cleaner_core::Git::new(
        &wrapper,
        std::time::Duration::from_secs(20),
    ));
    let res = scan_with(&root, cfg);
    let info = node(&res, &repo).repo.as_ref().unwrap();
    let issue = info.layout_issue.clone().unwrap_or_default();
    assert!(issue.contains("HEAD is a symlink"), "{issue}");
    assert!(
        info.notes.iter().all(|n| !n.contains("--local")),
        "{:?}",
        info.notes
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(calls.trim().is_empty(), "git ran: {calls}");
    let p = plan_of(&res);
    assert!(
        item(&p, &repo)
            .reason
            .contains("unusual repository layout: HEAD is a symlink"),
        "{}",
        item(&p, &repo).reason
    );
}

// ---------------------------------------------------------------- P3

#[test]
fn p3_a_tag_whose_direct_target_is_another_tag_is_not_plain() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    git(&repo, &["tag", "-a", "-m", "first", "t1"]);
    let t1 = git(&repo, &["rev-parse", "t1"]);
    git(&repo, &["tag", "-a", "-m", "second", "t2", &t1]);
    let res = scan_with(&root, network_everywhere());
    let info = node(&res, &repo).repo.as_ref().unwrap();
    let v = serde_json::to_value(info.facts).unwrap();
    assert_eq!(v["plain_layout"], "no", "{:?}", info.layout_issue);
    let issue = info.layout_issue.clone().unwrap_or_default();
    assert!(issue.contains("refs/tags/t2"), "{issue}");
}

// ---------------------------------------------------------------- P4

#[test]
fn p4_local_tags_are_explained_in_words() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    git(&repo, &["tag", "v1"]);
    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::Ask);
    assert!(
        it.reason
            .contains("tag names cannot be checked against the remote"),
        "{}",
        it.reason
    );
    assert!(it.reason.contains("verified when a removal is applied"));
}

#[cfg(unix)]
#[test]
fn p4_an_unknown_fact_says_what_was_not_checkable_and_what_would_settle_it() {
    let (tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let wrapper = script(
        tmp.path(),
        "git-no-reflog",
        "#!/bin/sh\ncase \"$*\" in *--reflog*) echo 'fatal: simulated' >&2; exit 128;; esac\nexec git \"$@\"\n",
    );
    let cfg = network_everywhere().with_git(disk_cleaner_core::Git::new(
        &wrapper,
        std::time::Duration::from_secs(20),
    ));
    let res = scan_with(&root, cfg);
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::Ask);
    assert!(it.reason.contains("could not be"), "{}", it.reason);
    assert!(it.reason.contains("reflog"), "{}", it.reason);
}
