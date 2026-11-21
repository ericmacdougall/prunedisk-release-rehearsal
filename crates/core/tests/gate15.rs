//! Regression tests for the coordinator's follow-up to the gate review of d1e3f23 (PR #2).
//!
//! X1/X2: a file a person put inside one of the git directory's own subdirectories
//! (`rr-cache/`, `objects/`, a fanout directory, `objects/pack/`, `refs/`, `logs/`) exists
//! nowhere else, so the repository asks, naming it. Whatever stock git operations write there
//! (`gc`, a bitmap repack, a commit-graph, a multi-pack-index, a recorded rerere resolution,
//! packed refs) stays archivable. Failure messages list what the git directory holds on the
//! system that ran them, so a name git writes that the shape rejects can be read off CI.
//!
//! Every fixture uses real git.

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

/// One fact by its JSON name, `missing` when the model has no such fact.
fn fact(res: &ScanResult, repo: &Path, name: &str) -> String {
    let v = serde_json::to_value(facts_of(res, repo)).unwrap();
    v.get(name)
        .and_then(|x| x.as_str())
        .unwrap_or("missing")
        .to_string()
}

/// Run git where failure is expected (a conflicting merge); true when it succeeded.
fn git_ok(dir: &Path, args: &[&str]) -> bool {
    git_command()
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git must be installed to run these tests")
        .status
        .success()
}

/// Every path inside the repository's git directory except `hooks/`, sorted, with `/`
/// separators; loose objects are shown once per fanout directory.
fn listing(repo: &Path) -> String {
    let gd = repo.join(".git");
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![gd.clone()];
    while let Some(dir) = stack.pop() {
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for e in rd {
            let p = e.unwrap().path();
            let rel = p
                .strip_prefix(&gd)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if rel.starts_with("hooks/") {
                continue;
            }
            let is_dir = fs::symlink_metadata(&p)
                .map(|m| m.is_dir())
                .unwrap_or(false);
            if is_dir {
                stack.push(p.clone());
            }
            let loose = rel.starts_with("objects/") && rel.len() > 11 && &rel[10..11] == "/";
            out.push(if loose {
                format!("{}/<loose>", &rel[..10])
            } else {
                rel
            });
        }
    }
    out.sort();
    out.dedup();
    out.join(",")
}

/// A clone, inside the scan root, of a repository pushed to a bare remote outside it.
fn clone_of(tmp: &Path, root: &Path, remotes: &Path, name: &str) -> PathBuf {
    pushed_repo(&tmp.join("src"), remotes, name);
    let clone = root.join(name);
    let url = remotes.join(format!("{name}.git"));
    git(
        root,
        &[
            "clone",
            "-q",
            url.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    clone
}

/// The plan for `repo` must ask, name `what`, reclaim nothing and set the fact to `no`.
fn assert_asks_naming(res: &ScanResult, repo: &Path, what: &str) {
    let p = plan_of(res);
    let it = item(&p, repo);
    let inv = listing(repo);
    assert_eq!(it.action, Action::Ask, "{}\ngit dir: {inv}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    assert!(it.reason.contains(what), "{}\ngit dir: {inv}", it.reason);
    assert_eq!(
        fact(res, repo, "no_local_admin_content"),
        "no",
        "{}",
        it.reason
    );
}

/// The plan for `repo`, after `what` was done to it, must still archive.
fn assert_archives(res: &ScanResult, repo: &Path, what: &str) {
    let p = plan_of(res);
    let it = item(&p, repo);
    let inv = listing(repo);
    assert_eq!(
        it.action,
        Action::BackupThenRemove,
        "after {what}: {}\ngit dir: {inv}",
        it.reason
    );
}

/// A stray file at `parts` inside the git directory of a clean pushed clone asks, naming it.
fn stray_asks(parts: &[&str], text: &str) {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let path = parts.iter().fold(repo.join(".git"), |p, part| p.join(part));
    write(&path, text);
    let res = scan_with(&root, network_everywhere());
    assert_asks_naming(&res, &repo, &parts.join("/"));
}

// ---------------------------------------------------------------- X2: stray files

#[test]
fn x2_a_note_in_rr_cache_asks() {
    stray_asks(&["rr-cache", "notes.txt"], "how I resolved it\n");
}

#[test]
fn x2_a_note_in_objects_asks() {
    stray_asks(&["objects", "notes.txt"], "which pack is which\n");
}

#[test]
fn x2_a_readme_in_a_fanout_directory_asks() {
    stray_asks(&["objects", "ab", "README"], "not an object\n");
}

#[test]
fn x2_a_backup_in_the_pack_directory_asks() {
    stray_asks(&["objects", "pack", "backup.tar"], "an archive\n");
}

#[test]
fn x2_a_file_in_refs_heads_that_is_not_a_ref_asks() {
    stray_asks(&["refs", "heads", "notes.txt"], "todo: rebase later\n");
}

#[test]
fn x2_a_note_in_logs_asks() {
    stray_asks(&["logs", "notes.txt"], "what I did today\n");
}

// ---------------------------------------------------------------- X2: stock controls

#[test]
fn x2_control_a_fresh_clone_is_archivable() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let res = scan_with(&root, network_everywhere());
    assert_archives(&res, &repo, "a fresh clone");
}

#[test]
fn x2_control_a_clone_after_gc_is_archivable() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    git(&repo, &["gc", "-q"]);
    let res = scan_with(&root, network_everywhere());
    assert_archives(&res, &repo, "git gc");
}

#[test]
fn x2_control_a_bitmap_repack_is_archivable() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    git(&repo, &["repack", "-a", "-d", "-q", "--write-bitmap-index"]);
    let res = scan_with(&root, network_everywhere());
    assert_archives(&res, &repo, "git repack -a -d --write-bitmap-index");
}

#[test]
fn x2_control_commit_graphs_are_archivable() {
    let (tmp, root, remotes) = workspace();
    let single = clone_of(tmp.path(), &root, &remotes, "single");
    git(&single, &["commit-graph", "write", "--reachable"]);
    let split = clone_of(tmp.path(), &root, &remotes, "split");
    git(&split, &["commit-graph", "write", "--reachable", "--split"]);
    let res = scan_with(&root, network_everywhere());
    assert_archives(&res, &single, "git commit-graph write --reachable");
    assert_archives(&res, &split, "git commit-graph write --reachable --split");
}

#[test]
fn x2_control_a_multi_pack_index_is_archivable() {
    let (tmp, root, remotes) = workspace();
    // A clone from a local path copies loose objects, so pack them first: a multi-pack-index
    // indexes packs, and git refuses to write one without any.
    let plain = clone_of(tmp.path(), &root, &remotes, "plain");
    git(&plain, &["repack", "-q", "-d"]);
    git(&plain, &["multi-pack-index", "write"]);
    let bitmap = clone_of(tmp.path(), &root, &remotes, "bitmap");
    git(&bitmap, &["repack", "-q", "-d"]);
    git(&bitmap, &["multi-pack-index", "write", "--bitmap"]);
    let res = scan_with(&root, network_everywhere());
    assert_archives(&res, &plain, "git multi-pack-index write");
    assert_archives(&res, &bitmap, "git multi-pack-index write --bitmap");
}

#[test]
fn x2_control_a_recorded_rerere_resolution_is_archivable() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    git(&repo, &["checkout", "-q", "-b", "side"]);
    commit_file(&repo, "README.md", "side\n");
    git(&repo, &["checkout", "-q", "main"]);
    commit_file(&repo, "README.md", "main\n");
    // `rerere.*` is not an allowed repository config key, so it is set per command.
    let merged = git_ok(&repo, &["-c", "rerere.enabled=true", "merge", "-q", "side"]);
    assert!(
        !merged,
        "the merge must conflict for rerere to record anything"
    );
    write(&repo.join("README.md"), "both\n");
    git(&repo, &["add", "README.md"]);
    git(
        &repo,
        &["-c", "rerere.enabled=true", "commit", "-q", "--no-edit"],
    );
    git(&repo, &["branch", "-q", "-d", "side"]);
    git(&repo, &["push", "-q", "origin", "main"]);
    let names = listing(&repo);
    assert!(
        names.contains("/preimage") && names.contains("/postimage"),
        "rerere recorded no resolution: {names}"
    );
    let res = scan_with(&root, network_everywhere());
    assert_archives(&res, &repo, "a recorded rerere resolution");
}

#[test]
fn x2_control_packed_refs_are_archivable() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    git(&repo, &["pack-refs", "--all"]);
    let res = scan_with(&root, network_everywhere());
    assert_archives(&res, &repo, "git pack-refs --all");
}
