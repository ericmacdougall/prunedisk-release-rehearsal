//! Regression tests for the sixth merge-gate review of PR #2: repositories that share an object
//! directory (M1) and alternates files written in git's full syntax (M2).
#![cfg(unix)]

mod common;

use common::*;
use disk_cleaner_core::{Action, Tri};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

fn not_removing(a: Action) -> bool {
    !matches!(a, Action::BackupThenRemove | Action::RemoveRegenerable)
}

/// A repo in `root` whose `.git/objects` is a symlink to `target_objects`, with one unpushed
/// commit (whose objects therefore land in the target's directory).
fn repo_with_shared_objects(root: &Path, name: &str, target_objects: &Path) -> PathBuf {
    let b = root.join(name);
    fs::create_dir_all(&b).unwrap();
    git(&b, &["init", "-q", "-b", "main"]);
    let objs = b.join(".git").join("objects");
    fs::remove_dir_all(&objs).unwrap();
    symlink(target_objects, &objs).unwrap();
    write(&b.join("f.txt"), &format!("work of {name}\n"));
    git(&b, &["add", "f.txt"]);
    git(&b, &["commit", "-q", "-m", "unpushed work"]);
    b
}

fn dependents(res: &disk_cleaner_core::ScanResult, p: &Path) -> Vec<String> {
    node(res, p).repo.as_ref().unwrap().dependents.clone()
}

// ---------------------------------------------------------------- M1

#[test]
fn m1_two_repos_sharing_one_object_directory_depend_on_each_other() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let b = repo_with_shared_objects(&root, "b", &a.join(".git").join("objects"));
    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &a).no_dependents, Tri::No);
    assert_eq!(facts_of(&res, &b).no_dependents, Tri::No);
    assert!(
        not_removing(action_for(&res, &a)),
        "{:?}",
        action_for(&res, &a)
    );
    assert!(dependents(&res, &a).iter().any(|d| d.ends_with("/b")));
    assert!(dependents(&res, &b).iter().any(|d| d.ends_with("/a")));
}

#[test]
fn m1_three_repos_sharing_one_object_directory_all_report_the_others() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let objs = a.join(".git").join("objects");
    let b = repo_with_shared_objects(&root, "b", &objs);
    let c = repo_with_shared_objects(&root, "c", &objs);
    let res = scan_with(&root, network_everywhere());
    for r in [&a, &b, &c] {
        assert_eq!(facts_of(&res, r).no_dependents, Tri::No, "{}", r.display());
        assert_eq!(dependents(&res, r).len(), 2, "{}", r.display());
    }
}

#[test]
fn m1_an_objects_symlink_to_a_place_outside_the_scan_makes_every_repo_ask() {
    let (tmp, root, remotes) = workspace();
    let other = pushed_repo(&root, &remotes, "other");
    let ext = tmp.path().join("ext");
    init_repo(&ext);
    let _b = repo_with_shared_objects(&root, "b", &ext.join(".git").join("objects"));
    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &other).no_dependents, Tri::Unknown);
    assert_eq!(action_for(&res, &other), Action::Ask);
}

#[test]
fn m1_a_symlinked_objects_info_directory_into_another_repo_is_a_dependency() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let b = root.join("b");
    fs::create_dir_all(&b).unwrap();
    git(&b, &["init", "-q", "-b", "main"]);
    let info = b.join(".git").join("objects").join("info");
    fs::remove_dir_all(&info).unwrap();
    symlink(a.join(".git").join("objects").join("info"), &info).unwrap();
    let res = scan_with(&root, network_everywhere());
    assert!(
        not_removing(action_for(&res, &a)),
        "{:?}",
        action_for(&res, &a)
    );
}

#[test]
fn m1_a_commondir_file_pointing_at_another_repo_is_a_dependency() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let b = root.join("b");
    fs::create_dir_all(&b).unwrap();
    git(&b, &["init", "-q", "-b", "main"]);
    write(&b.join(".git").join("commondir"), "../../a/.git\n");
    let res = scan_with(&root, network_everywhere());
    assert!(
        not_removing(action_for(&res, &a)),
        "{:?}",
        action_for(&res, &a)
    );
}

#[test]
fn m1_control_two_unrelated_repos_are_unaffected() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let b = pushed_repo(&root, &remotes, "b");
    let res = scan_with(&root, network_everywhere());
    assert_eq!(action_for(&res, &a), Action::BackupThenRemove);
    assert_eq!(action_for(&res, &b), Action::BackupThenRemove);
}

// ---------------------------------------------------------------- M2

/// `git clone --shared` of `provider` into `root/name`; returns the borrower and the path of its
/// alternates file.
fn shared_clone(root: &Path, provider: &Path, name: &str) -> (PathBuf, PathBuf) {
    let b = root.join(name);
    git(
        root,
        &[
            "clone",
            "-q",
            "--shared",
            provider.to_str().unwrap(),
            b.to_str().unwrap(),
        ],
    );
    let alt = b
        .join(".git")
        .join("objects")
        .join("info")
        .join("alternates");
    assert!(alt.exists(), "the clone must have an alternates file");
    (b, alt)
}

/// The object must still be readable by the borrower: the rewritten file is live.
fn assert_borrower_reads_provider_objects(borrower: &Path, provider: &Path) {
    let head = git(provider, &["rev-parse", "HEAD"]);
    git(borrower, &["cat-file", "-e", &head]);
}

fn c_quote(p: &str) -> String {
    let mut out = String::from("\"");
    for &b in p.as_bytes() {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out.push('"');
    out
}

#[test]
fn m2_a_quoted_alternates_entry_with_a_space_and_a_non_ascii_name_is_followed() {
    let (_tmp, root, remotes) = workspace();
    let prov = pushed_repo(&root, &remotes, "pro vider \u{e9}");
    let (b, alt) = shared_clone(&root, &prov, "borrower");
    let objs = fs::canonicalize(prov.join(".git").join("objects")).unwrap();
    fs::write(&alt, format!("{}\n", c_quote(objs.to_str().unwrap()))).unwrap();
    assert!(fs::read_to_string(&alt).unwrap().contains("\\303\\251"));
    assert_borrower_reads_provider_objects(&b, &prov);
    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &prov).no_dependents, Tri::No);
    assert!(not_removing(action_for(&res, &prov)));
}

#[test]
fn m2_a_relative_alternates_entry_is_resolved_against_the_objects_directory() {
    let (_tmp, root, remotes) = workspace();
    let prov = pushed_repo(&root, &remotes, "prov");
    let (b, alt) = shared_clone(&root, &prov, "borrower");
    // objects dir = <root>/borrower/.git/objects; three levels up is <root>.
    fs::write(&alt, "../../../prov/.git/objects\n").unwrap();
    assert_borrower_reads_provider_objects(&b, &prov);
    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &prov).no_dependents, Tri::No);
}

#[test]
fn m2_comment_and_blank_lines_are_ignored() {
    let (_tmp, root, remotes) = workspace();
    let prov = pushed_repo(&root, &remotes, "prov");
    let (b, alt) = shared_clone(&root, &prov, "borrower");
    let objs = fs::canonicalize(prov.join(".git").join("objects")).unwrap();
    fs::write(&alt, format!("# a comment\n\n{}\n\n", objs.display())).unwrap();
    assert_borrower_reads_provider_objects(&b, &prov);
    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &prov).no_dependents, Tri::No);
}

#[test]
fn m2_a_malformed_quoted_entry_makes_every_repo_unknown() {
    let (_tmp, root, remotes) = workspace();
    let prov = pushed_repo(&root, &remotes, "prov");
    let other = pushed_repo(&root, &remotes, "other");
    let (_b, alt) = shared_clone(&root, &prov, "borrower");
    fs::write(&alt, "\"/unterminated\\q\n").unwrap();
    let res = scan_with(&root, network_everywhere());
    for r in [&prov, &other] {
        assert_eq!(
            facts_of(&res, r).no_dependents,
            Tri::Unknown,
            "{}",
            r.display()
        );
        assert_eq!(action_for(&res, r), Action::Ask);
    }
}

#[test]
fn m2_an_entry_outside_the_scan_makes_every_repo_unknown() {
    let (tmp, root, remotes) = workspace();
    let other = pushed_repo(&root, &remotes, "other");
    let ext = tmp.path().join("ext");
    init_repo(&ext);
    let (_b, _alt) = shared_clone(&root, &ext, "borrower");
    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &other).no_dependents, Tri::Unknown);
    assert_eq!(action_for(&res, &other), Action::Ask);
}

#[test]
fn m2_chained_alternates_make_the_last_provider_report_every_borrower() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let (b, _) = shared_clone(&root, &a, "b");
    let (c, _) = shared_clone(&root, &b, "c");
    let res = scan_with(&root, network_everywhere());
    let d = dependents(&res, &a);
    assert_eq!(d.len(), 2, "{d:?}");
    assert!(d.iter().any(|x| x.ends_with("/c")));
    assert_eq!(facts_of(&res, &a).no_dependents, Tri::No);
    assert_eq!(facts_of(&res, &c).no_dependents, Tri::Yes);
}

// ---------------------------------------------------------------- round 16: every git directory
//
// Dependency discovery must not depend on how the borrower is classified: a `commondir`,
// alternates file, `.git` file or symlinked object store counts wherever the git directory that
// holds it lies (a repository's `.git`, a bare repository, a git directory that is no node at
// all, one inside another repository's git directory, inside an artifact, or in a separately
// walked root). Each fixture first shows that git itself reads the provider's history through
// the borrower.

/// The provider keeps a known dependent (or an unknown one) and is offered no removal.
fn assert_held(res: &disk_cleaner_core::ScanResult, a: &Path) {
    let info = node(res, a).repo.as_ref().unwrap();
    assert_ne!(
        info.facts.no_dependents,
        Tri::Yes,
        "{}: {:?}",
        a.display(),
        info.notes
    );
    assert!(not_removing(action_for(res, a)), "{:?}", action_for(res, a));
}

/// git reads `a`'s newest commit through the git directory `gitdir`.
fn assert_reads_history(gitdir: &Path, a: &Path) {
    let want = git(a, &["rev-parse", "HEAD"]);
    let got = git(
        a,
        &[
            "--git-dir",
            gitdir.to_str().unwrap(),
            "log",
            "--format=%H",
            "-1",
        ],
    );
    assert_eq!(
        got,
        want,
        "{} must read the history of {}",
        gitdir.display(),
        a.display()
    );
}

/// Make `dir` a git directory that keeps its own HEAD and takes refs and objects from `a`
/// through a `commondir` file holding `rel`. `bare` starts from `git init --bare` (its own empty
/// `objects` and `refs`, so the scan classifies it as a bare repository); otherwise the directory
/// holds only `HEAD` and `commondir`, which git accepts but which is no node.
fn commondir_borrower(dir: &Path, rel: &str, a: &Path, bare: bool) {
    fs::create_dir_all(dir).unwrap();
    if bare {
        git(dir, &["init", "-q", "--bare", "-b", "main"]);
    }
    fs::copy(a.join(".git").join("HEAD"), dir.join("HEAD")).unwrap();
    write(&dir.join("commondir"), &format!("{rel}\n"));
    assert_reads_history(dir, a);
}

fn scan_roots(roots: Vec<PathBuf>) -> disk_cleaner_core::ScanResult {
    let mut opts = disk_cleaner_core::ScanOptions::new(roots);
    opts.facts = network_everywhere();
    opts.git_jobs = 2;
    disk_cleaner_core::scan(&opts).expect("scan")
}

/// A bare repository beside `a` whose `commondir` names `a`'s git directory.
#[test]
fn r16_a_bare_repository_whose_commondir_is_another_repo_holds_it() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let b = root.join("b.git");
    commondir_borrower(&b, "../a/.git", &a, true);
    let res = scan_with(&root, network_everywhere());
    assert!(
        has_node(&res, &b),
        "b.git is classified as a bare repository"
    );
    assert_held(&res, &a);
}

/// A git directory with only `HEAD` and `commondir` is no node at all, and still a borrower.
#[test]
fn r16_a_git_directory_that_is_no_node_whose_commondir_is_another_repo_holds_it() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let m = root.join("m");
    commondir_borrower(&m, "../a/.git", &a, false);
    let res = scan_with(&root, network_everywhere());
    assert_held(&res, &a);
}

/// The same bare borrower inside an artifact directory (node_modules, target).
#[test]
fn r16_a_bare_borrower_inside_an_artifact_holds_its_provider() {
    for (project, marker, artifact) in [
        ("js", "package.json", ["node_modules", "x"]),
        ("rs", "Cargo.toml", ["target", "vendor"]),
    ] {
        let (_tmp, root, remotes) = workspace();
        let a = pushed_repo(&root, &remotes, "a");
        let proj = root.join(project);
        write(&proj.join(marker), "x\n");
        let b = proj.join(artifact[0]).join(artifact[1]).join("b.git");
        commondir_borrower(&b, "../../../../a/.git", &a, true);
        let res = scan_with(&root, network_everywhere());
        assert_held(&res, &a);
        assert!(not_removing(action_for(&res, &proj.join(artifact[0]))));
    }
}

/// A repository's `.git` inside an artifact whose `commondir` names `a` (a guard).
#[test]
fn r16_guard_a_repository_inside_an_artifact_with_a_commondir_holds_its_provider() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let js = root.join("js");
    write(&js.join("package.json"), "{}\n");
    let x = js.join("node_modules").join("x");
    fs::create_dir_all(&x).unwrap();
    git(&x, &["init", "-q", "-b", "main"]);
    let gd = x.join(".git");
    fs::copy(a.join(".git").join("HEAD"), gd.join("HEAD")).unwrap();
    write(&gd.join("commondir"), "../../../../a/.git\n");
    assert_reads_history(&gd, &a);
    let res = scan_with(&root, network_everywhere());
    assert_held(&res, &a);
}

/// A git directory inside another repository's git directory (`c/.git/modules/x`).
#[test]
fn r16_a_git_directory_inside_another_repos_git_directory_holds_its_provider() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let c = root.join("c");
    init_repo(&c);
    let x = c.join(".git").join("modules").join("x");
    commondir_borrower(&x, "../../../../a/.git", &a, false);
    let res = scan_with(&root, network_everywhere());
    assert_held(&res, &a);
}

/// A bare borrower in a scan root that is walked on its own (nested in another root).
#[test]
fn r16_a_bare_borrower_in_a_separately_walked_root_holds_its_provider() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let sub = root.join("sub");
    let b = sub.join("b.git");
    commondir_borrower(&b, "../../a/.git", &a, true);
    let res = scan_roots(vec![root.clone(), sub]);
    assert_held(&res, &a);
}

/// A `commondir` that cannot be read leaves every repository's dependents unknown.
#[test]
fn r16_an_unreadable_commondir_in_a_bare_directory_makes_every_provider_unknown() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let b = root.join("b.git");
    commondir_borrower(&b, "../a/.git", &a, true);
    let cd = b.join("commondir");
    fs::set_permissions(&cd, fs::Permissions::from_mode(0o000)).unwrap();
    // Running as root, the mode does not stop the read and this case cannot be built: CI runs
    // as an ordinary user, so there it must hold.
    assert!(
        fs::read(&cd).is_err() || std::env::var_os("CI").is_none(),
        "the commondir must be unreadable on CI"
    );
    let unreadable = fs::read(&cd).is_err();
    let res = scan_with(&root, network_everywhere());
    fs::set_permissions(&cd, fs::Permissions::from_mode(0o644)).unwrap();
    if unreadable {
        assert_held(&res, &a);
    }
}

/// A `commondir` that is a directory, or names nothing, is not a reference that can be placed.
#[test]
fn r16_a_commondir_that_cannot_be_resolved_makes_every_provider_unknown() {
    for broken in ["dir", "missing"] {
        let (_tmp, root, remotes) = workspace();
        let a = pushed_repo(&root, &remotes, "a");
        let b = root.join("b.git");
        fs::create_dir_all(&b).unwrap();
        git(&b, &["init", "-q", "--bare", "-b", "main"]);
        let cd = b.join("commondir");
        match broken {
            "dir" => fs::create_dir_all(&cd).unwrap(),
            _ => write(&cd, "../no-such-repo/.git\n"),
        }
        let res = scan_with(&root, network_everywhere());
        assert_eq!(facts_of(&res, &a).no_dependents, Tri::Unknown, "{broken}");
        assert!(not_removing(action_for(&res, &a)), "{broken}");
    }
}

/// `git clone --bare --shared`: alternates in a bare borrower, beside `a` and inside an
/// artifact (already found: a guard).
#[test]
fn r16_guard_a_bare_shared_clone_holds_its_provider() {
    for nested in [false, true] {
        let (_tmp, root, remotes) = workspace();
        let a = pushed_repo(&root, &remotes, "a");
        let b = if nested {
            let js = root.join("js");
            write(&js.join("package.json"), "{}\n");
            js.join("node_modules").join("x").join("b.git")
        } else {
            root.join("b.git")
        };
        fs::create_dir_all(b.parent().unwrap()).unwrap();
        git(
            &root,
            &[
                "clone",
                "-q",
                "--bare",
                "--shared",
                a.to_str().unwrap(),
                b.to_str().unwrap(),
            ],
        );
        assert!(b.join("objects").join("info").join("alternates").exists());
        assert_reads_history(&b, &a);
        let res = scan_with(&root, network_everywhere());
        assert_held(&res, &a);
    }
}

/// A shared clone in a separately walked root (a guard).
#[test]
fn r16_guard_a_shared_clone_in_a_separately_walked_root_holds_its_provider() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let sub = root.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let (b, _) = shared_clone(&sub, &a, "b");
    assert_borrower_reads_provider_objects(&b, &a);
    let res = scan_roots(vec![root.clone(), sub]);
    assert_held(&res, &a);
}

/// `objects/info/http-alternates` in a bare borrower names no place in the scan (a guard).
#[test]
fn r16_guard_http_alternates_make_every_provider_unknown() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let b = root.join("b.git");
    fs::create_dir_all(&b).unwrap();
    git(&b, &["init", "-q", "--bare", "-b", "main"]);
    write(
        &b.join("objects").join("info").join("http-alternates"),
        "https://example.invalid/a.git/objects\n",
    );
    let res = scan_with(&root, network_everywhere());
    assert_held(&res, &a);
}

/// A bare-style git directory whose `objects` is a symlink into `a` (so it is not shaped like a
/// bare repository and is no node): beside `a`, inside another repository's git directory, and
/// inside an artifact (guards). A repository's symlinked `objects` is covered by M1 above.
#[test]
fn r16_guard_a_symlinked_object_store_holds_its_provider_in_every_kind_of_git_directory() {
    for kind in ["no-node", "in-git-dir", "in-artifact"] {
        let (_tmp, root, remotes) = workspace();
        let a = pushed_repo(&root, &remotes, "a");
        let dir = match kind {
            "no-node" => root.join("m"),
            "in-git-dir" => {
                let c = root.join("c");
                init_repo(&c);
                c.join(".git").join("modules").join("x")
            }
            _ => {
                let js = root.join("js");
                write(&js.join("package.json"), "{}\n");
                js.join("node_modules").join("x").join("b.git")
            }
        };
        fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "--bare", "-b", "main"]);
        fs::copy(a.join(".git").join("HEAD"), dir.join("HEAD")).unwrap();
        let objs = dir.join("objects");
        fs::remove_dir_all(&objs).unwrap();
        symlink(a.join(".git").join("objects"), &objs).unwrap();
        let head = git(&a, &["rev-parse", "HEAD"]);
        git(
            &a,
            &["--git-dir", dir.to_str().unwrap(), "cat-file", "-e", &head],
        );
        let res = scan_with(&root, network_everywhere());
        assert_held(&res, &a);
    }
}

/// A checkout's `.git` file that names a git directory whose `commondir` is `a`: as a scanned
/// checkout, and inside an artifact (guards; the named directory's `commondir` is also checked
/// on its own as a git directory that is no node).
#[test]
fn r16_guard_a_dot_git_file_through_a_commondir_holds_its_provider() {
    for nested in [false, true] {
        let (_tmp, root, remotes) = workspace();
        let a = pushed_repo(&root, &remotes, "a");
        let g = root.join("g");
        commondir_borrower(&g, "../a/.git", &a, false);
        let checkout = if nested {
            let js = root.join("js");
            write(&js.join("package.json"), "{}\n");
            js.join("node_modules").join("x")
        } else {
            root.join("w")
        };
        write(
            &checkout.join(".git"),
            &format!("gitdir: {}\n", g.to_str().unwrap()),
        );
        let res = scan_with(&root, network_everywhere());
        assert_held(&res, &a);
    }
}

/// A linked worktree outside the scan: the back-link lives in the provider's own git directory,
/// whose non-empty `worktrees` makes it ask (a guard).
#[test]
fn r16_guard_a_worktree_outside_the_scan_keeps_its_provider() {
    let (tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let wt = tmp.path().join("outside-wt");
    git(
        &a,
        &["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()],
    );
    let res = scan_with(&root, network_everywhere());
    assert!(not_removing(action_for(&res, &a)));
}

/// A bare repository (real `objects`, so it is a node) whose `objects/pack` is a symlink to
/// `a`'s packs (a guard).
#[test]
fn r16_guard_a_bare_repository_with_symlinked_packs_holds_its_provider() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    git(&a, &["gc", "-q"]);
    let b = root.join("b.git");
    fs::create_dir_all(&b).unwrap();
    git(&b, &["init", "-q", "--bare", "-b", "main"]);
    let pack = b.join("objects").join("pack");
    fs::remove_dir_all(&pack).unwrap();
    symlink(a.join(".git").join("objects").join("pack"), &pack).unwrap();
    let head = git(&a, &["rev-parse", "HEAD"]);
    git(
        &a,
        &["--git-dir", b.to_str().unwrap(), "cat-file", "-e", &head],
    );
    let res = scan_with(&root, network_everywhere());
    assert!(has_node(&res, &b));
    assert_held(&res, &a);
}

/// In a separately walked root: a git directory whose `objects` is a symlink into `a`, and a
/// checkout whose `.git` file names `a`'s git directory (guards).
#[test]
fn r16_guard_symlink_and_dot_git_file_borrowers_in_a_separately_walked_root() {
    for kind in ["symlink", "dot-git-file"] {
        let (_tmp, root, remotes) = workspace();
        let a = pushed_repo(&root, &remotes, "a");
        let sub = root.join("sub");
        if kind == "symlink" {
            let m = sub.join("m");
            fs::create_dir_all(&m).unwrap();
            git(&m, &["init", "-q", "--bare", "-b", "main"]);
            let objs = m.join("objects");
            fs::remove_dir_all(&objs).unwrap();
            symlink(a.join(".git").join("objects"), &objs).unwrap();
        } else {
            // Named straight at `a`'s git directory, so only the `.git` file refers to `a`.
            let gd = a.join(".git");
            write(
                &sub.join("w").join(".git"),
                &format!("gitdir: {}\n", gd.to_str().unwrap()),
            );
        }
        let res = scan_roots(vec![root.clone(), sub]);
        assert_held(&res, &a);
    }
}

/// A `.git` file that is no scanned checkout: one inside another repository's git directory is
/// followed like any other (added after the fix: it was not run red), and one inside an
/// artifact that names nothing makes every provider unknown (a guard).
#[test]
fn r16_dot_git_files_that_are_no_checkout_are_followed_or_unresolved() {
    for kind in ["in-git-dir", "dangling-in-artifact"] {
        let (_tmp, root, remotes) = workspace();
        let a = pushed_repo(&root, &remotes, "a");
        if kind == "in-git-dir" {
            // The `.git` file names `a`'s git directory itself, so nothing else refers to `a`.
            let c = root.join("c");
            init_repo(&c);
            let gd = a.join(".git");
            write(
                &c.join(".git").join("stray").join(".git"),
                &format!("gitdir: {}\n", gd.to_str().unwrap()),
            );
            let res = scan_with(&root, network_everywhere());
            assert_held(&res, &a);
        } else {
            let js = root.join("js");
            write(&js.join("package.json"), "{}\n");
            write(
                &js.join("node_modules").join("x").join(".git"),
                "gitdir: ../no-such-git-dir\n",
            );
            let res = scan_with(&root, network_everywhere());
            assert_eq!(facts_of(&res, &a).no_dependents, Tri::Unknown);
            assert!(not_removing(action_for(&res, &a)));
        }
    }
}

// Git accepts a `HEAD` that is a symbolic link into `refs/`, so a git directory is not
// recognised by a regular-file `HEAD` alone.

/// Make `dir` a git directory holding only a `HEAD` symlink (`refs/heads/main`) and a `commondir`
/// file holding `rel`, then show that git reads `a`'s history through it.
fn symlinked_head_borrower(dir: &Path, rel: &str, a: &Path) {
    fs::create_dir_all(dir).unwrap();
    symlink("refs/heads/main", dir.join("HEAD")).unwrap();
    write(&dir.join("commondir"), &format!("{rel}\n"));
    assert_reads_history(dir, a);
}

/// A git directory beside `a` whose `HEAD` is a symlink and whose `commondir` names `a`.
#[test]
fn r16b_a_git_directory_with_a_symlinked_head_whose_commondir_is_another_repo_holds_it() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    symlinked_head_borrower(&root.join("m"), "../a/.git", &a);
    let res = scan_with(&root, network_everywhere());
    assert_held(&res, &a);
}

/// A `.git` directory inside an artifact whose `HEAD` is a symlink and whose `commondir` names
/// `a` (the round 15 build checked every `.git` inside an artifact, whatever its `HEAD`).
#[test]
fn r16b_a_dot_git_inside_an_artifact_with_a_symlinked_head_holds_its_provider() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let js = root.join("js");
    write(&js.join("package.json"), "{}\n");
    let gd = js.join("node_modules").join("x").join(".git");
    symlinked_head_borrower(&gd, "../../../../a/.git", &a);
    let res = scan_with(&root, network_everywhere());
    assert_held(&res, &a);
    assert!(not_removing(action_for(&res, &js.join("node_modules"))));
}

/// A scanned checkout made with `git init --separate-git-dir` whose git directory (its own
/// objects and refs, its only copy of its history) lies inside `a`'s git directory: removing `a`
/// removes that history.
#[test]
fn r16b_a_checkout_whose_git_directory_lies_inside_another_repos_git_directory_holds_it() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let w = root.join("w");
    let sep = a.join(".git").join("sub");
    git(
        &root,
        &[
            "init",
            "-q",
            "-b",
            "main",
            "--separate-git-dir",
            sep.to_str().unwrap(),
            w.to_str().unwrap(),
        ],
    );
    git(&w, &["config", "commit.gpgsign", "false"]);
    commit_file(&w, "w.txt", "only here\n");
    assert!(w.join(".git").is_file(), "w must use a .git file");
    let head = git(&w, &["rev-parse", "HEAD"]);
    git(
        &a,
        &["--git-dir", sep.to_str().unwrap(), "cat-file", "-e", &head],
    );
    let res = scan_with(&root, network_everywhere());
    assert!(has_node(&res, &w), "w is a scanned checkout");
    assert_held(&res, &a);
}

/// A scanned checkout whose `.git` file names a git directory outside the scan, whose `commondir`
/// names a repository kept inside `a`'s git directory (`a/.git/sub`, its own objects and refs):
/// the checkout's commits live only there.
#[test]
fn r16b_a_checkout_whose_object_store_lies_inside_another_repos_git_directory_holds_it() {
    let (tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let common = a.join(".git").join("sub");
    init_bare(&common);
    let gd = tmp.path().join("outside-gitdir");
    write(&gd.join("HEAD"), "ref: refs/heads/main\n");
    write(
        &gd.join("commondir"),
        &format!("{}\n", common.to_str().unwrap()),
    );
    let w = root.join("w");
    write(
        &w.join(".git"),
        &format!("gitdir: {}\n", gd.to_str().unwrap()),
    );
    write(&w.join("w.txt"), "only here\n");
    git(&w, &["add", "w.txt"]);
    git(
        &w,
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "w"],
    );
    let head = git(&w, &["rev-parse", "HEAD"]);
    git(
        &a,
        &[
            "--git-dir",
            common.to_str().unwrap(),
            "cat-file",
            "-e",
            &head,
        ],
    );
    let res = scan_with(&root, network_everywhere());
    assert!(has_node(&res, &w), "w is a scanned checkout");
    assert_held(&res, &a);
}

/// A `.git` directory inside an artifact holding only a `commondir` that names `a`. Git does not
/// read it as a repository (it has no `HEAD`), so this hold is conservative; it matches the round
/// 15 build, which checked every `.git` inside an artifact (a guard for the `.git` name rule).
#[test]
fn r16b_guard_a_dot_git_with_only_a_commondir_inside_an_artifact_holds_its_provider() {
    let (_tmp, root, remotes) = workspace();
    let a = pushed_repo(&root, &remotes, "a");
    let js = root.join("js");
    write(&js.join("package.json"), "{}\n");
    let gd = js.join("node_modules").join("x").join(".git");
    write(&gd.join("commondir"), "../../../../a/.git\n");
    let res = scan_with(&root, network_everywhere());
    assert_held(&res, &a);
}

// ---------------------------------------------------------------- round 16: controls

#[test]
fn r16_control_a_alone_and_beside_an_unrelated_bare_repository_is_backed_up_then_removed() {
    for with_bare in [false, true] {
        let (_tmp, root, remotes) = workspace();
        let a = pushed_repo(&root, &remotes, "a");
        if with_bare {
            let c = root.join("c.git");
            init_bare(&c);
            let src = root.join("c-src");
            init_repo(&src);
            push_to_remote(&src, &c);
            fs::remove_dir_all(&src).unwrap();
        }
        let res = scan_with(&root, network_everywhere());
        assert_eq!(facts_of(&res, &a).no_dependents, Tri::Yes, "{with_bare}");
        assert_eq!(
            action_for(&res, &a),
            Action::BackupThenRemove,
            "{with_bare}"
        );
    }
}

/// A submodule checkout inside its parent: its git directory lies in the parent's
/// `.git/modules`, but the checkout goes with the parent, so the parent keeps no dependent.
#[test]
fn r16b_control_a_submodule_checkout_inside_its_parent_is_no_dependent() {
    let (tmp, root, remotes) = workspace();
    let srcs = tmp.path().join("srcs");
    fs::create_dir_all(&srcs).unwrap();
    let subsrc = pushed_repo(&srcs, &remotes, "subsrc");
    let outer = pushed_repo(&root, &remotes, "outer");
    git(
        &outer,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            subsrc.to_str().unwrap(),
            "sub",
        ],
    );
    git(&outer, &["commit", "-q", "-m", "add submodule"]);
    git(&outer, &["push", "-q", "origin", "main"]);
    assert!(outer.join("sub").join(".git").is_file());
    let res = scan_with(&root, network_everywhere());
    assert_eq!(facts_of(&res, &outer).no_dependents, Tri::Yes);
}
