//! Regression tests for the independent click-test of 133cbfd (PR #2): hidden index entries
//! (S1), reason wording (S2) and facts said in words (S3).
//!
//! S1 and the git-made wording cases use real git; the rest build a scan by hand.

mod common;

use common::*;
use disk_cleaner_core::{
    plan, Action, Class, Coverage, Facts, Node, Plan, PlanItem, PlanOptions, ProcessCheck,
    RepoInfo, ScanOptionsRecord, ScanResult, Totals, Tri, SCHEMA_VERSION,
};
use std::path::Path;

const DAY: i64 = 86_400;

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

// ---------------------------------------------------------------- S1

/// An edit to `README.md` that `status`, `diff HEAD` and `add -A` do not see.
fn assert_edit_is_hidden_from_git(repo: &Path) {
    assert_eq!(git(repo, &["status", "--porcelain"]), "");
    assert_eq!(git(repo, &["diff", "HEAD", "--name-only"]), "");
    let text = std::fs::read_to_string(repo.join("README.md")).unwrap();
    assert!(text.contains("local edit"), "the edit is on disk");
}

fn assert_hidden_entry_asks(res: &ScanResult, a: &Path) {
    assert_eq!(hidden_entries(res, a), 1);
    let p = plan_of(res);
    let it = item(&p, a);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    assert!(
        it.reason.contains(
            "1 file is marked skip-worktree or assume-unchanged, so git hides edits to it and it \
             cannot be saved automatically; review it first"
        ),
        "{}",
        it.reason
    );
}

#[test]
fn s1_an_edited_skip_worktree_file_makes_the_repository_ask() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    git(&a, &["update-index", "--skip-worktree", "README.md"]);
    write(&a.join("README.md"), "hello\nlocal edit\n");
    assert_edit_is_hidden_from_git(&a);
    let res = scan_with(&root, network_everywhere());
    assert_hidden_entry_asks(&res, &a);
}

#[test]
fn s1_an_edited_assume_unchanged_file_makes_the_repository_ask() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    git(&a, &["update-index", "--assume-unchanged", "README.md"]);
    write(&a.join("README.md"), "hello\nlocal edit\n");
    assert_edit_is_hidden_from_git(&a);
    let res = scan_with(&root, network_everywhere());
    assert_hidden_entry_asks(&res, &a);
}

#[test]
fn s1_a_hidden_edit_still_asks_when_there_is_also_an_untracked_file() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    git(&a, &["update-index", "--skip-worktree", "README.md"]);
    write(&a.join("README.md"), "hello\nlocal edit\n");
    write(&a.join("scratch.txt"), "untracked\n");
    let res = scan_with(&root, network_everywhere());
    assert_hidden_entry_asks(&res, &a);
}

#[test]
fn s1_a_cone_sparse_checkout_with_everything_pushed_asks_only_for_its_pattern_file() {
    let (_tmp, root, remotes) = workspace();
    let a = root.join("a");
    init_repo(&a);
    write(&a.join("docs").join("guide.md"), "guide\n");
    write(&a.join("src").join("lib.txt"), "lib\n");
    git(&a, &["add", "docs", "src"]);
    git(&a, &["commit", "-q", "-m", "more"]);
    let remote = remotes.join("a.git");
    init_bare(&remote);
    push_to_remote(&a, &remote);
    git(&a, &["sparse-checkout", "init", "--cone"]);
    git(&a, &["sparse-checkout", "set", "src"]);
    // Proof: docs/ left the working tree and its entries carry the skip-worktree bit.
    assert!(!a.join("docs").join("guide.md").exists());
    assert!(git(&a, &["ls-files", "-v", "docs/guide.md"]).starts_with("S "));

    let res = scan_with(&root, network_everywhere());
    assert_eq!(hidden_entries(&res, &a), 0);
    assert_eq!(facts_of(&res, &a).tree_clean, Tri::Yes);
    // The pattern list in `info/sparse-checkout` exists only here: since gate d1e3f23 (W1)
    // every file in `info/` other than the stock `exclude` is the repository's own content, so
    // the plan asks for that file, and for nothing else.
    let mut f = facts_of(&res, &a);
    assert_eq!(f.no_local_admin_content, Tri::No, "{f:?}");
    f.no_local_admin_content = Tri::Yes;
    assert!(f.all_yes(), "{f:?}");
    let p = plan_of(&res);
    let it = item(&p, &a);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert!(it.reason.contains("info/sparse-checkout"), "{}", it.reason);
}

#[test]
fn s1_an_ordinary_untracked_file_is_still_recover_then_archive_in_words() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    write(&a.join("scratch.txt"), "untracked\n");
    let res = scan_with(&root, network_everywhere());
    assert_eq!(hidden_entries(&res, &a), 0);
    let p = plan_of(&res);
    let it = item(&p, &a);
    assert_eq!(it.action, Action::RecoverBackupThenRemove, "{}", it.reason);
    assert!(
        it.reason
            .contains("uncommitted changes or untracked files [tree_clean]"),
        "{}",
        it.reason
    );
}

// ---------------------------------------------------------------- S2, real git

#[test]
fn s2a_a_ref_to_a_blob_is_described_without_empty_parts() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let blob = git(&a, &["hash-object", "-w", "README.md"]);
    git(&a, &["update-ref", "refs/tags/blobref", &blob]);
    let res = scan_with(&root, network_everywhere());
    let issue = node(&res, &a)
        .repo
        .as_ref()
        .unwrap()
        .layout_issue
        .clone()
        .unwrap_or_default();
    assert_eq!(
        issue,
        "the ref refs/tags/blobref does not point at a commit (it is a blob)"
    );
}

#[test]
fn s2b_a_tracked_node_modules_reason_reads_as_a_sentence() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    write(&a.join("package.json"), "{}\n");
    write(
        &a.join("node_modules").join("dep").join("index.js"),
        "x=1\n",
    );
    git(&a, &["add", "-f", "package.json", "node_modules"]);
    git(&a, &["commit", "-q", "-m", "vendor"]);
    git(&a, &["push", "-q", "origin", "main"]);
    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &a.join("node_modules"));
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert!(
        it.reason.contains("tracked, partly tracked or not ignored"),
        "{}",
        it.reason
    );
    for bad in ["but it git", "but it no ", "but it cannot verify"] {
        assert!(!it.reason.contains(bad), "{bad:?} in {}", it.reason);
    }
}

#[test]
fn s2e_a_main_repository_names_its_linked_worktrees() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let wt = root.join("linked-wt");
    git(
        &a,
        &["worktree", "add", "-q", "-b", "side", wt.to_str().unwrap()],
    );
    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &a);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert!(
        it.reason.contains("worktrees is not empty: ") && it.reason.contains("linked-wt"),
        "{}",
        it.reason
    );
}

#[cfg(unix)]
#[test]
fn s2g_a_convenience_link_is_called_a_link_not_a_checkout() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let links = root.join("links");
    std::fs::create_dir_all(&links).unwrap();
    std::os::unix::fs::symlink("../a/.git/hooks", links.join("hooks")).unwrap();
    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &a);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    let want = format!(
        "a link at {} points into this repository's git directory",
        links.join("hooks").display()
    );
    assert!(it.reason.contains(&want), "{}", it.reason);
    assert!(!it.reason.contains("checkout"), "{}", it.reason);
}

// ---------------------------------------------------------------- S2, S3 by hand

fn synthetic(nodes: Vec<Node>, coverage: Coverage) -> ScanResult {
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
        coverage,
    }
}

fn complete() -> Coverage {
    Coverage {
        complete: true,
        ..Coverage::default()
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

fn repo_node(path: &str, facts: Facts) -> Node {
    let mut n = Node::new(path, Class::Repo);
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

fn opts() -> PlanOptions {
    PlanOptions::new(7, 365 * DAY)
}

#[test]
fn s2c_one_dependent_is_counted_in_the_singular() {
    let mut n = repo_node("/w/a", all_yes());
    let info = n.repo.as_mut().unwrap();
    info.facts.no_dependents = Tri::No;
    info.dependents = vec!["/w/b".to_string()];
    let p = plan(&synthetic(vec![n], complete()), &opts());
    let r = &p.items[0].reason;
    assert!(
        r.contains("1 other checkout or clone uses this repository's objects or refs: /w/b"),
        "{r}"
    );
    let mut n = repo_node("/w/a", all_yes());
    let info = n.repo.as_mut().unwrap();
    info.facts.no_dependents = Tri::No;
    info.dependents = vec!["/w/b".to_string(), "/w/c".to_string()];
    let p = plan(&synthetic(vec![n], complete()), &opts());
    let r = &p.items[0].reason;
    assert!(
        r.contains("2 other checkouts or clones use this repository's objects or refs"),
        "{r}"
    );
}

#[test]
fn s2d_an_archive_reason_says_no_dependents_were_found() {
    // Incomplete coverage asks by default (gate 98f8764, T2); the opt-in restores the archive.
    let incomplete = Coverage {
        complete: false,
        unreadable_dirs: 1,
        unreadable_dir_examples: vec!["/w/locked".to_string()],
        ..Coverage::default()
    };
    let p = plan(
        &synthetic(vec![repo_node("/w/a", all_yes())], incomplete.clone()),
        &opts(),
    );
    assert_eq!(p.items[0].action, Action::Ask, "{}", p.items[0].reason);
    let mut accept = opts();
    accept.accept_incomplete_coverage = true;
    let p = plan(
        &synthetic(vec![repo_node("/w/a", all_yes())], incomplete),
        &accept,
    );
    let it = &p.items[0];
    assert_eq!(it.action, Action::BackupThenRemove, "{}", it.reason);
    assert!(
        it.reason.contains("no dependents were found"),
        "{}",
        it.reason
    );
    assert!(it
        .reason
        .contains("1 folder in this scan could not be read"));
    assert!(p.accepted_incomplete_coverage);
    let p = plan(
        &synthetic(vec![repo_node("/w/a", all_yes())], complete()),
        &opts(),
    );
    let it = &p.items[0];
    assert_eq!(it.action, Action::BackupThenRemove, "{}", it.reason);
    assert!(
        it.reason.contains("no dependents were found"),
        "{}",
        it.reason
    );
    assert!(
        !it.reason.contains("nothing depends on it"),
        "{}",
        it.reason
    );
}

#[test]
fn s2f_a_nested_veto_leads_the_reason_with_or_without_the_process_check() {
    let outer = repo_node("/w/outer", all_yes());
    let mut build = Node::new("/w/outer/build", Class::Regenerable);
    build.parent = Some(0);
    build.bytes_self = 100;
    build.bytes_total = 100;
    build.newest_mtime = Some(0);
    build.contains_outputs = true;
    let s = synthetic(vec![outer, build], complete());
    let mut partial = opts();
    partial.process = ProcessCheck::Partial {
        cwds: vec![],
        unreadable: 3,
    };
    for o in [opts(), partial] {
        let p = plan(&s, &o);
        let it = &p.items[0];
        assert_eq!(it.action, Action::Ask, "{}", it.reason);
        assert!(
            it.reason
                .starts_with("the nested /w/outer/build needs a person's decision"),
            "{}",
            it.reason
        );
    }
}

const FACTS: [&str; 11] = [
    "has_network_remote",
    "head_on_remote",
    "all_branches_on_remote",
    "stash_empty",
    "tree_clean",
    "no_unique_ignored",
    "no_dependents",
    "reflog_saved",
    "no_pending_state",
    "plain_layout",
    "no_local_admin_content",
];

fn with_fact(name: &str, v: Tri) -> Facts {
    let mut f = all_yes();
    match name {
        "has_network_remote" => f.has_network_remote = v,
        "head_on_remote" => f.head_on_remote = v,
        "all_branches_on_remote" => f.all_branches_on_remote = v,
        "stash_empty" => f.stash_empty = v,
        "tree_clean" => f.tree_clean = v,
        "no_unique_ignored" => f.no_unique_ignored = v,
        "no_dependents" => f.no_dependents = v,
        "reflog_saved" => f.reflog_saved = v,
        "no_pending_state" => f.no_pending_state = v,
        "plain_layout" => f.plain_layout = v,
        _ => f.no_local_admin_content = v,
    }
    f
}

/// The reason with every `[fact]` tag removed.
fn untagged(reason: &str) -> String {
    let mut out = reason.to_string();
    for f in FACTS {
        out = out.replace(&format!(" [{f}]"), "");
    }
    out
}

#[test]
fn s3_every_fact_that_is_no_or_unknown_is_said_in_words_with_a_trailing_tag() {
    for name in FACTS {
        for v in [Tri::No, Tri::Unknown] {
            let p = plan(
                &synthetic(vec![repo_node("/w/a", with_fact(name, v))], complete()),
                &opts(),
            );
            let r = &p.items[0].reason;
            assert!(
                r.contains(&format!(" [{name}]")),
                "{name}={}: no trailing tag in {r}",
                v.as_str()
            );
            let words = untagged(r);
            assert!(
                !words.contains(name),
                "{name}={}: the raw fact name is the explanation in {r}",
                v.as_str()
            );
            assert!(!r.contains(&format!("({name}")), "{r}");
        }
    }
}
