//! Gate review of e3885ef, U2: a nested scan root that is walked on its own (another file system
//! under `--one-file-system`) belongs to the directories that physically enclose it, however the
//! user spelled it. A repository there must stop the enclosing `node_modules` from being offered
//! for removal when the root is spelled through a symlink alias, exactly as it does when the
//! spellings match.
//!
//! Needs real second file systems, so it only runs when `DC_TEST_ALIAS_MOUNT_ROOT` names a
//! directory in which `withrepo/proj/node_modules/mnt` and `control/proj/node_modules/mnt` are
//! empty mount points of other file systems (CI mounts two tmpfs there on Linux). Without the
//! variable the test is a no-op and says so; it is not counted as verification.
#![cfg(unix)]

mod common;

use common::*;
use disk_cleaner_core::{plan, scan, Action, PlanOptions, ScanOptions, ScanResult};
use std::path::{Path, PathBuf};

fn assert_is_mount(m: &Path, root: &Path) {
    use std::os::unix::fs::MetadataExt;
    let dev = |p: &Path| std::fs::symlink_metadata(p).unwrap().dev();
    assert_ne!(
        dev(m),
        dev(root),
        "{} must be a mount point of another file system",
        m.display()
    );
}

fn plan_item(res: &ScanResult, path: &Path) -> (Action, String, u64) {
    let p = plan(res, &PlanOptions::new(7, far_future()));
    let it = p
        .items
        .iter()
        .find(|i| i.path == path.to_string_lossy())
        .unwrap_or_else(|| panic!("no plan item for {}", path.display()));
    (it.action, it.reason.clone(), it.reclaimable_bytes)
}

fn scan_one_fs(roots: Vec<PathBuf>) -> ScanResult {
    let mut o = ScanOptions::new(roots);
    o.facts = network_everywhere();
    o.one_file_system = true;
    scan(&o).unwrap()
}

#[test]
fn a_nested_root_spelled_through_an_alias_still_protects_the_enclosing_artifact() {
    let base = match std::env::var_os("DC_TEST_ALIAS_MOUNT_ROOT") {
        Some(m) => PathBuf::from(m),
        None => {
            eprintln!(
                "DC_TEST_ALIAS_MOUNT_ROOT not set: alias mount test skipped (not verified here)"
            );
            return;
        }
    };
    let with = base.join("withrepo");
    let ctl = base.join("control");
    for d in [&with, &ctl] {
        assert_is_mount(&d.join("proj").join("node_modules").join("mnt"), d);
        write(&d.join("proj").join("package.json"), "{}");
        write_bytes(
            &d.join("proj")
                .join("node_modules")
                .join("dep")
                .join("index.js"),
            1024,
        );
    }
    // On the other file system under node_modules: a repository with unpushed work.
    let vendored = with
        .join("proj")
        .join("node_modules")
        .join("mnt")
        .join("vendored");
    init_repo(&vendored);
    let remotes = tempfile::tempdir().unwrap();
    let remote = remotes.path().join("vendored.git");
    init_bare(&remote);
    push_to_remote(&vendored, &remote);
    commit_file(&vendored, "local.txt", "the only copy\n");
    // The control holds only a plain file there.
    write_bytes(
        &ctl.join("proj")
            .join("node_modules")
            .join("mnt")
            .join("blob.dat"),
        1024,
    );

    // Aliases of the scan directories, outside them.
    let links = tempfile::tempdir().unwrap();
    let alias_with = links.path().join("alias-with");
    let alias_ctl = links.path().join("alias-ctl");
    std::os::unix::fs::symlink(&with, &alias_with).unwrap();
    std::os::unix::fs::symlink(&ctl, &alias_ctl).unwrap();
    let sub = |d: &Path| d.join("proj").join("node_modules").join("mnt");

    // Control, no nested repository: node_modules is removable with either spelling.
    let ctl_nm = ctl.join("proj").join("node_modules");
    for roots in [
        vec![ctl.clone(), sub(&alias_ctl)],
        vec![sub(&alias_ctl), ctl.clone()],
        vec![ctl.clone(), sub(&ctl)],
    ] {
        let res = scan_one_fs(roots.clone());
        let (a, why, _) = plan_item(&res, &ctl_nm);
        assert_eq!(a, Action::RemoveRegenerable, "{roots:?}: {why}");
    }

    // Matching spellings (existing behaviour) and the alias spelling, in both orders: the
    // repository inside makes node_modules ask, and nothing is reclaimable.
    let nm = with.join("proj").join("node_modules");
    for roots in [
        vec![with.clone(), sub(&with)],
        vec![with.clone(), sub(&alias_with)],
        vec![sub(&alias_with), with.clone()],
    ] {
        let res = scan_one_fs(roots.clone());
        let (a, why, bytes) = plan_item(&res, &nm);
        assert_eq!(a, Action::Ask, "{roots:?}: {why}");
        assert_eq!(bytes, 0, "{roots:?}: {why}");
    }
}
