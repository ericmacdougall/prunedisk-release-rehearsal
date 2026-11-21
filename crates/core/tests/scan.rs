//! Scan and classification: AC-1, AC-2, AC-3, AC-6, AC-8 and NEG-4, NEG-5, NEG-7, NEG-10.

mod common;

use common::*;
use disk_cleaner_core::{scan, scan_from_json, Class, ScanOptions, SCHEMA_VERSION};
use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MIB: u64 = 1024 * 1024;

fn set_mtime(path: &std::path::Path, secs: i64) {
    let t = UNIX_EPOCH + Duration::from_secs(secs as u64);
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[test]
fn neg5_build_without_project_marker_is_not_regenerable() {
    let (_tmp, root, _) = workspace();
    // No package.json, Cargo.toml, ... anywhere at or above `build` inside the scan root.
    write_bytes(&root.join("stray").join("build").join("out.o"), 4096);
    // The same name next to a marker is regenerable.
    write(&root.join("proj").join("package.json"), "{}");
    write_bytes(&root.join("proj").join("build").join("out.js"), 4096);

    let res = scan_plain(&root);
    assert!(
        !has_node(&res, &root.join("stray").join("build")),
        "build/ without a marker must not be classified"
    );
    let b = node(&res, &root.join("proj").join("build"));
    assert_eq!(b.class, Class::Regenerable);
    assert_eq!(b.rule.as_deref(), Some("build"));
}

#[test]
fn ambiguous_names_need_their_own_marker() {
    let (_tmp, root, _) = workspace();
    // `target` next to a package.json is not a Cargo target dir.
    write(&root.join("js").join("package.json"), "{}");
    write_bytes(&root.join("js").join("target").join("x.bin"), 100);
    // bin/obj need a .NET project file, found by suffix.
    write(&root.join("dn").join("App.csproj"), "<Project/>");
    write_bytes(&root.join("dn").join("bin").join("App.dll"), 100);
    write_bytes(&root.join("dn").join("obj").join("a.cache"), 100);
    // Rust target with its marker.
    write(&root.join("rs").join("Cargo.toml"), "[package]\n");
    write_bytes(&root.join("rs").join("target").join("debug").join("x"), 100);

    let res = scan_plain(&root);
    assert!(!has_node(&res, &root.join("js").join("target")));
    assert_eq!(
        node(&res, &root.join("dn").join("bin")).class,
        Class::Regenerable
    );
    assert_eq!(
        node(&res, &root.join("dn").join("obj")).class,
        Class::Regenerable
    );
    assert_eq!(
        node(&res, &root.join("rs").join("target")).class,
        Class::Regenerable
    );
}

#[test]
fn marker_beside_the_directory_counts_in_a_workspace_package() {
    let (_tmp, root, _) = workspace();
    write(&root.join("mono").join("package.json"), "{}");
    write(
        &root
            .join("mono")
            .join("packages")
            .join("a")
            .join("package.json"),
        "{}",
    );
    write_bytes(
        &root
            .join("mono")
            .join("packages")
            .join("a")
            .join("node_modules")
            .join("x.js"),
        100,
    );
    let res = scan_plain(&root);
    assert!(has_node(
        &res,
        &root
            .join("mono")
            .join("packages")
            .join("a")
            .join("node_modules")
    ));
}

#[test]
fn neg4_small_video_in_build_dir_is_listed_and_directory_flagged() {
    let (_tmp, root, _) = workspace();
    write(&root.join("app").join("package.json"), "{}");
    write_bytes(&root.join("app").join("out").join("render.mp4"), 1024);
    write_bytes(&root.join("app").join("out").join("bundle.js"), 1024);

    let res = scan_plain(&root);
    let out_dir = root.join("app").join("out");
    let out = node(&res, &out_dir);
    assert_eq!(out.class, Class::Regenerable);
    assert!(out.contains_outputs, "out/ holds an output file");
    let listed: Vec<_> = res
        .outputs
        .iter()
        .filter(|o| o.path.ends_with("render.mp4"))
        .collect();
    assert_eq!(listed.len(), 1, "the 1 KB mp4 must be listed individually");
    assert_eq!(listed[0].node, Some(node_index(&res, &out_dir)));
    assert!(!res.outputs.iter().any(|o| o.path.ends_with("bundle.js")));
}

#[test]
fn dependency_directories_do_not_get_flagged_for_fixture_archives() {
    let (_tmp, root, _) = workspace();
    write(&root.join("app").join("package.json"), "{}");
    // Packages routinely ship test fixtures such as zips; that must not pin node_modules. The
    // package's own package.json is what shows the zip is installed package content: without it
    // the zip is an output outside any package and the directory asks (gate19).
    write(
        &root
            .join("app")
            .join("node_modules")
            .join("pkg")
            .join("package.json"),
        "{}",
    );
    write_bytes(
        &root
            .join("app")
            .join("node_modules")
            .join("pkg")
            .join("fixture.zip"),
        1024,
    );
    let res = scan_plain(&root);
    let nm = node(&res, &root.join("app").join("node_modules"));
    assert!(!nm.contains_outputs);
}

#[test]
fn big_outputs_outside_build_dirs_are_listed_small_ones_are_not() {
    let (_tmp, root, _) = workspace();
    write_bytes(&root.join("media").join("big.mov"), 20_000);
    write_bytes(&root.join("media").join("small.mov"), 100);
    let mut opts = ScanOptions::new(vec![root.clone()]);
    opts.min_output_bytes = 8192;
    let res = scan(&opts).unwrap();
    assert!(res.outputs.iter().any(|o| o.path.ends_with("big.mov")));
    assert!(!res.outputs.iter().any(|o| o.path.ends_with("small.mov")));
    assert!(res.totals.output_bytes > 0);
}

#[cfg(unix)]
#[test]
fn neg7_symlink_loop_and_hard_links_terminate_and_count_once() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("scan");
    let outside = tmp.path().join("outside");
    write_bytes(&root.join("data").join("a.bin"), MIB as usize);
    fs::hard_link(
        root.join("data").join("a.bin"),
        root.join("data").join("b.bin"),
    )
    .unwrap();
    // A loop back to an ancestor and a link to a big directory outside the scan.
    symlink(&root, root.join("data").join("loop")).unwrap();
    write_bytes(&outside.join("huge.bin"), 2 * MIB as usize);
    symlink(&outside, root.join("data").join("away")).unwrap();

    let res = scan_plain(&root);
    let t = &res.totals;
    assert_eq!(t.hardlink_duplicates_skipped, 1);
    assert!(t.symlinks >= 2);
    assert!(
        t.allocated_bytes >= MIB && t.allocated_bytes < MIB + MIB / 2,
        "1 MiB hard-linked pair counted once, links not followed; got {}",
        t.allocated_bytes
    );
    assert!(t.logical_bytes >= MIB && t.logical_bytes < MIB + MIB / 2);
}

#[cfg(unix)]
#[test]
fn unreadable_directory_is_counted_and_reported_not_fatal() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, root, _) = workspace();
    let locked = root.join("locked");
    write(&locked.join("secret.txt"), "x");
    write(&root.join("fine.txt"), "y");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let readable_anyway = fs::read_dir(&locked).is_ok(); // running as root
    let res = scan_plain(&root);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if readable_anyway {
        return;
    }
    assert!(res.totals.unreadable >= 1);
    assert!(res.issues.iter().any(|i| i.path.ends_with("locked")));
    assert!(
        res.totals.files >= 1,
        "the rest of the tree is still scanned"
    );
}

#[test]
fn missing_root_is_an_error_not_a_panic() {
    let (_tmp, root, _) = workspace();
    let opts = ScanOptions::new(vec![root.join("does-not-exist")]);
    assert!(scan(&opts).is_err());
}

#[test]
fn repo_reports_total_git_regenerable_and_output_bytes_separately() {
    let (_tmp, root, _) = workspace();
    let repo = root.join("app");
    init_repo(&repo);
    write(&repo.join("package.json"), "{}");
    write_bytes(&repo.join("src").join("main.js"), 10_000);
    write_bytes(&repo.join("node_modules").join("dep").join("i.js"), 200_000);
    write_bytes(&repo.join("assets").join("clip.mp4"), 3_000);

    let res = scan_plain(&root);
    let r = node(&res, &repo);
    assert_eq!(r.class, Class::Repo);
    let nm_path = repo.join("node_modules");
    let nm = node(&res, &nm_path);
    assert_eq!(nm.class, Class::Regenerable);
    assert_eq!(nm.parent, Some(node_index(&res, &repo)));

    let sizes = &r.repo.as_ref().unwrap().sizes;
    assert!(sizes.git > 0, ".git bytes");
    assert!(sizes.regenerable >= 200_000, "regenerable bytes");
    assert!(sizes.output >= 3_000, "output bytes");
    assert_eq!(sizes.regenerable, nm.bytes_total);
    assert_eq!(sizes.total, r.bytes_total);
    // Size is attributed to the innermost classified ancestor: node_modules is not in the
    // repo's own bytes but is in its total.
    assert_eq!(r.bytes_total, r.bytes_self + nm.bytes_total);
    assert!(r.bytes_self > sizes.git);
    // A small mp4 outside a build directory is counted but not listed.
    assert!(res.outputs.is_empty());
}

#[test]
fn classes_repo_worktree_bare_loose_and_plain() {
    let (_tmp, root, _) = workspace();
    let repo = root.join("main-repo");
    init_repo(&repo);
    let wt = root.join("linked-wt");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "wt-branch",
        ],
    );
    let bare = root.join("mirror.git");
    init_bare(&bare);
    write(&root.join("tool").join("package.json"), "{}");
    write(&root.join("tool").join("src").join("a.js"), "1");
    write(&root.join("scripts").join("run.py"), "print(1)");
    write(&root.join("docs").join("notes.txt"), "text");

    let res = scan_plain(&root);
    assert_eq!(node(&res, &repo).class, Class::Repo);
    assert_eq!(node(&res, &wt).class, Class::Worktree);
    assert_eq!(node(&res, &bare).class, Class::BareRepo);
    assert_eq!(node(&res, &root.join("tool")).class, Class::LooseCode);
    assert_eq!(node(&res, &root.join("scripts")).class, Class::LooseCode);
    assert!(!has_node(&res, &root.join("docs")));
    // The scan root itself holds no manifest or source, so it is not loose code.
    assert!(!has_node(&res, &root));
    // The .git directory is part of the repo, not a node of its own.
    assert!(!has_node(&res, &repo.join(".git")));
}

#[test]
fn repos_inside_dependency_directories_are_not_classified() {
    let (_tmp, root, _) = workspace();
    write(&root.join("app").join("package.json"), "{}");
    let dep = root.join("app").join("node_modules").join("dep");
    fs::create_dir_all(&dep).unwrap();
    git(&dep, &["init", "-q", "-b", "main"]);
    let res = scan_plain(&root);
    assert!(
        !res.nodes.iter().any(|n| n.class.is_git()),
        "regenerable directories are opaque"
    );
}

#[test]
fn neg10_paths_with_spaces_and_non_ascii_round_trip_through_json() {
    let (_tmp, root, _) = workspace();
    let dir = root.join("résumé & notes").join("日本語 project");
    write(&dir.join("package.json"), "{}");
    write_bytes(&dir.join("node_modules").join("x.js"), 100);

    let res = scan_plain(&root);
    let n = node(&res, &dir);
    assert_eq!(n.class, Class::LooseCode);
    let json = serde_json::to_string(&res).unwrap();
    let back = scan_from_json(&json).unwrap();
    let n2 = node(&back, &dir);
    assert_eq!(n2.path, n.path);
    assert!(has_node(&back, &dir.join("node_modules")));
    assert_eq!(back.schema_version, SCHEMA_VERSION);
}

#[test]
fn schema_version_mismatch_is_rejected() {
    let err = scan_from_json("{\"schema_version\": 999}").unwrap_err();
    assert!(err.to_string().contains("999"));
}

#[test]
fn activity_ignores_regenerable_directories_and_dot_git() {
    let (_tmp, root, _) = workspace();
    let repo = root.join("proj");
    init_repo(&repo);
    write(&repo.join("package.json"), "{}");
    write_bytes(&repo.join("node_modules").join("fresh.js"), 100);
    let old = now_secs() - 400 * 86_400;
    set_mtime(&repo.join("README.md"), old);
    set_mtime(&repo.join("package.json"), old);

    let res = scan_plain(&root);
    let r = node(&res, &repo);
    assert_eq!(
        r.newest_mtime,
        Some(old),
        "fresh files in node_modules and .git must not count as project activity"
    );
    let info = r.repo.as_ref().unwrap();
    let commit = info.last_commit_time.expect("last commit time");
    assert!(commit >= now_secs() - 3600, "commit just made: {commit}");
}

#[test]
fn top_files_is_bounded_and_sorted() {
    let (_tmp, root, _) = workspace();
    for i in 0..10usize {
        write_bytes(&root.join(format!("f{i}.dat")), 10_000 * (i + 1));
    }
    let mut opts = ScanOptions::new(vec![root.clone()]);
    opts.top_files = 3;
    let res = scan(&opts).unwrap();
    assert_eq!(res.big_files.len(), 3);
    assert!(res.big_files[0].bytes >= res.big_files[1].bytes);
    assert!(res.big_files[0].path.ends_with("f9.dat"));
}

#[test]
fn rules_file_loads_from_toml_text() {
    use disk_cleaner_core::RuleSet;
    let rs = RuleSet::from_toml(
        "[[rule]]\nname = \"gen\"\nmarkers = [\"proj.cfg\"]\nclass = \"regenerable\"\n",
    )
    .unwrap();
    assert_eq!(rs.rules().len(), 1);
}
