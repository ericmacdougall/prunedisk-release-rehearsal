//! AC-1: `--one-file-system` does not cross mount points. G2: a directory that holds a mount
//! point it did not cross was not fully looked at, so it is never offered for removal.
//!
//! Needs real second filesystems, so it only runs when `DC_TEST_MOUNT_ROOT` names a directory in
//! which `proj/node_modules/mnt` and `repo/data` are empty mount points of other filesystems (CI
//! mounts two tmpfs there on Linux). Without the variable the test is a no-op and says so; it is
//! not counted as verification.

mod common;

use common::*;
use disk_cleaner_core::{plan, scan, Action, FactsConfig, PlanOptions, ScanOptions, ScanResult};
use std::path::{Path, PathBuf};

#[cfg(unix)]
fn assert_is_mount(m: &Path, root: &Path) {
    use std::os::unix::fs::MetadataExt;
    let dev = |p: &Path| std::fs::symlink_metadata(p).unwrap().dev();
    assert_ne!(
        dev(m),
        dev(root),
        "{} must be a mount point of another filesystem",
        m.display()
    );
}

#[cfg(not(unix))]
fn assert_is_mount(_: &Path, _: &Path) {}

fn plan_item(res: &ScanResult, path: &Path) -> (Action, String) {
    let p = plan(res, &PlanOptions::new(7, far_future()));
    let it = p
        .items
        .iter()
        .find(|i| i.path == path.to_string_lossy())
        .unwrap_or_else(|| panic!("no plan item for {}", path.display()));
    (it.action, it.reason.clone())
}

#[test]
fn one_file_system_does_not_cross_a_mount_point() {
    let root = match std::env::var_os("DC_TEST_MOUNT_ROOT") {
        Some(m) => PathBuf::from(m),
        None => {
            eprintln!("DC_TEST_MOUNT_ROOT not set: mount test skipped (not verified here)");
            return;
        }
    };
    let proj = root.join("proj");
    let nm = proj.join("node_modules");
    let nm_mount = nm.join("mnt");
    let repo = root.join("repo");
    let repo_mount = repo.join("data");
    for m in [&nm_mount, &repo_mount] {
        assert_is_mount(m, &root);
    }

    // A project whose node_modules holds a 4 MiB file on the other filesystem.
    write(&proj.join("package.json"), "{}");
    write_bytes(&nm.join("dep").join("index.js"), 1024);
    write_bytes(&nm_mount.join("big.dat"), 4 * 1024 * 1024);
    write_bytes(&root.join("small.dat"), 1024);
    // A pushed repo with an untracked file that lives only on the other filesystem.
    init_repo(&repo);
    let remotes = tempfile::tempdir().unwrap();
    let remote = remotes.path().join("repo.git");
    init_bare(&remote);
    push_to_remote(&repo, &remote);
    write(&repo_mount.join("notes.txt"), "the only copy\n");

    let mut crossing = ScanOptions::new(vec![root.clone()]);
    crossing.collect_facts = false;
    let all = scan(&crossing).unwrap();
    assert!(all.totals.allocated_bytes >= 4 * 1024 * 1024);
    assert_eq!(all.totals.mount_points_skipped, 0);

    let mut one_fs = crossing.clone();
    one_fs.one_file_system = true;
    let limited = scan(&one_fs).unwrap();
    assert!(
        limited.totals.allocated_bytes < 1024 * 1024,
        "mounted data must not be counted, got {}",
        limited.totals.allocated_bytes
    );
    assert_eq!(limited.totals.mount_points_skipped, 2);

    // Controls: scanned in full, node_modules is removable and the repo needs recovery (a
    // removing action), so an `ask` below comes from the skipped mounts and nothing else.
    let facts = FactsConfig::default().with_network_predicate(|_| true);
    let mut full = ScanOptions::new(vec![root.clone()]);
    full.facts = facts.clone();
    let full = scan(&full).unwrap();
    let (a, why) = plan_item(&full, &nm);
    assert_eq!(a, Action::RemoveRegenerable, "{why}");
    let (a, why) = plan_item(&full, &repo);
    assert_eq!(a, Action::RecoverBackupThenRemove, "{why}");

    // G2: with --one-file-system part of each was never looked at.
    let mut part = ScanOptions::new(vec![root.clone()]);
    part.facts = facts;
    part.one_file_system = true;
    let part = scan(&part).unwrap();
    for target in [&nm, &repo] {
        let (a, why) = plan_item(&part, target);
        assert_eq!(a, Action::Ask, "{}: {why}", target.display());
        assert!(
            why.contains("another file system"),
            "{}: {why}",
            target.display()
        );
    }

    // Q3: a child root on another device than its ancestor is not covered by the ancestor under
    // --one-file-system, so it is walked on its own: counted once, and not a skipped mount.
    // (In this function, not a test of its own: tests share the mount root and run in parallel.)
    let totals_of = |roots: Vec<PathBuf>| {
        let mut o = ScanOptions::new(roots);
        o.collect_facts = false;
        o.one_file_system = true;
        scan(&o).unwrap()
    };
    let parent_only = totals_of(vec![root.clone()]);
    let child_only = totals_of(vec![repo_mount.clone()]);
    let both = totals_of(vec![root.clone(), repo_mount.clone()]);
    assert_eq!(both.totals.mount_points_skipped, 1);
    // The mount point directory itself is seen by both single scans and counted once here.
    #[cfg(unix)]
    let mount_dir = {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(&repo_mount).unwrap().blocks() * 512
    };
    #[cfg(not(unix))]
    let mount_dir = 0;
    assert_eq!(
        both.totals.allocated_bytes,
        parent_only.totals.allocated_bytes + child_only.totals.allocated_bytes - mount_dir
    );
    assert_eq!(
        both.totals.files,
        parent_only.totals.files + child_only.totals.files
    );
    assert_eq!(
        both.totals.dirs,
        parent_only.totals.dirs + child_only.totals.dirs - 1
    );
    // The repo was looked at in full, so it is no longer incomplete: like the full scan above.
    let mut with_facts = ScanOptions::new(vec![root.clone(), repo_mount.clone()]);
    with_facts.facts = FactsConfig::default().with_network_predicate(|_| true);
    with_facts.one_file_system = true;
    let with_facts = scan(&with_facts).unwrap();
    let (a, why) = plan_item(&with_facts, &repo);

    // R1 and gate 98f8764 T2: the mount point under node_modules was not entered, so the scan's
    // coverage is incomplete. By default the repository's dependents are then unknown and it
    // asks, naming what was not entered and the opt-in.
    assert_eq!(a, Action::Ask, "{why}");
    assert!(why.contains("1 mount point was not entered"), "{why}");
    assert!(why.contains("--accept-incomplete-coverage"), "{why}");
    let v = serde_json::to_value(&with_facts).unwrap();
    let cov = &v["coverage"];
    assert_eq!(cov["complete"], false, "{cov}");
    assert_eq!(cov["mount_points_skipped"], 1, "{cov}");
    assert_eq!(cov["unreadable_dirs"], 0, "{cov}");
    assert_eq!(
        cov["mount_point_examples"],
        serde_json::json!([nm_mount.to_string_lossy()]),
        "{cov}"
    );
    let p = plan(&with_facts, &PlanOptions::new(7, far_future()));
    let p = serde_json::to_value(p).unwrap();
    assert_eq!(p["coverage_complete"], false);
    assert_eq!(p["accepted_incomplete_coverage"], false);
    let note = p["coverage_note"].as_str().unwrap_or_default();
    assert!(note.contains("1 mount point was not entered"), "{note}");

    // With --accept-incomplete-coverage the repository needs recovery again, and its reason
    // carries what was not entered.
    let mut accept = PlanOptions::new(7, far_future());
    accept.accept_incomplete_coverage = true;
    let p = plan(&with_facts, &accept);
    let it = p
        .items
        .iter()
        .find(|i| i.path == repo.to_string_lossy())
        .expect("plan item for the repo");
    assert_eq!(it.action, Action::RecoverBackupThenRemove, "{}", it.reason);
    let sentence = "1 mount point in this scan was not entered (--one-file-system); dependents \
                    inside it were not checked";
    assert!(it.reason.contains(sentence), "{}", it.reason);
    assert!(p.accepted_incomplete_coverage);
}
