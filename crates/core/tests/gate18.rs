//! Regression tests for gate round 15 at 087a1b3 (PR #2).
//!
//! F2: an output file (video, archive, model, dataset...) inside an artifact directory made the
//! artifact ask only when its rule searched for outputs (`scan_outputs`), so an idle project's
//! `.venv/trained.safetensors` above the output threshold was listed as an output while the
//! whole `.venv` was offered `remove-regenerable`. Now any listed output makes the artifact that
//! holds it ask and name it, whatever the rule, and the enclosing repository is not removed
//! either. An output file of any size directly in an artifact's top directory is listed too.
//!
//! F1, F3: `scan --verify-objects` is removed. This format defines no verification of object
//! data, so a scan file that claims one (`verified`, `checked`) is refused as invalid.
//!
//! Every fixture is synthetic; the repository fixtures use real git.

mod common;

use common::*;
use disk_cleaner_core::{
    plan, scan, scan_from_json, Action, Error, FactsConfig, Plan, PlanItem, PlanOptions,
    ScanOptions, ScanResult, Tri,
};
use std::path::{Path, PathBuf};

/// The output threshold in these scans (`scan --min-output-bytes`); the default is 100 MiB.
const THRESHOLD: u64 = 64 * 1024;
/// An output file comfortably above [`THRESHOLD`] on any file system.
const BIG: usize = 256 * 1024;

fn scan_at(root: &Path, facts: FactsConfig) -> ScanResult {
    let mut opts = ScanOptions::new(vec![root.to_path_buf()]);
    opts.facts = facts;
    opts.git_jobs = 2;
    opts.min_output_bytes = THRESHOLD;
    scan(&opts).expect("scan")
}

/// A clone, inside the scan root, of a repository pushed to a bare remote outside it (a fresh
/// clone holds no setting of its own, so nothing but the case under test makes it ask).
fn clone_of(tmp: &Path, root: &Path, remotes: &Path, name: &str) -> PathBuf {
    pushed_repo(&tmp.join("src"), remotes, name);
    let clone = root.join(name);
    let url = remotes.join(format!("{name}.git"));
    git(
        root,
        &[
            "clone",
            "-q",
            url.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    clone
}

fn plan_of(res: &ScanResult) -> Plan {
    plan(res, &PlanOptions::new(7, far_future()))
}

fn item<'a>(p: &'a Plan, path: &Path) -> &'a PlanItem {
    p.items
        .iter()
        .find(|i| i.path == path.to_string_lossy())
        .unwrap_or_else(|| panic!("no plan item for {}", path.display()))
}

/// A project directory holding `marker` (which makes the artifact rule match).
fn project(root: &Path, name: &str, marker: &str) -> PathBuf {
    let p = root.join(name);
    write(&p.join(marker), "x\n");
    p
}

/// Ordinary installed content of a `node_modules`.
fn stock_node_modules(nm: &Path) {
    write(&nm.join("pkg").join("package.json"), "{}\n");
    write(&nm.join("pkg").join("index.js"), "module.exports = 1;\n");
    write(&nm.join(".package-lock.json"), "{}\n");
}

/// Ordinary installed content of a virtual environment.
fn stock_venv(venv: &Path) {
    write(&venv.join("pyvenv.cfg"), "home = /usr/bin\n");
    let pkg = venv
        .join("lib")
        .join("python3.12")
        .join("site-packages")
        .join("pkg");
    write(&pkg.join("__init__.py"), "x = 1\n");
}

/// `artifact` asks, reclaims nothing, says why, and names `file`; the scan lists `file` as an
/// output of that artifact.
fn assert_asks_naming(res: &ScanResult, artifact: &Path, file: &Path) {
    let n = node(res, artifact);
    assert!(n.contains_outputs, "{} holds an output", artifact.display());
    let idx = node_index(res, artifact);
    let want = file.to_string_lossy();
    assert!(
        res.outputs
            .iter()
            .any(|o| o.path == want && o.node == Some(idx)),
        "{want} is not listed as an output of {}: {:?}",
        artifact.display(),
        res.outputs
    );
    let p = plan_of(res);
    let it = item(&p, artifact);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    assert!(it.reason.contains("not regenerable"), "{}", it.reason);
    let name = file.file_name().unwrap().to_string_lossy();
    assert!(
        it.reason.contains(name.as_ref()),
        "{name} not named in: {}",
        it.reason
    );
}

// ---------------------------------------------------------------- F2: every artifact class

#[test]
fn f2_an_output_above_the_threshold_makes_any_artifact_ask_and_names_it() {
    let (_tmp, root, _) = workspace();
    // Rules without `scan_outputs` (node_modules, .venv, .tox) and one with it (target).
    let js = project(&root, "js", "package.json");
    let nm = js.join("node_modules");
    stock_node_modules(&nm);
    let nm_out = nm.join("trained.safetensors");
    write_bytes(&nm_out, BIG);

    let py = project(&root, "py", "requirements.txt");
    let venv = py.join(".venv");
    stock_venv(&venv);
    let venv_out = venv.join("trained.safetensors");
    write_bytes(&venv_out, BIG);

    let tx = project(&root, "tx", "tox.ini");
    let tox = tx.join(".tox");
    write(&tox.join("py312").join("pyvenv.cfg"), "home = /usr/bin\n");
    let tox_out = tox.join("py312").join("results.parquet");
    write_bytes(&tox_out, BIG);

    let rs = project(&root, "rs", "Cargo.toml");
    let target = rs.join("target");
    write(
        &target.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    );
    let target_out = target.join("release").join("model.onnx");
    write_bytes(&target_out, BIG);

    // Inside an installed package (its own package.json): no exemption in this format.
    let pk = project(&root, "pk", "package.json");
    let pk_nm = pk.join("node_modules");
    stock_node_modules(&pk_nm);
    let pk_out = pk_nm.join("pkg").join("weights.bin");
    write_bytes(&pk_out, BIG);

    let res = scan_at(&root, FactsConfig::default());
    for (artifact, file) in [
        (&nm, &nm_out),
        (&venv, &venv_out),
        (&tox, &tox_out),
        (&target, &target_out),
        (&pk_nm, &pk_out),
    ] {
        assert_asks_naming(&res, artifact, file);
    }
}

#[test]
fn f2_an_output_of_any_size_directly_in_an_artifact_makes_it_ask() {
    let (_tmp, root, _) = workspace();
    let js = project(&root, "js", "package.json");
    let nm = js.join("node_modules");
    stock_node_modules(&nm);
    let nm_out = nm.join("notes.zip");
    write_bytes(&nm_out, 1024);

    let py = project(&root, "py", "pyproject.toml");
    let venv = py.join(".venv");
    stock_venv(&venv);
    let venv_out = venv.join("export.csv");
    write(&venv_out, "a,b\n1,2\n");

    let res = scan_at(&root, FactsConfig::default());
    assert_asks_naming(&res, &nm, &nm_out);
    assert_asks_naming(&res, &venv, &venv_out);
}

#[test]
fn f2_controls_stock_node_modules_and_venv_stay_regenerable() {
    let (_tmp, root, _) = workspace();
    let js = project(&root, "js", "package.json");
    let nm = js.join("node_modules");
    stock_node_modules(&nm);
    // A small fixture inside an installed package is not listed (below the threshold, not at
    // the top of the artifact), as before.
    write_bytes(&nm.join("pkg").join("test").join("fixture.zip"), 1024);

    let py = project(&root, "py", "requirements.txt");
    let venv = py.join(".venv");
    stock_venv(&venv);
    write_bytes(
        &venv
            .join("lib")
            .join("python3.12")
            .join("site-packages")
            .join("pkg")
            .join("icon.png"),
        512,
    );

    let res = scan_at(&root, FactsConfig::default());
    let p = plan_of(&res);
    for artifact in [&nm, &venv] {
        assert!(!node(&res, artifact).contains_outputs);
        let it = item(&p, artifact);
        assert_eq!(it.action, Action::RemoveRegenerable, "{}", it.reason);
        assert!(it.reclaimable_bytes > 0);
    }
    assert!(res.outputs.is_empty(), "{:?}", res.outputs);
}

// ---------------------------------------------------------------- F2: the enclosing repository

/// A clean, pushed clone whose ignored `.venv` holds a model: the `.venv` asks and names it, and
/// the repository is not removed: the nested ask vetoes it, and on its own the model counts as
/// an ignored file that exists nowhere else.
#[test]
fn f2_an_output_in_an_ignored_artifact_keeps_a_clean_clone_from_being_removed() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    commit_file(&repo, ".gitignore", ".venv/\n");
    commit_file(&repo, "requirements.txt", "numpy\n");
    git(&repo, &["push", "-q", "origin", "main"]);
    let venv = repo.join(".venv");
    stock_venv(&venv);
    let model = venv.join("trained.safetensors");
    write_bytes(&model, BIG);

    let res = scan_at(&root, network_everywhere());
    assert_asks_naming(&res, &venv, &model);
    let f = facts_of(&res, &repo);
    assert_eq!(f.tree_clean, Tri::Yes);
    assert_eq!(f.head_on_remote, Tri::Yes);
    assert_eq!(f.no_unique_ignored, Tri::No);

    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    assert!(it.preserve.is_none());
    let veto = format!(
        "the nested {} needs a person's decision, and everything inside goes with this one",
        venv.display()
    );
    assert!(it.reason.starts_with(&veto), "{}", it.reason);
    assert!(it.reason.contains("[no_unique_ignored]"), "{}", it.reason);
    assert!(!p.items.iter().any(|i| i.action.removes()), "{:?}", p.items);
}

/// Control: the same clone with a stock `.venv` (no output) is backed up then removed.
#[test]
fn f2_control_the_same_clone_with_a_stock_venv_is_removed_after_a_backup() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    commit_file(&repo, ".gitignore", ".venv/\n");
    commit_file(&repo, "requirements.txt", "numpy\n");
    git(&repo, &["push", "-q", "origin", "main"]);
    let venv = repo.join(".venv");
    stock_venv(&venv);

    let res = scan_at(&root, network_everywhere());
    assert_eq!(facts_of(&res, &repo).no_unique_ignored, Tri::Yes);
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::BackupThenRemove, "{}", it.reason);
    assert_eq!(item(&p, &venv).action, Action::RemoveRegenerable);
}

// ---------------------------------------------------------------- forged verification

fn clone_scan_json() -> (tempfile::TempDir, serde_json::Value, usize) {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let res = scan_at(&root, network_everywhere());
    let i = node_index(&res, &repo);
    let v = serde_json::to_value(&res).unwrap();
    assert!(v["nodes"][i]["repo"]["object_store"].is_object(), "{v}");
    (tmp, v, i)
}

#[test]
fn forged_a_scan_file_that_claims_a_verification_is_refused() {
    let (_tmp, v, i) = clone_scan_json();
    // The unmodified scan reads back, and carries no verification field.
    scan_from_json(&v.to_string()).expect("an unmodified scan reads back");
    let store = &v["nodes"][i]["repo"]["object_store"];
    for key in ["verified", "checked"] {
        assert!(store.get(key).is_none(), "{key} in {store}");
    }

    let cases: [(&str, &str, serde_json::Value); 6] = [
        ("object_store", "verified", serde_json::json!(true)),
        ("object_store", "verified", serde_json::json!(false)),
        ("object_store", "checked", serde_json::json!(true)),
        ("object_store", "verification", serde_json::json!("fsck")),
        ("git_admin", "verified", serde_json::json!(true)),
        ("", "verified", serde_json::json!(true)),
    ];
    for (block, key, val) in cases {
        let mut m = v.clone();
        let target = if block.is_empty() {
            &mut m["nodes"][i]["repo"]
        } else {
            &mut m["nodes"][i]["repo"][block]
        };
        target[key] = val.clone();
        match scan_from_json(&m.to_string()) {
            Err(Error::InvalidScan { field, message }) => {
                assert!(
                    field.contains(key),
                    "{block}.{key}={val}: {field}: {message}"
                );
            }
            other => panic!("{block}.{key}={val} accepted or wrong error: {other:?}"),
        }
    }

    // The same claim on the node or on the document itself.
    let mut on_node = v.clone();
    on_node["nodes"][i]["verified"] = serde_json::json!(true);
    let mut on_doc = v.clone();
    on_doc["object_store_verified"] = serde_json::json!(true);
    for m in [on_node, on_doc] {
        match scan_from_json(&m.to_string()) {
            Err(Error::InvalidScan { field, .. }) => {
                assert!(field.contains("verified"), "{field}");
            }
            other => panic!("accepted or wrong error: {other:?}"),
        }
    }
}

/// The refused names are matched whatever their case, at every level the check looks at.
#[test]
fn forged_a_verification_claim_in_any_case_is_refused() {
    let (_tmp, v, i) = clone_scan_json();
    let cases: [(&str, &str); 6] = [
        ("doc", "Verified"),
        ("doc", "VERIFY"),
        ("node", "Verified"),
        ("repo", "Checked"),
        ("object_store", "VerifiedAt"),
        ("git_admin", "VERIFICATION"),
    ];
    for (level, key) in cases {
        let mut m = v.clone();
        let target = match level {
            "doc" => &mut m,
            "node" => &mut m["nodes"][i],
            "repo" => &mut m["nodes"][i]["repo"],
            block => &mut m["nodes"][i]["repo"][block],
        };
        target[key] = serde_json::json!(true);
        match scan_from_json(&m.to_string()) {
            Err(Error::InvalidScan { field, .. }) => {
                assert!(field.contains(key), "{level}.{key}: {field}");
            }
            other => panic!("{level}.{key} accepted or wrong error: {other:?}"),
        }
    }
}

/// The plan JSON states no verification of object data.
#[test]
fn forged_the_plan_carries_no_verification_field() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let res = scan_at(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::BackupThenRemove, "{}", it.reason);
    let pre = serde_json::to_value(it.preserve.as_ref().unwrap()).unwrap();
    assert!(
        pre["object_store"]["files"].as_u64().unwrap_or(0) > 0,
        "{pre}"
    );
    assert!(pre["object_store"].get("verified").is_none(), "{pre}");
    assert!(
        it.reason
            .contains("holds object data that must be backed up too before removal"),
        "{}",
        it.reason
    );
    for gone in [
        "verified by this scan",
        "not yet verified",
        "verified against the remote",
    ] {
        assert!(!it.reason.contains(gone), "{gone:?} in {}", it.reason);
    }
}
