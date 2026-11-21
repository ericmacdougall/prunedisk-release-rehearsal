//! Regression tests for the merge-gate review of PR #2 (findings G3 and G4; G1 is an end-to-end
//! test in the CLI crate and G2 needs real mounts, see `mount.rs`). Each test fails on the code
//! the review was written against unless it says it is a guard or a behaviour pin.

mod common;

use common::*;
use disk_cleaner_core::{plan, Action, Class, PlanOptions, ScanResult, Tri};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

// ---------------------------------------------------------------- G1: a documented consequence

/// Not failing-first: pins a documented consequence of the G1 fix. The scanner reads no system or
/// global config, so `core.autocrlf=true` (Git for Windows' system default) no longer applies. A
/// working tree written with it holds CRLF while the blobs hold LF; once a file's stat data is
/// stale, git compares raw bytes and the tree reads as modified: never clean, never unknown.
#[test]
fn line_endings_converted_only_by_system_config_read_as_modified() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    commit_file(&repo, "notes.txt", "one\ntwo\n");
    git(&repo, &["push", "-q", "origin", "main"]);
    let file = repo.join("notes.txt");
    fs::remove_file(&file).unwrap();
    git(
        &repo,
        &["-c", "core.autocrlf=true", "checkout", "--", "notes.txt"],
    );
    assert_eq!(fs::read(&file).unwrap(), b"one\r\ntwo\r\n".to_vec());
    let later = SystemTime::now() + Duration::from_secs(120);
    fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(later)
        .unwrap();

    let res = scan_with(&root, network_everywhere());
    let f = facts_of(&res, &repo);
    assert_eq!(f.tree_clean, Tri::No, "{f:?}");
    assert_eq!(f.head_on_remote, Tri::Yes, "{f:?}");
    assert_eq!(f.stash_empty, Tri::Yes, "{f:?}");
    // A removing action that starts with recovery, which re-checks the facts before anything
    // is removed; not `archive`.
    assert_eq!(action_for(&res, &repo), Action::RecoverBackupThenRemove);
}

// ---------------------------------------------------------------- G3: unresolved alternates

/// `git clone --shared`: the clone's object store is empty and every object comes from `src`
/// through `.git/objects/info/alternates`.
fn shared_clone(root: &Path, src: &Path, name: &str) -> PathBuf {
    let dst = root.join(name);
    git(
        root,
        &[
            "clone",
            "-q",
            "--shared",
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
        ],
    );
    dst
}

/// The borrower has no objects of its own and still resolves HEAD: it really needs the source.
fn assert_borrows_every_object(borrower: &Path) {
    let objects = borrower.join(".git").join("objects");
    let own: Vec<String> = fs::read_dir(&objects)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "info" && n != "pack")
        .collect();
    assert!(own.is_empty(), "borrower has loose objects: {own:?}");
    let packs = fs::read_dir(objects.join("pack"))
        .map(|r| r.count())
        .unwrap_or(0);
    assert_eq!(packs, 0, "borrower has packs of its own");
    git(borrower, &["cat-file", "-e", "HEAD^{tree}"]);
}

#[test]
fn g3_oversized_alternates_file_makes_every_possible_provider_ask() {
    let (_tmp, root, remotes) = workspace();
    let src = pushed_repo(&root, &remotes, "source");
    let other = pushed_repo(&root, &remotes, "unrelated");
    let borrower = shared_clone(&root, &src, "borrower");
    let alt = borrower
        .join(".git")
        .join("objects")
        .join("info")
        .join("alternates");
    let original = fs::read_to_string(&alt).unwrap();
    // Comment lines are allowed in an alternates file; pad it past the 64 KiB read limit.
    let mut text = String::new();
    while text.len() <= 70 * 1024 {
        text.push_str("# padding so the file is larger than the scanner will read\n");
    }
    text.push_str(&original);
    fs::write(&alt, text).unwrap();
    assert_borrows_every_object(&borrower);

    let res = scan_with(&root, network_everywhere());
    assert!(
        res.warnings.iter().any(|w| w.contains("alternates")),
        "{:?}",
        res.warnings
    );
    // The scan cannot tell which repo the borrower depends on, so none may be removed.
    for provider in [&src, &other] {
        assert_eq!(
            facts_of(&res, provider).no_dependents,
            Tri::Unknown,
            "{}",
            provider.display()
        );
        assert_eq!(
            action_for(&res, provider),
            Action::Ask,
            "{}",
            provider.display()
        );
    }
}

// ---------------------------------------------------------------- G4: rules that hold unique data

fn plan_of(res: &ScanResult) -> disk_cleaner_core::Plan {
    plan(res, &PlanOptions::new(7, far_future()))
}

fn item<'a>(p: &'a disk_cleaner_core::Plan, path: &Path) -> &'a disk_cleaner_core::PlanItem {
    p.items
        .iter()
        .find(|i| i.path == path.to_string_lossy())
        .unwrap_or_else(|| panic!("no plan item for {}", path.display()))
}

#[test]
fn g4_maven_local_repository_with_a_locally_installed_artifact_is_never_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let repo = home.join(".m2").join("repository");
    // `mvn install:install-file` of a vendor jar that exists nowhere else: no
    // `_remote.repositories`, only the local metadata Maven writes for a local install.
    let ga = repo.join("com").join("example").join("inhouse");
    let art = ga.join("1.0.0");
    write_bytes(&art.join("inhouse-1.0.0.jar"), 4096);
    write(
        &art.join("inhouse-1.0.0.pom"),
        "<project><groupId>com.example</groupId><artifactId>inhouse</artifactId>\
         <version>1.0.0</version></project>\n",
    );
    write(
        &ga.join("maven-metadata-local.xml"),
        "<metadata><versioning><release>1.0.0</release></versioning></metadata>\n",
    );
    let res = scan_plain(&home);
    assert_eq!(node(&res, &repo).class, Class::Cache);
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    assert_eq!(p.totals.ready_bytes, 0);
    assert_eq!(p.totals.reclaimable_bytes, 0);
}

#[test]
fn g4_gradle_user_home_matched_as_a_project_cache_is_never_removed() {
    // Someone ran Gradle in their home directory, so `~/build.gradle` sits beside `~/.gradle`,
    // which is the Gradle user home: init scripts and gradle.properties (often credentials).
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    write(&home.join("build.gradle"), "// scratch build\n");
    let gh = home.join(".gradle");
    write(&gh.join("gradle.properties"), "org.gradle.jvmargs=-Xmx2g\n");
    write(
        &gh.join("init.d").join("repos.gradle"),
        "allprojects { repositories { mavenLocal() } }\n",
    );
    let modules = gh.join("caches").join("modules-2");
    write_bytes(&modules.join("files.lock"), 2048);
    // Control: a real project cache directory is still removable.
    let proj = home.join("code").join("app");
    write(&proj.join("build.gradle"), "plugins { id 'java' }\n");
    let hashes = proj.join(".gradle").join("8.5").join("fileHashes");
    write_bytes(&hashes.join("fileHashes.lock"), 2048);

    let res = scan_plain(&home);
    let p = plan_of(&res);
    let it = item(&p, &gh);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    let ctl = item(&p, &proj.join(".gradle"));
    assert_eq!(ctl.action, Action::RemoveRegenerable, "{}", ctl.reason);
}

#[test]
fn g4_gradle_properties_in_the_user_home_is_outside_every_removing_item() {
    // Guard (passes before the fix too): with no build file beside it, only `~/.gradle/caches`
    // is classified, never the directory that holds gradle.properties and init scripts.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let gh = home.join(".gradle");
    write(&gh.join("gradle.properties"), "org.gradle.daemon=true\n");
    write(&gh.join("init.d").join("a.gradle"), "// init\n");
    let modules = gh.join("caches").join("modules-2");
    write_bytes(&modules.join("files.lock"), 2048);
    let res = scan_plain(&home);
    let p = plan_of(&res);
    for it in p.items.iter().filter(|i| i.action.removes()) {
        for kept in [gh.join("gradle.properties"), gh.join("init.d")] {
            assert!(
                !kept.starts_with(&it.path),
                "{} would go with {}",
                kept.display(),
                it.path
            );
        }
    }
}

#[test]
fn g4_terraform_directory_with_backend_state_is_never_removed() {
    // `.terraform/terraform.tfstate` records the backend configuration given to `terraform init`
    // (possibly with -backend-config values that exist nowhere else); `environment` records the
    // selected workspace.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("scan");
    let proj = root.join("infra");
    write(&proj.join("main.tf"), "terraform {}\n");
    let tf = proj.join(".terraform");
    write(
        &tf.join("terraform.tfstate"),
        "{\"version\":3,\"backend\":{\"type\":\"s3\",\"config\":{\"bucket\":\"example\"}}}\n",
    );
    write(&tf.join("environment"), "staging");
    let providers = tf.join("providers").join("registry.example");
    write_bytes(&providers.join("p"), 4096);
    let res = scan_plain(&root);
    let p = plan_of(&res);
    let it = item(&p, &tf);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
}
