//! Regression tests for the click-test defects of PR #2 (L1: hostile scan files, L2: git
//! directories reached through a `.git` symlink, D3: wording in a subtree scan).

mod common;

use common::*;
use disk_cleaner_core::{plan, scan_from_json, Action, PlanOptions, ScanResult};
use serde_json::Value;

// ---------------------------------------------------------------- L1: scan document checks

/// A valid scan (project with a node_modules) as JSON, and the temp dir that keeps it alive.
fn valid_scan() -> (tempfile::TempDir, ScanResult, Value) {
    let (tmp, root, _) = workspace();
    let proj = root.join("proj");
    write(&proj.join("package.json"), "{}\n");
    write(&proj.join("node_modules").join("dep").join("a.js"), "x\n");
    let res = scan_plain(&root);
    let v = serde_json::to_value(&res).unwrap();
    (tmp, res, v)
}

fn load(v: &Value) -> Result<ScanResult, String> {
    scan_from_json(&v.to_string()).map_err(|e| e.to_string())
}

fn child_index(v: &Value) -> usize {
    v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|n| n.get("parent").is_some())
        .expect("a node with a parent")
}

#[test]
fn l1_the_valid_scan_loads_and_plans() {
    let (_t, _res, v) = valid_scan();
    let s = load(&v).expect("control");
    let p = plan(&s, &PlanOptions::new(7, far_future()));
    assert!(!p.items.is_empty());
}

#[test]
fn l1_out_of_range_parent_is_a_clean_error() {
    let (_t, _res, mut v) = valid_scan();
    let i = child_index(&v);
    v["nodes"][i]["parent"] = Value::from(999_999u64);
    let e = load(&v).expect_err("must be rejected");
    assert!(e.contains(&format!("nodes[{i}].parent")), "{e}");
}

#[test]
fn l1_self_parent_is_a_clean_error() {
    let (_t, _res, mut v) = valid_scan();
    let i = child_index(&v);
    v["nodes"][i]["parent"] = Value::from(i as u64);
    let e = load(&v).expect_err("must be rejected");
    assert!(e.contains(&format!("nodes[{i}].parent")), "{e}");
}

#[test]
fn l1_a_two_node_parent_cycle_is_a_clean_error() {
    let (_t, _res, mut v) = valid_scan();
    let i = child_index(&v);
    let p = v["nodes"][i]["parent"].as_u64().unwrap();
    v["nodes"][p as usize]["parent"] = Value::from(i as u64);
    let e = load(&v).expect_err("must be rejected");
    assert!(e.contains("cycle"), "{e}");
}

#[test]
fn l1_out_of_range_cross_references_are_clean_errors() {
    let (_t, _res, v) = valid_scan();
    assert!(!v["big_files"].as_array().unwrap().is_empty());
    let mut a = v.clone();
    a["big_files"][0]["node"] = Value::from(999_999u64);
    let e = load(&a).expect_err("big_files");
    assert!(e.contains("big_files[0].node"), "{e}");

    if !v["outputs"].as_array().unwrap().is_empty() {
        let mut b = v.clone();
        b["outputs"][0]["node"] = Value::from(999_999u64);
        let e = load(&b).expect_err("outputs");
        assert!(e.contains("outputs[0].node"), "{e}");
    }
}

#[test]
fn l1_byte_counts_that_overflow_when_summed_are_a_clean_error() {
    let (_t, _res, mut v) = valid_scan();
    let n = v["nodes"].as_array().unwrap().len();
    assert!(n >= 2);
    v["nodes"][0]["bytes_total"] = Value::from(u64::MAX);
    v["nodes"][1]["bytes_total"] = Value::from(5u64);
    let e = load(&v).expect_err("must be rejected");
    assert!(e.contains("overflow"), "{e}");
}

#[test]
fn l1_negative_counts_are_rejected_by_the_parser() {
    let (_t, _res, mut v) = valid_scan();
    v["nodes"][0]["bytes_self"] = Value::from(-1);
    assert!(load(&v).is_err());
}

// ---------------------------------------------------------------- L2: odd .git targets

#[cfg(unix)]
mod odd_git {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn l2_a_git_directory_reached_through_two_git_symlinks_has_dependents() {
        let (_tmp, root, remotes) = workspace();
        let store = pushed_repo(&root, &remotes, "store");
        let v1 = root.join("v1");
        write(&v1.join("work.txt"), "local edits\n");
        symlink(store.join(".git"), v1.join(".git")).unwrap();
        let proj = root.join("proj");
        write(&proj.join("package.json"), "{}\n");
        let pkg = proj.join("node_modules").join("pkg");
        write(&pkg.join("index.js"), "x\n");
        symlink(store.join(".git"), pkg.join(".git")).unwrap();

        let res = scan_with(&root, network_everywhere());
        let f = facts_of(&res, &store);
        assert_eq!(f.no_dependents, disk_cleaner_core::Tri::No, "{f:?}");
        let a = action_for(&res, &store);
        assert!(
            !matches!(a, Action::BackupThenRemove | Action::RemoveRegenerable),
            "{a:?}"
        );
        let notes = node(&res, &store).repo.as_ref().unwrap().notes.join(" ");
        assert!(notes.contains("v1"), "the dependent is named: {notes}");
    }

    #[test]
    fn l2_a_git_symlink_pointing_outside_the_scan_makes_every_repo_ask() {
        let (tmp, root, remotes) = workspace();
        let other = pushed_repo(&root, &remotes, "other");
        let ext = tmp.path().join("ext");
        init_repo(&ext);
        let proj = root.join("proj");
        write(&proj.join("package.json"), "{}\n");
        symlink(ext.join(".git"), proj.join(".git")).unwrap();

        let res = scan_with(&root, network_everywhere());
        assert_eq!(action_for(&res, &other), Action::Ask);
        let f = facts_of(&res, &other);
        assert_eq!(f.no_dependents, disk_cleaner_core::Tri::Unknown, "{f:?}");
    }

    #[test]
    fn l2_a_dangling_git_symlink_makes_every_repo_ask() {
        let (tmp, root, remotes) = workspace();
        let other = pushed_repo(&root, &remotes, "other");
        let proj = root.join("proj");
        write(&proj.join("package.json"), "{}\n");
        symlink(tmp.path().join("nowhere").join(".git"), proj.join(".git")).unwrap();
        let res = scan_with(&root, network_everywhere());
        assert_eq!(action_for(&res, &other), Action::Ask);
    }
}

#[test]
fn l2_control_without_odd_git_entries_a_pushed_repo_is_still_archivable() {
    let (_tmp, root, remotes) = workspace();
    let other = pushed_repo(&root, &remotes, "other");
    write(&root.join("proj").join("package.json"), "{}\n");
    let res = scan_with(&root, network_everywhere());
    assert_eq!(action_for(&res, &other), Action::BackupThenRemove);
}

// ---------------------------------------------------------------- D3: wording

#[test]
fn d3_loose_code_in_a_subtree_of_a_repo_is_not_described_as_outside_any_repo() {
    let (_tmp, root, _remotes) = workspace();
    let repo = root.join("repo");
    init_repo(&repo);
    let app = repo.join("packages").join("app");
    write(&app.join("package.json"), "{}\n");
    let res = scan_with(&app, network_everywhere());
    let p = plan(&res, &PlanOptions::new(7, far_future()));
    let item = p
        .items
        .iter()
        .find(|i| i.path == app.to_string_lossy())
        .expect("app item");
    assert!(
        !item.reason.contains("outside any repo"),
        "reason: {}",
        item.reason
    );
    assert!(item.reason.contains("repository"), "{}", item.reason);
}
