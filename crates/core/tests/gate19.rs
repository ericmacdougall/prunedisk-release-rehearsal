//! Regression tests for gate round 15b at 3a6384d (PR #2).
//!
//! An output file (video, archive, model, dataset...) below the output threshold, nested below
//! the top of an artifact whose rule does not search for outputs, was not listed: a 50 MiB
//! `.venv/data/model.pt` at the default 100 MiB threshold left the whole `.venv` offered
//! `remove-regenerable`. Now an output inside a regenerable or cache directory is listed at any
//! size, and makes that directory ask, unless it sits in an installed package's directory (the
//! same package evidence the `fixture_prone` exemption uses: a `package.json` above it in
//! `node_modules`, a package directory below `site-packages`). Inside a package directory the
//! threshold still applies.
//!
//! The thresholds here are small (`--min-output-bytes`), so "below the threshold" needs no big
//! file. Every fixture is synthetic except the last control, which runs the real `venv` module.

mod common;

use common::*;
use disk_cleaner_core::{
    plan, scan, scan_from_json, Action, FactsConfig, Plan, PlanItem, PlanOptions, ScanOptions,
    ScanResult,
};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The output threshold in most of these scans; the default is 100 MiB.
const THRESHOLD: u64 = 64 * 1024;
/// A small output file.
const SMALL: usize = 1024;
/// An output file below [`THRESHOLD`] on any file system, as 50 MiB is below 100 MiB.
const NEAR: usize = 48 * 1024;

fn scan_with_threshold(root: &Path, threshold: u64) -> ScanResult {
    let mut opts = ScanOptions::new(vec![root.to_path_buf()]);
    opts.facts = FactsConfig::default();
    opts.git_jobs = 2;
    opts.min_output_bytes = threshold;
    scan(&opts).expect("scan")
}

fn scan_at(root: &Path) -> ScanResult {
    scan_with_threshold(root, THRESHOLD)
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

/// The package directory `pkg` below a virtual environment's `site-packages`.
fn site_pkg(venv: &Path) -> PathBuf {
    venv.join("lib")
        .join("python3.12")
        .join("site-packages")
        .join("pkg")
}

/// Ordinary installed content of a virtual environment.
fn stock_venv(venv: &Path) {
    write(&venv.join("pyvenv.cfg"), "home = /usr/bin\n");
    write(&site_pkg(venv).join("__init__.py"), "x = 1\n");
}

/// `artifact` asks, reclaims nothing, says why, and names `file`; the scan lists `file` as an
/// output of that artifact.
fn assert_asks_naming(res: &ScanResult, artifact: &Path, file: &Path) {
    let n = node(res, artifact);
    assert!(
        n.contains_outputs,
        "{} holds an output: {:?}",
        artifact.display(),
        res.outputs
    );
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

/// `artifact` holds no listed output and is offered `remove-regenerable`.
fn assert_regenerable(res: &ScanResult, artifact: &Path) {
    let p = plan_of(res);
    let it = item(&p, artifact);
    assert!(
        !node(res, artifact).contains_outputs,
        "{}: {:?}",
        artifact.display(),
        res.outputs
    );
    assert_eq!(
        it.action,
        Action::RemoveRegenerable,
        "{}: {}",
        artifact.display(),
        it.reason
    );
    assert!(it.reclaimable_bytes > 0);
}

// ---------------------------------------------------------------- outside a package directory

#[test]
fn r15b_a_small_output_outside_any_package_makes_the_artifact_ask_and_names_it() {
    let (_tmp, root, _) = workspace();
    let mut cases: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (i, size) in [SMALL, NEAR].into_iter().enumerate() {
        let py = project(&root, &format!("py{i}"), "requirements.txt");
        let venv = py.join(".venv");
        stock_venv(&venv);
        let model = venv.join("data").join("model.pt");
        write_bytes(&model, size);
        cases.push((venv, model));

        let js = project(&root, &format!("js{i}"), "package.json");
        let nm = js.join("node_modules");
        stock_node_modules(&nm);
        // `.stuff` holds no package.json: not an installed package.
        let clip = nm.join(".stuff").join("clip.mp4");
        write_bytes(&clip, size);
        cases.push((nm, clip));
    }
    let res = scan_at(&root);
    for (artifact, file) in &cases {
        assert_asks_naming(&res, artifact, file);
    }
}

/// A build-output directory (`scan_outputs`) already listed its outputs at any size; this pins
/// it (a guard, not a failing-first case).
#[test]
fn r15b_guard_a_small_output_in_a_build_directory_asks() {
    let (_tmp, root, _) = workspace();
    let mut cases: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (i, size) in [SMALL, NEAR].into_iter().enumerate() {
        let rs = project(&root, &format!("rs{i}"), "Cargo.toml");
        let target = rs.join("target");
        write(
            &target.join("CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        );
        let data = target.join("notes").join("data.parquet");
        write_bytes(&data, size);
        cases.push((target, data));
    }
    let res = scan_at(&root);
    for (artifact, file) in &cases {
        assert_asks_naming(&res, artifact, file);
    }
}

/// More small outputs in one directory than are listed one by one: the directory still asks,
/// and its reason counts every one of them.
#[test]
fn r15b_many_small_outputs_in_one_directory_are_all_counted() {
    let (_tmp, root, _) = workspace();
    let py = project(&root, "py", "requirements.txt");
    let venv = py.join(".venv");
    stock_venv(&venv);
    for k in 0..20 {
        write_bytes(&venv.join("frames").join(format!("f{k:02}.png")), SMALL);
    }
    let res = scan_at(&root);
    assert!(node(&res, &venv).contains_outputs, "{:?}", res.outputs);
    let idx = node_index(&res, &venv);
    let listed = res.outputs.iter().filter(|o| o.node == Some(idx)).count();
    assert!(listed > 0 && listed <= 20, "{listed} listed");
    let p = plan_of(&res);
    let it = item(&p, &venv);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert!(it.reason.contains("20 output files"), "{}", it.reason);
    assert!(it.reason.contains("frames/f"), "{}", it.reason);
}

/// `--min-output-bytes` narrows only what is inside an installed package (and what lies outside
/// every artifact); outside a package directory an output asks whatever the threshold.
#[test]
fn r15b_the_threshold_applies_only_inside_a_package_directory() {
    let (_tmp, root, _) = workspace();
    let py = project(&root, "py", "requirements.txt");
    let venv = py.join(".venv");
    stock_venv(&venv);
    let loose = venv.join("data").join("model.pt");
    write_bytes(&loose, SMALL);

    let js = project(&root, "js", "package.json");
    let nm = js.join("node_modules");
    stock_node_modules(&nm);
    let in_pkg = nm.join("pkg").join("weights.bin");
    write_bytes(&in_pkg, NEAR);

    // A threshold far above every file: the loose model still asks; the package file does not.
    let res = scan_with_threshold(&root, 1 << 40);
    assert_asks_naming(&res, &venv, &loose);
    assert_regenerable(&res, &nm);

    // A threshold below the package file: now it asks too.
    let res = scan_with_threshold(&root, 32 * 1024);
    assert_asks_naming(&res, &venv, &loose);
    assert_asks_naming(&res, &nm, &in_pkg);
}

/// A small output whose path has a build-directory name in it (`bin`, `build`, `deps`, or a
/// scan root below a directory named `build`) was kept by the walk on a different path that
/// listed it only inside a build directory or at an artifact's top: it went unlisted.
#[test]
fn r15b_a_small_output_below_a_build_named_directory_inside_an_artifact_asks() {
    let (_tmp, root, _) = workspace();
    let mut cases: Vec<(PathBuf, PathBuf)> = Vec::new();

    let py = project(&root, "py", "requirements.txt");
    let venv = py.join(".venv");
    stock_venv(&venv);
    let in_bin = venv.join("bin").join("model.pt");
    write_bytes(&in_bin, SMALL);
    cases.push((venv.clone(), in_bin));

    let py2 = project(&root, "py2", "requirements.txt");
    let venv2 = py2.join(".venv");
    stock_venv(&venv2);
    let in_deps = venv2.join("data").join("deps").join("model.pt");
    write_bytes(&in_deps, NEAR);
    cases.push((venv2, in_deps));

    let js = project(&root, "js", "package.json");
    let nm = js.join("node_modules");
    stock_node_modules(&nm);
    let in_build = nm.join(".stuff").join("build").join("clip.mp4");
    write_bytes(&in_build, SMALL);
    cases.push((nm, in_build));

    // Everything below a plain directory named `build` (not itself an artifact: nothing beside
    // it marks a project).
    let under = root.join("build");
    let py3 = project(&under, "py3", "requirements.txt");
    let venv3 = py3.join(".venv");
    stock_venv(&venv3);
    let loose = venv3.join("data").join("model.pt");
    write_bytes(&loose, SMALL);
    cases.push((venv3, loose));

    let res = scan_at(&root);
    for (artifact, file) in &cases {
        assert_asks_naming(&res, artifact, file);
    }
}

/// A `.pth` file straight in `site-packages` is a Python path file, but `.pth` is also a
/// model-weight extension and nothing there is package evidence: the environment asks. This pins
/// the documented over-ask, so a change to it is deliberate (a guard, not a failing-first case).
#[test]
fn r15b_known_over_ask_a_pth_file_straight_in_site_packages() {
    let (_tmp, root, _) = workspace();
    let py = project(&root, "py", "requirements.txt");
    let venv = py.join(".venv");
    stock_venv(&venv);
    let pth = venv
        .join("lib")
        .join("python3.12")
        .join("site-packages")
        .join("_virtualenv.pth");
    write(&pth, "import _virtualenv\n");
    let res = scan_at(&root);
    assert_asks_naming(&res, &venv, &pth);
}

/// A version 14 scan file may leave a small nested output in an artifact unlisted, so it is
/// refused; the current version is 15.
#[test]
fn r15b_a_version_14_scan_file_is_refused() {
    let (_tmp, root, _) = workspace();
    let py = project(&root, "py", "requirements.txt");
    stock_venv(&py.join(".venv"));
    let res = scan_at(&root);
    assert_eq!(res.schema_version, 15);
    let mut v = serde_json::to_value(&res).unwrap();
    assert!(scan_from_json(&v.to_string()).is_ok());
    v["schema_version"] = serde_json::json!(14);
    assert!(scan_from_json(&v.to_string()).is_err());
}

// ---------------------------------------------------------------- controls

/// The same small files inside an installed package's directory are not listed, and stock
/// dependency directories stay regenerable.
#[test]
fn r15b_control_small_outputs_inside_a_package_directory_do_not_ask() {
    let (_tmp, root, _) = workspace();
    let mut artifacts: Vec<PathBuf> = Vec::new();
    for (i, size) in [SMALL, NEAR].into_iter().enumerate() {
        let js = project(&root, &format!("js{i}"), "package.json");
        let nm = js.join("node_modules");
        stock_node_modules(&nm);
        write_bytes(&nm.join("pkg").join("model.pt"), size);
        write_bytes(&nm.join("pkg").join("media").join("clip.mp4"), size);
        // Packages ship built files under names that build directories use.
        write_bytes(&nm.join("pkg").join("dist").join("logo.png"), size);
        write_bytes(&nm.join("pkg").join("build").join("addon.bin"), size);
        artifacts.push(nm);

        let py = project(&root, &format!("py{i}"), "requirements.txt");
        let venv = py.join(".venv");
        stock_venv(&venv);
        write_bytes(&site_pkg(&venv).join("model.pt"), size);
        write_bytes(&site_pkg(&venv).join("data").join("data.parquet"), size);
        write_bytes(&site_pkg(&venv).join("bin").join("tool.bin"), size);
        artifacts.push(venv);
    }
    let js = project(&root, "stock-js", "package.json");
    stock_node_modules(&js.join("node_modules"));
    artifacts.push(js.join("node_modules"));
    let py = project(&root, "stock-py", "requirements.txt");
    stock_venv(&py.join(".venv"));
    artifacts.push(py.join(".venv"));

    let res = scan_at(&root);
    for artifact in &artifacts {
        assert_regenerable(&res, artifact);
    }
    assert!(res.outputs.is_empty(), "{:?}", res.outputs);
}

/// A virtual environment made by the real `venv` module (no network) stays regenerable. Python
/// versions that still seed setuptools leave `distutils-precedence.pth` straight in
/// `site-packages`, where no package evidence applies, and `.pth` is also a model-weight
/// extension: that environment asks and names the file. That is a known over-ask, accepted only
/// for `.pth` files straight in `site-packages`; any other trigger fails this test.
#[test]
fn r15b_control_a_real_venv_stays_regenerable() {
    let (_tmp, root, _) = workspace();
    let py = project(&root, "py", "requirements.txt");
    let venv = py.join(".venv");
    let made = ["python3", "python"].into_iter().find(|exe| {
        Command::new(exe)
            .args(["-m", "venv"])
            .arg(&venv)
            .output()
            .is_ok_and(|o| o.status.success())
    });
    // Written straight to stderr, which the test harness does not capture, so the CI log shows
    // which branch ran on each system.
    let say = |line: String| {
        let _ = writeln!(std::io::stderr(), "{line}");
    };
    let Some(exe) = made else {
        // Every CI runner has Python: a skip there would read as a pass.
        assert!(
            std::env::var_os("CI").is_none(),
            "no python3 or python could make a venv on CI"
        );
        say("SKIPPED r15b_control_a_real_venv_stays_regenerable: no python3 or python".into());
        return;
    };
    let version = Command::new(exe)
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    // The default threshold, as a person would scan.
    let res = scan_with_threshold(&root, ScanOptions::new(vec![]).min_output_bytes);
    let idx = node_index(&res, &venv);
    let mine: Vec<&str> = res
        .outputs
        .iter()
        .filter(|o| o.node == Some(idx))
        .map(|o| o.path.as_str())
        .collect();
    let p = plan_of(&res);
    let it = item(&p, &venv);
    let over_ask = !mine.is_empty()
        && mine.iter().all(|o| {
            let f = Path::new(o);
            f.extension().is_some_and(|e| e == "pth")
                && f.parent()
                    .and_then(Path::file_name)
                    .is_some_and(|n| n == "site-packages")
        });
    if over_ask {
        say(format!(
            "KNOWN OVER-ASK r15b_control_a_real_venv_stays_regenerable with {exe} ({version}): \
             {mine:?}"
        ));
        assert_eq!(it.action, Action::Ask, "{}", it.reason);
        return;
    }
    say(format!(
        "RAN r15b_control_a_real_venv_stays_regenerable with {exe} ({version}): {:?}",
        it.action
    ));
    assert_eq!(
        it.action,
        Action::RemoveRegenerable,
        "{exe} ({version}): {}; outputs {mine:?}",
        it.reason
    );
    assert!(mine.is_empty(), "{mine:?}");
}
