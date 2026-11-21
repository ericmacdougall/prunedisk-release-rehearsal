//! Regression tests for the ninth merge-gate review of PR #2, the parts that need no symlinks:
//! repositories hidden inside artifact directories still count as dependents (Q1), and
//! overlapping scan roots are counted once (Q3). Symlink cases are in `gate9_links.rs`.
//!
//! Every fixture ends with a git command that proves the dependency is real, so a failure here is
//! an assertion about the scanner, not a broken fixture.

mod common;

use common::*;
use disk_cleaner_core::{
    plan, scan, Action, Plan, PlanItem, PlanOptions, ProcessCheck, ScanOptions, ScanResult, Tri,
};
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

/// `git clone --shared <src> <dst>` run from `cwd`.
fn shared_clone(cwd: &Path, src: &Path, dst: &Path) {
    git(
        cwd,
        &[
            "clone",
            "-q",
            "--shared",
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
        ],
    );
}

/// A JavaScript project `app` whose `node_modules/dep` is the path returned.
fn app_with_dep(root: &Path) -> PathBuf {
    let app = root.join("app");
    write(&app.join("package.json"), "{}\n");
    app.join("node_modules").join("dep")
}

fn scan_roots(roots: Vec<PathBuf>) -> ScanResult {
    let mut opts = ScanOptions::new(roots);
    opts.facts = network_everywhere();
    opts.git_jobs = 2;
    scan(&opts).expect("scan")
}

// ---------------------------------------------------------------- Q1

#[test]
fn q1_a_shared_clone_hidden_in_node_modules_makes_its_provider_ask() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let dep = app_with_dep(&root);
    shared_clone(&root, &a, &dep);
    // The clone has no objects of its own: it reads A's through its alternates file.
    git(&dep, &["cat-file", "-e", "HEAD^{tree}"]);
    let alternates = dep.join(".git/objects/info/alternates");
    assert!(alternates.is_file());

    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &a).no_dependents, Tri::No);
    let p = plan_of(&res);
    let it = item(&p, &a);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    assert!(
        it.reason.contains(&*dep.to_string_lossy()),
        "the hidden dependent is named: {}",
        it.reason
    );
}

#[test]
fn q1_a_hidden_shared_clone_of_a_repo_outside_the_scan_makes_every_repo_ask() {
    let (tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let ext = tmp.path().join("ext");
    init_repo(&ext);
    let dep = app_with_dep(&root);
    shared_clone(&root, &ext, &dep);
    git(&dep, &["cat-file", "-e", "HEAD^{tree}"]);

    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &a).no_dependents, Tri::Unknown);
    assert_eq!(action_for(&res, &a), Action::Ask);
}

#[test]
fn q1_a_hidden_dot_git_file_naming_a_scanned_git_directory_makes_it_ask() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let dep = app_with_dep(&root);
    write(&dep.join("index.js"), "local edits\n");
    let gitdir = a.join(".git");
    write(
        &dep.join(".git"),
        &format!("gitdir: {}\n", gitdir.to_string_lossy()),
    );
    // A second checkout of A's git directory: git works there and commits would land in A.
    git(&dep, &["rev-parse", "--verify", "HEAD"]);

    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &a).no_dependents, Tri::No);
    let p = plan_of(&res);
    let it = item(&p, &a);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
}

#[test]
fn q1_alternates_inside_git_modules_mark_their_provider() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let s = pushed_repo(&root, &remotes, "s");
    let info = s
        .join(".git")
        .join("modules")
        .join("sub")
        .join("objects")
        .join("info");
    let provider = a.join(".git").join("objects");
    write(
        &info.join("alternates"),
        &format!("{}\n", provider.to_string_lossy()),
    );
    assert!(provider.is_dir());

    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &a).no_dependents, Tri::No);
    assert_eq!(action_for(&res, &a), Action::Ask);
}

#[test]
fn q1_control_two_unrelated_clean_clones_stay_archivable() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let b = pushed_repo(&root, &remotes, "b");
    // An ordinary installed package next to them changes nothing.
    let dep = app_with_dep(&root);
    write(&dep.join("package.json"), "{}\n");
    write(&dep.join("index.js"), "x\n");

    let res = scan_with(&root, network_everywhere());
    assert_eq!(action_for(&res, &a), Action::BackupThenRemove);
    assert_eq!(action_for(&res, &b), Action::BackupThenRemove);
    // With the trust flag and a partial process check they are still archivable.
    let mut o = PlanOptions::new(7, far_future());
    o.process = ProcessCheck::Partial {
        cwds: vec![],
        unreadable: 1,
    };
    o.trust_partial_process_check = true;
    let p = plan(&res, &o);
    assert_eq!(
        item(&p, &a).action,
        Action::BackupThenRemove,
        "{}",
        item(&p, &a).reason
    );
    assert_eq!(
        item(&p, &b).action,
        Action::BackupThenRemove,
        "{}",
        item(&p, &b).reason
    );
}

// ---------------------------------------------------------------- Q3

/// The facts of a scan that must not depend on how the roots overlap.
#[derive(Debug, PartialEq)]
struct Summary {
    files: u64,
    dirs: u64,
    allocated: u64,
    logical: u64,
    node_count: usize,
    nodes: Vec<(String, u64)>,
    big_files: Vec<String>,
}

fn summary(res: &ScanResult) -> Summary {
    let mut nodes: Vec<(String, u64)> = res
        .nodes
        .iter()
        .map(|n| (n.path.clone(), n.bytes_total))
        .collect();
    nodes.sort();
    let mut big_files: Vec<String> = res.big_files.iter().map(|b| b.path.clone()).collect();
    big_files.sort();
    Summary {
        files: res.totals.files,
        dirs: res.totals.dirs,
        allocated: res.totals.allocated_bytes,
        logical: res.totals.logical_bytes,
        node_count: res.nodes.len(),
        nodes,
        big_files,
    }
}

fn plan_bytes(res: &ScanResult) -> (u64, Vec<(String, u64)>) {
    let p = plan_of(res);
    let mut items: Vec<(String, u64)> = p
        .items
        .iter()
        .map(|i| (i.path.clone(), i.reclaimable_bytes))
        .collect();
    items.sort();
    (p.totals.reclaimable_bytes, items)
}

#[test]
fn q3_a_parent_and_child_root_count_like_the_parent_alone() {
    let (_tmp, root, remotes) = workspace();
    let app = pushed_repo(&root, &remotes, "app");
    // Untracked work, so the repo gets a removing action with bytes behind it.
    write_bytes(&app.join("data.bin"), 300_000);
    let loose = root.join("loose");
    write(&loose.join("package.json"), "{}\n");
    write_bytes(
        &loose.join("node_modules").join("pkg").join("index.js"),
        50_000,
    );
    // Settle the index once, so no scan below sees it rewritten.
    git(&app, &["status", "--porcelain"]);

    let alone = scan_roots(vec![root.clone()]);
    for roots in [
        vec![root.clone(), app.clone()],
        vec![app.clone(), root.clone()],
        vec![root.clone(), loose.join("node_modules")],
        // The same directory spelled twice.
        vec![root.clone(), app.join("..")],
    ] {
        let both = scan_roots(roots.clone());
        assert_eq!(summary(&both), summary(&alone), "roots {roots:?}");
        assert_eq!(plan_bytes(&both), plan_bytes(&alone), "roots {roots:?}");
    }
}
