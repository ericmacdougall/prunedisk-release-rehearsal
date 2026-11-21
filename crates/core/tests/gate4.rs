//! Regression tests for the fourth merge-gate review of PR #2 (findings K1 and K2).

mod common;

use common::*;
use disk_cleaner_core::Action;
use std::fs;
use std::path::PathBuf;

// ---------------------------------------------------------------- K1: root inside a repo

/// A repo whose `packages/app` holds a project and a `node_modules` with a dependency file.
/// Returns (tmp, directory holding the repo, the repo, the app).
fn enclosing_repo_fixture(track_deps: bool) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let (tmp, root, _remotes) = workspace();
    let repo = root.join("repo");
    init_repo(&repo);
    let app = repo.join("packages").join("app");
    write(&app.join("package.json"), "{}\n");
    let dep = app.join("node_modules").join("dep").join("index.js");
    if track_deps {
        write(&dep, "module.exports = 1;\n");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "app with vendored deps"]);
        // A local edit to a tracked dependency file: unique work.
        write(&dep, "module.exports = 2; // local patch\n");
    } else {
        write(&repo.join(".gitignore"), "node_modules/\n");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "app"]);
        write(&dep, "module.exports = 1;\n");
    }
    (tmp, root, repo, app)
}

#[test]
fn k1_tracked_and_edited_dependency_blocks_removal_in_a_full_and_in_a_subtree_scan() {
    let (_tmp, root, repo, app) = enclosing_repo_fixture(true);
    let nm = app.join("node_modules");

    // Full-repo scan: git knows node_modules is tracked.
    let full = scan_with(&root, network_everywhere());
    assert_eq!(action_for(&full, &nm), Action::Ask, "full-repo scan");

    // Subtree scan: the repo is above the root, but the answer must be the same.
    let sub = scan_with(&app, network_everywhere());
    assert_eq!(action_for(&sub, &nm), Action::Ask, "subtree scan");
    let reason = node(&sub, &nm).ask_reason.clone().unwrap_or_default();
    assert!(!reason.is_empty());
    assert!(repo.exists());
}

#[test]
fn k1_wholly_ignored_untracked_node_modules_in_a_subtree_stays_removable() {
    let (_tmp, _root, _repo, app) = enclosing_repo_fixture(false);
    let sub = scan_with(&app, network_everywhere());
    assert_eq!(
        action_for(&sub, &app.join("node_modules")),
        Action::RemoveRegenerable,
        "control: git says it is ignored and untracked"
    );
}

#[test]
fn k1_the_scan_records_that_the_root_is_inside_a_repo_it_did_not_cover() {
    let (_tmp, _root, repo, app) = enclosing_repo_fixture(false);
    let sub = scan_with(&app, network_everywhere());
    let v = serde_json::to_value(&sub).unwrap();
    let list = v["enclosing_repos"].as_array().expect("enclosing_repos");
    assert_eq!(list.len(), 1, "{list:?}");
    let text = list[0].to_string();
    assert!(text.contains(&repo.file_name().unwrap().to_string_lossy().into_owned()));
    // The enclosing repo itself never gets a plan item.
    let p = disk_cleaner_core::plan(&sub, &disk_cleaner_core::PlanOptions::new(7, far_future()));
    assert!(p.items.iter().all(|i| !i.class.is_git()));
}

#[test]
fn k1_enclosing_repo_with_a_refused_config_makes_the_artifact_ask() {
    let (_tmp, _root, repo, app) = enclosing_repo_fixture(false);
    let cfg = repo.join(".git").join("config");
    let mut text = fs::read_to_string(&cfg).unwrap();
    text.push_str("[filter \"evil\"]\n\tclean = true\n");
    fs::write(&cfg, text).unwrap();
    let sub = scan_with(&app, network_everywhere());
    assert_eq!(
        action_for(&sub, &app.join("node_modules")),
        Action::Ask,
        "ownership could not be established"
    );
}

// ---------------------------------------------------------------- K2: odd .git entries

/// An external git directory (outside the scan root) with one commit.
#[cfg(unix)]
fn external_git(tmp: &std::path::Path) -> PathBuf {
    let ext = tmp.join("ext");
    init_repo(&ext);
    ext.join(".git")
}

#[cfg(unix)]
#[test]
fn k2_symlinked_dot_git_inside_a_dependency_directory_blocks_removal() {
    let (tmp, root, _remotes) = workspace();
    let ext = external_git(tmp.path());
    let proj = root.join("proj");
    write(&proj.join("package.json"), "{}\n");
    let pkg = proj.join("node_modules").join("pkg");
    write(&pkg.join("index.js"), "local source edits\n");
    std::os::unix::fs::symlink(&ext, pkg.join(".git")).unwrap();
    let res = scan_plain(&root);
    let nm = proj.join("node_modules");
    assert_eq!(action_for(&res, &nm), Action::Ask);
    assert!(!action_for(&res, &proj).removes());
}

#[cfg(unix)]
#[test]
fn k2_a_project_whose_dot_git_is_a_symlink_is_not_plain_loose_code() {
    let (tmp, root, _remotes) = workspace();
    let ext = external_git(tmp.path());
    let proj = root.join("proj");
    write(&proj.join("package.json"), "{}\n");
    write(&proj.join("node_modules").join("dep").join("x.js"), "x\n");
    std::os::unix::fs::symlink(&ext, proj.join(".git")).unwrap();
    let res = scan_plain(&root);
    let nm = proj.join("node_modules");
    assert_eq!(action_for(&res, &nm), Action::Ask);
    assert!(!action_for(&res, &proj).removes());
    // No git was run through the symlink: its facts are unknown, with a note.
    let n = node(&res, &proj);
    let notes = n
        .repo
        .as_ref()
        .map(|r| r.notes.join(" "))
        .unwrap_or_default();
    assert!(notes.contains(".git"), "{notes}");
}

#[cfg(unix)]
#[test]
fn k2_a_symlinked_dot_git_at_the_scan_root_is_not_plain_loose_code() {
    let (tmp, root, _remotes) = workspace();
    let ext = external_git(tmp.path());
    write(&root.join("package.json"), "{}\n");
    write(&root.join("node_modules").join("dep").join("x.js"), "x\n");
    std::os::unix::fs::symlink(&ext, root.join(".git")).unwrap();
    let res = scan_plain(&root);
    assert_eq!(action_for(&res, &root.join("node_modules")), Action::Ask);
}

#[test]
fn k2_control_an_ordinary_package_without_dot_git_stays_removable() {
    let (_tmp, root, _remotes) = workspace();
    let proj = root.join("proj");
    write(&proj.join("package.json"), "{}\n");
    write(
        &proj.join("node_modules").join("pkg").join("index.js"),
        "x\n",
    );
    let res = scan_plain(&root);
    assert_eq!(
        action_for(&res, &proj.join("node_modules")),
        Action::RemoveRegenerable
    );
}
