//! End-to-end tests of the `disk-cleaner` binary on synthetic fixtures.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_disk-cleaner"))
}

fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("run disk-cleaner")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Line endings are pinned: the scanner does not read system config, so a fixture checked out
/// with a platform's default `autocrlf=true` would look modified to it.
fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        // No background maintenance after a commit: it briefly creates and removes lock files
        // and would make the byte-identical .git check flaky.
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "maintenance.auto")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "gc.auto")
        .env("GIT_CONFIG_VALUE_1", "0")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// Names and sizes of everything under `dir`, sorted.
fn listing(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let md = e.metadata().unwrap();
            out.push((e.path(), md.len()));
            if md.is_dir() {
                stack.push(e.path());
            }
        }
    }
    out.sort();
    out
}

fn far_future() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    (now + 3 * 365 * 86_400).to_string()
}

#[test]
fn scan_then_plan_end_to_end_without_leaking_the_remote_token() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("work dir");
    let repo = root.join("app");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    write(&repo.join("package.json"), "{}");
    write(&repo.join("node_modules").join("dep").join("i.js"), "x");
    git(&repo, &["add", "package.json"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://carol:cli-SECRET-tok-77@example.invalid/x.git",
        ],
    );
    let out_file = tmp.path().join("scan.json");

    let o = run(bin().args([
        "scan",
        root.to_str().unwrap(),
        "--out",
        out_file.to_str().unwrap(),
    ]));
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let human = stdout(&o);
    assert!(human.contains("Repos and worktrees"), "{human}");
    assert!(human.contains("Space by class"));

    let json = fs::read_to_string(&out_file).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["schema_version"], 15);

    let o = run(bin()
        .args([
            "plan",
            "--scan",
            out_file.to_str().unwrap(),
            "--active-window-days",
            "30",
        ])
        .args(["--now", &far_future()]));
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let plan_text = stdout(&o);
    assert!(plan_text.contains("Dry-run plan"));

    let o = run(bin()
        .args([
            "plan",
            "--scan",
            out_file.to_str().unwrap(),
            "--active-window-days",
            "30",
            "--json",
        ])
        .args(["--now", &far_future()]));
    let plan_json = stdout(&o);
    let pv: serde_json::Value = serde_json::from_str(&plan_json).unwrap();
    assert_eq!(pv["dry_run"], true);
    // The only remote has no refs containing HEAD, so the repo is never offered for archive.
    let repo_path = repo.to_string_lossy().into_owned();
    let repo_item = pv["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["path"].as_str() == Some(repo_path.as_str()))
        .expect("repo item");
    // node_modules is neither ignored nor tracked here, so it needs a person's decision and
    // the repo that encloses it asks too; it must never be archived.
    let action = repo_item["action"].as_str().unwrap();
    assert!(
        action == "ask" || action == "recover-backup-then-remove",
        "{action}"
    );

    for text in [&human, &json, &plan_text, &plan_json] {
        assert!(!text.contains("cli-SECRET-tok-77"), "token leaked");
        assert!(!text.contains("carol"), "user leaked");
    }
    // The URL itself is kept, credential-free.
    assert!(json.contains("https://example.invalid/x.git"));
}

#[test]
fn scan_json_on_stdout_and_unicode_paths_round_trip_into_the_plan() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("scan");
    let proj = root.join("café ☕").join("проект one");
    write(&proj.join("package.json"), "{}");
    write(&proj.join("node_modules").join("a.js"), "1");
    let before = listing(&root);

    let o = run(bin().args([
        "scan",
        root.to_str().unwrap(),
        "--json",
        "--no-git",
        "--one-file-system",
    ]));
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let scan_json = stdout(&o);
    let v: serde_json::Value = serde_json::from_str(&scan_json).unwrap();
    let paths: Vec<String> = v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["path"].as_str().unwrap().to_string())
        .collect();
    assert!(
        paths.contains(&proj.to_string_lossy().into_owned()),
        "{paths:?}"
    );
    // Read-only: scanning changed nothing.
    assert_eq!(before, listing(&root));

    let scan_file = tmp.path().join("scan.json");
    fs::write(&scan_file, &scan_json).unwrap();
    let o = run(bin()
        .args([
            "plan",
            "--scan",
            scan_file.to_str().unwrap(),
            "--active-window-days",
            "1",
            "--json",
        ])
        .args(["--now", &far_future()]));
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let pv: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    let items: Vec<&str> = pv["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["path"].as_str().unwrap())
        .collect();
    let proj_path = proj.to_string_lossy().into_owned();
    assert!(items.contains(&proj_path.as_str()));
    // Loose code is always ask.
    let loose = pv["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["path"].as_str() == Some(proj_path.as_str()))
        .unwrap();
    assert_eq!(loose["action"], "ask");
}

#[test]
fn active_flag_keeps_everything_under_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("scan");
    let proj = root.join("proj");
    write(&proj.join("package.json"), "{}");
    write(&proj.join("node_modules").join("a.js"), "1");
    let scan_file = tmp.path().join("scan.json");
    let o = run(bin().args([
        "scan",
        root.to_str().unwrap(),
        "--no-git",
        "--out",
        scan_file.to_str().unwrap(),
    ]));
    assert!(o.status.success());

    let plan = |active: Option<&Path>| -> serde_json::Value {
        let mut c = bin();
        c.args([
            "plan",
            "--scan",
            scan_file.to_str().unwrap(),
            "--active-window-days",
            "1",
            "--json",
            "--trust-partial-process-check",
        ])
        .args(["--now", &far_future()]);
        if let Some(a) = active {
            c.args(["--active", a.to_str().unwrap()]);
        }
        let o = run(&mut c);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_str(&stdout(&o)).unwrap()
    };
    let nm = proj.join("node_modules").to_string_lossy().into_owned();
    let action = |v: &serde_json::Value| -> String {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["path"].as_str() == Some(nm.as_str()))
            .unwrap()["action"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(action(&plan(None)), "remove-regenerable");
    assert_eq!(action(&plan(Some(&proj))), "keep");
}

/// Point every platform's config-dir variable at `home` so the user's real config is never read.
fn isolate(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .env("APPDATA", home.join("appdata"));
}

/// Where the tool looks for its config when none is named, under an isolated `home`.
fn default_config_under(home: &Path) -> PathBuf {
    let base = if cfg!(windows) {
        home.join("appdata")
    } else if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else {
        home.join("xdg")
    };
    base.join("disk-cleaner").join("config.toml")
}

/// A scan of a project whose only own file was last touched `days` ago, with a node_modules.
fn scan_with_project_touched(tmp: &Path, days: u64) -> (PathBuf, String) {
    let root = tmp.join("scan");
    let proj = root.join("proj");
    write(&proj.join("package.json"), "{}");
    write(&proj.join("node_modules").join("a.js"), "1");
    let t = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
    fs::OpenOptions::new()
        .write(true)
        .open(proj.join("package.json"))
        .unwrap()
        .set_modified(t)
        .unwrap();
    let scan_file = tmp.join("scan.json");
    let o = run(bin().args([
        "scan",
        root.to_str().unwrap(),
        "--no-git",
        "--out",
        scan_file.to_str().unwrap(),
    ]));
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    (
        scan_file,
        proj.join("node_modules").to_string_lossy().into_owned(),
    )
}

/// `plan --json` as the end-to-end tests want it: a runner may not be able to read every
/// process, and a partial process check otherwise turns every removal into `ask`.
fn plan_json(scan_file: &Path, home: &Path, extra: &[&str]) -> Output {
    let mut args = vec!["--trust-partial-process-check"];
    args.extend_from_slice(extra);
    plan_json_raw(scan_file, home, &args)
}

fn plan_json_raw(scan_file: &Path, home: &Path, extra: &[&str]) -> Output {
    let mut c = bin();
    c.args(["plan", "--scan", scan_file.to_str().unwrap(), "--json"])
        .args(extra);
    isolate(&mut c, home);
    run(&mut c)
}

fn nm_action(o: &Output, nm: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(&stdout(o)).expect("plan json");
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["path"].as_str() == Some(nm))
        .expect("node_modules item")["action"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn neg11_default_window_is_seven_days_and_a_shorter_one_changes_the_answer() {
    let tmp = tempfile::tempdir().unwrap();
    let (scan_file, nm) = scan_with_project_touched(tmp.path(), 6);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let o = plan_json(&scan_file, &home, &[]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["active_window_days"], 7);
    assert_eq!(v["window_source"], "default");
    assert!(v["process_check"].is_string());
    assert_eq!(nm_action(&o, &nm), "keep");

    let o = plan_json(&scan_file, &home, &["--active-window-days", "3"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let a = nm_action(&o, &nm);
    assert_ne!(a, "keep", "6 days is outside a 3-day window");
    assert!(a == "remove-regenerable" || a == "ask", "{a}");

    // The human output states the window and why each project was judged.
    let mut c = bin();
    c.args(["plan", "--scan", scan_file.to_str().unwrap()]);
    isolate(&mut c, &home);
    let text = stdout(&run(&mut c));
    assert!(text.contains("Active window: 7 days"), "{text}");
    assert!(text.contains("within the 7-day active window"), "{text}");
}

#[test]
fn config_file_sets_the_window_and_the_flag_overrides_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (scan_file, nm) = scan_with_project_touched(tmp.path(), 6);
    let home = tmp.path().join("home");
    let cfg = default_config_under(&home);
    write(&cfg, "active_window_days = 3\n");

    // Found at the platform location.
    let o = plan_json(&scan_file, &home, &[]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_ne!(nm_action(&o, &nm), "keep");
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["active_window_days"], 3);

    // The flag wins over the file.
    let o = plan_json(&scan_file, &home, &["--active-window-days", "30"]);
    assert_eq!(nm_action(&o, &nm), "keep");

    // --config names another file.
    let other = tmp.path().join("other.toml");
    write(&other, "active_window_days = 30\n");
    let o = plan_json(&scan_file, &home, &["--config", other.to_str().unwrap()]);
    assert_eq!(nm_action(&o, &nm), "keep");
}

#[test]
fn neg12_malformed_config_is_an_error_never_a_silent_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let (scan_file, _nm) = scan_with_project_touched(tmp.path(), 6);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    for bad in [
        "active_window_days = \"soon\"\n",
        "active_window_days = -2\n",
        "active_window_days = \n",
        "not toml at all\n",
        "active_window_dayz = 3\n",
    ] {
        let cfg = tmp.path().join("bad.toml");
        write(&cfg, bad);
        // Named explicitly, even when the flag would override the value.
        for extra in [
            vec!["--config", cfg.to_str().unwrap()],
            vec![
                "--config",
                cfg.to_str().unwrap(),
                "--active-window-days",
                "30",
            ],
        ] {
            let o = plan_json(&scan_file, &home, &extra);
            assert!(!o.status.success(), "accepted {bad:?}");
            assert!(stdout(&o).trim().is_empty(), "no plan on a bad config");
            let err = String::from_utf8_lossy(&o.stderr).into_owned();
            assert!(err.contains("bad.toml"), "{err}");
        }
        // And at the default location.
        write(&default_config_under(&home), bad);
        let o = plan_json(&scan_file, &home, &[]);
        assert!(
            !o.status.success(),
            "default-location config {bad:?} accepted"
        );
        assert!(String::from_utf8_lossy(&o.stderr).contains("config.toml"));
        fs::remove_file(default_config_under(&home)).unwrap();
    }

    // A named config that does not exist is also an error.
    let o = plan_json(
        &scan_file,
        &home,
        &["--config", tmp.path().join("absent.toml").to_str().unwrap()],
    );
    assert!(!o.status.success());
}

fn idle_fixture() -> (tempfile::TempDir, PathBuf, String, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let (scan_file, nm) = scan_with_project_touched(tmp.path(), 100);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    (tmp, scan_file, nm, home)
}

fn removable_without_active(scan_file: &Path, home: &Path, nm: &str) {
    let o = plan_json(scan_file, home, &[]);
    let a = nm_action(&o, nm);
    assert!(a == "remove-regenerable" || a == "ask", "control: {a}");
}

#[test]
fn f5_active_path_with_dot_dot_still_covers_the_project() {
    let (tmp, scan_file, nm, home) = idle_fixture();
    removable_without_active(&scan_file, &home, &nm);
    // `other/..` is a detour that a lexical prefix comparison cannot see through.
    fs::create_dir_all(tmp.path().join("scan").join("other")).unwrap();
    let spelled = tmp
        .path()
        .join("scan")
        .join("other")
        .join("..")
        .join("proj");
    let o = plan_json(&scan_file, &home, &["--active", spelled.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(nm_action(&o, &nm), "keep");
}

#[cfg(unix)]
#[test]
fn f5_active_path_through_a_symlink_still_covers_the_project() {
    let (tmp, scan_file, nm, home) = idle_fixture();
    let alias = tmp.path().join("alias");
    std::os::unix::fs::symlink(tmp.path().join("scan"), &alias).unwrap();
    let spelled = alias.join("proj");
    let o = plan_json(&scan_file, &home, &["--active", spelled.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(nm_action(&o, &nm), "keep");
}

#[test]
fn f5_active_path_that_does_not_exist_is_an_error() {
    let (tmp, scan_file, _nm, home) = idle_fixture();
    let nope = tmp.path().join("scan").join("prooj");
    let o = plan_json(&scan_file, &home, &["--active", nope.to_str().unwrap()]);
    assert!(
        !o.status.success(),
        "a mistyped --active must not be accepted"
    );
    assert!(String::from_utf8_lossy(&o.stderr).contains("prooj"));
    assert!(stdout(&o).trim().is_empty());
}

#[test]
fn f5_active_path_that_matches_nothing_scanned_is_an_error() {
    let (tmp, scan_file, _nm, home) = idle_fixture();
    let elsewhere = tmp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let o = plan_json(
        &scan_file,
        &home,
        &["--active", elsewhere.to_str().unwrap()],
    );
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("elsewhere"));
}

#[cfg(unix)]
#[test]
fn f16_control_characters_in_paths_are_escaped_in_human_output() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("scan");
    let proj = root.join("evil\u{1b}[31m\n  archive  FORGED-LINE");
    write(&proj.join("package.json"), "{}");
    write(&proj.join("node_modules").join("a.js"), "1");
    let scan_file = tmp.path().join("scan.json");
    let o = run(bin().args([
        "scan",
        root.to_str().unwrap(),
        "--no-git",
        "--out",
        scan_file.to_str().unwrap(),
    ]));
    let human_scan = stdout(&o);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let mut c = bin();
    c.args(["plan", "--scan", scan_file.to_str().unwrap()]);
    isolate(&mut c, &home);
    let human_plan = stdout(&run(&mut c));
    for text in [&human_scan, &human_plan] {
        assert!(!text.contains('\u{1b}'), "raw escape sequence printed");
        assert!(
            !text.contains("\n  archive  FORGED-LINE"),
            "a path forged a line"
        );
    }
    assert!(
        human_plan.contains("FORGED-LINE"),
        "the path is still shown, escaped"
    );
}

fn snapshot_git_state(repo: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![repo.join(".git")];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == "maintenance.lock") {
                // Only git's background-maintenance lock is transient; a leaked index.lock
                // or any other lock must still fail this test.
            } else {
                out.push((p.clone(), fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn scan_with_git_enabled_leaves_the_index_refs_and_objects_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("scan");
    let repo = root.join("app");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    write(&repo.join("a.txt"), "one\n");
    write(&repo.join("b.txt"), "two\n");
    git(&repo, &["add", "a.txt", "b.txt"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    // Make the index stale (same content, new mtime) so a status that may refresh it would.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
    fs::OpenOptions::new()
        .write(true)
        .open(repo.join("a.txt"))
        .unwrap()
        .set_modified(later)
        .unwrap();
    let before = snapshot_git_state(&repo);
    let working_before = listing(&root);

    let scan_file = tmp.path().join("scan.json");
    let o = run(bin().args([
        "scan",
        root.to_str().unwrap(),
        "--out",
        scan_file.to_str().unwrap(),
    ]));
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&scan_file).unwrap()).unwrap();
    assert_eq!(v["options"]["git_facts"], true);
    let o = run(bin().args([
        "plan",
        "--scan",
        scan_file.to_str().unwrap(),
        "--active-window-days",
        "1",
    ]));
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

    let after = snapshot_git_state(&repo);
    let mut diffs: Vec<String> = Vec::new();
    for (p, bytes) in &before {
        match after.iter().find(|(q, _)| q == p) {
            None => diffs.push(format!("removed {}", p.display())),
            Some((_, b2)) if b2 != bytes => diffs.push(format!(
                "changed {} ({} -> {} bytes)",
                p.display(),
                bytes.len(),
                b2.len()
            )),
            _ => {}
        }
    }
    for (p, _) in &after {
        if !before.iter().any(|(q, _)| q == p) {
            diffs.push(format!("created {}", p.display()));
        }
    }
    assert!(diffs.is_empty(), ".git changed during scan: {diffs:?}");
    // Only the scan file was created, and it is outside the scanned tree.
    assert_eq!(working_before.len(), listing(&root).len());
}

#[test]
fn bad_scan_file_and_missing_root_fail_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("scan.json");
    fs::write(&f, "{\"schema_version\": 99}").unwrap();
    let o = run(bin().args([
        "plan",
        "--scan",
        f.to_str().unwrap(),
        "--active-window-days",
        "1",
    ]));
    assert!(!o.status.success());
    let o = run(bin().args(["scan", tmp.path().join("nope").to_str().unwrap()]));
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("cannot"));
}

// ---------------------------------------------------------------- G1: inherited git config

/// Every file under `dir` (including `.git`) with its bytes, sorted. Git's transient
/// background-maintenance lock is skipped; anything else that appears or changes is a write.
fn snapshot_tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == "maintenance.lock") {
                continue;
            } else {
                out.push((p.clone(), fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

fn touch_later(path: &Path) {
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(later)
        .unwrap();
}

/// The filter command a hostile config would name: it leaves `marker-<tag>` two levels above
/// the repo (outside the scanned tree) and passes the content through.
fn filter_cmd(tag: &str) -> String {
    format!("echo ran >> ../../marker-{tag}; cat")
}

/// Config-file text defining the `probe` clean filter and the `probeproc` process filter, and a
/// global ignore file that hides an untracked file.
fn hostile_config(tag: &str, excludes: &Path) -> String {
    format!(
        "[filter \"probe\"]\n\tclean = \"{}\"\n[filter \"probeproc\"]\n\tprocess = \"{}\"\n\
         [core]\n\texcludesFile = \"{}\"\n",
        filter_cmd(tag),
        filter_cmd(&format!("{tag}-process")),
        excludes.to_string_lossy().replace('\\', "/"),
    )
}

fn repo_fact(scan: &serde_json::Value, repo: &Path, fact: &str) -> String {
    let want = repo.to_string_lossy();
    scan["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["path"].as_str() == Some(want.as_ref()))
        .unwrap_or_else(|| panic!("no node for {want}"))["repo"]["facts"][fact]
        .as_str()
        .unwrap_or("missing")
        .to_string()
}

type Setup = Box<dyn Fn(&mut Command)>;

#[test]
fn g1_inherited_git_config_never_runs_a_filter_and_is_not_applied() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let root = t.join("scan");

    // Repo 1: `.gitattributes` routes files through filters named `probe` and `probeproc`; its
    // own config is plain data. Tracked files are changed in place (same size, newer mtime), so
    // `git status` has to hash them through whatever driver is configured.
    let filtered = root.join("app");
    fs::create_dir_all(&filtered).unwrap();
    git(&filtered, &["init", "-q", "-b", "main"]);
    write(
        &filtered.join(".gitattributes"),
        "*.txt filter=probe\n*.dat filter=probeproc\n",
    );
    write(&filtered.join("a.txt"), "one\n");
    write(&filtered.join("b.dat"), "one\n");
    git(&filtered, &["add", ".gitattributes", "a.txt", "b.dat"]);
    git(&filtered, &["commit", "-q", "-m", "init"]);
    write(&filtered.join("a.txt"), "two\n");
    write(&filtered.join("b.dat"), "two\n");
    touch_later(&filtered.join("a.txt"));
    touch_later(&filtered.join("b.dat"));

    // Repo 2: clean apart from one untracked file that only a global ignore file hides.
    let hidden = root.join("notes");
    fs::create_dir_all(&hidden).unwrap();
    git(&hidden, &["init", "-q", "-b", "main"]);
    write(&hidden.join("README.md"), "hello\n");
    git(&hidden, &["add", "README.md"]);
    git(&hidden, &["commit", "-q", "-m", "init"]);
    write(&hidden.join("local-notes.txt"), "only copy\n");
    let excludes = t.join("global-ignore");
    write(&excludes, "local-notes.txt\n");

    let empty_home = t.join("empty-home");
    fs::create_dir_all(&empty_home).unwrap();
    let no_xdg = t.join("empty-xdg");

    // One configuration source per run, each with its own marker.
    let mut vectors: Vec<(&str, Setup)> = Vec::new();
    {
        let home = t.join("v-home");
        write(&home.join(".gitconfig"), &hostile_config("home", &excludes));
        let x = no_xdg.clone();
        vectors.push((
            "home",
            Box::new(move |c: &mut Command| {
                c.env("HOME", &home).env("XDG_CONFIG_HOME", &x);
            }),
        ));
    }
    {
        let xdg = t.join("v-xdg");
        write(
            &xdg.join("git").join("config"),
            &hostile_config("xdg", &excludes),
        );
        let h = empty_home.clone();
        vectors.push((
            "xdg",
            Box::new(move |c: &mut Command| {
                c.env("HOME", &h).env("XDG_CONFIG_HOME", &xdg);
            }),
        ));
    }
    {
        let file = t.join("v-global.gitconfig");
        write(&file, &hostile_config("global", &excludes));
        let (h, x) = (empty_home.clone(), no_xdg.clone());
        vectors.push((
            "global",
            Box::new(move |c: &mut Command| {
                c.env("HOME", &h)
                    .env("XDG_CONFIG_HOME", &x)
                    .env("GIT_CONFIG_GLOBAL", &file);
            }),
        ));
    }
    {
        let file = t.join("v-system.gitconfig");
        write(&file, &hostile_config("system", &excludes));
        let (h, x) = (empty_home.clone(), no_xdg.clone());
        vectors.push((
            "system",
            Box::new(move |c: &mut Command| {
                c.env("HOME", &h)
                    .env("XDG_CONFIG_HOME", &x)
                    .env("GIT_CONFIG_SYSTEM", &file);
            }),
        ));
    }
    {
        let ex = excludes.to_string_lossy().replace('\\', "/");
        let (h, x) = (empty_home.clone(), no_xdg.clone());
        vectors.push((
            "count",
            Box::new(move |c: &mut Command| {
                c.env("HOME", &h)
                    .env("XDG_CONFIG_HOME", &x)
                    .env("GIT_CONFIG_COUNT", "2")
                    .env("GIT_CONFIG_KEY_0", "filter.probe.clean")
                    .env("GIT_CONFIG_VALUE_0", filter_cmd("count"))
                    .env("GIT_CONFIG_KEY_1", "core.excludesFile")
                    .env("GIT_CONFIG_VALUE_1", &ex);
            }),
        ));
    }
    {
        let ex = excludes.to_string_lossy().replace('\\', "/");
        let (h, x) = (empty_home.clone(), no_xdg.clone());
        vectors.push((
            "params",
            Box::new(move |c: &mut Command| {
                c.env("HOME", &h).env("XDG_CONFIG_HOME", &x).env(
                    "GIT_CONFIG_PARAMETERS",
                    format!(
                        "'filter.probe.clean={}' 'core.excludesFile={}'",
                        filter_cmd("params"),
                        ex
                    ),
                );
            }),
        ));
    }

    let markers = || -> Vec<String> {
        fs::read_dir(t)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("marker-"))
            .collect()
    };
    assert!(markers().is_empty(), "fixture setup ran a filter");
    let before = (snapshot_tree(&filtered), snapshot_tree(&hidden));

    let mut failures: Vec<String> = Vec::new();
    for (tag, setup) in &vectors {
        let out_file = t.join(format!("scan-{tag}.json"));
        let mut c = bin();
        c.args([
            "scan",
            root.to_str().unwrap(),
            "--out",
            out_file.to_str().unwrap(),
        ]);
        for v in [
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_SYSTEM",
            "GIT_CONFIG_NOSYSTEM",
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_PARAMETERS",
        ] {
            c.env_remove(v);
        }
        c.env("APPDATA", t.join("appdata"));
        setup(&mut c);
        let o = run(&mut c);
        if !o.status.success() {
            failures.push(format!(
                "{tag}: scan failed: {}",
                String::from_utf8_lossy(&o.stderr)
            ));
            continue;
        }
        let ran = markers();
        if !ran.is_empty() {
            failures.push(format!("{tag}: a configured filter ran: {ran:?}"));
            for m in ran {
                let _ = fs::remove_file(t.join(m));
            }
        }
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&out_file).unwrap()).unwrap();
        // The filter attribute has no driver once inherited config is ignored, so git passes the
        // content through and sees the change: not clean, and not unknown.
        let clean = repo_fact(&v, &filtered, "tree_clean");
        if clean != "no" {
            failures.push(format!(
                "{tag}: filtered repo tree_clean = {clean}, want no"
            ));
        }
        // An inherited ignore rule must not hide the untracked file. Not applying it moves the
        // file from "ignored" to "untracked": `tree_clean` becomes `no` and `no_unique_ignored`
        // becomes `yes` (documented; the repo still needs recovery, never a plain archive).
        let clean = repo_fact(&v, &hidden, "tree_clean");
        if clean != "no" {
            failures.push(format!(
                "{tag}: globally ignored untracked file was hidden (tree_clean = {clean})"
            ));
        }
        let ignored = repo_fact(&v, &hidden, "no_unique_ignored");
        if ignored != "yes" {
            failures.push(format!(
                "{tag}: no_unique_ignored = {ignored}, want yes (the file is untracked now)"
            ));
        }
    }
    let after = (snapshot_tree(&filtered), snapshot_tree(&hidden));
    if before != after {
        failures.push("repository bytes changed during the scans".to_string());
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// ---------------------------------------------------------------- H1 (second merge gate)

/// A config key's subsection can carry a credential URL. A repo with such a key is refused, and
/// the refusal must name the section and variable only.
#[test]
fn h1_credential_in_a_config_subsection_never_reaches_any_output() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("scan");
    let repo = root.join("app");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    write(&repo.join("README.md"), "x\n");
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let config = repo.join(".git").join("config");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(
        "[credential \"https://user:SYNTHETIC_TOKEN@example.invalid\"]\n\thelper = store\n",
    );
    fs::write(&config, text).unwrap();
    let out_file = tmp.path().join("scan.json");

    let human_scan = run(bin().args([
        "scan",
        root.to_str().unwrap(),
        "--out",
        out_file.to_str().unwrap(),
    ]));
    assert!(human_scan.status.success());
    let scan_stdout = stdout(&human_scan);
    let o = run(bin().args(["scan", root.to_str().unwrap(), "--json"]));
    let scan_json_stdout = stdout(&o);
    let scan_file = fs::read_to_string(&out_file).unwrap();
    let plan_args = |json: bool| {
        let mut c = bin();
        c.args(["plan", "--scan", out_file.to_str().unwrap()])
            .args(["--active-window-days", "30", "--now", &far_future()]);
        if json {
            c.arg("--json");
        }
        run(&mut c)
    };
    let plan_human = stdout(&plan_args(false));
    let plan_json = stdout(&plan_args(true));

    for (what, text) in [
        ("scan summary", &scan_stdout),
        ("scan --json", &scan_json_stdout),
        ("scan file", &scan_file),
        ("plan summary", &plan_human),
        ("plan --json", &plan_json),
    ] {
        assert!(!text.contains("SYNTHETIC_TOKEN"), "token leaked in {what}");
        assert!(
            !text.contains("example.invalid"),
            "the subsection was printed in {what}"
        );
    }
    // The refusal itself is still reported, naming the section and variable only.
    let v: serde_json::Value = serde_json::from_str(&scan_file).unwrap();
    let notes = v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|n| n.get("repo").and_then(|r| r.get("notes")))
        .unwrap()
        .to_string();
    assert!(notes.contains("credential.<redacted>.helper"), "{notes}");
    // And the repo is not offered for removal.
    let pv: serde_json::Value = serde_json::from_str(&plan_json).unwrap();
    let repo_path = repo.to_string_lossy().into_owned();
    let item = pv["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["path"].as_str() == Some(repo_path.as_str()))
        .unwrap();
    assert_eq!(item["action"], "ask");
}

// ---------------------------------------------------------------- J4 (third merge gate)

#[test]
fn j4_trust_partial_process_check_flag_and_config_are_accepted_and_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    let (scan_file, _nm) = scan_with_project_touched(tmp.path(), 30);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let o = plan_json_raw(&scan_file, &home, &["--trust-partial-process-check"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["trusted_partial_process_check"], true);

    let o = plan_json_raw(&scan_file, &home, &[]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["trusted_partial_process_check"], false);

    let cfg = tmp.path().join("trust.toml");
    fs::write(&cfg, "trust_partial_process_check = true\n").unwrap();
    let o = plan_json_raw(&scan_file, &home, &["--config", cfg.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["trusted_partial_process_check"], true);

    let mut c = bin();
    c.args([
        "plan",
        "--scan",
        scan_file.to_str().unwrap(),
        "--trust-partial-process-check",
    ]);
    isolate(&mut c, &home);
    let text = stdout(&run(&mut c));
    assert!(text.contains("accepted"), "the header must say so: {text}");
}

// ---------------------------------------------------------------- L1 (click-test)

fn plan_exit(scan_json: &serde_json::Value, dir: &Path, name: &str) -> (Option<i32>, String) {
    let f = dir.join(name);
    fs::write(&f, scan_json.to_string()).unwrap();
    let o = run(bin().args(["plan", "--scan", f.to_str().unwrap(), "--json"]));
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
fn l1_hostile_scan_files_are_a_clean_error_with_exit_code_1() {
    let tmp = tempfile::tempdir().unwrap();
    let (scan_file, _nm) = scan_with_project_touched(tmp.path(), 30);
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&scan_file).unwrap()).unwrap();
    let child = v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|n| n.get("parent").is_some())
        .unwrap();

    let mut a = v.clone();
    a["nodes"][child]["parent"] = serde_json::json!(999_999);
    let (code, err) = plan_exit(&a, tmp.path(), "a.json");
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("nodes["), "{err}");

    let mut b = v.clone();
    b["nodes"][child]["parent"] = serde_json::json!(child);
    let (code, err) = plan_exit(&b, tmp.path(), "b.json");
    assert_eq!(code, Some(1), "{err}");
}

#[test]
fn l1_changing_one_index_at_a_time_gives_an_error_or_a_plan_never_a_crash() {
    let tmp = tempfile::tempdir().unwrap();
    let (scan_file, _nm) = scan_with_project_touched(tmp.path(), 30);
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&scan_file).unwrap()).unwrap();
    let n = v["nodes"].as_array().unwrap().len();
    let mut cases = 0;
    for i in 0..n {
        for val in [0usize, i, n, n + 7, usize::MAX >> 12] {
            let mut m = v.clone();
            m["nodes"][i]["parent"] = serde_json::json!(val);
            let (code, err) = plan_exit(&m, tmp.path(), "m.json");
            assert!(
                matches!(code, Some(0) | Some(1)),
                "parent {i}={val}: {code:?} {err}"
            );
            cases += 1;
        }
    }
    for key in ["big_files", "outputs"] {
        let len = v[key].as_array().map(|a| a.len()).unwrap_or(0);
        for i in 0..len.min(5) {
            for val in [0usize, n, n + 7] {
                let mut m = v.clone();
                m[key][i]["node"] = serde_json::json!(val);
                let (code, err) = plan_exit(&m, tmp.path(), "m.json");
                assert!(
                    matches!(code, Some(0) | Some(1)),
                    "{key}[{i}]={val}: {code:?} {err}"
                );
                cases += 1;
            }
        }
    }
    assert!(cases > 5);
}

// ---------------------------------------------------------------- R1, R2 (coverage)

/// A repository every safety fact calls saved: its only commit is on the remote-tracking ref of
/// a network remote. No network is contacted; the ref is written locally.
fn archivable_repo(repo: &Path) {
    fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "-q", "-b", "main"]);
    write(&repo.join("README.md"), "hello\n");
    git(repo, &["add", "README.md"]);
    git(repo, &["commit", "-q", "-m", "init"]);
    git(
        repo,
        &["remote", "add", "origin", "https://example.invalid/a.git"],
    );
    git(repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
}

fn scan_to_file(root: &Path, out: &Path) -> serde_json::Value {
    let o = run(bin().args([
        "scan",
        root.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]));
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_str(&fs::read_to_string(out).unwrap()).unwrap()
}

fn plan_value(o: &Output) -> serde_json::Value {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_str(&stdout(o)).expect("plan json")
}

fn item_of<'a>(plan: &'a serde_json::Value, path: &Path) -> &'a serde_json::Value {
    let want = path.to_string_lossy();
    plan["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["path"].as_str() == Some(&*want))
        .unwrap_or_else(|| panic!("no plan item for {want}"))
}

/// Running processes must be inspectable; otherwise every removal asks and the assertions that
/// follow would say nothing.
fn assert_process_check_usable(plan: &serde_json::Value) {
    assert_ne!(
        plan["process_check"], "unknown",
        "running processes must be inspectable for this test: {}",
        plan["process_check_note"]
    );
}

#[cfg(unix)]
#[test]
fn r1_t2_unreadable_folder_clone_asks_by_default_and_archives_when_accepted() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("scan");
    let a = root.join("a");
    archivable_repo(&a);
    // A shared clone of A in a folder the scan cannot read: it borrows A's objects.
    let vault = root.join("vault");
    let dep = vault.join("dep");
    fs::create_dir_all(&vault).unwrap();
    git(
        &root,
        &[
            "clone",
            "-q",
            "--shared",
            a.to_str().unwrap(),
            dep.to_str().unwrap(),
        ],
    );
    git(&dep, &["cat-file", "-e", "HEAD^{tree}"]);
    assert!(dep.join(".git/objects/info/alternates").is_file());

    fs::set_permissions(&vault, fs::Permissions::from_mode(0o000)).unwrap();
    let readable_anyway = fs::read_dir(&vault).is_ok(); // running as root
    let scan_file = tmp.path().join("scan.json");
    let sv = scan_to_file(&root, &scan_file);
    fs::set_permissions(&vault, fs::Permissions::from_mode(0o755)).unwrap();
    if readable_anyway {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI must not run this test as a user who can read a mode-000 folder"
        );
        eprintln!("running as a user who reads mode-000 folders: test not meaningful here");
        return;
    }

    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let now = far_future();

    // Gate 98f8764 T2, default: the clone was not seen (it cannot be), so A's dependents are
    // unknown and it asks, naming what was not read and the opt-in.
    let pv = plan_value(&plan_json(&scan_file, &home, &["--now", &now]));
    assert_process_check_usable(&pv);
    let it = item_of(&pv, &a);
    let reason = it["reason"].as_str().unwrap();
    assert_eq!(it["action"], "ask", "{reason}");
    assert_eq!(it["reclaimable_bytes"], 0);
    assert_eq!(it["facts"]["no_dependents"], "unknown");
    assert!(reason.contains("1 folder could not be read"), "{reason}");
    assert!(reason.contains("--accept-incomplete-coverage"), "{reason}");
    assert_eq!(pv["coverage_complete"], false);
    assert_eq!(pv["accepted_incomplete_coverage"], false);
    assert!(pv.get("required_full_coverage").is_none(), "{pv}");
    let note = pv["coverage_note"].as_str().unwrap_or_default();
    assert!(
        note.contains("1 folder could not be read") && note.contains("links or clones inside"),
        "{note}"
    );

    // The scan file records where coverage stopped.
    let cov = &sv["coverage"];
    assert_eq!(cov["complete"], false, "{cov}");
    assert_eq!(cov["unreadable_dirs"], 1, "{cov}");
    assert_eq!(cov["mount_points_skipped"], 0, "{cov}");
    let examples: Vec<&str> = cov["unreadable_dir_examples"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e.as_str())
        .collect();
    assert_eq!(examples, vec![&*vault.to_string_lossy()]);

    // The human plan says it in the header.
    let mut c = bin();
    c.args(["plan", "--scan", scan_file.to_str().unwrap(), "--now", &now]);
    isolate(&mut c, &home);
    let text = stdout(&run(&mut c));
    assert!(text.contains("1 folder could not be read"), "{text}");
    assert!(text.contains("--accept-incomplete-coverage"), "{text}");

    // Opt-in, by flag: the disclosure behaviour. A is archived, its reason carries the coverage
    // sentence, and the plan records that the user accepted incomplete coverage.
    let accepted = |pv: &serde_json::Value| {
        let it = item_of(pv, &a);
        let reason = it["reason"].as_str().unwrap();
        assert_eq!(it["action"], "backup-then-remove", "{reason}");
        assert!(
            reason.contains(
                "1 folder in this scan could not be read; dependents inside it were not checked"
            ),
            "the archive reason must carry the coverage sentence: {reason}"
        );
        assert_eq!(pv["accepted_incomplete_coverage"], true);
    };
    let flag = ["--now", &now, "--accept-incomplete-coverage"];
    accepted(&plan_value(&plan_json(&scan_file, &home, &flag)));

    // Opt-in, by config.
    let cfg = tmp.path().join("accept.toml");
    fs::write(&cfg, "accept_incomplete_coverage = true\n").unwrap();
    let by_config = ["--now", &now, "--config", cfg.to_str().unwrap()];
    accepted(&plan_value(&plan_json(&scan_file, &home, &by_config)));

    let mut c = bin();
    c.args(["plan", "--scan", scan_file.to_str().unwrap()])
        .args(flag);
    isolate(&mut c, &home);
    let text = stdout(&run(&mut c));
    assert!(
        text.contains("Incomplete coverage was accepted"),
        "the header must say the user accepted it: {text}"
    );

    // The old strict setting is refused, by flag and by config, naming the new ones.
    let old_flag = ["--now", &now, "--require-full-coverage"];
    let o = plan_json(&scan_file, &home, &old_flag);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "{err}");
    assert!(err.contains("--accept-incomplete-coverage"), "{err}");
    let old = tmp.path().join("old.toml");
    fs::write(&old, "require_full_coverage = true\n").unwrap();
    let old_config = ["--now", &now, "--config", old.to_str().unwrap()];
    let o = plan_json(&scan_file, &home, &old_config);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "{err}");
    assert!(err.contains("accept_incomplete_coverage"), "{err}");
}

#[test]
fn r1_a_fully_readable_scan_is_complete_and_adds_no_coverage_sentence() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("scan");
    let a = root.join("a");
    archivable_repo(&a);
    let scan_file = tmp.path().join("scan.json");
    let sv = scan_to_file(&root, &scan_file);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let now = far_future();

    let pv = plan_value(&plan_json(&scan_file, &home, &["--now", &now]));
    assert_process_check_usable(&pv);
    let it = item_of(&pv, &a);
    let reason = it["reason"].as_str().unwrap();
    assert_eq!(it["action"], "backup-then-remove", "{reason}");
    assert_eq!(pv["coverage_complete"], true);
    assert!(pv["coverage_note"].is_null(), "{}", pv["coverage_note"]);
    assert!(
        !reason.contains("could not be read") && !reason.contains("not entered"),
        "{reason}"
    );
    let cov = &sv["coverage"];
    assert_eq!(cov["complete"], true, "{cov}");
    assert_eq!(cov["unreadable_dirs"], 0, "{cov}");

    // The opt-in changes nothing when coverage is complete.
    let flag = ["--now", &now, "--accept-incomplete-coverage"];
    let pv = plan_value(&plan_json(&scan_file, &home, &flag));
    assert_eq!(item_of(&pv, &a)["action"], "backup-then-remove");
    assert_eq!(pv["accepted_incomplete_coverage"], true);
}

// ---------------------------------------------------------------- Y1, Y5 (preserve obligation)

#[test]
fn y5_a_removal_of_a_repository_says_what_it_backs_up_first() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("scan");
    let a = root.join("a");
    archivable_repo(&a);
    let scan_file = tmp.path().join("scan.json");
    scan_to_file(&root, &scan_file);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let now = far_future();

    let pv = plan_value(&plan_json(&scan_file, &home, &["--now", &now]));
    assert_eq!(pv["schema_version"], 15);
    let it = item_of(&pv, &a);
    assert_eq!(it["action"], "backup-then-remove", "{it}");
    let pre = &it["preserve"]["git_admin"];
    assert!(pre["files"].as_u64().unwrap_or(0) > 0, "{it}");
    assert!(pre["bytes"].as_u64().unwrap_or(0) > 0, "{it}");
    let gd = a.join(".git").to_string_lossy().into_owned();
    assert_eq!(pre["git_dirs"][0].as_str(), Some(gd.as_str()), "{it}");

    // The human plan shows it under the item.
    let mut c = bin();
    c.args(["plan", "--scan", scan_file.to_str().unwrap(), "--now", &now])
        .arg("--trust-partial-process-check");
    isolate(&mut c, &home);
    let text = stdout(&run(&mut c));
    assert!(text.contains("backup-then-remove"), "{text}");
    assert!(text.contains("back up first:"), "{text}");
    assert!(text.contains(&gd), "{text}");
}

/// Round 15: there is no object verification. `scan --verify-objects` (and `scan --config`,
/// which only carried its setting) is refused as an unknown argument, a config file that still
/// sets `verify_objects` is refused, the plan states both backup obligations with no
/// verification field, and a scan file that claims a verification is an invalid scan file
/// (exit 1). The human plan says what "the manifest" is once, in its header.
#[test]
fn r15_object_data_is_backed_up_and_no_verification_is_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("scan");
    let a = root.join("a");
    archivable_repo(&a);
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let now = far_future();

    let cfg = tmp.path().join("verify.toml");
    write(&cfg, "verify_objects = true\n");
    let refused: [&[&str]; 2] = [&["--verify-objects"], &["--config", cfg.to_str().unwrap()]];
    for extra in refused {
        let mut c = bin();
        c.args(["scan", root.to_str().unwrap()]).args(extra);
        isolate(&mut c, &home);
        let o = run(&mut c);
        assert_eq!(o.status.code(), Some(2), "{extra:?} was accepted");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains("unexpected argument"), "{extra:?}: {err}");
        assert!(err.contains(extra[0]), "{extra:?}: {err}");
    }

    let scan_file = tmp.path().join("scan.json");
    let v = scan_to_file(&root, &scan_file);
    let o = plan_json_raw(
        &scan_file,
        &home,
        &["--now", &now, "--config", cfg.to_str().unwrap()],
    );
    assert!(
        !o.status.success(),
        "a config with verify_objects was accepted"
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("verify_objects"), "{err}");

    let pv = plan_value(&plan_json(&scan_file, &home, &["--now", &now]));
    let it = item_of(&pv, &a);
    assert_eq!(it["action"], "backup-then-remove", "{it}");
    let store = &it["preserve"]["object_store"];
    assert!(store["files"].as_u64().unwrap_or(0) > 0, "{store}");
    assert!(store["bytes"].as_u64().unwrap_or(0) > 0, "{store}");
    assert!(store.get("verified").is_none(), "{store}");

    // A scan file that claims a verification is refused, whatever the claim.
    let i = v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|n| n["path"].as_str() == Some(a.to_str().unwrap()))
        .expect("the repository node");
    for (k, (block, key, val)) in [
        ("object_store", "verified", serde_json::json!(true)),
        ("object_store", "checked", serde_json::json!(false)),
        ("", "verified", serde_json::json!(true)),
    ]
    .into_iter()
    .enumerate()
    {
        let mut m = v.clone();
        if block.is_empty() {
            m["nodes"][i]["repo"][key] = val;
        } else {
            m["nodes"][i]["repo"][block][key] = val;
        }
        let (code, err) = plan_exit(&m, tmp.path(), &format!("forged{k}.json"));
        assert_eq!(code, Some(1), "{block}.{key} accepted: {err}");
        assert!(err.contains("invalid scan file"), "{block}.{key}: {err}");
        assert!(err.contains(key), "{block}.{key}: {err}");
        assert!(!err.contains("verified by this scan"), "{err}");
    }

    let mut c = bin();
    c.args(["plan", "--scan", scan_file.to_str().unwrap(), "--now", &now])
        .arg("--trust-partial-process-check");
    isolate(&mut c, &home);
    let text = stdout(&run(&mut c));
    assert!(
        text.contains("the manifest is the restore record a later apply step writes"),
        "{text}"
    );
    assert!(
        text.contains("and its object store is backed up too"),
        "{text}"
    );
    assert!(
        text.contains("of object data, to be backed up before removal"),
        "{text}"
    );
    for gone in [
        "verified by this scan",
        "not yet verified",
        "verify against the remote",
        "verified against the remote or saved",
    ] {
        assert!(!text.contains(gone), "{gone:?} in {text}");
    }
}
