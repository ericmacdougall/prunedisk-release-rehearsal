//! Regression tests for the gate review of d1e3f23 (PR #2).
//!
//! W1: files of a repository's own in its git directory (a custom hook, a note in `info/`, a
//! real `info/exclude` pattern, an edited `description`, a branch description or identity in
//! the config, an unsent tag message) exist nowhere else, so the repository asks, naming them.
//! A fresh clone, exactly as git creates it on each system, stays archivable.
//!
//! W2: a pseudoref naming an annotated tag object (or a tag of a tag) keeps the tag's own
//! content alive even when the commit it points to is pushed, so it cannot be shown saved.
//!
//! Low finding: a pseudoref whose name cannot be shown exactly is refused (ask), so the names
//! listed for recovery are always the exact names.
//!
//! Every fixture uses real git. Each test that fails on d1e3f23 does so by assertion: new state
//! is read from the serialized facts and the plan's reason text, not from new fields. Failure
//! messages carry the stock template git installed on the system that ran them.

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

/// The names in a directory, sorted (`-` when it is absent).
fn names_in(dir: &Path) -> String {
    match fs::read_dir(dir) {
        Ok(rd) => {
            let mut v: Vec<String> = rd
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v.join(",")
        }
        Err(_) => "-".to_string(),
    }
}

/// What git put in a repository's git directory: hooks, info, description and local config.
fn inventory(repo: &Path) -> String {
    let gd = repo.join(".git");
    let desc = fs::read(gd.join("description"))
        .map(|b| format!("{:?}", String::from_utf8_lossy(&b)))
        .unwrap_or_else(|_| "-".to_string());
    let exclude = fs::read(gd.join("info").join("exclude"))
        .map(|b| format!("{:?}", String::from_utf8_lossy(&b)))
        .unwrap_or_else(|_| "-".to_string());
    let config = git(repo, &["config", "--local", "--name-only", "--list"]).replace('\n', ",");
    format!(
        "top=[{}] hooks=[{}] info=[{}] description={desc} exclude={exclude} config=[{config}]",
        names_in(&gd),
        names_in(&gd.join("hooks")),
        names_in(&gd.join("info"))
    )
}

/// A fresh clone of a pushed repository, as git creates it: the clone, and the git version with
/// the inventories of the repository `git init` made and of the clone.
fn fresh_clone(tmp: &Path, root: &Path, remotes: &Path) -> (PathBuf, String) {
    let src = pushed_repo(&tmp.join("src"), remotes, "origin-src");
    fs::create_dir_all(root).unwrap();
    let clone = root.join("clone");
    git(
        root,
        &[
            "clone",
            "-q",
            remotes.join("origin-src.git").to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    let inv = format!(
        "git version {}; init: {}; clone: {}",
        git(root, &["--version"]),
        inventory(&src),
        inventory(&clone)
    );
    (clone, inv)
}

/// The plan for `repo` must ask, name `what`, reclaim nothing and set the fact to `no`.
fn assert_asks_naming(res: &ScanResult, repo: &Path, what: &str, inv: &str) {
    let p = plan_of(res);
    let it = item(&p, repo);
    assert_eq!(it.action, Action::Ask, "{}\nstock: {inv}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    assert!(it.reason.contains(what), "{}\nstock: {inv}", it.reason);
    assert_eq!(
        fact(res, repo, "no_local_admin_content"),
        "no",
        "{}",
        it.reason
    );
}

// ---------------------------------------------------------------- W1: local git-dir content

#[test]
fn w1_a_custom_pre_commit_hook_asks_naming_it() {
    let (tmp, root, remotes) = workspace();
    let (_clone, inv) = fresh_clone(tmp.path(), &tmp.path().join("probe"), &remotes);
    let repo = pushed_repo(&root, &remotes, "app");
    let hook = repo.join(".git").join("hooks").join("pre-commit");
    write(
        &hook,
        "#!/bin/sh\n# a check of this project's own\nexit 0\n",
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let res = scan_with(&root, network_everywhere());
    assert_asks_naming(&res, &repo, "hooks/pre-commit", &inv);
}

#[test]
fn w1_a_note_in_info_asks_naming_it() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    write(
        &repo.join(".git").join("info").join("notes.txt"),
        "kept only here\n",
    );
    let res = scan_with(&root, network_everywhere());
    assert_asks_naming(&res, &repo, "info/notes.txt", "-");
}

#[test]
fn w1_a_real_pattern_in_info_exclude_asks() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let exclude = repo.join(".git").join("info").join("exclude");
    let mut text = fs::read_to_string(&exclude).unwrap_or_default();
    text.push_str("local-secrets.db\n");
    write(&exclude, &text);
    let res = scan_with(&root, network_everywhere());
    assert_asks_naming(&res, &repo, "info/exclude", "-");
}

#[test]
fn w1_an_edited_description_asks() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    write(
        &repo.join(".git").join("description"),
        "The billing service, private notes\n",
    );
    let res = scan_with(&root, network_everywhere());
    assert_asks_naming(&res, &repo, "description", "-");
}

#[test]
fn w1_a_branch_description_and_a_repository_identity_ask() {
    let (_tmp, root, remotes) = workspace();
    let described = pushed_repo(&root, &remotes, "described");
    git(
        &described,
        &[
            "config",
            "branch.main.description",
            "what this branch is for",
        ],
    );
    let identity = pushed_repo(&root, &remotes, "identity");
    git(&identity, &["config", "user.email", "work@example.invalid"]);
    let res = scan_with(&root, network_everywhere());
    assert_asks_naming(&res, &described, "branch.", "-");
    assert_asks_naming(&res, &identity, "user.email", "-");
}

#[test]
fn w1_an_unsent_tag_message_asks() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    write(
        &repo.join(".git").join("TAG_EDITMSG"),
        "release notes that were never tagged\n",
    );
    let res = scan_with(&root, network_everywhere());
    assert_asks_naming(&res, &repo, "TAG_EDITMSG", "-");
}

#[test]
fn w1_control_a_fresh_clone_is_archivable_on_this_system() {
    let (tmp, root, remotes) = workspace();
    let (clone, inv) = fresh_clone(tmp.path(), &root, &remotes);
    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &clone);
    assert_eq!(
        it.action,
        Action::BackupThenRemove,
        "{}\nstock: {inv}",
        it.reason
    );
}

/// `git gc` (and auto-gc) runs `update-server-info`, which writes `info/refs`, a listing of the
/// refs: a repository that was ever garbage-collected must not ask for it. Fails against a rule
/// that accepts only an absent or empty `info/refs`. A note written there still asks.
#[test]
fn w1_control_a_garbage_collected_repo_is_archivable_and_a_note_in_info_refs_asks() {
    let (_tmp, root, remotes) = workspace();
    let gcd = pushed_repo(&root, &remotes, "gcd");
    git(&gcd, &["gc", "-q"]);
    let inv = inventory(&gcd);
    let noted = pushed_repo(&root, &remotes, "noted");
    write(
        &noted.join(".git").join("info").join("refs"),
        "remember to rotate the deploy key\n",
    );
    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &gcd);
    assert_eq!(
        it.action,
        Action::BackupThenRemove,
        "{}\ngc: {inv}",
        it.reason
    );
    assert_asks_naming(&res, &noted, "info/refs", &inv);
}

#[cfg(unix)]
#[test]
fn w1_control_a_symlinked_hook_is_already_not_plain() {
    let (tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let target = tmp.path().join("shared-hook");
    write(&target, "#!/bin/sh\nexit 0\n");
    std::os::unix::fs::symlink(&target, repo.join(".git").join("hooks").join("pre-push")).unwrap();
    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(fact(&res, &repo, "plain_layout"), "no", "{}", it.reason);
}

#[cfg(unix)]
#[test]
fn w1_an_unreadable_hooks_directory_is_unknown_and_asks() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let hooks = repo.join(".git").join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    fs::set_permissions(&hooks, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&hooks).is_ok() {
        // Running as root: permissions do not stop the read, so there is nothing to test.
        fs::set_permissions(&hooks, fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("hooks stayed readable at mode 000 (root?): test not meaningful here");
        return;
    }
    let res = scan_with(&root, network_everywhere());
    fs::set_permissions(&hooks, fs::Permissions::from_mode(0o755)).unwrap();
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    assert_eq!(
        fact(&res, &repo, "no_local_admin_content"),
        "unknown",
        "{}",
        it.reason
    );
}

// ---------------------------------------------------------------- W2: tag objects

#[test]
fn w2_orig_head_naming_an_annotated_tag_object_asks() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    git(
        &repo,
        &["tag", "-a", "v1", "-m", "a signed-off note kept only here"],
    );
    let tag = git(&repo, &["rev-parse", "v1"]);
    assert_eq!(git(&repo, &["cat-file", "-t", &tag]), "tag");
    git(&repo, &["tag", "-d", "v1"]);
    write(&repo.join(".git").join("ORIG_HEAD"), &format!("{tag}\n"));

    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert!(it.reason.contains("ORIG_HEAD"), "{}", it.reason);
    assert!(it.reason.contains("tag"), "{}", it.reason);
}

#[test]
fn w2_orig_head_naming_a_tag_of_a_tag_asks() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    git(&repo, &["tag", "-a", "inner", "-m", "inner note"]);
    git(&repo, &["tag", "-a", "outer", "inner", "-m", "outer note"]);
    let outer = git(&repo, &["rev-parse", "outer"]);
    assert_eq!(git(&repo, &["cat-file", "-t", &outer]), "tag");
    git(&repo, &["tag", "-d", "outer"]);
    git(&repo, &["tag", "-d", "inner"]);
    write(&repo.join(".git").join("ORIG_HEAD"), &format!("{outer}\n"));

    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert!(it.reason.contains("tag"), "{}", it.reason);
}

#[test]
fn w2_control_orig_head_naming_a_pushed_commit_archives() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let head = git(&repo, &["rev-parse", "HEAD"]);
    write(&repo.join(".git").join("ORIG_HEAD"), &format!("{head}\n"));
    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::BackupThenRemove, "{}", it.reason);
}

// ---------------------------------------------------------------- exact pseudoref names

#[cfg(unix)]
#[test]
fn low_a_pseudoref_whose_name_cannot_be_shown_exactly_asks() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    commit_file(&repo, "unpushed.txt", "work that exists nowhere else\n");
    let b = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["reset", "-q", "--hard", "HEAD~1"]);
    git(&repo, &["reflog", "expire", "--expire=all", "--all"]);
    fs::remove_file(repo.join(".git").join("ORIG_HEAD")).unwrap();
    // An escape character: shown as a space, so the shown name is not the file's name.
    let odd = repo.join(".git").join("ODD\u{1b}_HEAD");
    if let Err(e) = fs::write(&odd, format!("{b}\n")) {
        if cfg!(target_os = "linux") {
            panic!("Linux must accept the name: {e}");
        }
        eprintln!("this file system refused the name ({e}): test not meaningful here");
        return;
    }

    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    let v = serde_json::to_value(node(&res, &repo).repo.as_ref().unwrap()).unwrap();
    let unsaved = v["unsaved_pseudorefs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        unsaved.is_empty(),
        "a name that is not the file's exact name must not be listed for recovery: {unsaved:?}"
    );
}
