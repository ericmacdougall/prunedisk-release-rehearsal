//! Repo safety facts: AC-4, AC-5 and NEG-1, NEG-2, NEG-3, NEG-6, NEG-9, plus controls that
//! prove the tests can see a `Yes`.

mod common;

use common::*;
use disk_cleaner_core::{
    plan, Action, Class, Facts, FactsConfig, Git, GitError, PlanOptions, ScanResult, Tri,
};
use std::path::Path;
use std::time::Duration;

fn facts_of(res: &ScanResult, path: &Path) -> Facts {
    node(res, path).repo.as_ref().expect("repo info").facts
}

fn action_for(res: &ScanResult, path: &Path) -> Action {
    let p = plan(res, &PlanOptions::new(7, far_future()));
    p.items
        .iter()
        .find(|i| i.path == path.to_string_lossy())
        .unwrap_or_else(|| panic!("no plan item for {}", path.display()))
        .action
}

/// A repo with one commit pushed to a bare "remote" outside the scan root.
fn pushed_repo(root: &Path, remotes: &Path, name: &str) -> std::path::PathBuf {
    let repo = root.join(name);
    init_repo(&repo);
    let remote = remotes.join(format!("{name}.git"));
    init_bare(&remote);
    push_to_remote(&repo, &remote);
    repo
}

#[test]
fn control_fully_pushed_clean_repo_has_every_fact_yes_and_is_archivable() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let res = scan_with(&root, network_everywhere());
    let f = facts_of(&res, &repo);
    assert!(f.all_yes(), "facts: {f:?}");
    assert_eq!(action_for(&res, &repo), Action::BackupThenRemove);
}

#[test]
fn neg1_missing_git_binary_makes_every_fact_unknown_and_plan_asks() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let cfg = FactsConfig::default().with_git(Git::new(
        "definitely-not-a-git-binary-0x1f",
        Duration::from_secs(5),
    ));
    let res = scan_with(&root, cfg);
    let f = facts_of(&res, &repo);
    assert!(
        f.entries().iter().all(|(_, t)| *t == Tri::Unknown),
        "facts: {f:?}"
    );
    assert_eq!(action_for(&res, &repo), Action::Ask);
    let notes = &node(&res, &repo).repo.as_ref().unwrap().notes;
    assert!(!notes.is_empty(), "the reason must be recorded");
}

#[cfg(unix)]
fn fake_git(dir: &Path, body: &str) -> std::path::PathBuf {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = dir.join("fake-git");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(&path)
        .unwrap();
    f.write_all(body.as_bytes()).unwrap();
    f.sync_all().unwrap();
    drop(f);
    path
}

#[cfg(unix)]
#[test]
fn neg1_git_timeout_is_a_typed_error_and_facts_are_unknown() {
    let (tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let script = fake_git(tmp.path(), "#!/bin/sh\nexec sleep 5\n");
    let git = Git::new(&script, Duration::from_millis(150));

    // The wrapper reports a timeout, not empty output. (Retry if the OS briefly refuses to
    // execute a just-written file because another test thread forked.)
    let mut last = None;
    for _ in 0..10 {
        match git.run(&repo, &["status"]) {
            Err(GitError::Spawn(_)) => std::thread::sleep(Duration::from_millis(50)),
            other => {
                last = Some(other);
                break;
            }
        }
    }
    assert!(
        matches!(last, Some(Err(GitError::Timeout(_)))),
        "got {last:?}"
    );

    let started = std::time::Instant::now();
    let res = scan_with(&root, FactsConfig::default().with_git(git));
    assert!(started.elapsed() < Duration::from_secs(20));
    let f = facts_of(&res, &repo);
    assert!(
        f.entries().iter().all(|(_, t)| *t == Tri::Unknown),
        "facts: {f:?}"
    );
    assert_eq!(action_for(&res, &repo), Action::Ask);
}

#[cfg(unix)]
#[test]
fn neg6_credentials_in_git_error_output_are_redacted() {
    let (tmp, root, remotes) = workspace();
    let _repo = pushed_repo(&root, &remotes, "app");
    let script = fake_git(
        tmp.path(),
        "#!/bin/sh\necho \"fatal: unable to access 'https://bob:tok-LEAK-123@example.invalid/x.git/'\" >&2\nexit 128\n",
    );
    let git = Git::new(&script, Duration::from_secs(10));
    let res = scan_with(&root, FactsConfig::default().with_git(git));
    let json = serde_json::to_string(&res).unwrap();
    assert!(!json.contains("tok-LEAK-123"), "secret leaked: {json}");
    assert!(!json.contains("bob:"));
}

#[test]
fn neg2_detached_head_with_unpushed_commit_is_not_on_remote() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");

    // Control: detached at the pushed commit is on the remote.
    git(&repo, &["checkout", "-q", "--detach"]);
    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &repo).head_on_remote, Tri::Yes);

    // A commit on the detached HEAD belongs to no branch and no remote.
    commit_file(&repo, "wip.txt", "unsaved");
    let res = scan_with(&root, network_everywhere());
    let f = facts_of(&res, &repo);
    assert_eq!(f.head_on_remote, Tri::No);
    // A check that only looks at branch tips would call this repo fully pushed.
    assert_eq!(f.all_branches_on_remote, Tri::Yes);
    assert_ne!(action_for(&res, &repo), Action::BackupThenRemove);
}

#[test]
fn neg2b_a_linked_worktrees_own_head_is_checked() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let wt = root.join("app-wt");
    git(
        &repo,
        &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()],
    );
    commit_file(&wt, "only-in-worktree.txt", "x");

    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &repo).head_on_remote, Tri::Yes);
    assert_eq!(facts_of(&res, &wt).head_on_remote, Tri::No);
    assert_ne!(action_for(&res, &wt), Action::BackupThenRemove);
}

#[test]
fn neg3_local_directory_remote_is_not_a_network_remote() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");

    // Production predicate: a remote that is a directory path is not a backup.
    let res = scan_with(&root, FactsConfig::default());
    let f = facts_of(&res, &repo);
    assert_eq!(f.has_network_remote, Tri::No);
    assert_eq!(f.head_on_remote, Tri::No);
    assert_eq!(f.all_branches_on_remote, Tri::No);
    assert_ne!(action_for(&res, &repo), Action::BackupThenRemove);
    let remotes_seen = &node(&res, &repo).repo.as_ref().unwrap().remotes;
    assert_eq!(remotes_seen.len(), 1);
    assert!(!remotes_seen[0].network);

    // Injected predicate (the other branch): the same fixture counts when told it is "network".
    let res = scan_with(&root, network_everywhere());
    let f = facts_of(&res, &repo);
    assert_eq!(f.has_network_remote, Tri::Yes);
    assert_eq!(f.head_on_remote, Tri::Yes);
}

#[test]
fn neg6_token_in_remote_url_is_absent_from_all_output() {
    let (_tmp, root, _) = workspace();
    let repo = root.join("app");
    init_repo(&repo);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://alice:ghp-SECRET-token-9f2@example.invalid/x.git",
        ],
    );
    let res = scan_plain(&root);
    let json = serde_json::to_string_pretty(&res).unwrap();
    let debug = format!("{res:?}");
    for text in [&json, &debug] {
        assert!(!text.contains("ghp-SECRET-token-9f2"), "token leaked");
        assert!(!text.contains("alice"), "user name leaked");
    }
    let remotes = &node(&res, &repo).repo.as_ref().unwrap().remotes;
    assert_eq!(remotes[0].url, "https://example.invalid/x.git");
    assert!(remotes[0].network);
    // The plan echoes facts, never URLs, but check its JSON as well.
    let p = plan(&res, &PlanOptions::new(1, far_future()));
    assert!(!serde_json::to_string(&p).unwrap().contains("ghp-SECRET"));
}

#[test]
fn neg9_a_stash_alone_blocks_removal() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    write(&repo.join("README.md"), "changed but not committed\n");
    git(&repo, &["stash", "push", "-q"]);

    let res = scan_with(&root, network_everywhere());
    let f = facts_of(&res, &repo);
    assert_eq!(f.stash_empty, Tri::No);
    // Everything else is the safe value: the stash is the only problem.
    assert_eq!(f.tree_clean, Tri::Yes);
    assert_eq!(f.head_on_remote, Tri::Yes);
    assert_eq!(f.all_branches_on_remote, Tri::Yes);
    assert_eq!(f.has_network_remote, Tri::Yes);
    let a = action_for(&res, &repo);
    assert_eq!(a, Action::RecoverBackupThenRemove);
    assert!(!matches!(
        a,
        Action::BackupThenRemove | Action::RemoveRegenerable
    ));
}

#[test]
fn unpushed_branch_makes_all_branches_on_remote_no() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    git(&repo, &["checkout", "-q", "-b", "feature"]);
    commit_file(&repo, "f.txt", "feature work");
    git(&repo, &["checkout", "-q", "main"]);

    let res = scan_with(&root, network_everywhere());
    let f = facts_of(&res, &repo);
    assert_eq!(f.head_on_remote, Tri::Yes, "HEAD (main) is pushed");
    assert_eq!(f.all_branches_on_remote, Tri::No, "feature is not");
}

#[test]
fn untracked_and_modified_files_make_tree_not_clean() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    write(&repo.join("new.txt"), "untracked");
    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &repo).tree_clean, Tri::No);
}

#[test]
fn ignored_unique_files_count_but_regenerable_directories_do_not() {
    let (_tmp, root, remotes) = workspace();
    let repo = root.join("app");
    init_repo(&repo);
    write(&repo.join(".gitignore"), ".env\nnode_modules/\n*.sqlite\n");
    write(&repo.join("package.json"), "{}");
    git(&repo, &["add", ".gitignore", "package.json"]);
    git(&repo, &["commit", "-q", "-m", "ignore rules"]);
    let remote = remotes.join("app.git");
    init_bare(&remote);
    push_to_remote(&repo, &remote);
    write_bytes(&repo.join("node_modules").join("dep").join("i.js"), 1000);

    // Only the regenerable directory is ignored: nothing unique.
    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &repo).no_unique_ignored, Tri::Yes);
    assert!(facts_of(&res, &repo).all_yes());

    // A secret and a local database are ignored and unique.
    write(&repo.join(".env"), "TOKEN=placeholder\n");
    write(&repo.join("local.sqlite"), "db");
    let res = scan_with(&root, network_everywhere());
    let info = node(&res, &repo).repo.as_ref().unwrap();
    assert_eq!(info.facts.no_unique_ignored, Tri::No);
    assert!(info.notes.iter().any(|n| n.contains(".env")));
    assert_eq!(action_for(&res, &repo), Action::RecoverBackupThenRemove);
}

#[test]
fn ac5_linked_worktree_is_tied_to_its_parent_and_parent_has_dependents() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let wt = root.join("app-wt");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "wt-branch",
        ],
    );

    let res = scan_with(&root, network_everywhere());
    let parent_idx = node_index(&res, &repo);
    let wt_info = node(&res, &wt).repo.as_ref().unwrap();
    assert_eq!(node(&res, &wt).class, Class::Worktree);
    assert_eq!(wt_info.parent_node, Some(parent_idx));
    assert!(wt_info.parent_repo.is_some());
    assert_eq!(wt_info.facts.no_dependents, Tri::Yes);

    let parent = node(&res, &repo).repo.as_ref().unwrap();
    assert_eq!(parent.facts.no_dependents, Tri::No);
    assert_eq!(parent.worktrees.len(), 1);
    assert_ne!(action_for(&res, &repo), Action::BackupThenRemove);
}

#[test]
fn alternates_clone_marks_the_source_as_having_dependents() {
    let (_tmp, root, remotes) = workspace();
    let src = pushed_repo(&root, &remotes, "source");
    let dst = root.join("borrower");
    git(
        &root,
        &[
            "clone",
            "-q",
            "--shared",
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
        ],
    );
    let res = scan_with(&root, network_everywhere());
    let s = node(&res, &src).repo.as_ref().unwrap();
    assert_eq!(s.facts.no_dependents, Tri::No);
    assert_eq!(s.dependents, vec![dst.to_string_lossy().into_owned()]);
}

#[test]
fn bare_repo_never_claims_working_tree_facts() {
    let (_tmp, root, _) = workspace();
    let bare = root.join("mirror.git");
    init_bare(&bare);
    let res = scan_plain(&root);
    let f = facts_of(&res, &bare);
    assert_eq!(f.tree_clean, Tri::Unknown);
    assert_eq!(f.no_unique_ignored, Tri::Unknown);
    assert!(!f.all_yes());
    assert_eq!(action_for(&res, &bare), Action::Ask);
}

#[test]
fn collect_facts_off_leaves_every_fact_unknown() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    let mut opts = disk_cleaner_core::ScanOptions::new(vec![root.clone()]);
    opts.collect_facts = false;
    let res = disk_cleaner_core::scan(&opts).unwrap();
    let f = facts_of(&res, &repo);
    assert!(f.entries().iter().all(|(_, t)| *t == Tri::Unknown));
}
