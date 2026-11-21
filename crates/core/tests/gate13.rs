//! Regression tests for the gate review of e3885ef (PR #2).
//!
//! U1: files at the top of the git directory that name a commit (`ORIG_HEAD` and any other
//! pseudoref) keep history alive after every ref and reflog has let go of it, and any entry there
//! that this tool does not know may keep data where it does not look.
//!
//! U2 (the part that needs no second file system): a scan root whose real path cannot be
//! established may lie inside any other walked directory, so the directories that could enclose
//! it ask. The mount half of U2 is in `mount_alias.rs`.
//!
//! Every fixture uses real git. Each test fails on e3885ef by assertion (new state is read from
//! the plan's reason text and the serialized facts, not from new fields).

mod common;

use common::*;
use disk_cleaner_core::{plan, Action, Plan, PlanItem, PlanOptions, ScanResult};
use std::fs;
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

/// One fact by its JSON name.
fn fact(res: &ScanResult, repo: &Path, name: &str) -> String {
    let v = serde_json::to_value(facts_of(res, repo)).unwrap();
    v.get(name)
        .and_then(|x| x.as_str())
        .unwrap_or("missing")
        .to_string()
}

/// Every regular file below `dir` (none when `dir` is absent).
fn files_below(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = match fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for e in rd {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                stack.push(e.path());
            } else {
                out.push(e.path());
            }
        }
    }
    out
}

/// No reflog entry is left anywhere: every file under `.git/logs` is empty or absent.
fn assert_no_reflog(repo: &Path) {
    for f in files_below(&repo.join(".git").join("logs")) {
        let len = fs::metadata(&f).unwrap().len();
        assert_eq!(len, 0, "reflog {} still has entries", f.display());
    }
}

/// A pushed repository with an unpushed commit B that `reset --hard` dropped from the branch,
/// and every reflog emptied with `git reflog expire --expire=all --all`. Returns (repo, B).
fn reset_with_expired_reflog(root: &Path, remotes: &Path, name: &str) -> (PathBuf, String) {
    let repo = pushed_repo(root, remotes, name);
    commit_file(&repo, "unpushed.txt", "work that exists nowhere else\n");
    let b = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["reset", "-q", "--hard", "HEAD~1"]);
    git(&repo, &["reflog", "expire", "--expire=all", "--all"]);
    assert_no_reflog(&repo);
    (repo, b)
}

// ---------------------------------------------------------------- U1: pseudorefs

#[test]
fn u1_orig_head_at_an_unpushed_commit_with_expired_reflogs_is_not_archived() {
    let (_tmp, root, remotes) = workspace();
    let (repo, b) = reset_with_expired_reflog(&root, &remotes, "app");
    let orig = fs::read_to_string(repo.join(".git").join("ORIG_HEAD")).unwrap();
    assert_eq!(orig.trim(), b, "ORIG_HEAD must name the dropped commit");

    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_ne!(
        it.action,
        Action::BackupThenRemove,
        "B survives only through ORIG_HEAD; archive would delete it: {}",
        it.reason
    );
    assert!(
        it.reason.contains("kept only by ORIG_HEAD"),
        "{}",
        it.reason
    );
    assert_eq!(fact(&res, &repo, "reflog_saved"), "no");
}

#[test]
fn u1_orig_head_at_an_unpushed_commit_with_reflogs_disabled_is_not_archived() {
    let (_tmp, root, remotes) = workspace();
    let repo = root.join("app");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    // Before the first commit: git appends to a reflog that exists whatever this setting says.
    git(&repo, &["config", "core.logAllRefUpdates", "false"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    commit_file(&repo, "README.md", "hello\n");
    let remote = remotes.join("app.git");
    init_bare(&remote);
    push_to_remote(&repo, &remote);
    commit_file(&repo, "unpushed.txt", "work that exists nowhere else\n");
    git(&repo, &["reset", "-q", "--hard", "HEAD~1"]);
    assert!(
        files_below(&repo.join(".git").join("logs")).is_empty(),
        "no reflog may exist"
    );
    assert!(repo.join(".git").join("ORIG_HEAD").is_file());

    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_ne!(it.action, Action::BackupThenRemove, "{}", it.reason);
    assert!(
        it.reason.contains("kept only by ORIG_HEAD"),
        "{}",
        it.reason
    );
}

#[test]
fn u1_control_a_pull_leaves_orig_head_and_fetch_head_at_pushed_commits_and_archives() {
    let (tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    // Someone else pushes; this clone pulls it (a fast-forward).
    let other = tmp.path().join("other");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            remotes.join("app.git").to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    git(&other, &["config", "user.name", "Test"]);
    git(&other, &["config", "user.email", "test@example.invalid"]);
    git(&other, &["config", "commit.gpgsign", "false"]);
    commit_file(&other, "theirs.txt", "from elsewhere\n");
    git(&other, &["push", "-q", "origin", "main"]);
    git(&repo, &["pull", "-q", "--ff-only", "origin", "main"]);
    for name in ["ORIG_HEAD", "FETCH_HEAD"] {
        assert!(
            repo.join(".git").join(name).is_file(),
            "{name} must exist for this control to mean anything"
        );
    }

    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::BackupThenRemove, "{}", it.reason);
}

#[test]
fn u1_control_orig_head_at_a_pushed_commit_with_expired_reflogs_archives() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    commit_file(&repo, "later.txt", "pushed too\n");
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["reset", "-q", "--hard", "HEAD~1"]);
    git(&repo, &["reflog", "expire", "--expire=all", "--all"]);
    assert_no_reflog(&repo);
    assert!(repo.join(".git").join("ORIG_HEAD").is_file());

    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::BackupThenRemove, "{}", it.reason);
}

#[test]
fn u1_an_unknown_pseudoref_at_an_unpushed_commit_is_not_archived() {
    let (_tmp, root, remotes) = workspace();
    let (repo, b) = reset_with_expired_reflog(&root, &remotes, "app");
    fs::remove_file(repo.join(".git").join("ORIG_HEAD")).unwrap();
    write(&repo.join(".git").join("FOO_HEAD"), &format!("{b}\n"));

    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_ne!(it.action, Action::BackupThenRemove, "{}", it.reason);
    assert!(it.reason.contains("FOO_HEAD"), "{}", it.reason);
}

#[test]
fn u1_a_pseudoref_naming_a_tree_cannot_be_checked_and_asks() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let tree = git(&repo, &["rev-parse", "HEAD^{tree}"]);
    write(&repo.join(".git").join("FOO_HEAD"), &format!("{tree}\n"));

    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert!(it.reason.contains("FOO_HEAD"), "{}", it.reason);
}

#[test]
fn u1_a_garbage_orig_head_asks() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    write(&repo.join(".git").join("ORIG_HEAD"), "not a commit id\n");

    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    let want = "ORIG_HEAD at the top of the git directory";
    assert!(it.reason.contains(want), "{}", it.reason);
}

#[test]
fn u1_an_unknown_directory_or_file_at_the_top_of_the_git_directory_makes_the_layout_not_plain() {
    let (_tmp, root, remotes) = workspace();
    let with_dir = pushed_repo(&root, &remotes, "with-dir");
    write(
        &with_dir.join(".git").join("stuff").join("kept.txt"),
        "data\n",
    );
    let with_file = pushed_repo(&root, &remotes, "with-file");
    write(&with_file.join(".git").join("notes.txt"), "data\n");
    let control = pushed_repo(&root, &remotes, "control");

    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    assert_eq!(
        item(&p, &control).action,
        Action::BackupThenRemove,
        "{}",
        item(&p, &control).reason
    );
    for (repo, name) in [(&with_dir, "stuff"), (&with_file, "notes.txt")] {
        let it = item(&p, repo);
        assert_eq!(it.action, Action::Ask, "{}", it.reason);
        assert_ne!(fact(&res, repo, "plain_layout"), "yes", "{}", it.reason);
        assert!(it.reason.contains(name), "{}", it.reason);
    }
}

// ------------------------------------------- U1 without a network remote (gate delta review)

/// A repository with commits and no remote at all.
fn local_only(root: &Path, name: &str) -> PathBuf {
    let repo = root.join(name);
    init_repo(&repo);
    repo
}

#[test]
fn u1_local_only_orig_head_commit_is_named_for_recovery() {
    let (_tmp, root, _remotes) = workspace();
    let repo = local_only(&root, "app");
    commit_file(&repo, "dropped.txt", "only ORIG_HEAD keeps this\n");
    git(&repo, &["reset", "-q", "--hard", "HEAD~1"]);
    git(&repo, &["reflog", "expire", "--expire=all", "--all"]);
    assert_no_reflog(&repo);

    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::RecoverBackupThenRemove, "{}", it.reason);
    assert!(
        it.reason.contains("kept only by ORIG_HEAD"),
        "recovery must be told to capture ORIG_HEAD: {}",
        it.reason
    );
    let v = serde_json::to_value(node(&res, &repo).repo.as_ref().unwrap()).unwrap();
    assert_eq!(v["unsaved_pseudorefs"], serde_json::json!(["ORIG_HEAD"]));
}

#[test]
fn u1_local_only_pseudorefs_that_cannot_be_checked_ask() {
    let (_tmp, root, _remotes) = workspace();
    let garbage = local_only(&root, "garbage");
    write(&garbage.join(".git").join("ORIG_HEAD"), "not a commit id\n");
    let tree = local_only(&root, "tree");
    let id = git(&tree, &["rev-parse", "HEAD^{tree}"]);
    write(&tree.join(".git").join("FOO_HEAD"), &format!("{id}\n"));

    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    for (repo, name) in [(&garbage, "ORIG_HEAD"), (&tree, "FOO_HEAD")] {
        let it = item(&p, repo);
        assert_eq!(it.action, Action::Ask, "{name}: {}", it.reason);
        let want = format!("{name} at the top of the git directory");
        assert!(it.reason.contains(&want), "{}", it.reason);
    }
}

// ---------------------------------------------------------------- U2: unresolvable root

#[cfg(unix)]
#[test]
fn u2_a_scan_root_without_a_real_path_makes_every_directory_that_could_enclose_it_ask() {
    let (tmp, root, _remotes) = workspace();
    let proj = root.join("proj");
    let nm = proj.join("node_modules");
    write(&proj.join("package.json"), "{}");
    write_bytes(&nm.join("dep").join("index.js"), 1024);
    // A root whose real path cannot be established: a symlink to nothing.
    let dangling = tmp.path().join("gone");
    std::os::unix::fs::symlink(tmp.path().join("nowhere"), &dangling).unwrap();
    assert!(fs::canonicalize(&dangling).is_err());

    let scan_roots = |roots: Vec<PathBuf>| {
        let mut o = disk_cleaner_core::ScanOptions::new(roots);
        o.facts = network_everywhere();
        disk_cleaner_core::scan(&o).unwrap()
    };
    // Control: alone, the artifact is removable.
    let alone = scan_roots(vec![root.clone()]);
    let p = plan_of(&alone);
    assert_eq!(
        item(&p, &nm).action,
        Action::RemoveRegenerable,
        "{}",
        item(&p, &nm).reason
    );

    let both = scan_roots(vec![root.clone(), dangling.clone()]);
    let p = plan_of(&both);
    let it = item(&p, &nm);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    assert!(it.reason.contains("real path"), "{}", it.reason);
}
