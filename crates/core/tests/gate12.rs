//! Regression tests for the gate review of 98f8764 (PR #2): file names git prints are compared
//! byte for byte (T1), and incomplete coverage makes repositories ask by default (T2).
//!
//! The T1 fixtures need file names that are not valid UTF-8, which only Unix file systems other
//! than APFS accept. Where the file system refuses such a name the test says so and returns; on
//! Linux it must not refuse, so the CI job on Linux always exercises them.

mod common;

use common::*;
use disk_cleaner_core::{plan, Action, Coverage, Plan, PlanItem, PlanOptions, ScanResult, Tri};
use std::path::Path;

fn plan_of(res: &ScanResult) -> Plan {
    plan(res, &PlanOptions::new(7, far_future()))
}

fn item<'a>(p: &'a Plan, path: &Path) -> &'a PlanItem {
    p.items
        .iter()
        .find(|i| i.path == path.to_string_lossy())
        .unwrap_or_else(|| panic!("no plan item for {}", path.display()))
}

/// `repo.hidden_index_entries` as JSON (0 when absent).
fn hidden_entries(res: &ScanResult, repo: &Path) -> u64 {
    let v = serde_json::to_value(node(res, repo).repo.as_ref().expect("repo info")).unwrap();
    v["hidden_index_entries"].as_u64().unwrap_or(0)
}

#[cfg(unix)]
mod bytes {
    use super::*;
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::OsStrExt;

    /// A file name with the byte 0xFF, which is never valid UTF-8.
    pub fn bad_name(stem: &str) -> OsString {
        let mut b = stem.as_bytes().to_vec();
        b.push(0xFF);
        b.extend_from_slice(b".txt");
        OsStr::from_bytes(&b).to_os_string()
    }

    /// Can `dir` hold a file named `name`? APFS refuses names that are not valid UTF-8; every
    /// Linux file system accepts any bytes but `/` and NUL, so there a refusal is a failure.
    pub fn accepts(dir: &Path, name: &OsStr) -> bool {
        let probe = dir.join(name);
        match std::fs::write(&probe, "probe") {
            Ok(()) => {
                std::fs::remove_file(&probe).unwrap();
                true
            }
            Err(e) => {
                if cfg!(target_os = "linux") {
                    panic!("a Linux file system refused a non-UTF-8 name: {e}");
                }
                eprintln!("SKIPPED: this file system refuses the name {name:?} ({e})");
                false
            }
        }
    }

    /// Run git with arguments that may not be valid UTF-8.
    pub fn git_os(dir: &Path, args: &[&OsStr]) {
        let out = crate::common::git_command()
            .args(args)
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A pushed repository `a` that also tracks (and has pushed) a file named `name`.
    pub fn pushed_with(root: &Path, remotes: &Path, name: &OsStr) -> std::path::PathBuf {
        let a = pushed_repo(root, remotes, "a");
        write(&a.join(name), "tracked\n");
        git(&a, &["add", "-A"]);
        git(&a, &["commit", "-q", "-m", "odd name"]);
        git(&a, &["push", "-q", "origin", "main"]);
        assert_eq!(git(&a, &["status", "--porcelain"]), "");
        a
    }

    /// Mark `name` with `flag`, edit it, and check git no longer shows the edit.
    pub fn hide_edit(a: &Path, name: &OsStr, flag: &str) {
        git_os(a, &[OsStr::new("update-index"), OsStr::new(flag), name]);
        write(&a.join(name), "tracked\nlocal edit\n");
        assert_eq!(git(a, &["status", "--porcelain"]), "", "hidden");
    }

    pub fn assert_asks_for_one_hidden_file(res: &ScanResult, a: &Path) {
        assert_eq!(hidden_entries(res, a), 1, "{:?}", node(res, a).repo);
        let p = plan_of(res);
        let it = item(&p, a);
        assert_eq!(it.action, Action::Ask, "{}", it.reason);
        assert_eq!(it.reclaimable_bytes, 0);
        assert!(
            it.reason
                .contains("1 file is marked skip-worktree or assume-unchanged"),
            "{}",
            it.reason
        );
    }
}

#[cfg(unix)]
#[test]
fn t1_a_non_utf8_skip_worktree_file_with_an_edit_makes_the_repository_ask() {
    use bytes::*;
    let (_tmp, root, remotes) = workspace();
    let name = bad_name("notes");
    if !accepts(&root, &name) {
        return;
    }
    let a = pushed_with(&root, &remotes, &name);
    hide_edit(&a, &name, "--skip-worktree");
    let res = scan_with(&root, network_everywhere());
    assert_asks_for_one_hidden_file(&res, &a);
}

#[cfg(unix)]
#[test]
fn t1_a_non_utf8_assume_unchanged_file_with_an_edit_makes_the_repository_ask() {
    use bytes::*;
    let (_tmp, root, remotes) = workspace();
    let name = bad_name("notes");
    if !accepts(&root, &name) {
        return;
    }
    let a = pushed_with(&root, &remotes, &name);
    hide_edit(&a, &name, "--assume-unchanged");
    let res = scan_with(&root, network_everywhere());
    assert_asks_for_one_hidden_file(&res, &a);
}

#[cfg(unix)]
#[test]
fn t1_an_untracked_non_utf8_file_makes_the_tree_not_clean() {
    use bytes::*;
    let (_tmp, root, remotes) = workspace();
    let name = bad_name("scratch");
    if !accepts(&root, &name) {
        return;
    }
    let a = pushed_repo(&root, &remotes, "a");
    std::fs::write(a.join(&name), "untracked\n").unwrap();
    let res = scan_with(&root, network_everywhere());
    assert_ne!(facts_of(&res, &a).tree_clean, Tri::Yes);
    assert_ne!(action_for(&res, &a), Action::BackupThenRemove);
}

#[cfg(unix)]
#[test]
fn t1_an_ignored_non_utf8_file_outside_regenerable_directories_is_unique() {
    use bytes::*;
    let (_tmp, root, remotes) = workspace();
    let name = bad_name("keys");
    if !accepts(&root, &name) {
        return;
    }
    let a = pushed_repo(&root, &remotes, "a");
    commit_file(&a, ".gitignore", "*.txt\n");
    git(&a, &["push", "-q", "origin", "main"]);
    std::fs::write(a.join(&name), "secret\n").unwrap();
    let res = scan_with(&root, network_everywhere());
    assert_ne!(facts_of(&res, &a).no_unique_ignored, Tri::Yes);
    assert_ne!(action_for(&res, &a), Action::BackupThenRemove);
}

#[cfg(unix)]
#[test]
fn t1_a_clean_clone_inside_a_non_utf8_directory_is_read_exactly_and_asks() {
    use bytes::*;
    let (_tmp, root, remotes) = workspace();
    let dir_name = bad_name("projects");
    if !accepts(&root, &dir_name) {
        return;
    }
    let c = root.join(&dir_name).join("clone");
    init_repo(&c);
    let remote = remotes.join("clone.git");
    init_bare(&remote);
    push_to_remote(&c, &remote);

    let res = scan_with(&root, network_everywhere());
    // Git's output names this directory in bytes that are not UTF-8; read exactly, they are the
    // scanned directory, so the facts are real rather than unknown.
    let f = facts_of(&res, &c);
    assert_eq!(f.tree_clean, Tri::Yes, "{:?}", node(&res, &c).repo);
    assert_eq!(f.head_on_remote, Tri::Yes, "{:?}", node(&res, &c).repo);
    assert_eq!(f.no_dependents, Tri::Yes, "{:?}", node(&res, &c).repo);
    assert_eq!(f.plain_layout, Tri::Yes, "{:?}", node(&res, &c).repo);
    // Its path cannot be stored exactly in the scan file, so the plan cannot match it against
    // --active paths or running processes: it asks.
    let p = plan_of(&res);
    let it = item(&p, &c);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert!(it.reason.contains("not valid UTF-8"), "{}", it.reason);
    // The scan file still round-trips.
    let text = serde_json::to_string(&res).unwrap();
    let back = disk_cleaner_core::scan_from_json(&text);
    assert!(back.is_ok(), "the scan file reads back: {back:?}");
}

#[cfg(unix)]
#[test]
fn t1_names_with_a_newline_or_a_quote_are_not_misread() {
    use bytes::*;
    use std::ffi::OsStr;
    for name in ["new\nline.txt", "quo\"te.txt"] {
        let (_tmp, root, remotes) = workspace();
        let name = OsStr::new(name);
        if !accepts(&root, name) {
            continue;
        }
        let a = pushed_with(&root, &remotes, name);
        hide_edit(&a, name, "--skip-worktree");
        let res = scan_with(&root, network_everywhere());
        assert_asks_for_one_hidden_file(&res, &a);
    }
}

// ---------------------------------------------------------------- T2

fn one_unreadable_folder() -> Coverage {
    Coverage {
        complete: false,
        unreadable_dirs: 1,
        unreadable_dir_examples: vec!["/w/locked".to_string()],
        ..Coverage::default()
    }
}

fn synthetic(nodes: Vec<disk_cleaner_core::Node>, coverage: Coverage) -> ScanResult {
    ScanResult {
        schema_version: disk_cleaner_core::SCHEMA_VERSION,
        tool_version: "test".into(),
        generated_at: 0,
        roots: vec![],
        options: disk_cleaner_core::ScanOptionsRecord::default(),
        totals: disk_cleaner_core::Totals::default(),
        nodes,
        outputs: vec![],
        big_files: vec![],
        issues: vec![],
        warnings: vec![],
        enclosing_repos: vec![],
        scope: "subtree".to_string(),
        coverage,
    }
}

fn saved_repo(path: &str) -> disk_cleaner_core::Node {
    use disk_cleaner_core::{Class, Facts, Node, RepoInfo};
    let mut n = Node::new(path, Class::Repo);
    n.bytes_self = 5_000;
    n.bytes_total = 5_000;
    n.newest_mtime = Some(0);
    n.repo = Some(RepoInfo {
        facts: Facts {
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
        },
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

fn year_later() -> PlanOptions {
    PlanOptions::new(7, 365 * 86_400)
}

#[test]
fn t2_incomplete_coverage_makes_a_saved_repository_ask_by_default() {
    let p = plan(
        &synthetic(vec![saved_repo("/w/a")], one_unreadable_folder()),
        &year_later(),
    );
    let it = &p.items[0];
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    assert_eq!(it.facts.unwrap().no_dependents, Tri::Unknown);
    assert!(
        it.reason.contains("1 folder could not be read"),
        "{}",
        it.reason
    );
    assert!(
        it.reason.contains("--accept-incomplete-coverage"),
        "the reason names the opt-in: {}",
        it.reason
    );
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["accepted_incomplete_coverage"], false, "{v}");
    assert!(v.get("required_full_coverage").is_none(), "{v}");
}

#[test]
fn t2_a_repository_that_asks_for_another_reason_also_names_what_was_not_read() {
    // Its facts show `no_dependents` as unknown, so its reason must say why in words too.
    let mut n = saved_repo("/w/a");
    n.repo.as_mut().unwrap().facts.stash_empty = Tri::Unknown;
    let res = synthetic(vec![n], one_unreadable_folder());
    let p = plan(&res, &year_later());
    let it = &p.items[0];
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.facts.unwrap().no_dependents, Tri::Unknown);
    assert!(it.reason.contains("[stash_empty]"), "{}", it.reason);
    assert!(
        it.reason.contains("1 folder could not be read"),
        "{}",
        it.reason
    );
    assert!(
        it.reason.contains("--accept-incomplete-coverage"),
        "{}",
        it.reason
    );
}

#[test]
fn t2_a_skipped_mount_point_makes_a_saved_repository_ask_by_default() {
    let coverage = Coverage {
        complete: false,
        mount_points_skipped: 1,
        mount_point_examples: vec!["/w/mnt".to_string()],
        ..Coverage::default()
    };
    let p = plan(
        &synthetic(vec![saved_repo("/w/a")], coverage),
        &year_later(),
    );
    let it = &p.items[0];
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert!(
        it.reason.contains("1 mount point was not entered"),
        "{}",
        it.reason
    );
}

#[test]
fn t2_regenerable_data_is_still_ready_when_coverage_is_incomplete() {
    use disk_cleaner_core::{Class, Node};
    let mut proj = Node::new("/w/app", Class::LooseCode);
    proj.newest_mtime = Some(0);
    let mut nm = Node::new("/w/app/node_modules", Class::Regenerable);
    nm.parent = Some(0);
    nm.rule = Some("node_modules".into());
    nm.bytes_self = 700;
    nm.bytes_total = 700;
    nm.newest_mtime = Some(0);
    let p = plan(
        &synthetic(vec![proj, nm], one_unreadable_folder()),
        &year_later(),
    );
    let it = p
        .items
        .iter()
        .find(|i| i.path == "/w/app/node_modules")
        .unwrap();
    assert_eq!(it.action, Action::RemoveRegenerable, "{}", it.reason);
    assert_eq!(p.totals.ready_bytes, 700);
}

#[test]
fn t2_the_old_setting_is_refused_with_the_names_of_the_new_one() {
    use disk_cleaner_core::config::parse;
    let err = parse("require_full_coverage = true\n", Path::new("c.toml"))
        .expect_err("the old key must not be accepted");
    let msg = err.to_string();
    assert!(msg.contains("accept_incomplete_coverage"), "{msg}");
    let ok = |t: &str| parse(t, Path::new("c.toml")).is_ok();
    assert!(ok("accept_incomplete_coverage = true\n"));
    assert!(ok("accept_incomplete_coverage = false\n"));
}
