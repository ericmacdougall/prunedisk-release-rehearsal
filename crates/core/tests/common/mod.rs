//! Shared test helpers. Every fixture is synthetic and built in a temporary directory with real
//! `git` commands.
#![allow(dead_code)]

use disk_cleaner_core::{scan, FactsConfig, Node, ScanOptions, ScanResult};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A `git` command for building fixtures: no automatic maintenance, for it and every git it
/// starts (the settings travel in the environment).
///
/// After a commit, merge, rebase or fetch git starts `git maintenance run --auto --detach`
/// (`maintenance.autoDetach` defaults to true). The detached child holds
/// `objects/maintenance.lock` after the command has returned, and a scan that walks the git
/// directory meanwhile sees it as the repository's own content (`no_local_admin_content` = `no`).
/// That made `f1_filter_clean_in_repo_config_is_never_executed` fail now and then on macOS; the
/// CLI tests turn it off the same way.
pub fn git_command() -> Command {
    let mut cmd = Command::new("git");
    cmd.env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "maintenance.auto")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "gc.auto")
        .env("GIT_CONFIG_VALUE_1", "0");
    cmd
}

/// Run git in `dir`, panic with stderr on failure, return trimmed stdout.
///
/// Line endings are pinned (`core.autocrlf=false`, which `args` can override): the scanner does
/// not read system config, so a fixture checked out with a platform's default `autocrlf=true`
/// would look modified to it (see `line_endings_converted_only_by_system_config_read_as_modified`).
/// Automatic maintenance is off ([`git_command`]).
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_command()
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git must be installed to run these tests");
    assert!(
        out.status.success(),
        "git {:?} failed in {}: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

/// `git init` with a fixed branch name, plus one commit. The identity comes from the
/// environment [`git`] sets, not from the repository config: a per-repository `user.*` setting
/// is local content the scanner reports (gate d1e3f23, W1).
pub fn init_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    write(&dir.join("README.md"), "hello\n");
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

/// Create an empty bare repository to act as a remote.
pub fn init_bare(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "--bare", "-b", "main"]);
}

/// Wire `repo` to a bare remote at `remote` as `origin` and push `main`.
pub fn push_to_remote(repo: &Path, remote: &Path) {
    git(repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(repo, &["push", "-q", "origin", "main"]);
}

pub fn commit_file(repo: &Path, name: &str, content: &str) {
    write(&repo.join(name), content);
    git(repo, &["add", name]);
    git(repo, &["commit", "-q", "-m", name]);
}

/// Facts config that treats every remote (even a local bare repo) as a network location, so
/// tests can model a pushed repo without a network. Production uses `is_network_url`.
pub fn network_everywhere() -> FactsConfig {
    FactsConfig::default().with_network_predicate(|_| true)
}

pub fn scan_with(root: &Path, facts: FactsConfig) -> ScanResult {
    let mut opts = ScanOptions::new(vec![root.to_path_buf()]);
    opts.facts = facts;
    opts.git_jobs = 2;
    scan(&opts).expect("scan")
}

pub fn scan_plain(root: &Path) -> ScanResult {
    scan_with(root, FactsConfig::default())
}

/// Find the node whose path equals `path`.
pub fn node<'a>(res: &'a ScanResult, path: &Path) -> &'a Node {
    let want = path.to_string_lossy();
    res.nodes
        .iter()
        .find(|n| n.path == want)
        .unwrap_or_else(|| {
            let have: Vec<&str> = res.nodes.iter().map(|n| n.path.as_str()).collect();
            panic!("no node for {want}; nodes: {have:?}")
        })
}

pub fn node_index(res: &ScanResult, path: &Path) -> usize {
    let want = path.to_string_lossy();
    res.nodes.iter().position(|n| n.path == want).expect("node")
}

pub fn has_node(res: &ScanResult, path: &Path) -> bool {
    let want = path.to_string_lossy();
    res.nodes.iter().any(|n| n.path == want)
}

pub fn write_bytes(path: &Path, len: usize) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, vec![7u8; len]).unwrap();
}

/// Scan root and remote live side by side so the remote is not part of the scan.
pub fn workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let scan_root = tmp.path().join("scan");
    let remotes = tmp.path().join("remotes");
    fs::create_dir_all(&scan_root).unwrap();
    fs::create_dir_all(&remotes).unwrap();
    (tmp, scan_root, remotes)
}

/// A repo with one commit pushed to a bare "remote" outside the scan root.
pub fn pushed_repo(root: &Path, remotes: &Path, name: &str) -> PathBuf {
    let repo = root.join(name);
    init_repo(&repo);
    let remote = remotes.join(format!("{name}.git"));
    init_bare(&remote);
    push_to_remote(&repo, &remote);
    repo
}

pub fn facts_of(res: &ScanResult, path: &Path) -> disk_cleaner_core::Facts {
    node(res, path).repo.as_ref().expect("repo info").facts
}

/// Plan action for `path` with a 7-day window and a far-future clock (everything is idle).
pub fn action_for(res: &ScanResult, path: &Path) -> disk_cleaner_core::Action {
    let p = disk_cleaner_core::plan(res, &disk_cleaner_core::PlanOptions::new(7, far_future()));
    p.items
        .iter()
        .find(|i| i.path == path.to_string_lossy())
        .unwrap_or_else(|| panic!("no plan item for {}", path.display()))
        .action
}

/// Write an executable shell script.
#[cfg(unix)]
pub fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = dir.join(name);
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(&path)
        .unwrap();
    f.write_all(body.as_bytes()).unwrap();
    f.sync_all().unwrap();
    path
}

/// A year and a bit past `now`, so everything counts as idle for any sane threshold.
pub fn far_future() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    now + 3 * 365 * 86_400
}
