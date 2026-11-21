//! Dry-run plan: AC-7, AC-6 (thresholds are inputs) and NEG-4 (plan side), NEG-8.

mod common;

use common::*;
use disk_cleaner_core::{
    plan, running_cwds, Action, Class, Facts, Node, Plan, PlanOptions, ProcessCheck, ProjectState,
    RepoInfo, ScanOptionsRecord, ScanResult, Totals, Tri, DEFAULT_ACTIVE_WINDOW_DAYS,
    SCHEMA_VERSION,
};
use std::path::{Path, PathBuf};

const DAY: i64 = 86_400;

fn opts(active: Vec<PathBuf>, window_days: u64, now: i64) -> PlanOptions {
    let mut o = PlanOptions::new(window_days, now);
    o.active = active;
    o
}

fn action_of(p: &Plan, path: &Path) -> Action {
    p.items
        .iter()
        .find(|i| i.path == path.to_string_lossy())
        .unwrap_or_else(|| panic!("no plan item for {}", path.display()))
        .action
}

fn synthetic(nodes: Vec<Node>) -> ScanResult {
    ScanResult {
        schema_version: SCHEMA_VERSION,
        tool_version: "test".into(),
        generated_at: 0,
        roots: vec![],
        options: ScanOptionsRecord::default(),
        totals: Totals::default(),
        nodes,
        outputs: vec![],
        big_files: vec![],
        issues: vec![],
        warnings: vec![],
        enclosing_repos: vec![],
        scope: "subtree".to_string(),
        coverage: disk_cleaner_core::Coverage {
            complete: true,
            ..Default::default()
        },
    }
}

fn all_yes() -> Facts {
    Facts {
        has_network_remote: Tri::Yes,
        head_on_remote: Tri::Yes,
        all_branches_on_remote: Tri::Yes,
        stash_empty: Tri::Yes,
        tree_clean: Tri::Yes,
        no_unique_ignored: Tri::Yes,
        no_dependents: Tri::Yes,
        reflog_saved: Tri::Yes,
        no_pending_state: Tri::Yes,
        plain_layout: Tri::Yes,
        no_local_admin_content: Tri::Yes,
    }
}

fn with_fact(index: usize, v: Tri) -> Facts {
    let mut f = all_yes();
    match index {
        0 => f.has_network_remote = v,
        1 => f.head_on_remote = v,
        2 => f.all_branches_on_remote = v,
        3 => f.stash_empty = v,
        4 => f.tree_clean = v,
        5 => f.no_unique_ignored = v,
        6 => f.no_dependents = v,
        7 => f.reflog_saved = v,
        8 => f.no_pending_state = v,
        9 => f.plain_layout = v,
        _ => f.no_local_admin_content = v,
    }
    f
}

fn repo_node(path: &str, class: Class, facts: Facts) -> Node {
    let mut n = Node::new(path, class);
    n.bytes_self = 5_000;
    n.bytes_total = 5_000;
    n.newest_mtime = Some(0);
    n.repo = Some(RepoInfo {
        facts,
        // The scan counted the git directory's own files, as it does for every real `.git`.
        git_admin: Some(disk_cleaner_core::GitAdminSurvey {
            files: 12,
            bytes: 49_152,
            ..Default::default()
        }),
        object_store: Some(disk_cleaner_core::ObjectStoreSurvey {
            files: 3,
            bytes: 12_288,
        }),
        ..RepoInfo::default()
    });
    n
}

#[test]
fn a_repo_with_any_fact_not_yes_never_gets_a_removing_action() {
    let now = 365 * DAY;
    // Control: all Yes and idle is archivable, so the matrix below is not vacuous.
    for class in [Class::Repo, Class::Worktree] {
        let s = synthetic(vec![repo_node("/r", class, all_yes())]);
        assert_eq!(
            plan(&s, &opts(vec![], 30, now)).items[0].action,
            Action::BackupThenRemove
        );
    }
    for class in [Class::Repo, Class::Worktree] {
        for index in 0..11 {
            for (v, expected) in [
                (Tri::No, Action::RecoverBackupThenRemove),
                (Tri::Unknown, Action::Ask),
            ] {
                let s = synthetic(vec![repo_node("/r", class, with_fact(index, v))]);
                let p = plan(&s, &opts(vec![], 30, now));
                let a = p.items[0].action;
                // An unusual layout (fact 9), other repos depending on this one (fact 6) and
                // files of the git directory's own (fact 10) are never recovered then archived:
                // they ask, because recovery cannot save or fix them.
                let expected = if index >= 9 || (index == 6 && v == Tri::No) {
                    Action::Ask
                } else {
                    expected
                };
                assert_eq!(a, expected, "fact {index} = {v:?} on {class:?}");
                assert!(!matches!(
                    a,
                    Action::BackupThenRemove | Action::RemoveRegenerable
                ));
                assert!(!p.items[0].reason.is_empty());
            }
        }
    }
}

#[test]
fn unknown_beats_no_so_a_mixed_repo_asks() {
    let mut f = all_yes();
    f.stash_empty = Tri::No;
    f.tree_clean = Tri::Unknown;
    let s = synthetic(vec![repo_node("/r", Class::Repo, f)]);
    assert_eq!(
        plan(&s, &opts(vec![], 30, 365 * DAY)).items[0].action,
        Action::Ask
    );
}

#[test]
fn idle_threshold_is_an_input_not_a_constant() {
    let s = synthetic(vec![repo_node("/r", Class::Repo, all_yes())]);
    // Last activity at t=0; now is 100 days later.
    let now = 100 * DAY;
    assert_eq!(
        plan(&s, &opts(vec![], 99, now)).items[0].action,
        Action::BackupThenRemove
    );
    // Exactly at the window edge still counts as active.
    assert_eq!(
        plan(&s, &opts(vec![], 100, now)).items[0].action,
        Action::Keep
    );
    assert_eq!(
        plan(&s, &opts(vec![], 101, now)).items[0].action,
        Action::Keep
    );
    assert_eq!(
        plan(&s, &opts(vec![], 365, now)).items[0].action,
        Action::Keep
    );
}

#[test]
fn neg11_default_window_keeps_a_repo_touched_six_days_ago_but_a_shorter_window_does_not() {
    let now = 1_000 * DAY;
    let mut n = repo_node("/r", Class::Repo, all_yes());
    n.newest_mtime = Some(now - 6 * DAY);
    n.repo.as_mut().unwrap().last_commit_time = Some(now - 6 * DAY);
    let s = synthetic(vec![n]);

    let default = opts(vec![], DEFAULT_ACTIVE_WINDOW_DAYS, now);
    assert_eq!(DEFAULT_ACTIVE_WINDOW_DAYS, 7);
    let p = plan(&s, &default);
    assert_eq!(p.items[0].action, Action::Keep);
    assert_eq!(p.items[0].state, ProjectState::Active);
    assert!(
        p.items[0].state_reason.contains("7-day"),
        "{}",
        p.items[0].state_reason
    );
    assert_eq!(p.active_window_days, 7);

    let p = plan(&s, &opts(vec![], 3, now));
    assert_ne!(p.items[0].action, Action::Keep);
    assert_eq!(p.items[0].state, ProjectState::Idle);
    assert_eq!(p.items[0].action, Action::BackupThenRemove);
    assert!(p.items[0].state_reason.contains("3-day"));
}

#[test]
fn last_commit_alone_can_make_a_project_active() {
    let now = 1_000 * DAY;
    let mut n = repo_node("/r", Class::Repo, all_yes());
    n.newest_mtime = Some(now - 90 * DAY);
    n.repo.as_mut().unwrap().last_commit_time = Some(now - 2 * DAY);
    let s = synthetic(vec![n]);
    assert_eq!(
        plan(&s, &opts(vec![], 7, now)).items[0].action,
        Action::Keep
    );
}

#[test]
fn a_running_process_inside_a_project_makes_it_and_its_regenerable_dirs_active() {
    let now = 1_000 * DAY;
    let mut repo = repo_node("/work/r", Class::Repo, all_yes());
    repo.newest_mtime = Some(now - 90 * DAY);
    let mut nm = Node::new("/work/r/node_modules", Class::Regenerable);
    nm.parent = Some(0);
    nm.bytes_total = 100;
    let s = synthetic(vec![repo, nm]);

    // Control: nothing running, both idle.
    let p = plan(&s, &opts(vec![], 7, now));
    assert_eq!(p.items[0].action, Action::BackupThenRemove);
    assert_eq!(p.items[1].action, Action::RemoveRegenerable);

    let mut o = opts(vec![], 7, now);
    o.process = ProcessCheck::Ok {
        cwds: vec![PathBuf::from("/work/r/src/deep")],
    };
    let p = plan(&s, &o);
    assert_eq!(p.items[0].action, Action::Keep);
    assert_eq!(p.items[1].action, Action::Keep);
    assert!(p.items[0].state_reason.contains("running process"));
    assert_eq!(p.process_check, "ok");
}

#[test]
fn unknown_process_check_is_recorded_and_blocks_removal() {
    let now = 1_000 * DAY;
    let mut repo = repo_node("/work/r", Class::Repo, all_yes());
    repo.newest_mtime = Some(now - 90 * DAY);
    let mut nm = Node::new("/work/r/node_modules", Class::Regenerable);
    nm.parent = Some(0);
    nm.bytes_total = 100;
    let s = synthetic(vec![repo, nm]);
    let mut o = opts(vec![], 7, now);
    o.process = ProcessCheck::Unknown {
        reason: "denied".to_string(),
    };
    let p = plan(&s, &o);
    assert_eq!(p.process_check, "unknown");
    assert_eq!(p.process_check_note.as_deref(), Some("denied"));
    for it in &p.items {
        assert_eq!(it.action, Action::Ask, "{}", it.path);
        assert_eq!(it.reclaimable_bytes, 0);
    }
    assert_eq!(p.totals.reclaimable_bytes, 0);
}

#[test]
fn real_process_check_does_not_panic_and_reports_a_status() {
    let st = running_cwds();
    assert!(matches!(st.status(), "ok" | "partial" | "unknown"));
    // This test process runs somewhere; if cwds are readable, one is inside the current dir.
    if let Ok(cwd) = std::env::current_dir() {
        if !st.cwds().is_empty() {
            let real = std::fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());
            assert!(
                st.inside(&real).is_some() || st.inside(&cwd).is_some(),
                "the test process itself should be seen in {}",
                cwd.display()
            );
        }
    }
}

#[test]
fn no_activity_evidence_means_ask_never_remove() {
    let mut n = repo_node("/r", Class::Repo, all_yes());
    n.newest_mtime = None;
    let s = synthetic(vec![n]);
    assert_eq!(
        plan(&s, &opts(vec![], 1, 365 * DAY)).items[0].action,
        Action::Ask
    );
}

#[test]
fn loose_code_is_always_ask_unless_active() {
    let mut n = Node::new("/code/thing", Class::LooseCode);
    n.newest_mtime = Some(0);
    n.bytes_total = 10;
    let s = synthetic(vec![n]);
    assert_eq!(
        plan(&s, &opts(vec![], 1, 365 * DAY)).items[0].action,
        Action::Ask
    );
    assert_eq!(
        plan(&s, &opts(vec![], 1, 0)).items[0].action,
        Action::Keep,
        "recently touched loose code is active, so it is left alone"
    );
    let active = vec![PathBuf::from("/code")];
    assert_eq!(
        plan(&s, &opts(active, 1, 365 * DAY)).items[0].action,
        Action::Keep
    );
}

#[test]
fn bare_repo_is_kept_with_dependents_otherwise_ask() {
    let mut with_deps = all_yes();
    with_deps.no_dependents = Tri::No;
    let s = synthetic(vec![
        repo_node("/a.git", Class::BareRepo, with_deps),
        repo_node("/b.git", Class::BareRepo, all_yes()),
    ]);
    let p = plan(&s, &opts(vec![], 1, 365 * DAY));
    assert_eq!(p.items[0].action, Action::Keep);
    assert_eq!(p.items[1].action, Action::Ask);
}

#[test]
fn neg8_repo_under_an_active_path_is_kept_with_its_huge_node_modules() {
    let (_tmp, root, remotes) = workspace();
    let work = root.join("work");
    let repo = work.join("app");
    init_repo(&repo);
    write(&repo.join("package.json"), "{}");
    write(&repo.join(".gitignore"), "node_modules/\n");
    git(&repo, &["add", ".gitignore", "package.json"]);
    git(&repo, &["commit", "-q", "-m", "project files"]);
    write_bytes(
        &repo.join("node_modules").join("big").join("x.js"),
        2_000_000,
    );
    let remote = remotes.join("app.git");
    init_bare(&remote);
    push_to_remote(&repo, &remote);
    let res = scan_with(&root, network_everywhere());
    let now = far_future();

    // Control: not active, so the idle project's node_modules would be removed.
    let p = plan(&res, &opts(vec![], 30, now));
    assert_eq!(
        action_of(&p, &repo.join("node_modules")),
        Action::RemoveRegenerable
    );
    assert_eq!(action_of(&p, &repo), Action::BackupThenRemove);

    // Active: everything under it is kept, however idle.
    let p = plan(&res, &opts(vec![work.clone()], 30, now));
    assert_eq!(action_of(&p, &repo), Action::Keep);
    assert_eq!(action_of(&p, &repo.join("node_modules")), Action::Keep);
    assert_eq!(p.totals.reclaimable_bytes, 0);
    for it in &p.items {
        assert_eq!(it.reclaimable_bytes, 0, "{}", it.path);
    }
}

#[test]
fn an_active_path_inside_a_project_keeps_the_project_and_its_regenerable_dirs() {
    let (_tmp, root, _) = workspace();
    write(&root.join("proj").join("package.json"), "{}");
    write(&root.join("proj").join("src").join("a.js"), "1");
    write_bytes(&root.join("proj").join("node_modules").join("x.js"), 5000);
    let res = scan_plain(&root);
    let active = vec![root.join("proj").join("src")];
    let p = plan(&res, &opts(active, 30, far_future()));
    assert_eq!(action_of(&p, &root.join("proj")), Action::Keep);
    assert_eq!(
        action_of(&p, &root.join("proj").join("node_modules")),
        Action::Keep
    );
}

#[test]
fn neg4_build_dir_with_an_output_file_is_never_offered_for_removal() {
    let (_tmp, root, _) = workspace();
    let with_video = root.join("a");
    let without = root.join("b");
    for proj in [&with_video, &without] {
        // Real repos that ignore out/, so git proves it is generated, untracked output.
        init_repo(proj);
        write(&proj.join("package.json"), "{}");
        write(&proj.join(".gitignore"), "out/\n");
        git(proj, &["add", "package.json", ".gitignore"]);
        git(proj, &["commit", "-q", "-m", "project"]);
        write_bytes(&proj.join("out").join("bundle.js"), 5000);
    }
    write_bytes(&with_video.join("out").join("render.mp4"), 1024);
    let res = scan_plain(&root);
    let p = plan(&res, &opts(vec![], 30, far_future()));
    // Control: the same directory without the video is removable.
    assert_eq!(
        action_of(&p, &without.join("out")),
        Action::RemoveRegenerable
    );
    let flagged = action_of(&p, &with_video.join("out"));
    assert_eq!(flagged, Action::Ask);
    let item = p
        .items
        .iter()
        .find(|i| i.path == with_video.join("out").to_string_lossy())
        .unwrap();
    assert_eq!(item.reclaimable_bytes, 0);
    assert!(item.reason.contains("output"));
}

#[test]
fn recent_projects_keep_their_regenerable_directories() {
    let (_tmp, root, _) = workspace();
    write(&root.join("proj").join("package.json"), "{}");
    write_bytes(&root.join("proj").join("node_modules").join("x.js"), 5000);
    let res = scan_plain(&root);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let p = plan(&res, &opts(vec![], 30, now + 2 * DAY));
    assert_eq!(
        action_of(&p, &root.join("proj").join("node_modules")),
        Action::Keep
    );
}

#[test]
fn totals_do_not_double_count_nested_removals() {
    let mut repo = repo_node("/r", Class::Repo, all_yes());
    repo.bytes_total = 10_000;
    let mut nm = Node::new("/r/node_modules", Class::Regenerable);
    nm.parent = Some(0);
    nm.bytes_total = 6_000;
    nm.bytes_self = 6_000;
    let s = synthetic(vec![repo, nm]);
    let p = plan(&s, &opts(vec![], 30, 365 * DAY));
    assert_eq!(action_of(&p, Path::new("/r")), Action::BackupThenRemove);
    assert_eq!(
        action_of(&p, Path::new("/r/node_modules")),
        Action::RemoveRegenerable
    );
    assert_eq!(p.totals.reclaimable_bytes, 10_000);
}

#[test]
fn g2_a_skipped_mount_inside_a_node_makes_it_and_every_enclosing_node_ask() {
    let now = 365 * DAY;
    let mut repo = repo_node("/w/r", Class::Repo, all_yes());
    repo.newest_mtime = Some(0);
    let mut nm = Node::new("/w/r/node_modules", Class::Regenerable);
    nm.parent = Some(0);
    nm.bytes_total = 100;
    // Control: fully scanned, both are removable.
    let s = synthetic(vec![repo.clone(), nm.clone()]);
    let p = plan(&s, &opts(vec![], 7, now));
    assert_eq!(action_of(&p, Path::new("/w/r")), Action::BackupThenRemove);
    assert_eq!(
        action_of(&p, Path::new("/w/r/node_modules")),
        Action::RemoveRegenerable
    );
    // A mount point inside node_modules was not entered.
    nm.skipped_mounts = 1;
    let s = synthetic(vec![repo, nm]);
    let p = plan(&s, &opts(vec![], 7, now));
    for it in &p.items {
        assert_eq!(it.action, Action::Ask, "{}: {}", it.path, it.reason);
        assert_eq!(it.reclaimable_bytes, 0);
    }
    assert!(p.items[1].reason.contains("another file system"));
    assert_eq!(p.totals.ready_bytes, 0);
}

fn child(path: &str, class: Class, parent: usize) -> Node {
    let mut n = Node::new(path, class);
    n.parent = Some(parent);
    n.bytes_self = 100;
    n.bytes_total = 100;
    n.newest_mtime = Some(0);
    n
}

#[test]
fn f3_a_nested_node_that_asks_stops_the_enclosing_repo_from_being_archived() {
    let now = 365 * DAY;
    let outer = repo_node("/w/outer", Class::Repo, all_yes());
    let mut build = child("/w/outer/build", Class::Regenerable, 0);
    build.contains_outputs = true;
    let s = synthetic(vec![outer, build]);
    let p = plan(&s, &opts(vec![], 7, now));
    assert_eq!(p.items[1].action, Action::Ask);
    assert_eq!(p.items[0].action, Action::Ask);
    assert_eq!(p.items[0].reclaimable_bytes, 0);
    assert_eq!(p.totals.reclaimable_bytes, 0);
}

#[test]
fn f3_a_nested_git_node_with_a_fact_not_yes_stops_the_enclosing_repo() {
    let now = 365 * DAY;
    let outer = repo_node("/w/outer", Class::Repo, all_yes());
    let mut f = all_yes();
    f.stash_empty = Tri::No;
    let sub = {
        let mut n = repo_node("/w/outer/sub", Class::Worktree, f);
        n.parent = Some(0);
        n
    };
    let s = synthetic(vec![outer, sub]);
    let p = plan(&s, &opts(vec![], 7, now));
    assert_eq!(p.items[1].action, Action::RecoverBackupThenRemove);
    assert_eq!(p.items[0].action, Action::Ask);
}

#[test]
fn f3_a_recently_active_nested_project_keeps_the_enclosing_repo_out_of_removal() {
    let now = 365 * DAY;
    let outer = repo_node("/w/outer", Class::Repo, all_yes());
    let mut sub = repo_node("/w/outer/sub", Class::Worktree, all_yes());
    sub.parent = Some(0);
    sub.newest_mtime = Some(now - DAY);
    sub.repo.as_mut().unwrap().last_commit_time = Some(now - DAY);
    let s = synthetic(vec![outer, sub]);
    let p = plan(&s, &opts(vec![], 7, now));
    assert_eq!(p.items[1].action, Action::Keep);
    assert_eq!(p.items[0].action, Action::Ask);
}

#[test]
fn f3_a_removable_regenerable_child_does_not_block_the_parent() {
    let now = 365 * DAY;
    let outer = repo_node("/w/outer", Class::Repo, all_yes());
    let nm = child("/w/outer/node_modules", Class::Regenerable, 0);
    let s = synthetic(vec![outer, nm]);
    let p = plan(&s, &opts(vec![], 7, now));
    assert_eq!(p.items[0].action, Action::BackupThenRemove);
    assert_eq!(p.items[1].action, Action::RemoveRegenerable);
}

#[test]
fn f8_archive_is_not_counted_as_ready_and_remote_facts_are_labelled_unverified() {
    let s = synthetic(vec![repo_node("/r", Class::Repo, all_yes())]);
    let p = plan(&s, &opts(vec![], 7, 365 * DAY));
    assert_eq!(p.items[0].action, Action::BackupThenRemove);
    assert_eq!(
        p.totals.ready_bytes, 0,
        "no recovery step cannot be claimed from local refs"
    );
    assert!(
        p.items[0].reason.contains("unverified"),
        "{}",
        p.items[0].reason
    );
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["remote_facts_verified"], false);
}

#[test]
fn f9_an_absurd_window_keeps_everything_instead_of_wrapping() {
    let mut n = repo_node("/r", Class::Repo, all_yes());
    n.newest_mtime = Some(0);
    let s = synthetic(vec![n]);
    for w in [u64::MAX, u64::MAX / 86_400 + 1, i64::MAX as u64 + 1] {
        let p = plan(&s, &opts(vec![], w, 365 * DAY));
        assert_eq!(p.items[0].action, Action::Keep, "window {w}");
    }
}

#[test]
fn plan_is_a_dry_run_and_serializes_with_a_schema_version() {
    let s = synthetic(vec![repo_node("/r", Class::Repo, all_yes())]);
    let p = plan(&s, &opts(vec![], 30, 365 * DAY));
    assert!(p.dry_run);
    let v: serde_json::Value = serde_json::to_value(&p).unwrap();
    assert_eq!(v["schema_version"], SCHEMA_VERSION);
    assert_eq!(v["items"][0]["action"], "backup-then-remove");
}
