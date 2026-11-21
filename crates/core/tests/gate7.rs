//! Regression tests for the seventh merge-gate review of PR #2: the positive `plain_layout`
//! fact (only a plain repository may be archived), object stores reached from non-plain repos,
//! refs that point at non-commits, and the scope disclosure.
//!
//! The new fact is read from the serialized form so these tests compile on the old model.

mod common;

use common::*;
use disk_cleaner_core::{plan, Action, PlanOptions, ScanResult};
use std::fs;
use std::path::{Path, PathBuf};

fn layout(res: &ScanResult, repo: &Path) -> String {
    let v = serde_json::to_value(facts_of(res, repo)).unwrap();
    v["plain_layout"].as_str().unwrap_or("missing").to_string()
}

fn never_archivable(res: &ScanResult, repo: &Path) -> bool {
    !matches!(
        action_for(res, repo),
        Action::BackupThenRemove | Action::RecoverBackupThenRemove | Action::RemoveRegenerable
    )
}

// ---------------------------------------------------------------- the gate's scenarios

#[cfg(unix)]
mod unix_layouts {
    use super::*;
    use std::os::unix::fs::symlink;

    /// A repo in `root` whose `.git/objects` is a symlink to `objects`, with an unpushed commit.
    fn repo_linked_to(root: &Path, name: &str, objects: &Path) -> PathBuf {
        let b = root.join(name);
        fs::create_dir_all(&b).unwrap();
        git(&b, &["init", "-q", "-b", "main"]);
        let objs = b.join(".git").join("objects");
        fs::remove_dir_all(&objs).unwrap();
        symlink(objects, &objs).unwrap();
        write(&b.join("f.txt"), &format!("work of {name}\n"));
        git(&b, &["add", "f.txt"]);
        git(&b, &["commit", "-q", "-m", "unpushed work"]);
        b
    }

    #[test]
    fn two_symlinked_borrowers_of_an_external_store_that_borrows_from_a_third_repo() {
        let (tmp, root, remotes) = workspace();
        let c = pushed_repo(&root, &remotes, "c");
        // An object store outside the scan whose alternates name the scanned repo C.
        let store = tmp.path().join("store.git");
        fs::create_dir_all(&store).unwrap();
        git(&store, &["init", "-q", "--bare", "-b", "main"]);
        let objs = store.join("objects");
        fs::write(
            objs.join("info").join("alternates"),
            format!(
                "{}\n",
                fs::canonicalize(c.join(".git/objects")).unwrap().display()
            ),
        )
        .unwrap();
        let _a = repo_linked_to(&root, "a", &objs);
        let _b = repo_linked_to(&root, "b", &objs);
        let res = scan_with(&root, network_everywhere());
        assert_ne!(action_for(&res, &c), Action::BackupThenRemove);
        assert!(never_archivable(&res, &root.join("a")));
        assert!(never_archivable(&res, &root.join("b")));
    }

    #[test]
    fn a_separate_git_dir_clone_outside_the_scan_borrowing_from_a_scanned_provider() {
        let (tmp, root, remotes) = workspace();
        let p = pushed_repo(&root, &remotes, "p");
        let gitdirs = tmp.path().join("gitdirs");
        fs::create_dir_all(&gitdirs).unwrap();
        let b = root.join("borrower");
        git(
            &root,
            &[
                "clone",
                "-q",
                "--shared",
                "--separate-git-dir",
                gitdirs.join("b.git").to_str().unwrap(),
                p.to_str().unwrap(),
                b.to_str().unwrap(),
            ],
        );
        assert!(b.join(".git").is_file(), "the clone must use a .git file");
        let head = git(&p, &["rev-parse", "HEAD"]);
        git(&b, &["cat-file", "-e", &head]);
        let res = scan_with(&root, network_everywhere());
        assert_ne!(action_for(&res, &p), Action::BackupThenRemove);
        assert_eq!(layout(&res, &b), "no");
        assert_eq!(action_for(&res, &b), Action::Ask);
    }

    #[test]
    fn a_symlinked_head_or_objects_directory_is_not_plain() {
        let (tmp, root, remotes) = workspace();
        let a = pushed_repo(&root, &remotes, "a");
        let real = tmp.path().join("real-objects");
        fs::rename(a.join(".git/objects"), &real).unwrap();
        symlink(&real, a.join(".git/objects")).unwrap();
        let res = scan_with(&root, network_everywhere());
        assert_ne!(layout(&res, &a), "yes");
        assert!(never_archivable(&res, &a));
    }
}

// ---------------------------------------------------------------- refs that are not commits

fn commit_oid(repo: &Path) -> String {
    git(repo, &["rev-parse", "HEAD"])
}

fn blob(repo: &Path, text: &str) -> String {
    write(&repo.join("scratch.bin"), text);
    let oid = git(repo, &["hash-object", "-w", "scratch.bin"]);
    fs::remove_file(repo.join("scratch.bin")).unwrap();
    oid
}

#[test]
fn a_unique_blob_under_refs_backup_and_non_commit_replace_refs_are_not_plain() {
    for (what, setup) in [
        (
            "refs/backup/important -> blob",
            Box::new(|r: &Path| {
                let b = blob(r, "unique content\n");
                git(r, &["update-ref", "refs/backup/important", &b]);
            }) as Box<dyn Fn(&Path)>,
        ),
        (
            "refs/replace/<oid> -> blob",
            Box::new(|r: &Path| {
                let b = blob(r, "replacement\n");
                let c = commit_oid(r);
                git(r, &["update-ref", &format!("refs/replace/{c}"), &b]);
            }),
        ),
        (
            "refs/replace/<oid> -> tree",
            Box::new(|r: &Path| {
                let c = commit_oid(r);
                let t = git(r, &["rev-parse", "HEAD^{tree}"]);
                git(r, &["update-ref", &format!("refs/replace/{c}"), &t]);
            }),
        ),
    ] {
        let (_tmp, root, remotes) = workspace();
        let repo = pushed_repo(&root, &remotes, "app");
        setup(&repo);
        let res = scan_with(&root, network_everywhere());
        assert_ne!(layout(&res, &repo), "yes", "{what}");
        assert_eq!(action_for(&res, &repo), Action::Ask, "{what}");
    }
}

// ---------------------------------------------------------------- one mutation per check

#[test]
fn every_shape_check_fails_on_its_own_mutation() {
    type Mutation = (&'static str, Box<dyn Fn(&Path)>);
    let mutations: Vec<Mutation> = vec![
        (
            "alternates file",
            Box::new(|r: &Path| {
                write(
                    &r.join(".git/objects/info/alternates"),
                    "/nowhere/objects\n",
                )
            }),
        ),
        (
            "http-alternates file",
            Box::new(|r: &Path| {
                write(&r.join(".git/objects/info/http-alternates"), "http://h/x\n")
            }),
        ),
        (
            "commondir file",
            Box::new(|r: &Path| write(&r.join(".git/commondir"), "../other/.git\n")),
        ),
        (
            "info/grafts",
            Box::new(|r: &Path| write(&r.join(".git/info/grafts"), "\n")),
        ),
        (
            "non-empty worktrees dir",
            Box::new(|r: &Path| write(&r.join(".git/worktrees/x/gitdir"), "/x/.git\n")),
        ),
        (
            "non-empty modules dir",
            Box::new(|r: &Path| write(&r.join(".git/modules/m/HEAD"), "ref: refs/heads/main\n")),
        ),
        (
            "annex directory",
            Box::new(|r: &Path| fs::create_dir_all(r.join(".git/annex")).unwrap()),
        ),
        (
            "lfs objects",
            Box::new(|r: &Path| write(&r.join(".git/lfs/objects/aa/bb/aabb"), "content")),
        ),
        (
            "missing objects/pack",
            Box::new(|r: &Path| fs::remove_dir_all(r.join(".git/objects/pack")).unwrap()),
        ),
        (
            "refs/notes",
            Box::new(|r: &Path| {
                let c = commit_oid(r);
                git(r, &["update-ref", "refs/notes/commits", &c]);
            }),
        ),
        (
            "refs/original",
            Box::new(|r: &Path| {
                let c = commit_oid(r);
                git(r, &["update-ref", "refs/original/refs/heads/main", &c]);
            }),
        ),
        (
            "refs/replace to a commit",
            Box::new(|r: &Path| {
                let c = commit_oid(r);
                git(r, &["update-ref", &format!("refs/replace/{c}"), &c]);
            }),
        ),
        (
            "refs/bisect",
            Box::new(|r: &Path| {
                let c = commit_oid(r);
                git(r, &["update-ref", "refs/bisect/bad", &c]);
            }),
        ),
        (
            "refs/pull",
            Box::new(|r: &Path| {
                let c = commit_oid(r);
                git(r, &["update-ref", "refs/pull/1/head", &c]);
            }),
        ),
        (
            "tag ref to a blob",
            Box::new(|r: &Path| {
                let b = blob(r, "tagged blob\n");
                git(r, &["update-ref", "refs/tags/blobtag", &b]);
            }),
        ),
        (
            "tag ref to a tree",
            Box::new(|r: &Path| {
                let t = git(r, &["rev-parse", "HEAD^{tree}"]);
                git(r, &["update-ref", "refs/tags/treetag", &t]);
            }),
        ),
        (
            "annotated tag of a blob",
            Box::new(|r: &Path| {
                let b = blob(r, "annotated blob\n");
                git(r, &["tag", "-a", "-m", "blob tag", "annotated-blob", &b]);
            }),
        ),
    ];
    assert!(mutations.len() >= 15);
    for (what, mutate) in &mutations {
        let (_tmp, root, remotes) = workspace();
        let repo = pushed_repo(&root, &remotes, "app");
        mutate(&repo);
        let res = scan_with(&root, network_everywhere());
        let l = layout(&res, &repo);
        assert!(l == "no" || l == "unknown", "{what}: plain_layout {l}");
        assert!(
            never_archivable(&res, &repo),
            "{what}: {:?}",
            action_for(&res, &repo)
        );
        assert_eq!(action_for(&res, &repo), Action::Ask, "{what}");
    }
}

#[test]
fn a_linked_worktree_and_its_main_repo_ask() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let wt = root.join("wt");
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()],
    );
    let res = scan_with(&root, network_everywhere());
    assert_eq!(layout(&res, &wt), "no");
    assert_eq!(action_for(&res, &wt), Action::Ask);
    assert_eq!(action_for(&res, &repo), Action::Ask);
}

// ---------------------------------------------------------------- controls

#[test]
fn control_a_fresh_plain_clone_and_a_repo_with_an_annotated_tag_are_plain() {
    let (_tmp, root, remotes) = workspace();
    let origin = pushed_repo(&root, &remotes, "origin-copy");
    let clone = root.join("clone");
    git(
        &root,
        &[
            "clone",
            "-q",
            remotes.join("origin-copy.git").to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    let tagged = pushed_repo(&root, &remotes, "tagged");
    git(&tagged, &["tag", "-a", "-m", "release", "v1"]);
    let res = scan_with(&root, network_everywhere());
    assert_eq!(layout(&res, &clone), "yes");
    assert_eq!(layout(&res, &origin), "yes");
    assert_eq!(layout(&res, &tagged), "yes");
    assert_eq!(action_for(&res, &clone), Action::BackupThenRemove);
    assert_eq!(action_for(&res, &origin), Action::BackupThenRemove);
}

// ---------------------------------------------------------------- scope disclosure

#[test]
fn the_plan_header_records_the_scope_and_archive_reasons_say_clones_were_not_checked() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let res = scan_with(&root, network_everywhere());
    let p = plan(&res, &PlanOptions::new(7, far_future()));
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["scope"], "subtree");
    let item = p
        .items
        .iter()
        .find(|i| i.path == repo.to_string_lossy())
        .unwrap();
    assert_eq!(item.action, Action::BackupThenRemove);
    assert!(
        item.reason.contains("outside the scanned folders"),
        "{}",
        item.reason
    );
}
