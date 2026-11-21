//! Regression tests for GoodQA #16418: `extensions.worktreeConfig` without a `config.worktree`.
//!
//! git creates `config.worktree` only when something is first written with `--worktree`, so a
//! repository with the extension on and no such file is normal. The config allowlist ran
//! `git config --worktree --list`, which exits 128 when the file does not exist; the repository
//! then had every fact unknown (71 repos and worktrees on the first real run). A missing file is
//! no worktree-scoped keys. A file that exists is read and judged as before, and one that is not
//! a regular file (a symlink, a directory) is refused, as `.git/modules` configs are.
//!
//! Accepting a missing file must not let git read a git directory other than the scanned one:
//! a directory git rejects as a repository makes it walk up to the next one. For a work tree
//! `--show-toplevel` catches that; for a bare repo the git directory git uses must be the scanned
//! directory. Every fixture is built with real git commands.

mod common;

use common::*;
use disk_cleaner_core::{Class, ScanResult, Tri};
use std::fs;
use std::path::{Path, PathBuf};

/// The refusal of a `config.worktree` that is a symlink or another kind of entry git's own start
/// does not already reject (a directory it does: see [`LISTING_FAILED`]).
const NOT_REGULAR: &str = "config.worktree is not a regular file";

/// The refusal when this git directory has no `config.worktree` and git still lists keys of the
/// worktree scope.
const UNJUDGED: &str = "git would read worktree-scoped config that was not judged (not this git \
                        directory's config.worktree); no other git command was run in this repo";

/// The refusal when git uses another git directory than the scanned bare repo.
const OTHER_GIT_DIR: &str =
    "git's git directory is not the scanned directory; facts would describe another repository";

/// The refusal of an unsafe key whose label (section and variable) is `label`.
fn unsafe_key(label: &str) -> String {
    format!(
        "repo config can run programs or is not understood ({label}); no git command was run in \
         this repo"
    )
}

fn notes(res: &ScanResult, repo: &Path) -> Vec<String> {
    node(res, repo)
        .repo
        .as_ref()
        .expect("repo info")
        .notes
        .clone()
}

/// The config allowlist let git run: no note is a config refusal or a failed config read.
fn assert_not_refused(res: &ScanResult, repo: &Path) {
    let notes = notes(res, repo);
    for n in &notes {
        assert!(
            n != NOT_REGULAR
                && n != UNJUDGED
                && n != OTHER_GIT_DIR
                && !n.contains("unable to read")
                && !n.contains("can run programs"),
            "{}: no config refusal expected: {notes:?}",
            repo.display()
        );
    }
    let f = facts_of(res, repo);
    assert_eq!(
        f.has_network_remote,
        Tri::Yes,
        "{}: {notes:?}",
        repo.display()
    );
}

/// As [`assert_not_refused`], and the facts that need a work tree and git are known.
fn assert_inspected(res: &ScanResult, repo: &Path) {
    assert_not_refused(res, repo);
    let f = facts_of(res, repo);
    let notes = notes(res, repo);
    assert_eq!(f.head_on_remote, Tri::Yes, "{}: {notes:?}", repo.display());
    assert_eq!(f.tree_clean, Tri::Yes, "{}: {notes:?}", repo.display());
}

/// Every fact is unknown (`plain_layout` may be `no` when `layout_no` is set) and one note is
/// exactly `want`.
fn refused(res: &ScanResult, repo: &Path, want: &str, exact: bool, layout_no: bool) {
    let f = facts_of(res, repo);
    let notes = notes(res, repo);
    let unknown = |(name, t): &(&str, Tri)| {
        *t == Tri::Unknown || (layout_no && *name == "plain_layout" && *t == Tri::No)
    };
    assert!(
        f.entries().iter().all(unknown),
        "{}: every fact must be unknown: {f:?} ({notes:?})",
        repo.display()
    );
    let hit = |n: &String| {
        if exact {
            n == want
        } else {
            n.starts_with(want)
        }
    };
    assert!(
        notes.iter().any(hit),
        "{}: no note {} {want:?}: {notes:?}",
        repo.display(),
        if exact { "is" } else { "starts with" }
    );
}

fn assert_refused(res: &ScanResult, repo: &Path, want: &str) {
    refused(res, repo, want, true, false);
}

/// For a symlink in the git directory: the scan's own walk sets `plain_layout` to `no` whatever
/// the config check found, and a `no` never offers anything.
#[cfg(unix)]
fn assert_refused_symlink(res: &ScanResult, repo: &Path, want: &str) {
    refused(res, repo, want, true, true);
}

/// A pushed repo `app` with `extensions.worktreeConfig` on (through `git config`, as a person
/// would) and no `config.worktree`.
fn extension_repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    git(&repo, &["config", "extensions.worktreeConfig", "true"]);
    assert!(!repo.join(".git").join("config.worktree").exists());
    (tmp, root, repo)
}

/// A linked worktree `wt` of `repo`, detached at the pushed commit; returns it and its own git
/// directory.
fn linked(root: &Path, repo: &Path) -> (PathBuf, PathBuf) {
    let wt = root.join("wt");
    git(
        repo,
        &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()],
    );
    let gd = repo.join(".git").join("worktrees").join("wt");
    assert!(gd.join("HEAD").is_file(), "{}", gd.display());
    (wt, gd)
}

/// Append raw text to a config file.
fn append(file: &Path, text: &str) {
    let mut s = fs::read_to_string(file).unwrap_or_default();
    s.push_str(text);
    fs::write(file, s).unwrap();
}

/// End the `.git` file of `wt` with CRLF, as an editor on Windows may save it.
fn crlf_git_file(wt: &Path) {
    let file = wt.join(".git");
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("gitdir: "), "{text:?}");
    fs::write(&file, format!("{}\r\n", text.trim_end())).unwrap();
}

/// A directory with what makes the scan class it a bare repo (`HEAD`, `config`, `refs`,
/// `objects`), whose `HEAD` holds `head`.
fn bare_shaped(dir: &Path, head: &str) {
    for d in ["refs/heads", "objects/info", "objects/pack"] {
        fs::create_dir_all(dir.join(d)).unwrap();
    }
    fs::write(dir.join("HEAD"), head).unwrap();
    fs::write(dir.join("config"), "[core]\n\tbare = true\n").unwrap();
}

// ---------------------------------------------------------------- the defect

#[test]
fn missing_worktree_config_in_the_main_worktree_means_no_worktree_keys() {
    let (_tmp, root, repo) = extension_repo();
    let res = scan_with(&root, network_everywhere());
    assert_inspected(&res, &repo);
}

#[test]
fn missing_worktree_config_in_a_linked_worktree_means_no_worktree_keys() {
    let (_tmp, root, repo) = extension_repo();
    let (wt, gd) = linked(&root, &repo);
    assert!(!gd.join("config.worktree").exists());
    let res = scan_with(&root, network_everywhere());
    assert_inspected(&res, &wt);
    assert_inspected(&res, &repo);
}

#[test]
fn the_extension_spelled_in_another_case_with_the_file_missing_means_no_worktree_keys() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    append(
        &repo.join(".git").join("config"),
        "[Extensions]\n\tWorkTreeCONFIG = true\n",
    );
    let res = scan_with(&root, network_everywhere());
    assert_inspected(&res, &repo);
}

#[test]
fn absent_in_the_main_worktree_and_present_in_a_linked_one_reads_each_as_it_is() {
    let (_tmp, root, repo) = extension_repo();
    let (wt, gd) = linked(&root, &repo);
    git(&wt, &["config", "--worktree", "core.abbrev", "12"]);
    assert!(gd.join("config.worktree").is_file());
    assert!(!repo.join(".git").join("config.worktree").exists());
    let res = scan_with(&root, network_everywhere());
    assert_inspected(&res, &repo);
    assert_inspected(&res, &wt);
}

/// A real bare repo `root/mirror.git` (a bare clone of a pushed repo) with the extension on and no
/// `config.worktree`.
fn bare_clone(root: &Path, remotes: &Path) -> PathBuf {
    pushed_repo(remotes, remotes, "src");
    let bare = root.join("mirror.git");
    git(
        root,
        &[
            "clone",
            "-q",
            "--bare",
            remotes.join("src.git").to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    git(&bare, &["config", "extensions.worktreeConfig", "true"]);
    assert!(!bare.join("config.worktree").exists());
    bare
}

/// The one node named `mirror.git`, whatever spelling of the scan root it was reported under.
fn mirror_node(res: &ScanResult) -> PathBuf {
    let found: Vec<&str> = res
        .nodes
        .iter()
        .map(|n| n.path.as_str())
        .filter(|p| p.ends_with("mirror.git"))
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    PathBuf::from(found[0])
}

/// A real bare repo (a bare clone) with the extension and no file is inspected.
#[test]
fn control_a_real_bare_repo_with_the_extension_and_no_file_is_inspected() {
    let (_tmp, root, remotes) = workspace();
    let bare = bare_clone(&root, &remotes);
    let res = scan_with(&root, network_everywhere());
    assert_eq!(node(&res, &bare).class, Class::BareRepo);
    assert_not_refused(&res, &bare);
}

/// Is the file system holding `dir` case-insensitive? Found by a probe file, not by OS name.
fn case_insensitive(dir: &Path) -> bool {
    let probe = dir.join("Case-Probe");
    fs::write(&probe, "").unwrap();
    let other = fs::symlink_metadata(dir.join("cASE-pROBE")).is_ok();
    fs::remove_file(&probe).unwrap();
    other
}

/// Gate guard: where the file system ignores case, a scan root spelled in the wrong case reaches
/// the same bare repo; git's git directory and the scanned one are the same directory, so the
/// git-directory check must not refuse it. Skipped on a case-sensitive file system, where the
/// wrong spelling names nothing.
#[test]
fn a_bare_repo_under_a_wrong_case_scan_root_is_inspected_where_case_is_ignored() {
    let (tmp, root, remotes) = workspace();
    // CI runs this test again with `--nocapture` so the log shows which branch was taken.
    if !case_insensitive(tmp.path()) {
        eprintln!("wrong-case guard skipped: the file system is case-sensitive");
        return;
    }
    eprintln!("wrong-case guard ran: the file system ignores case");
    bare_clone(&root, &remotes);
    let name = root.file_name().unwrap().to_str().unwrap();
    let wrong = tmp.path().join(name.to_uppercase());
    assert_ne!(wrong, root);
    assert!(wrong.is_dir());
    let res = scan_with(&wrong, network_everywhere());
    let bare = mirror_node(&res);
    assert_eq!(node(&res, &bare).class, Class::BareRepo);
    assert_not_refused(&res, &bare);
}

/// A pushed `outer` repo with a submodule checkout `sub` (whose `.git` file names
/// `../.git/modules/sub`), committed and pushed.
fn with_submodule(tmp: &Path, root: &Path, remotes: &Path) -> (PathBuf, PathBuf) {
    let srcs = tmp.join("srcs");
    fs::create_dir_all(&srcs).unwrap();
    let subsrc = pushed_repo(&srcs, remotes, "subsrc");
    let outer = pushed_repo(root, remotes, "outer");
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
    let sub = outer.join("sub");
    (outer, sub)
}

/// A submodule checkout's `.git` file names its git directory relatively
/// (`gitdir: ../.git/modules/sub`); the outer repo checks the submodule's config to compare its
/// commit, and the checkout is its own node.
#[test]
fn a_submodule_checkout_with_a_relative_git_file_and_no_config_worktree_is_inspected() {
    let (tmp, root, remotes) = workspace();
    let (outer, sub) = with_submodule(tmp.path(), &root, &remotes);
    git(&sub, &["config", "extensions.worktreeConfig", "true"]);
    let text = fs::read_to_string(sub.join(".git")).unwrap();
    assert!(text.starts_with("gitdir: ../"), "{text:?}");
    let modgd = outer.join(".git").join("modules").join("sub");
    assert!(!modgd.join("config.worktree").exists());
    let res = scan_with(&root, network_everywhere());
    assert_eq!(
        facts_of(&res, &outer).tree_clean,
        Tri::Yes,
        "{:?}",
        notes(&res, &outer)
    );
    assert_not_refused(&res, &sub);
}

#[test]
fn a_crlf_git_file_with_no_config_worktree_is_inspected() {
    let (_tmp, root, repo) = extension_repo();
    let (wt, gd) = linked(&root, &repo);
    crlf_git_file(&wt);
    assert!(!gd.join("config.worktree").exists());
    let res = scan_with(&root, network_everywhere());
    assert_inspected(&res, &wt);
}

/// The CRLF is not part of the git directory's name: the existing file is found and read there
/// (were it looked for under a name ending in CR, it would seem missing while git lists its key).
#[test]
fn a_crlf_git_file_whose_config_worktree_exists_is_read_there() {
    let (_tmp, root, repo) = extension_repo();
    let (wt, gd) = linked(&root, &repo);
    git(&wt, &["config", "--worktree", "core.abbrev", "12"]);
    assert!(gd.join("config.worktree").is_file());
    crlf_git_file(&wt);
    let res = scan_with(&root, network_everywhere());
    assert_inspected(&res, &wt);
}

// ---------------------------------------------------------------- an existing file is still judged

#[test]
fn a_linked_worktrees_own_config_worktree_with_an_unsafe_key_is_refused() {
    let (_tmp, root, repo) = extension_repo();
    let (wt, gd) = linked(&root, &repo);
    git(&wt, &["config", "--worktree", "core.pager", "cat"]);
    assert!(gd.join("config.worktree").is_file());
    let res = scan_with(&root, network_everywhere());
    assert_refused(&res, &wt, &unsafe_key("core.pager"));
    // The main worktree does not read the linked worktree's file.
    assert_inspected(&res, &repo);
}

#[test]
fn the_extension_in_another_case_still_reads_an_existing_file() {
    let (_tmp, root, remotes) = workspace();
    let repo = pushed_repo(&root, &remotes, "app");
    append(
        &repo.join(".git").join("config"),
        "[EXTENSIONS]\n\tworktreeconfig = true\n",
    );
    fs::write(
        repo.join(".git").join("config.worktree"),
        "[core]\n\tpager = cat\n",
    )
    .unwrap();
    let res = scan_with(&root, network_everywhere());
    assert_refused(&res, &repo, &unsafe_key("core.pager"));
}

/// `core.bare` in the main worktree's file: git then sees no work tree, and the scanned
/// directory is refused as before (the note carries git's own message after this prefix).
/// (`core.worktree` there is gate3's
/// `j2_worktree_config_overriding_core_worktree_makes_every_fact_unknown`.)
#[test]
fn core_bare_in_the_main_worktrees_config_worktree_is_still_refused() {
    let (_tmp, root, repo) = extension_repo();
    git(&repo, &["config", "--worktree", "core.bare", "true"]);
    assert!(repo.join(".git").join("config.worktree").is_file());
    let res = scan_with(&root, network_everywhere());
    refused(&res, &repo, "work tree: git exited", false, false);
}

#[test]
fn control_a_safe_key_in_the_main_worktrees_config_worktree_is_read() {
    let (_tmp, root, repo) = extension_repo();
    git(&repo, &["config", "--worktree", "core.abbrev", "12"]);
    assert!(repo.join(".git").join("config.worktree").is_file());
    let res = scan_with(&root, network_everywhere());
    assert_inspected(&res, &repo);
}

// ---------------------------------------------------------------- not a regular file

/// With the extension on, git reads `config.worktree` when it starts, so a directory there makes
/// the first command, the `--local` listing, fail ("Is a directory", or "Permission denied" on
/// Windows): the repo is refused by that listing, before any other git command and before the
/// `lstat` of the file is reached.
const LISTING_FAILED: &str = "config: git exited";

#[test]
fn a_config_worktree_that_is_a_directory_is_refused() {
    let (_tmp, root, repo) = extension_repo();
    fs::create_dir(repo.join(".git").join("config.worktree")).unwrap();
    let res = scan_with(&root, network_everywhere());
    refused(&res, &repo, LISTING_FAILED, false, false);
}

#[test]
fn a_linked_worktrees_config_worktree_that_is_a_directory_is_refused() {
    let (_tmp, root, repo) = extension_repo();
    let (wt, gd) = linked(&root, &repo);
    fs::create_dir(gd.join("config.worktree")).unwrap();
    let res = scan_with(&root, network_everywhere());
    refused(&res, &wt, LISTING_FAILED, false, false);
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::symlink;

    /// A symlink to a file git would read as harmless is still not read: what it points at can
    /// change between the check and the next git command.
    #[test]
    fn a_config_worktree_that_is_a_symlink_is_refused() {
        let (tmp, root, repo) = extension_repo();
        let target = tmp.path().join("elsewhere.config");
        fs::write(&target, "[core]\n\tabbrev = 12\n").unwrap();
        symlink(&target, repo.join(".git").join("config.worktree")).unwrap();
        let res = scan_with(&root, network_everywhere());
        assert_refused_symlink(&res, &repo, NOT_REGULAR);
    }

    /// A dangling symlink is not a missing file.
    #[test]
    fn a_config_worktree_that_is_a_dangling_symlink_is_refused() {
        let (tmp, root, repo) = extension_repo();
        symlink(
            tmp.path().join("no-such-file"),
            repo.join(".git").join("config.worktree"),
        )
        .unwrap();
        let res = scan_with(&root, network_everywhere());
        assert_refused_symlink(&res, &repo, NOT_REGULAR);
    }

    #[test]
    fn a_linked_worktrees_config_worktree_that_is_a_symlink_is_refused() {
        let (tmp, root, repo) = extension_repo();
        let (wt, gd) = linked(&root, &repo);
        let target = tmp.path().join("elsewhere.config");
        fs::write(&target, "[core]\n\tabbrev = 12\n").unwrap();
        symlink(&target, gd.join("config.worktree")).unwrap();
        let res = scan_with(&root, network_everywhere());
        assert_refused_symlink(&res, &wt, NOT_REGULAR);
    }

    // ------------------------------------------------------------ a .git that is not a plain entry

    /// Gate F2: the outer repo compares each checked-out submodule's commit after checking the
    /// submodule's config. With the extension on, a `.git` that is neither a directory nor a
    /// regular file is refused when `config.worktree` is looked for (by `lstat`, not by git), and
    /// the outer repo's `tree_clean` is unknown. (A node's own odd `.git` is never given to git.)
    const ODD_DOTGIT: &str =
        "tree_clean: submodule sub: .git is neither a directory nor a regular file";

    fn assert_odd_dotgit_refused(res: &ScanResult, outer: &Path) {
        let notes = notes(res, outer);
        assert!(
            notes.iter().any(|n| n == ODD_DOTGIT),
            "no note {ODD_DOTGIT:?}: {notes:?}"
        );
        assert_eq!(facts_of(res, outer).tree_clean, Tri::Unknown, "{notes:?}");
    }

    /// git follows the symlink to the submodule's git directory and reads the extension there.
    #[test]
    fn a_submodule_checkout_whose_git_entry_is_a_symlink_is_refused() {
        let (tmp, root, remotes) = workspace();
        let (outer, sub) = with_submodule(tmp.path(), &root, &remotes);
        git(&sub, &["config", "extensions.worktreeConfig", "true"]);
        fs::remove_file(sub.join(".git")).unwrap();
        symlink(Path::new("../.git/modules/sub"), sub.join(".git")).unwrap();
        assert_eq!(
            git(&sub, &["rev-parse", "--absolute-git-dir"]),
            fs::canonicalize(outer.join(".git/modules/sub"))
                .unwrap()
                .to_str()
                .unwrap()
        );
        let res = scan_with(&root, network_everywhere());
        assert_odd_dotgit_refused(&res, &outer);
    }

    /// A FIFO `.git`: git (2.56 on CI) itself refuses it, so the submodule's first config
    /// listing fails before the `lstat`, and the refusal is git's (an older git that walked up
    /// instead would reach the outer repo's config, extension on, and the `lstat` refusal). The
    /// outer repo's `tree_clean` is unknown either way, and nothing opens the FIFO.
    #[test]
    fn a_submodule_checkout_whose_git_entry_is_a_fifo_is_refused() {
        let (tmp, root, remotes) = workspace();
        let (outer, sub) = with_submodule(tmp.path(), &root, &remotes);
        git(&outer, &["config", "extensions.worktreeConfig", "true"]);
        fs::remove_file(sub.join(".git")).unwrap();
        let status = std::process::Command::new("mkfifo")
            .arg(sub.join(".git"))
            .status()
            .unwrap();
        assert!(status.success());
        let res = scan_with(&root, network_everywhere());
        let either = |n: &String| {
            n == ODD_DOTGIT || n.starts_with("tree_clean: submodule sub: config: git exited")
        };
        let notes = notes(&res, &outer);
        assert!(notes.iter().any(either), "{notes:?}");
        assert_eq!(facts_of(&res, &outer).tree_clean, Tri::Unknown, "{notes:?}");
    }

    // ------------------------------------------------------------ a symlinked scan root

    /// Gate guard: the scan root is a symlink to the directory holding a real bare repo. git
    /// reports the real path of its git directory; that is still the scanned directory.
    #[test]
    fn a_bare_repo_under_a_symlinked_scan_root_is_inspected() {
        let (tmp, root, remotes) = workspace();
        bare_clone(&root, &remotes);
        let alias = tmp.path().join("alias");
        symlink(&root, &alias).unwrap();
        let res = scan_with(&alias, network_everywhere());
        let bare = mirror_node(&res);
        assert_eq!(node(&res, &bare).class, Class::BareRepo);
        assert_not_refused(&res, &bare);
    }

    // ------------------------------------------------------------ git older than 2.26

    /// Review M4: the cross-check uses `git config --show-scope` (git 2.26). When that listing
    /// fails the repo is refused, and the note says what the check needs in words, then git's
    /// own error.
    const OLD_GIT: &str = "git config --show-scope failed (it needs git 2.26 or newer), so \
                           whether git reads worktree-scoped config that was not judged is \
                           unknown: config: git exited";

    /// The git the tests use, by absolute path (the scanner's git runs with a scrubbed
    /// environment).
    fn real_git() -> PathBuf {
        let path = std::env::var_os("PATH").expect("PATH");
        std::env::split_paths(&path)
            .map(|d| d.join("git"))
            .find(|p| p.is_file())
            .expect("git on PATH")
    }

    /// A git that rejects `--show-scope` as git before 2.26 does, and is the real git otherwise.
    fn old_git(dir: &Path, probe_in: &Path) -> disk_cleaner_core::Git {
        let body = format!(
            "#!/bin/sh\nfor a in \"$@\"; do\n  if [ \"$a\" = --show-scope ]; then\n    \
             echo \"error: unknown option \\`show-scope'\" >&2\n    exit 129\n  fi\ndone\n\
             exec '{}' \"$@\"\n",
            real_git().display()
        );
        let wrapper = script(dir, "old-git", &body);
        let git = disk_cleaner_core::Git::new(&wrapper, std::time::Duration::from_secs(60));
        // Retry while the OS refuses to run a just-written file another test thread's fork still
        // holds open; then the wrapper must work as git.
        let mut last = None;
        for _ in 0..20 {
            match git.run(probe_in, &["rev-parse", "--is-inside-work-tree"]) {
                Err(disk_cleaner_core::GitError::Spawn(_)) => {
                    std::thread::sleep(std::time::Duration::from_millis(50))
                }
                other => {
                    last = Some(other);
                    break;
                }
            }
        }
        assert_eq!(
            last.map(|r| r.map(|s| s.trim().to_string()).map_err(|e| e.to_string())),
            Some(Ok("true".to_string()))
        );
        git
    }

    #[test]
    fn a_git_without_show_scope_is_refused_with_the_version_it_needs_in_words() {
        let (tmp, root, repo) = extension_repo();
        let git = old_git(tmp.path(), &repo);
        let res = scan_with(&root, network_everywhere().with_git(git));
        refused(&res, &repo, OLD_GIT, false, false);
    }

    /// Control: the same wrapper inspects a repo that does not need the cross-check.
    #[test]
    fn control_a_git_without_show_scope_inspects_a_repo_without_the_extension() {
        let (tmp, root, remotes) = workspace();
        let repo = pushed_repo(&root, &remotes, "app");
        let git = old_git(tmp.path(), &repo);
        let res = scan_with(&root, network_everywhere().with_git(git));
        assert_inspected(&res, &repo);
    }
}

// ---------------------------------------------------------------- git uses another git directory

/// A `.git` directory git does not accept (its `HEAD` names no ref) inside a repo whose
/// `config.worktree` holds an unsafe key: git walks up and reads the outer repo's config,
/// `config.worktree` included. That this directory's own `.git/config.worktree` is missing must
/// not let the outer file go unjudged: the refusal is the config's, not a later check's.
#[test]
fn a_missing_file_where_git_reads_another_git_directorys_file_is_refused() {
    let (_tmp, root, remotes) = workspace();
    let outer = pushed_repo(&root, &remotes, "outer");
    git(&outer, &["config", "extensions.worktreeConfig", "true"]);
    git(&outer, &["config", "--worktree", "core.pager", "cat"]);
    let inner = outer.join("inner");
    let gd = inner.join(".git");
    for d in ["refs/heads", "refs/tags", "objects/info", "objects/pack"] {
        fs::create_dir_all(gd.join(d)).unwrap();
    }
    fs::write(gd.join("HEAD"), "ref: nowhere\n").unwrap();
    fs::write(gd.join("config"), "[core]\n\trepositoryformatversion = 0\n").unwrap();
    assert!(!gd.join("config.worktree").exists());
    let res = scan_with(&root, network_everywhere());
    assert_refused(&res, &inner, UNJUDGED);
}

/// Review F1 (a): a bare-shaped directory git does not accept (an empty `HEAD`) inside a repo
/// with the extension and no file. git walks up to the outer repo; with nothing in its worktree
/// scope the config check passes, and the facts would be the outer repo's.
#[test]
fn a_bare_shaped_dir_git_rejects_under_a_repo_with_the_extension_and_no_file_is_refused() {
    let (_tmp, root, repo) = extension_repo();
    let inner = repo.join("inner.git");
    bare_shaped(&inner, "");
    let res = scan_with(&root, network_everywhere());
    assert_eq!(node(&res, &inner).class, Class::BareRepo);
    assert_refused(&res, &inner, OTHER_GIT_DIR);
}

/// Review F1 (b): the same under a repo without the extension, a hole older than this change.
#[test]
fn a_bare_shaped_dir_git_rejects_under_a_repo_without_the_extension_is_refused() {
    let (_tmp, root, remotes) = workspace();
    let outer = pushed_repo(&root, &remotes, "outer");
    let inner = outer.join("inner.git");
    bare_shaped(&inner, "");
    let res = scan_with(&root, network_everywhere());
    assert_eq!(node(&res, &inner).class, Class::BareRepo);
    assert_refused(&res, &inner, OTHER_GIT_DIR);
}

/// Review F1 (c): the bare twin of the walk-up test above. git reads the outer repo's unsafe
/// `config.worktree`, so the config check refuses before any other git command.
#[test]
fn a_bare_shaped_dir_git_rejects_under_a_repo_whose_config_worktree_is_unsafe_is_refused() {
    let (_tmp, root, repo) = extension_repo();
    git(&repo, &["config", "--worktree", "core.pager", "cat"]);
    let inner = repo.join("inner.git");
    bare_shaped(&inner, "ref: nowhere\n");
    assert!(!inner.join("config.worktree").exists());
    let res = scan_with(&root, network_everywhere());
    assert_eq!(node(&res, &inner).class, Class::BareRepo);
    assert_refused(&res, &inner, UNJUDGED);
}
