//! Regression tests for the ninth merge-gate review of PR #2, the symlink cases: references INTO a
//! repository's git directory are found wherever they are (inside artifact directories, deep in
//! another object store, anywhere in the tree), and a symlink anywhere inside a git directory
//! makes its layout not plain (Q1, Q2). Also the symlinked spelling of overlapping roots (Q3).
#![cfg(unix)]

mod common;

use common::*;
use disk_cleaner_core::{
    plan, scan, Action, Plan, PlanItem, PlanOptions, ScanOptions, ScanResult, Tri,
};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

fn plan_of(res: &ScanResult) -> Plan {
    plan(res, &PlanOptions::new(7, far_future()))
}

fn item<'a>(p: &'a Plan, path: &Path) -> &'a PlanItem {
    p.items
        .iter()
        .find(|i| i.path == path.to_string_lossy())
        .unwrap_or_else(|| panic!("no plan item for {}", path.display()))
}

/// The provider asks, offers nothing, and its reason names `who`.
fn assert_provider_asks(res: &ScanResult, provider: &Path, who: &Path) {
    assert_eq!(
        facts_of(res, provider).no_dependents,
        Tri::No,
        "{:?}",
        node(res, provider).repo.as_ref().map(|r| &r.notes)
    );
    let p = plan_of(res);
    let it = item(&p, provider);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    assert!(
        it.reason.contains(&*who.to_string_lossy()),
        "the dependent {} is named: {}",
        who.display(),
        it.reason
    );
}

fn assert_not_plain(res: &ScanResult, repo: &Path) {
    let info = node(res, repo).repo.as_ref().unwrap();
    assert_ne!(info.facts.plain_layout, Tri::Yes, "{:?}", info.layout_issue);
    let a = action_for(res, repo);
    assert!(!a.removes(), "{a:?}");
}

// ---------------------------------------------------------------- Q1

#[test]
fn q1_a_hidden_checkout_whose_objects_are_a_symlink_to_a_makes_a_ask() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let app = root.join("app");
    write(&app.join("package.json"), "{}\n");
    let dep = app.join("node_modules").join("dep");
    fs::create_dir_all(&dep).unwrap();
    git(&dep, &["init", "-q", "-b", "main"]);
    let objs = dep.join(".git").join("objects");
    fs::remove_dir_all(&objs).unwrap();
    symlink(a.join(".git").join("objects"), &objs).unwrap();
    commit_file(&dep, "index.js", "unpushed work made through the link\n");
    let head = git(&dep, &["rev-parse", "HEAD"]);
    // The commit exists only in A's object directory.
    git(&a, &["cat-file", "-e", &head]);

    let res = scan_with(&root, network_everywhere());
    assert_provider_asks(&res, &a, &dep);
}

// ---------------------------------------------------------------- Q2

/// B, a pushed repo with one unpushed commit; returns (B, the commit id).
fn b_with_unpushed_commit(root: &Path, remotes: &Path) -> (PathBuf, String) {
    let b = pushed_repo(root, remotes, "b");
    commit_file(&b, "work.txt", "unpushed work\n");
    let head = git(&b, &["rev-parse", "HEAD"]);
    (b, head)
}

#[test]
fn q2_b_with_a_fanout_directory_symlinked_into_a_makes_a_ask_and_b_not_plain() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let (b, head) = b_with_unpushed_commit(&root, &remotes);
    let fan = &head[..2];
    let b_fan = b.join(".git").join("objects").join(fan);
    let a_fan = a.join(".git").join("objects").join(fan);
    fs::create_dir_all(&a_fan).unwrap();
    for e in fs::read_dir(&b_fan).unwrap() {
        let e = e.unwrap();
        fs::rename(e.path(), a_fan.join(e.file_name())).unwrap();
    }
    fs::remove_dir(&b_fan).unwrap();
    symlink(&a_fan, &b_fan).unwrap();
    // B's unpushed commit is now stored only in A, and B reads it through the link.
    assert!(a_fan.join(&head[2..]).is_file());
    git(&b, &["cat-file", "-e", &head]);

    let res = scan_with(&root, network_everywhere());
    assert_provider_asks(&res, &a, &b);
    assert_not_plain(&res, &b);
}

#[test]
fn q2_b_with_pack_files_symlinked_into_a_makes_a_ask_and_b_not_plain() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    git(&a, &["repack", "-a", "-d", "-q"]);
    let (b, head) = b_with_unpushed_commit(&root, &remotes);
    git(&b, &["repack", "-a", "-d", "-q"]);
    let b_pack = b.join(".git").join("objects").join("pack");
    let a_pack = a.join(".git").join("objects").join("pack");
    let mut moved = 0;
    for e in fs::read_dir(&b_pack).unwrap() {
        let e = e.unwrap();
        let name = e.file_name();
        fs::rename(e.path(), a_pack.join(&name)).unwrap();
        symlink(a_pack.join(&name), b_pack.join(&name)).unwrap();
        moved += 1;
    }
    assert!(moved >= 2, "a .pack and its .idx");
    // B's objects, its unpushed commit included, now live only in A's pack directory.
    git(&b, &["cat-file", "-e", &head]);

    let res = scan_with(&root, network_everywhere());
    assert_provider_asks(&res, &a, &b);
    assert_not_plain(&res, &b);
}

#[test]
fn q2_a_convenience_link_into_a_git_directory_makes_that_repo_ask() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let link = root.join("a-hooks");
    symlink(a.join(".git").join("hooks"), &link).unwrap();
    assert!(link.join(".").is_dir());

    let res = scan_with(&root, network_everywhere());
    assert_provider_asks(&res, &a, &link);
}

#[test]
fn q2_a_symlink_anywhere_in_a_git_directory_makes_the_layout_not_plain() {
    let (tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let c = pushed_repo(&root, &remotes, "c");
    let exclude = a.join(".git").join("info").join("exclude");
    let shared = tmp.path().join("shared-exclude");
    write(&shared, "# shared exclude rules\n");
    if exclude.exists() {
        fs::remove_file(&exclude).unwrap();
    }
    fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    symlink(&shared, &exclude).unwrap();
    git(&a, &["status", "--porcelain"]);

    let res = scan_with(&root, network_everywhere());
    assert_not_plain(&res, &a);
    let issue = node(&res, &a)
        .repo
        .as_ref()
        .unwrap()
        .layout_issue
        .clone()
        .unwrap_or_default();
    assert!(issue.contains("info/exclude"), "{issue}");
    // A link that leaves the scan from inside one repo does not make unrelated repos ask.
    assert_eq!(action_for(&res, &c), Action::BackupThenRemove);
}

#[test]
fn q2_control_links_into_a_working_tree_do_not_block() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    symlink(a.join("README.md"), root.join("readme-link")).unwrap();
    let app = root.join("app");
    write(&app.join("package.json"), "{}\n");
    // A `file:` dependency: node_modules links to the other project's working tree.
    fs::create_dir_all(app.join("node_modules")).unwrap();
    symlink(&a, app.join("node_modules").join("a")).unwrap();

    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &a).no_dependents, Tri::Yes);
    assert_eq!(action_for(&res, &a), Action::BackupThenRemove);
}

// ---------------------------------------------------------------- Q3

#[test]
fn q3_a_child_root_spelled_through_a_symlink_is_counted_once() {
    let (tmp, root, remotes) = workspace();
    let app = pushed_repo(&root, &remotes, "app");
    write_bytes(&app.join("data.bin"), 200_000);
    let alias = tmp.path().join("alias");
    symlink(&root, &alias).unwrap();

    let one = |roots: Vec<PathBuf>| {
        let mut o = ScanOptions::new(roots);
        o.collect_facts = false;
        scan(&o).unwrap()
    };
    let alone = one(vec![root.clone()]);
    let both = one(vec![root.clone(), alias.join("app")]);
    assert_eq!(both.totals.allocated_bytes, alone.totals.allocated_bytes);
    assert_eq!(both.totals.files, alone.totals.files);
    assert_eq!(both.nodes.len(), alone.nodes.len());
}
