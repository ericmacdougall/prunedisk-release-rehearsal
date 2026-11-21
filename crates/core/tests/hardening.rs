//! Regression tests for the adversarial review of PR #2 (findings F1-F7, F11; F11 was tightened
//! for merge-gate finding G3).
//! Each test fails on the code the review was written against and passes after the fix.

mod common;

use common::*;
use disk_cleaner_core::{
    plan, scan, Action, Class, FactsConfig, Git, GitError, PlanOptions, ScanOptions, Tri,
};
use std::fs;
use std::path::Path;
use std::time::Duration;

fn append(path: &Path, text: &str) {
    let mut cur = fs::read_to_string(path).unwrap();
    cur.push_str(text);
    fs::write(path, cur).unwrap();
}

fn all_unknown(f: &disk_cleaner_core::Facts) -> bool {
    f.entries().iter().all(|(_, t)| *t == Tri::Unknown)
}

// ---------------------------------------------------------------- F1

#[cfg(unix)]
#[test]
fn f1_filter_clean_in_repo_config_is_never_executed() {
    let (tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    write(&repo.join(".gitattributes"), "* filter=evil\n");
    write(&repo.join("a.txt"), "one\n");
    git(&repo, &["add", ".gitattributes", "a.txt"]);
    git(&repo, &["commit", "-q", "-m", "tracked file"]);
    let marker = tmp.path().join("pwned-by-filter");
    append(
        &repo.join(".git").join("config"),
        &format!(
            "[filter \"evil\"]\n\tclean = \"sh -c 'touch {}; cat'\"\n",
            marker.display()
        ),
    );
    // A changed tracked file forces git status to run the clean filter.
    write(&repo.join("a.txt"), "two\n");

    let res = scan_with(&root, network_everywhere());
    assert!(!marker.exists(), "a program named in .git/config ran");
    let f = facts_of(&res, &repo);
    let notes = &node(&res, &repo).repo.as_ref().unwrap().notes;
    assert!(all_unknown(&f), "facts: {f:?}; notes: {notes:?}");
    assert_eq!(action_for(&res, &repo), Action::Ask);
    assert!(notes.iter().any(|n| n.contains("config")), "{notes:?}");
}

/// The lock `git maintenance run` holds in the object directory (`objects/maintenance.lock`) is
/// not what git leaves there at rest: while it exists the repository asks. git takes it for the
/// detached auto maintenance that commands such as `git commit` start, so a scan just after a
/// commit can see it; a fixture must not leave that to chance (see `common::git`).
#[test]
fn a_maintenance_lock_in_the_object_directory_is_local_admin_content() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    fs::write(
        repo.join(".git").join("objects").join("maintenance.lock"),
        "",
    )
    .unwrap();
    let res = scan_with(&root, network_everywhere());
    let f = facts_of(&res, &repo);
    let notes = &node(&res, &repo).repo.as_ref().unwrap().notes;
    assert_eq!(f.no_local_admin_content, Tri::No, "{notes:?}");
    assert!(
        notes.iter().any(|n| n.contains("objects/maintenance.lock")),
        "{notes:?}"
    );
    assert_eq!(action_for(&res, &repo), Action::Ask);
}

#[cfg(unix)]
#[test]
fn f1_gpg_program_via_log_show_signature_is_never_executed() {
    let (tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let marker = tmp.path().join("pwned-by-gpg");
    let gpg = script(
        tmp.path(),
        "fake-gpg",
        &format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display()),
    );
    // A commit with a gpgsig header, as in a downloaded repo.
    let tree = git(&repo, &["rev-parse", "HEAD^{tree}"]);
    let parent = git(&repo, &["rev-parse", "HEAD"]);
    let obj = format!(
        "tree {tree}\nparent {parent}\nauthor T <t@example.invalid> 1700000000 +0000\n\
         committer T <t@example.invalid> 1700000000 +0000\n\
         gpgsig -----BEGIN PGP SIGNATURE-----\n \n abcd\n -----END PGP SIGNATURE-----\n\nsigned\n"
    );
    let file = tmp.path().join("commit-object");
    fs::write(&file, obj).unwrap();
    let sha = git(
        &repo,
        &["hash-object", "-t", "commit", "-w", file.to_str().unwrap()],
    );
    git(&repo, &["update-ref", "refs/heads/main", &sha]);
    append(
        &repo.join(".git").join("config"),
        &format!(
            "[gpg]\n\tprogram = \"{}\"\n[log]\n\tshowSignature = true\n",
            gpg.display()
        ),
    );

    let res = scan_with(&root, network_everywhere());
    assert!(!marker.exists(), "gpg.program from .git/config ran");
    assert!(all_unknown(&facts_of(&res, &repo)));
}

#[cfg(unix)]
#[test]
fn f1_timeout_kills_the_whole_process_group() {
    let (tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let pidfile = tmp.path().join("child.pid");
    let body = format!(
        "#!/bin/sh\nsleep 60 &\necho $! > '{}'\nwait\n",
        pidfile.display()
    );
    let fake = script(tmp.path(), "fake-git", &body);
    let git = Git::new(&fake, Duration::from_millis(400));
    let mut outcome = None;
    for _ in 0..10 {
        match git.run(&repo, &["status"]) {
            Err(GitError::Spawn(_)) => std::thread::sleep(Duration::from_millis(50)),
            other => {
                outcome = Some(other);
                break;
            }
        }
    }
    assert!(
        matches!(outcome, Some(Err(GitError::Timeout(_)))),
        "{outcome:?}"
    );
    let pid = fs::read_to_string(&pidfile)
        .expect("child pid")
        .trim()
        .to_string();
    std::thread::sleep(Duration::from_millis(600));
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    // Never leave the sleeper behind, whatever the outcome.
    let _ = std::process::Command::new("kill")
        .args(["-KILL", &pid])
        .status();
    assert!(!alive, "a grandchild of git survived the timeout");
}

#[test]
fn f1_ordinary_repo_config_is_not_rejected() {
    // Guards the allowlist: remotes, branches, users and submodule keys must keep working.
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    git(&repo, &["config", "pull.rebase", "true"]);
    git(&repo, &["config", "branch.main.description", "x"]);
    let res = scan_with(&root, network_everywhere());
    // Every key passed the allowlist (git ran and every fact was decided). The branch
    // description is text that exists only in this repository, so it is reported as the
    // repository's own content (gate d1e3f23, W1); nothing else is.
    let mut f = facts_of(&res, &repo);
    assert_eq!(f.no_local_admin_content, Tri::No, "{f:?}");
    f.no_local_admin_content = Tri::Yes;
    assert!(f.all_yes(), "{f:?}");
    let info = node(&res, &repo).repo.as_ref().unwrap();
    assert_eq!(
        info.local_admin_content,
        vec!["config: branch.<redacted>.description".to_string()]
    );
}

// ---------------------------------------------------------------- F2

#[test]
fn f2_skip_worktree_edit_is_not_a_clean_tree() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    commit_file(&repo, "conf.yml", "a\n");
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["update-index", "--skip-worktree", "conf.yml"]);
    write(&repo.join("conf.yml"), "password=hunter2\n");
    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &repo).tree_clean, Tri::No);
    assert_ne!(action_for(&res, &repo), Action::BackupThenRemove);
}

#[test]
fn f2_assume_unchanged_edit_is_not_a_clean_tree() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    commit_file(&repo, "k.txt", "a\n");
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["update-index", "--assume-unchanged", "k.txt"]);
    write(&repo.join("k.txt"), "locally edited\n");
    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &repo).tree_clean, Tri::No);
    assert_ne!(action_for(&res, &repo), Action::BackupThenRemove);
}

// ---------------------------------------------------------------- F3

#[test]
fn f3a_ignored_build_dir_with_a_video_stops_the_enclosing_repo_from_being_archived() {
    let (_tmp, root, remotes) = workspace();
    let repo = root.join("app");
    init_repo(&repo);
    write(&repo.join(".gitignore"), "build/\n");
    write(&repo.join("CMakeLists.txt"), "project(x)\n");
    git(&repo, &["add", ".gitignore", "CMakeLists.txt"]);
    git(&repo, &["commit", "-q", "-m", "files"]);
    let remote = remotes.join("app.git");
    init_bare(&remote);
    push_to_remote(&repo, &remote);
    write_bytes(&repo.join("build").join("render.mp4"), 1024);

    let res = scan_with(&root, network_everywhere());
    let build = node(&res, &repo.join("build"));
    assert!(build.contains_outputs);
    assert_eq!(action_for(&res, &repo.join("build")), Action::Ask);
    assert_eq!(
        action_for(&res, &repo),
        Action::Ask,
        "the video would be deleted with the repo"
    );
}

/// Outer repo with a submodule whose stash and local-only branch live in outer/.git/modules.
fn repo_with_dirty_submodule(
    tmp: &Path,
    root: &Path,
    remotes: &Path,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let srcs = tmp.join("srcs");
    fs::create_dir_all(&srcs).unwrap();
    let subsrc = pushed_repo(&srcs, remotes, "subsrc");
    let outer = pushed_repo(root, remotes, "outer");
    git(
        &outer,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            subsrc.to_str().unwrap(),
            "sub",
        ],
    );
    git(&outer, &["commit", "-q", "-m", "add submodule"]);
    git(&outer, &["push", "-q", "origin", "main"]);
    let sub = outer.join("sub");
    let recorded = git(&sub, &["rev-parse", "HEAD"]);
    git(&sub, &["checkout", "-q", "-b", "local-only"]);
    commit_file(&sub, "unpushed.txt", "only here");
    git(&sub, &["checkout", "-q", "--detach", &recorded]);
    write(&sub.join("README.md"), "work in progress\n");
    git(&sub, &["stash", "push", "-q"]);
    (outer, sub)
}

#[test]
fn f3b_submodule_with_a_stash_and_local_branch_stops_the_outer_repo_from_being_archived() {
    let (tmp, root, remotes) = workspace();
    let (outer, sub) = repo_with_dirty_submodule(tmp.path(), &root, &remotes);
    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &sub).stash_empty, Tri::No);
    // The reviewer's case: the outer repo itself looks perfectly clean. Every fact is yes except
    // the plain-layout rule, which now also catches it (a non-empty .git/modules).
    let f = facts_of(&res, &outer);
    assert!(
        f.tree_clean == Tri::Yes && f.stash_empty == Tri::Yes && f.no_dependents == Tri::Yes,
        "{f:?}"
    );
    assert_eq!(f.plain_layout, Tri::No, "{f:?}");
    assert_eq!(
        action_for(&res, &outer),
        Action::Ask,
        "the submodule's stash and branch live inside outer/.git"
    );
}

#[test]
fn f3b2_removed_submodule_whose_git_data_remains_under_dot_git_modules_asks() {
    let (tmp, root, remotes) = workspace();
    let (outer, _sub) = repo_with_dirty_submodule(tmp.path(), &root, &remotes);
    git(&outer, &["rm", "-q", "-f", "sub"]);
    git(&outer, &["commit", "-q", "-m", "drop submodule"]);
    git(&outer, &["push", "-q", "origin", "main"]);
    assert!(outer.join(".git").join("modules").join("sub").exists());

    let res = scan_with(&root, network_everywhere());
    assert!(!res.nodes.iter().any(|n| n.path.ends_with("sub")));
    assert_eq!(action_for(&res, &outer), Action::Ask);
}

// ---------------------------------------------------------------- F4

#[test]
fn f4_marker_must_be_in_the_parent_directory() {
    let (_tmp, root, _) = workspace();
    write(&root.join("mono").join("package.json"), "{}");
    write_bytes(
        &root
            .join("mono")
            .join("src")
            .join("build")
            .join("hand-written.js"),
        100,
    );
    write_bytes(
        &root
            .join("mono")
            .join("packages")
            .join("a")
            .join("node_modules")
            .join("x.js"),
        100,
    );
    let res = scan_plain(&root);
    assert!(!has_node(
        &res,
        &root.join("mono").join("src").join("build")
    ));
    assert!(!has_node(
        &res,
        &root
            .join("mono")
            .join("packages")
            .join("a")
            .join("node_modules")
    ));
}

#[test]
fn f4_tracked_build_directory_in_a_repo_is_never_removable() {
    let (_tmp, root, remotes) = workspace();
    let repo = root.join("app");
    init_repo(&repo);
    write(&repo.join("package.json"), "{}");
    write(&repo.join("build").join("gen.js"), "tracked source\n");
    git(&repo, &["add", "package.json", "build/gen.js"]);
    git(&repo, &["commit", "-q", "-m", "tracked build dir"]);
    let remote = remotes.join("app.git");
    init_bare(&remote);
    push_to_remote(&repo, &remote);
    write(&repo.join("build").join("gen.js"), "edited, unpushed\n");

    let res = scan_with(&root, network_everywhere());
    let a = action_for(&res, &repo.join("build"));
    assert_eq!(a, Action::Ask, "tracked files are not regenerable");
}

#[test]
fn f4_build_directory_in_loose_code_is_asked_not_removed() {
    let (_tmp, root, _) = workspace();
    write(&root.join("proj").join("package.json"), "{}");
    write_bytes(&root.join("proj").join("build").join("only-copy.js"), 100);
    let res = scan_plain(&root);
    assert_eq!(
        action_for(&res, &root.join("proj").join("build")),
        Action::Ask
    );
}

#[test]
fn f4_a_git_clone_inside_a_dependency_directory_makes_it_ask() {
    let (_tmp, root, _) = workspace();
    write(&root.join("proj").join("package.json"), "{}");
    let inner = root.join("proj").join("node_modules").join("pkg");
    fs::create_dir_all(&inner).unwrap();
    git(&inner, &["init", "-q", "-b", "main"]);
    write_bytes(&inner.join("patched.js"), 100);
    let res = scan_plain(&root);
    assert_eq!(
        action_for(&res, &root.join("proj").join("node_modules")),
        Action::Ask
    );
}

#[test]
fn f4_outputs_in_the_design_table_are_found_in_build_directories() {
    let (_tmp, root, _) = workspace();
    for (i, file) in [
        "frame.png",
        "data.parquet",
        "model.bin",
        "app.db",
        "paper.pdf",
        "scan.EXR",
    ]
    .iter()
    .enumerate()
    {
        let proj = root.join(format!("p{i}"));
        write(&proj.join("package.json"), "{}");
        write_bytes(&proj.join("out").join(file), 10);
    }
    let res = scan_plain(&root);
    for i in 0..6 {
        let out = node(&res, &root.join(format!("p{i}")).join("out"));
        assert!(out.contains_outputs, "p{i} output file not recognised");
    }
}

// ---------------------------------------------------------------- F6

#[cfg(unix)]
#[test]
fn f6_unreadable_subdirectory_makes_the_enclosing_directory_ask() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, root, _) = workspace();
    write(&root.join("app").join("package.json"), "{}");
    let locked = root
        .join("app")
        .join("node_modules")
        .join("pkg")
        .join("hidden");
    write(&locked.join("x.js"), "1");
    write_bytes(&root.join("app").join("node_modules").join("ok.js"), 10);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let readable_anyway = fs::read_dir(&locked).is_ok();
    let res = scan_plain(&root);
    let a = action_for(&res, &root.join("app").join("node_modules"));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if readable_anyway {
        return;
    }
    assert_eq!(a, Action::Ask, "part of the directory could not be read");
}

// ---------------------------------------------------------------- F7

#[test]
fn f7_unpushed_tag_on_a_pushed_commit_is_not_all_on_remote() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    git(&repo, &["tag", "v1-local-only"]);
    let res = scan_with(&root, network_everywhere());
    assert_ne!(facts_of(&res, &repo).all_branches_on_remote, Tri::Yes);
    assert_ne!(action_for(&res, &repo), Action::BackupThenRemove);
}

#[test]
fn f7_unpushed_notes_ref_is_not_all_on_remote() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    git(&repo, &["notes", "add", "-m", "private review notes"]);
    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &repo).all_branches_on_remote, Tri::No);
    assert_ne!(action_for(&res, &repo), Action::BackupThenRemove);
}

// ---------------------------------------------------------------- N1, N2 (delta review)

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

#[cfg(unix)]
#[test]
fn n1_submodule_config_filter_is_never_executed_by_the_outer_status() {
    let (tmp, root, remotes) = workspace();
    let srcs = tmp.path().join("srcs");
    fs::create_dir_all(&srcs).unwrap();
    let subsrc = pushed_repo(&srcs, &remotes, "subsrc");
    write(&subsrc.join(".gitattributes"), "* filter=evil\n");
    write(&subsrc.join("s.txt"), "one\n");
    git(&subsrc, &["add", ".gitattributes", "s.txt"]);
    git(&subsrc, &["commit", "-q", "-m", "attrs"]);
    git(&subsrc, &["push", "-q", "origin", "main"]);
    let outer = pushed_repo(&root, &remotes, "outer");
    add_submodule(&outer, &subsrc, "sub");
    git(&outer, &["commit", "-q", "-m", "add submodule"]);
    git(&outer, &["push", "-q", "origin", "main"]);

    let marker = tmp.path().join("pwned-by-submodule-filter");
    append(
        &outer
            .join(".git")
            .join("modules")
            .join("sub")
            .join("config"),
        &format!(
            "[filter \"evil\"]\n\tclean = \"sh -c 'touch {}; cat'\"\n",
            marker.display()
        ),
    );
    // A changed file in the submodule makes a recursive status run the clean filter.
    write(&outer.join("sub").join("s.txt"), "two\n");

    let res = scan_with(&root, network_everywhere());
    assert!(!marker.exists(), "a submodule's config ran a program");
    assert!(
        all_unknown(&facts_of(&res, &outer)),
        "{:?}",
        facts_of(&res, &outer)
    );
}

#[test]
fn n2_in_place_submodule_plus_a_deinited_one_with_a_stash_asks() {
    let (tmp, root, remotes) = workspace();
    let srcs = tmp.path().join("srcs");
    fs::create_dir_all(&srcs).unwrap();
    let bsrc = pushed_repo(&srcs, &remotes, "bsrc");
    let outer = pushed_repo(&root, &remotes, "outer");
    // A is an existing repo added in place: its .git stays a directory.
    let a = outer.join("A");
    init_repo(&a);
    let aremote = remotes.join("aremote.git");
    init_bare(&aremote);
    push_to_remote(&a, &aremote);
    add_submodule(&outer, &aremote, "A");
    add_submodule(&outer, &bsrc, "B");
    git(&outer, &["commit", "-q", "-m", "add submodules"]);
    git(&outer, &["push", "-q", "origin", "main"]);
    let b = outer.join("B");
    write(&b.join("README.md"), "work in progress\n");
    git(&b, &["stash", "push", "-q"]);
    git(&b, &["checkout", "-q", "-b", "local-only"]);
    commit_file(&b, "x.txt", "only here");
    git(&outer, &["submodule", "deinit", "-q", "-f", "B"]);
    assert!(a.join(".git").is_dir());
    assert!(outer.join(".git").join("modules").join("B").exists());

    let res = scan_with(&root, network_everywhere());
    assert_eq!(
        action_for(&res, &outer),
        Action::Ask,
        "B's stash and local branch live in outer/.git/modules/B"
    );
}

#[test]
fn n2_control_two_checked_out_clean_submodules_make_the_parent_ask_for_its_layout() {
    let (tmp, root, remotes) = workspace();
    let srcs = tmp.path().join("srcs");
    fs::create_dir_all(&srcs).unwrap();
    let s1 = pushed_repo(&srcs, &remotes, "s1");
    let s2 = pushed_repo(&srcs, &remotes, "s2");
    let outer = pushed_repo(&root, &remotes, "outer");
    add_submodule(&outer, &s1, "sub1");
    add_submodule(&outer, &s2, "sub2");
    git(&outer, &["commit", "-q", "-m", "add submodules"]);
    git(&outer, &["push", "-q", "origin", "main"]);
    let res = scan_with(&root, network_everywhere());
    let p = plan(&res, &PlanOptions::new(7, far_future()));
    let why: Vec<String> = p
        .items
        .iter()
        .map(|i| format!("{} {:?}: {}", i.path, i.action, i.reason))
        .collect();
    let notes: Vec<&String> = res
        .nodes
        .iter()
        .filter_map(|n| n.repo.as_ref())
        .flat_map(|r| r.notes.iter())
        .collect();
    // A repository with submodules has a non-empty .git/modules, which is not the plain layout:
    // it asks (a deliberate limit of this phase), whatever the state of its submodules.
    assert_eq!(
        action_for(&res, &outer),
        Action::Ask,
        "{why:#?}\n{notes:#?}"
    );
    assert!(
        why.iter()
            .any(|w| w.contains("unusual repository layout: modules")),
        "{why:#?}"
    );
}

#[test]
fn core_worktree_pointing_elsewhere_is_refused() {
    let (tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let elsewhere = tmp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    git(
        &repo,
        &["config", "core.worktree", elsewhere.to_str().unwrap()],
    );
    let res = scan_with(&root, network_everywhere());
    let f = facts_of(&res, &repo);
    assert!(all_unknown(&f), "facts about another directory: {f:?}");
}

// ---------------------------------------------------------------- F11

#[cfg(unix)]
#[test]
fn f11_alternates_that_is_a_symlink_is_not_followed() {
    use std::os::unix::fs::symlink;
    let (tmp, root, remotes) = workspace();
    let src = pushed_repo(&root, &remotes, "source");
    // A real borrower: `--shared` leaves its object store empty, so every object it has comes
    // from the source through the alternates file, which is then replaced by a symlink to a copy.
    let borrower = root.join("borrower");
    git(
        &root,
        &[
            "clone",
            "-q",
            "--shared",
            src.to_str().unwrap(),
            borrower.to_str().unwrap(),
        ],
    );
    let info = borrower.join(".git").join("objects").join("info");
    let target = tmp.path().join("alternates.txt");
    fs::copy(info.join("alternates"), &target).unwrap();
    fs::remove_file(info.join("alternates")).unwrap();
    symlink(&target, info.join("alternates")).unwrap();
    let own_packs = fs::read_dir(borrower.join(".git").join("objects").join("pack"))
        .map(|r| r.count())
        .unwrap_or(0);
    assert_eq!(own_packs, 0, "the borrower must hold no objects of its own");
    // git follows the symlink, so the borrower still works and still needs the source.
    git(&borrower, &["cat-file", "-e", "HEAD^{tree}"]);

    let mut opts = ScanOptions::new(vec![root.clone()]);
    opts.facts = FactsConfig::default().with_network_predicate(|_| true);
    let res = scan(&opts).unwrap();
    // The scanner does not follow the symlink, so it cannot see that the source is a provider:
    // the source's dependents are unknown and it is never offered for removal.
    assert_eq!(
        facts_of(&res, &src).no_dependents,
        Tri::Unknown,
        "an unread alternates file must not leave the provider marked free of dependents"
    );
    assert_eq!(action_for(&res, &src), Action::Ask);
    assert!(
        res.warnings.iter().any(|w| w.contains("alternates")),
        "{:?}",
        res.warnings
    );
}

// Keep the compiler honest about imports used only by some cfgs.
#[allow(dead_code)]
fn _uses(_: Class, _: PlanOptions) {
    let _ = plan;
}

// ---------------------------------------------------------------- H1 (second merge gate)

#[test]
fn h1_credential_in_a_submodule_config_subsection_is_not_echoed() {
    let (tmp, root, remotes) = workspace();
    let srcs = tmp.path().join("srcs");
    fs::create_dir_all(&srcs).unwrap();
    let subsrc = pushed_repo(&srcs, &remotes, "subsrc");
    let outer = pushed_repo(&root, &remotes, "outer");
    add_submodule(&outer, &subsrc, "sub");
    git(&outer, &["commit", "-q", "-m", "add submodule"]);
    git(&outer, &["push", "-q", "origin", "main"]);
    append(
        &outer
            .join(".git")
            .join("modules")
            .join("sub")
            .join("config"),
        "[credential \"https://user:SYNTHETIC_TOKEN@example.invalid\"]\n\thelper = store\n",
    );
    let res = scan_with(&root, network_everywhere());
    assert!(all_unknown(&facts_of(&res, &outer)));
    let json = serde_json::to_string(&res).unwrap();
    assert!(!json.contains("SYNTHETIC_TOKEN"), "token leaked: {json}");
    assert!(json.contains("credential.<redacted>.helper"), "{json}");
}
