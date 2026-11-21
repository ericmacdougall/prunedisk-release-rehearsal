//! Regression tests for the third merge-gate review of PR #2 (findings J1 to J4).
//! Each test marked as failing first fails on the code the review was written against.

mod common;

use common::*;
use disk_cleaner_core::{plan, Action, PlanOptions, ProcessCheck, ScanResult, Tri};
use std::fs;
use std::path::{Path, PathBuf};

fn tri_of(res: &ScanResult, repo: &Path, name: &str) -> String {
    let v = serde_json::to_value(facts_of(res, repo)).unwrap();
    v[name].as_str().unwrap_or("missing").to_string()
}

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

// ---------------------------------------------------------------- J1: gitlinks

fn add_submodule(outer: &Path, url: &Path, name: &str) {
    git(
        outer,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            url.to_str().unwrap(),
            name,
        ],
    );
}

/// `outer` records submodule `sub` at commit A (committed and pushed). The submodule source has a
/// second pushed commit B, and `outer/sub` is checked out at B without touching the parent.
struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    outer: PathBuf,
}

fn submodule_fixture(checkout_second: bool) -> Fixture {
    let (tmp, root, remotes) = workspace();
    let srcs = tmp.path().join("srcs");
    fs::create_dir_all(&srcs).unwrap();
    let subsrc = pushed_repo(&srcs, &remotes, "subsrc");
    let outer = pushed_repo(&root, &remotes, "outer");
    add_submodule(&outer, &subsrc, "sub");
    git(&outer, &["commit", "-q", "-m", "add submodule"]);
    git(&outer, &["push", "-q", "origin", "main"]);
    commit_file(&subsrc, "b.txt", "second\n");
    git(&subsrc, &["push", "-q", "origin", "main"]);
    if checkout_second {
        let sub = outer.join("sub");
        git(
            &sub,
            &["-c", "protocol.file.allow=always", "fetch", "-q", "origin"],
        );
        git(&sub, &["checkout", "-q", "--detach", "origin/main"]);
    }
    Fixture {
        _tmp: tmp,
        root,
        outer,
    }
}

#[test]
fn j1_submodule_at_another_commit_than_the_parent_index_is_not_clean() {
    let f = submodule_fixture(true);
    let res = scan_with(&f.root, network_everywhere());
    assert_eq!(tri_of(&res, &f.outer, "tree_clean"), "no");
    assert_ne!(action_for(&res, &f.outer), Action::BackupThenRemove);
}

#[test]
fn j1_staged_gitlink_change_is_not_clean() {
    let f = submodule_fixture(true);
    git(&f.outer, &["add", "sub"]);
    let res = scan_with(&f.root, network_everywhere());
    assert_eq!(tri_of(&res, &f.outer, "tree_clean"), "no");
    assert_ne!(action_for(&res, &f.outer), Action::BackupThenRemove);
}

#[test]
fn j1_control_submodule_at_the_recorded_commit_is_clean() {
    let f = submodule_fixture(false);
    let res = scan_with(&f.root, network_everywhere());
    assert_eq!(tri_of(&res, &f.outer, "tree_clean"), "yes");
}

#[test]
fn j1_control_recorded_submodule_without_a_checkout_is_clean() {
    let f = submodule_fixture(false);
    git(&f.outer, &["submodule", "deinit", "-q", "-f", "sub"]);
    let res = scan_with(&f.root, network_everywhere());
    assert_eq!(tri_of(&res, &f.outer, "tree_clean"), "yes");
}

#[test]
fn j1_plain_directory_with_files_at_a_gitlink_path_is_not_clean() {
    let f = submodule_fixture(false);
    git(&f.outer, &["submodule", "deinit", "-q", "-f", "sub"]);
    write(&f.outer.join("sub").join("stray.txt"), "not in any repo\n");
    let res = scan_with(&f.root, network_everywhere());
    assert_eq!(tri_of(&res, &f.outer, "tree_clean"), "no");
    assert_ne!(action_for(&res, &f.outer), Action::BackupThenRemove);
}

// ---------------------------------------------------------------- J2: core.worktree

/// A pushed repo with `extensions.worktreeConfig`, the shared `core.worktree` naming the node and
/// the per-worktree file naming `per_worktree`.
fn worktree_config_repo(per_worktree: Option<&Path>) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    git(&repo, &["config", "core.repositoryformatversion", "1"]);
    let cfg = repo.join(".git").join("config");
    let mut text = fs::read_to_string(&cfg).unwrap();
    text.push_str(&format!(
        "[extensions]\n\tworktreeConfig = true\n[core]\n\tworktree = {}\n",
        slash(&repo)
    ));
    fs::write(&cfg, text).unwrap();
    if let Some(p) = per_worktree {
        fs::write(
            repo.join(".git").join("config.worktree"),
            format!("[core]\n\tworktree = {}\n", slash(p)),
        )
        .unwrap();
    }
    (tmp, root, repo)
}

#[test]
fn j2_worktree_config_overriding_core_worktree_makes_every_fact_unknown() {
    let elsewhere = tempfile::tempdir().unwrap();
    let (_tmp, root, repo) = worktree_config_repo(Some(elsewhere.path()));
    let res = scan_with(&root, network_everywhere());
    let facts = facts_of(&res, &repo);
    assert!(
        facts.entries().iter().all(|(_, t)| *t == Tri::Unknown),
        "{facts:?}"
    );
    assert_eq!(action_for(&res, &repo), Action::Ask);
}

#[test]
fn j2_control_matching_worktree_config_keeps_the_facts_known() {
    let (_tmp, root, repo) = worktree_config_repo(None);
    git(
        &repo,
        &["config", "--worktree", "core.worktree", &slash(&repo)],
    );
    let res = scan_with(&root, network_everywhere());
    assert_eq!(tri_of(&res, &repo, "has_network_remote"), "yes");
    assert_eq!(tri_of(&res, &repo, "tree_clean"), "yes");
}

// ---------------------------------------------------------------- J3: package evidence

fn proj_with(manifest: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (tmp, root, _) = workspace();
    let proj = root.join("proj");
    write(&proj.join(manifest), "x");
    (tmp, root, proj)
}

#[test]
fn j3_key_in_node_modules_outside_any_package_asks() {
    let (_t, root, proj) = proj_with("package.json");
    let nm = proj.join("node_modules");
    write(&nm.join("dep").join("package.json"), "{}");
    write(&nm.join("local").join("server.key"), "private");
    let res = scan_plain(&root);
    assert_eq!(action_for(&res, &nm), Action::Ask);
}

#[test]
fn j3_database_in_a_virtualenv_outside_site_packages_asks() {
    let (_t, root, proj) = proj_with("pyproject.toml");
    let venv = proj.join(".venv");
    write(&venv.join("runtime.db"), "rows");
    let res = scan_plain(&root);
    assert_eq!(action_for(&res, &venv), Action::Ask);
}

#[test]
fn j3_file_directly_in_site_packages_is_not_a_package_file() {
    let (_t, root, proj) = proj_with("pyproject.toml");
    let venv = proj.join(".venv");
    write(
        &venv
            .join("lib")
            .join("python3.12")
            .join("site-packages")
            .join("local.db"),
        "rows",
    );
    let res = scan_plain(&root);
    assert_eq!(action_for(&res, &venv), Action::Ask);
}

#[test]
fn j3_control_fixtures_inside_installed_packages_stay_removable() {
    let (_t, root, proj) = proj_with("package.json");
    let nm = proj.join("node_modules");
    write(&nm.join("pkg").join("package.json"), "{}");
    write(
        &nm.join("pkg")
            .join("test")
            .join("fixtures")
            .join("cert.pem"),
        "x",
    );
    // Scoped package with a nested dependency.
    write(&nm.join("@scope").join("p").join("package.json"), "{}");
    write(
        &nm.join("@scope")
            .join("p")
            .join("node_modules")
            .join("q")
            .join("package.json"),
        "{}",
    );
    write(
        &nm.join("@scope")
            .join("p")
            .join("node_modules")
            .join("q")
            .join("test")
            .join("a.key"),
        "x",
    );
    let res = scan_plain(&root);
    assert_eq!(action_for(&res, &nm), Action::RemoveRegenerable);
}

#[test]
fn j3_control_database_inside_an_installed_python_package_stays_removable() {
    let (_t, root, proj) = proj_with("pyproject.toml");
    let venv = proj.join(".venv");
    write(
        &venv
            .join("lib")
            .join("python3.12")
            .join("site-packages")
            .join("pkg")
            .join("data.db"),
        "rows",
    );
    let res = scan_plain(&root);
    assert_eq!(action_for(&res, &venv), Action::RemoveRegenerable);
}

// ---------------------------------------------------------------- J4: partial process check

#[test]
fn j4_partial_process_check_asks_and_leaves_no_ready_bytes() {
    let (_t, root, proj) = proj_with("package.json");
    let nm = proj.join("node_modules");
    write(&nm.join("dep").join("package.json"), "{}");
    let res = scan_plain(&root);
    let mut o = PlanOptions::new(7, far_future());
    o.process = ProcessCheck::Partial {
        cwds: vec![PathBuf::from("/somewhere/unrelated")],
        unreadable: 1,
    };
    let p = plan(&res, &o);
    let item = p
        .items
        .iter()
        .find(|i| i.path == nm.to_string_lossy())
        .unwrap();
    assert_eq!(item.action, Action::Ask, "{}", item.reason);
    assert!(
        item.reason.contains("could not be inspected"),
        "{}",
        item.reason
    );
    assert_eq!(p.totals.ready_bytes, 0);
    assert_eq!(p.totals.reclaimable_bytes, 0);
}

#[test]
fn j4_with_the_opt_in_a_partial_check_still_allows_removal_and_the_plan_says_so() {
    let (_t, root, proj) = proj_with("package.json");
    let nm = proj.join("node_modules");
    write(&nm.join("dep").join("package.json"), "{}");
    let res = scan_plain(&root);
    let mut o = PlanOptions::new(7, far_future());
    o.process = ProcessCheck::Partial {
        cwds: vec![PathBuf::from("/somewhere/unrelated")],
        unreadable: 1,
    };
    o.trust_partial_process_check = true;
    let p = plan(&res, &o);
    let item = p
        .items
        .iter()
        .find(|i| i.path == nm.to_string_lossy())
        .unwrap();
    assert_eq!(item.action, Action::RemoveRegenerable, "{}", item.reason);
    assert!(p.trusted_partial_process_check);
    assert!(p.totals.ready_bytes > 0);
    // A partial check that does find a process inside still keeps it.
    o.process = ProcessCheck::Partial {
        cwds: vec![nm.join("dep")],
        unreadable: 1,
    };
    let p = plan(&res, &o);
    let item = p
        .items
        .iter()
        .find(|i| i.path == nm.to_string_lossy())
        .unwrap();
    assert_eq!(item.action, Action::Keep);
}
