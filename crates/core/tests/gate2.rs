//! Regression tests for the second merge-gate review of PR #2 (findings H2 and H3) and the
//! sibling hunt that followed. Each test fails on the code the review was written against.
//!
//! The new facts are read from the serialized form (`fact`), so these tests compile against
//! either version of the model and fail on the old one by assertion, not by a build error.

mod common;

use common::*;
use disk_cleaner_core::Action;
use std::fs;
use std::path::{Path, PathBuf};

/// One fact by its JSON name, `"missing"` when the model has no such fact.
fn fact(res: &disk_cleaner_core::ScanResult, repo: &Path, name: &str) -> String {
    let v = serde_json::to_value(facts_of(res, repo)).unwrap();
    v.get(name)
        .and_then(|x| x.as_str())
        .unwrap_or("missing")
        .to_string()
}

fn action(res: &disk_cleaner_core::ScanResult, p: &Path) -> Action {
    action_for(res, p)
}

fn not_removing(a: Action) -> bool {
    matches!(a, Action::Ask | Action::Keep)
}

/// Contents of every file under `dir`, to prove a scan wrote nothing.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let ft = e.file_type().unwrap();
            if ft.is_dir() {
                stack.push(e.path());
            } else if ft.is_file() {
                out.push((e.path(), fs::read(e.path()).unwrap()));
            }
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------- H2: reflog-only commits

#[test]
fn h2_unpushed_commit_then_reset_to_the_pushed_parent_is_not_fully_saved() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    commit_file(&repo, "unpushed.txt", "work that exists nowhere else\n");
    git(&repo, &["reset", "-q", "--hard", "HEAD~1"]);

    let res = scan_with(&root, network_everywhere());
    // Every other fact reads as saved: HEAD, the branch and the tree match the pushed commit.
    let f = facts_of(&res, &repo);
    assert!(f.head_on_remote == disk_cleaner_core::Tri::Yes, "{f:?}");
    assert!(
        f.all_branches_on_remote == disk_cleaner_core::Tri::Yes,
        "{f:?}"
    );
    assert!(f.tree_clean == disk_cleaner_core::Tri::Yes, "{f:?}");
    assert_eq!(fact(&res, &repo, "reflog_saved"), "no");
    assert_eq!(
        action(&res, &repo),
        Action::RecoverBackupThenRemove,
        "the commit is only in the reflog; archive would delete it"
    );
}

#[test]
fn h2_commit_amended_away_is_not_fully_saved() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    commit_file(&repo, "draft.txt", "first draft\n");
    git(&repo, &["commit", "-q", "--amend", "-m", "final wording"]);
    // The amended commit is pushed; the commit it replaced survives only in the reflog.
    git(&repo, &["push", "-q", "origin", "main"]);

    let res = scan_with(&root, network_everywhere());
    let f = facts_of(&res, &repo);
    assert!(
        f.all_branches_on_remote == disk_cleaner_core::Tri::Yes,
        "{f:?}"
    );
    assert_eq!(fact(&res, &repo, "reflog_saved"), "no");
    assert_ne!(action(&res, &repo), Action::BackupThenRemove);
}

#[test]
fn h2_control_fresh_clone_has_reflog_saved_and_is_archivable() {
    let (_tmp, root, remotes) = workspace();
    let origin = pushed_repo(&root.parent().unwrap().join("src"), &remotes, "app");
    let clone = root.join("clone");
    git(
        &root,
        &[
            "clone",
            "-q",
            remotes.join("app.git").to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    assert!(origin.exists());
    let res = scan_with(&root, network_everywhere());
    assert_eq!(fact(&res, &clone, "reflog_saved"), "yes");
    assert!(
        facts_of(&res, &clone).all_yes(),
        "{:?}",
        facts_of(&res, &clone)
    );
    assert_eq!(action(&res, &clone), Action::BackupThenRemove);
}

#[test]
fn h2_zero_commit_repo_is_never_reflog_saved() {
    let (_tmp, root, _remotes) = workspace();
    let repo = root.join("empty");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    let res = scan_with(&root, network_everywhere());
    let v = fact(&res, &repo, "reflog_saved");
    assert!(v == "no" || v == "unknown", "got {v}");
    assert_ne!(action(&res, &repo), Action::BackupThenRemove);
}

#[cfg(unix)]
#[test]
fn h2_a_failing_reflog_check_is_unknown_and_asks() {
    let (tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    // Forwards everything to the real git except the reflog walk, which fails.
    let script = script(
        tmp.path(),
        "git-no-reflog",
        "#!/bin/sh\ncase \"$*\" in *--reflog*) echo 'fatal: simulated failure' >&2; exit 128;; esac\nexec git \"$@\"\n",
    );
    let mut cfg = network_everywhere();
    cfg = cfg.with_git(disk_cleaner_core::Git::new(
        &script,
        std::time::Duration::from_secs(20),
    ));
    let res = scan_with(&root, cfg);
    assert_eq!(fact(&res, &repo, "reflog_saved"), "unknown");
    assert_eq!(action(&res, &repo), Action::Ask);
}

#[test]
fn h2_linked_worktree_with_a_reset_away_commit_is_not_fully_saved() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let wt = root.join("wt");
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()],
    );
    commit_file(&wt, "wt-only.txt", "made in the linked worktree\n");
    git(&wt, &["reset", "-q", "--hard", "HEAD~1"]);
    let res = scan_with(&root, network_everywhere());
    assert_eq!(fact(&res, &wt, "reflog_saved"), "no");
    assert_ne!(action(&res, &wt), Action::BackupThenRemove);
}

#[test]
fn h2_checking_the_reflog_writes_nothing() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    commit_file(&repo, "unpushed.txt", "x\n");
    git(&repo, &["reset", "-q", "--hard", "HEAD~1"]);
    git(&repo, &["config", "gc.auto", "0"]);
    let before = snapshot(&repo.join(".git"));
    let res = scan_with(&root, network_everywhere());
    assert_eq!(fact(&res, &repo, "reflog_saved"), "no");
    let after = snapshot(&repo.join(".git"));
    assert_eq!(before.len(), after.len());
    assert!(before == after, "a scan changed something under .git");
}

// ---------------------------------------------------------------- H3: stores under artifacts

/// A repo whose `build/` is ignored and pushed, with the given files created under it.
fn repo_with_ignored_build(root: &Path, remotes: &Path, files: &[(&str, &str)]) -> PathBuf {
    let repo = root.join("app");
    init_repo(&repo);
    write(&repo.join(".gitignore"), "build/\n");
    write(&repo.join("CMakeLists.txt"), "project(x)\n");
    git(&repo, &["add", ".gitignore", "CMakeLists.txt"]);
    git(&repo, &["commit", "-q", "-m", "files"]);
    let remote = remotes.join("app.git");
    init_bare(&remote);
    push_to_remote(&repo, &remote);
    for (rel, text) in files {
        write(&repo.join("build").join(rel), text);
    }
    repo
}

#[test]
fn h3_terraform_state_inside_an_ignored_build_directory_asks_for_build_and_repo() {
    let (_tmp, root, remotes) = workspace();
    let repo = repo_with_ignored_build(
        &root,
        &remotes,
        &[
            ("main.tf", "terraform {}\n"),
            (
                ".terraform/terraform.tfstate",
                "{\"serial\": 7, \"note\": \"SYNTH_STATE_MARK\"}\n",
            ),
            ("CMakeCache.txt", "x\n"),
        ],
    );
    let res = scan_with(&root, network_everywhere());
    let build = repo.join("build");
    assert!(
        not_removing(action(&res, &build)),
        "{:?}",
        action(&res, &build)
    );
    assert!(
        !action(&res, &repo).removes(),
        "the enclosing repo would take the state file with it: {:?}",
        action(&res, &repo)
    );
    let reason = node(&res, &build).ask_reason.clone().unwrap_or_default();
    assert!(reason.to_lowercase().contains("terraform"), "{reason}");
    assert!(
        !reason.contains("SYNTH_STATE_MARK"),
        "contents are never read or quoted"
    );
}

#[test]
fn h3_a_state_file_name_alone_is_enough_without_a_terraform_directory() {
    let (_tmp, root, remotes) = workspace();
    let repo = repo_with_ignored_build(
        &root,
        &remotes,
        &[("env/prod/terraform.tfstate.backup", "{}")],
    );
    let res = scan_with(&root, network_everywhere());
    assert!(not_removing(action(&res, &repo.join("build"))));
    assert!(!action(&res, &repo).removes());
}

#[test]
fn h3_dotenv_inside_a_build_directory_asks_but_a_template_does_not() {
    let (_tmp, root, remotes) = workspace();
    let repo = repo_with_ignored_build(&root, &remotes, &[(".env", "A=1\n")]);
    let res = scan_with(&root, network_everywhere());
    assert!(not_removing(action(&res, &repo.join("build"))));

    let (_tmp2, root2, remotes2) = workspace();
    let repo2 = repo_with_ignored_build(
        &root2,
        &remotes2,
        &[
            (".env.example", "A=\n"),
            (".env.template", "A=\n"),
            ("CMakeCache.txt", "x\n"),
        ],
    );
    let res2 = scan_with(&root2, network_everywhere());
    assert_eq!(
        action(&res2, &repo2.join("build")),
        Action::RemoveRegenerable,
        "control: a template is not a secret"
    );
    assert_eq!(action(&res2, &repo2), Action::BackupThenRemove, "control");
}

#[test]
fn h3_maven_local_repository_nested_in_node_modules_asks() {
    let (_tmp, root, _remotes) = workspace();
    let proj = root.join("proj");
    write(&proj.join("package.json"), "{}");
    write(&proj.join("node_modules").join("dep").join("index.js"), "x");
    write(
        &proj
            .join("node_modules")
            .join("tool")
            .join(".m2")
            .join("repository")
            .join("com")
            .join("example")
            .join("lib.jar"),
        "locally installed, not on any remote",
    );
    let res = scan_plain(&root);
    let nm = proj.join("node_modules");
    assert!(not_removing(action(&res, &nm)), "{:?}", action(&res, &nm));
    let reason = node(&res, &nm).ask_reason.clone().unwrap_or_default();
    assert!(reason.contains(".m2/repository"), "{reason}");
}

#[test]
fn h3_database_file_inside_a_non_scanned_artifact_directory_asks() {
    // `obj` is not searched for output files, so only the unique-state names can catch this.
    let (_tmp, root, _remotes) = workspace();
    let proj = root.join("proj");
    write(&proj.join("App.csproj"), "<Project/>");
    write(&proj.join("obj").join("Debug").join("app.db"), "rows");
    let res = scan_plain(&root);
    assert!(not_removing(action(&res, &proj.join("obj"))));
}

#[test]
fn h3_sqlite_database_inside_target_asks() {
    let (_tmp, root, remotes) = workspace();
    let repo = root.join("app");
    init_repo(&repo);
    write(&repo.join(".gitignore"), "target/\n");
    write(&repo.join("Cargo.toml"), "[package]\n");
    git(&repo, &["add", ".gitignore", "Cargo.toml"]);
    git(&repo, &["commit", "-q", "-m", "files"]);
    let remote = remotes.join("app.git");
    init_bare(&remote);
    push_to_remote(&repo, &remote);
    write(
        &repo.join("target").join("debug").join("app.sqlite"),
        "rows",
    );
    let res = scan_with(&root, network_everywhere());
    assert!(not_removing(action(&res, &repo.join("target"))));
    assert!(!action(&res, &repo).removes());
}

#[test]
fn h3_dotenv_and_state_inside_node_modules_ask() {
    let (_tmp, root, _remotes) = workspace();
    let proj = root.join("proj");
    write(&proj.join("package.json"), "{}");
    write(
        &proj.join("node_modules").join("pkg").join(".env"),
        "TOKEN=1\n",
    );
    let res = scan_plain(&root);
    assert!(not_removing(action(&res, &proj.join("node_modules"))));
}

#[test]
fn h3_control_ordinary_node_modules_with_package_fixtures_stays_removable() {
    // Dependency trees routinely ship key, certificate and database test fixtures and
    // templates; those names are not evidence of the user's data there.
    let (_tmp, root, _remotes) = workspace();
    let proj = root.join("proj");
    write(&proj.join("package.json"), "{}");
    let nm = proj.join("node_modules");
    write(&nm.join("pkg").join("package.json"), "{}");
    write(&nm.join("pkg").join("index.js"), "module.exports = 1;\n");
    write(
        &nm.join("pkg")
            .join("test")
            .join("fixtures")
            .join("cert.pem"),
        "x",
    );
    write(
        &nm.join("pkg")
            .join("test")
            .join("fixtures")
            .join("server.key"),
        "x",
    );
    write(
        &nm.join("pkg")
            .join("test")
            .join("support")
            .join("prepare.db"),
        "x",
    );
    write(&nm.join("pkg").join(".env.example"), "A=\n");
    let res = scan_plain(&root);
    assert_eq!(action(&res, &nm), Action::RemoveRegenerable);
    assert!(node(&res, &nm).ask_reason.is_none());
}

// ---------------------------------------------------------------- sibling hunt (a)

/// The fact `no_pending_state` and the plan action for a pushed repo after `setup` ran.
fn pending_case(setup: impl Fn(&Path)) -> (String, Action) {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    setup(&repo);
    let res = scan_with(&root, network_everywhere());
    (fact(&res, &repo, "no_pending_state"), action(&res, &repo))
}

#[test]
fn sibling_a_paused_rebase_is_not_removable() {
    let (state, act) = pending_case(|repo| {
        commit_file(repo, "second.txt", "x\n");
        git(repo, &["push", "-q", "origin", "main"]);
        // Stops at an `edit` step with the tree clean and the rebase unfinished.
        let status = git_command()
            .args(["-c", "core.autocrlf=false", "rebase", "-q", "-i", "HEAD~1"])
            .current_dir(repo)
            .env("GIT_SEQUENCE_EDITOR", "sed -i.bak s/^pick/edit/")
            .env("GIT_EDITOR", "true")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap();
        let _ = status;
        assert!(repo.join(".git").join("rebase-merge").exists());
    });
    assert_eq!(state, "unknown");
    assert_eq!(act, Action::Ask);
}

#[test]
fn sibling_a_merge_cherry_pick_revert_and_bisect_state_are_not_removable() {
    for marker in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
    ] {
        let (state, act) = pending_case(|repo| {
            let head = git(repo, &["rev-parse", "HEAD"]);
            write(&repo.join(".git").join(marker), &format!("{head}\n"));
        });
        assert_eq!(state, "unknown", "{marker}");
        assert_eq!(act, Action::Ask, "{marker}");
    }
}

#[test]
fn sibling_a_local_lfs_objects_and_annex_content_are_not_removable() {
    for rel in [
        "lfs/objects/ab/cd/abcdef0123",
        "annex/objects/xx/yy/KEY/KEY",
    ] {
        let (state, act) = pending_case(|repo| {
            write(
                &repo.join(".git").join(rel),
                "content that may not be on the remote",
            );
        });
        assert_eq!(state, "unknown", "{rel}");
        assert_eq!(act, Action::Ask, "{rel}");
    }
}

#[test]
fn sibling_a_control_rr_cache_and_a_quiet_repo_keep_the_fact_yes() {
    // rr-cache only records conflict resolutions that can be re-derived: not unique work.
    // git names each entry by the conflict's full hash, so the fixture uses a 40-hex id.
    let id = "0123456789abcdef0123456789abcdef01234567";
    let (state, act) = pending_case(|repo| {
        write(
            &repo.join(".git").join("rr-cache").join(id).join("preimage"),
            "x",
        );
    });
    assert_eq!(state, "yes");
    assert_eq!(act, Action::BackupThenRemove);
}
