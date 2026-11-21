//! Regression tests for the coordinator's round Y after the gate review of 4ff80a6 (PR #2).
//!
//! Y1: a git node is never removed without a backup of its git directory's own files: every
//! removing action on one carries `preserve.git_admin`, which counts exactly the files
//! `git_admin_files` lists, and a git directory holding more than `max_git_admin_bytes` of them
//! asks. Y2: the four findings (a recorded rerere resolution after an aborted merge, an edited or
//! invented `.sample` hook, a draft in `COMMIT_EDITMSG`, a file under `logs/refs/`) are covered
//! by that backup, and so is any file dropped anywhere in the git directory. Y4 (as amended by
//! rounds Z and 15): a file named like an object file is never exempt: it is under the
//! object-store obligation, which is a backup too (this format defines no verification of
//! object data).
//!
//! Every fixture uses real git.

mod common;

use common::*;
use disk_cleaner_core::{
    git_admin_files, object_store_files, plan, Action, Facts, Plan, PlanItem, PlanOptions,
    ScanResult, Tri,
};
use std::fs;
use std::path::{Path, PathBuf};

fn plan_of(res: &ScanResult) -> Plan {
    plan(res, &PlanOptions::new(7, far_future()))
}

fn plan_capped(res: &ScanResult, max: u64) -> Plan {
    let mut opts = PlanOptions::new(7, far_future());
    opts.max_git_admin_bytes = max;
    plan(res, &opts)
}

fn item<'a>(p: &'a Plan, path: &Path) -> &'a PlanItem {
    p.items
        .iter()
        .find(|i| i.path == path.to_string_lossy())
        .unwrap_or_else(|| panic!("no plan item for {}", path.display()))
}

/// Run git where failure is expected (a conflicting merge); true when it succeeded.
fn git_ok(dir: &Path, args: &[&str]) -> bool {
    git_command()
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git must be installed to run these tests")
        .status
        .success()
}

/// A clone, inside the scan root, of a repository pushed to a bare remote outside it.
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

/// What `git_admin_files` lists for the scanned node at `repo`.
fn admin_files(res: &ScanResult, repo: &Path) -> Vec<String> {
    git_admin_files(node(res, repo)).expect("the git directory's own files can be listed")
}

/// Y2 (a) and (b): `rel` (below the git directory) is among the files a removal must save, the
/// plan removes the repository only after backing up exactly those files, and it does remove it
/// (the backup is what makes it safe, not another ask).
fn assert_backed_up(res: &ScanResult, repo: &Path, rel: &str) {
    let files = admin_files(res, repo);
    assert!(
        files.iter().any(|f| f == rel),
        "{rel} is not among the git directory's own files: {files:?}"
    );
    let p = plan_of(res);
    let it = item(&p, repo);
    assert_eq!(it.action, Action::BackupThenRemove, "{}", it.reason);
    assert_preserves(res, it, repo, &files);
}

/// What `object_store_files` lists for the scanned node at `repo`.
fn store_files(res: &ScanResult, repo: &Path) -> Vec<String> {
    object_store_files(node(res, repo)).expect("the object store's files can be listed")
}

/// A removing item for `repo` carries both obligations for its git directory, counting exactly
/// `files` (its own files) and the object store's files, both backed up, and says so in its
/// reason.
fn assert_preserves(res: &ScanResult, it: &PlanItem, repo: &Path, files: &[String]) {
    let pre = it
        .preserve
        .as_ref()
        .unwrap_or_else(|| panic!("a removing item has no preserve obligation: {}", it.reason));
    let gd = repo.join(".git").to_string_lossy().into_owned();
    assert!(
        pre.git_admin.git_dirs.contains(&gd),
        "{gd} not in {:?}",
        pre.git_admin.git_dirs
    );
    assert_eq!(pre.git_admin.files, files.len() as u64, "{files:?}");
    assert!(pre.git_admin.files > 0 && pre.git_admin.bytes > 0);
    let store = store_files(res, repo);
    assert_eq!(pre.object_store.files, store.len() as u64, "{store:?}");
    let v = serde_json::to_value(pre).unwrap();
    assert!(v["object_store"].get("verified").is_none(), "{v}");
    for want in [
        "are saved with the manifest first",
        "its object store (",
        "holds object data that must be backed up too before removal",
        "then it is removed",
    ] {
        assert!(it.reason.contains(want), "{want:?} not in {}", it.reason);
    }
    assert!(!it.reason.contains("not yet verified"), "{}", it.reason);
}

/// Every path inside the git directory, sorted, `/`-separated (for failure messages).
fn listing(repo: &Path) -> String {
    let gd = repo.join(".git");
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![gd.clone()];
    while let Some(dir) = stack.pop() {
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for e in rd {
            let p = e.unwrap().path();
            let rel = p
                .strip_prefix(&gd)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if fs::symlink_metadata(&p)
                .map(|m| m.is_dir())
                .unwrap_or(false)
            {
                stack.push(p.clone());
            }
            out.push(rel);
        }
    }
    out.sort();
    out.join(",")
}

fn all_yes() -> Facts {
    Facts {
        has_network_remote: Tri::Yes,
        head_on_remote: Tri::Yes,
        all_branches_on_remote: Tri::Yes,
        stash_empty: Tri::Yes,
        tree_clean: Tri::Yes,
        no_unique_ignored: Tri::Yes,
        no_dependents: Tri::Yes,
        reflog_saved: Tri::Yes,
        no_pending_state: Tri::Yes,
        plain_layout: Tri::Yes,
        no_local_admin_content: Tri::Yes,
    }
}

// ---------------------------------------------------------------- Y1: the contract

#[test]
fn y1_a_clean_pushed_clone_is_backed_up_then_removed() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let res = scan_with(&root, network_everywhere());
    let files = admin_files(&res, &repo);
    for must in ["HEAD", "config", "packed-refs"] {
        assert!(files.iter().any(|f| f == must), "{must} missing: {files:?}");
    }
    assert!(
        !files
            .iter()
            .any(|f| f.starts_with("objects/") && f.len() == 49),
        "loose objects are not the git directory's own files: {files:?}"
    );
    let store = store_files(&res, &repo);
    assert!(
        !store.is_empty() && store.iter().all(|f| f.starts_with("objects/")),
        "the repository's objects are its object store: {store:?}"
    );
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::BackupThenRemove, "{}", it.reason);
    assert_preserves(&res, it, &repo, &files);
    // Recovered or not, only regenerable directories are ready without a verification step.
    assert_eq!(p.totals.ready_bytes, 0);
}

#[test]
fn y1_a_git_node_whose_files_were_not_counted_asks() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let mut res = scan_with(&root, network_everywhere());
    let i = node_index(&res, &repo);
    res.nodes[i].repo.as_mut().unwrap().git_admin = None;
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    assert!(it.preserve.is_none());
    assert!(
        it.reason.contains("own files were not counted"),
        "{}",
        it.reason
    );
}

#[test]
fn y1_a_git_directory_over_the_cap_asks_naming_its_largest_file() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    write_bytes(&repo.join(".git").join("hooks").join("big.sample"), 300_000);
    let res = scan_with(&root, network_everywhere());
    // Under the default cap (64 MiB) it is backed up then removed.
    assert_backed_up(&res, &repo, "hooks/big.sample");
    let p = plan_capped(&res, 200_000);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert_eq!(it.reclaimable_bytes, 0);
    assert!(it.reason.contains("hooks/big.sample"), "{}", it.reason);
    assert!(it.reason.contains("max_git_admin_bytes"), "{}", it.reason);
}

#[test]
fn y1_a_repo_removed_with_its_parent_has_its_git_directory_in_the_parents_obligation() {
    let (tmp, root, remotes) = workspace();
    let outer = clone_of(tmp.path(), &root, &remotes, "outer");
    pushed_repo(&tmp.path().join("src"), &remotes, "inner");
    let inner = outer.join("inner");
    let url = remotes.join("inner.git");
    git(
        &outer,
        &[
            "clone",
            "-q",
            url.to_str().unwrap(),
            inner.to_str().unwrap(),
        ],
    );
    let mut res = scan_with(&root, network_everywhere());
    // The plan's aggregation is under test, not the outer repository's facts about an
    // untracked nested clone: declare the outer one safe.
    let o = node_index(&res, &outer);
    res.nodes[o].repo.as_mut().unwrap().facts = all_yes();
    let p = plan_of(&res);
    let inner_item = item(&p, &inner);
    assert_eq!(
        inner_item.action,
        Action::BackupThenRemove,
        "{}",
        inner_item.reason
    );
    let it = item(&p, &outer);
    assert_eq!(it.action, Action::BackupThenRemove, "{}", it.reason);
    let pre = it.preserve.as_ref().expect("preserve");
    let inner_gd = inner.join(".git").to_string_lossy().into_owned();
    assert!(
        pre.git_admin.git_dirs.contains(&inner_gd),
        "{:?}",
        pre.git_admin.git_dirs
    );
    let both = admin_files(&res, &outer).len() + admin_files(&res, &inner).len();
    assert_eq!(pre.git_admin.files, both as u64);
}

// ---------------------------------------------------------------- Y2: the four findings

#[test]
fn y2_a_rerere_resolution_recorded_before_an_aborted_merge_is_backed_up() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    git(&repo, &["checkout", "-q", "-b", "side"]);
    commit_file(&repo, "README.md", "side\n");
    git(&repo, &["push", "-q", "origin", "side"]);
    git(&repo, &["checkout", "-q", "main"]);
    commit_file(&repo, "README.md", "main\n");
    git(&repo, &["push", "-q", "origin", "main"]);
    let merged = git_ok(&repo, &["-c", "rerere.enabled=true", "merge", "-q", "side"]);
    assert!(!merged, "the merge must conflict");
    write(
        &repo.join("README.md"),
        "the only copy of this resolution\n",
    );
    git(&repo, &["-c", "rerere.enabled=true", "rerere"]);
    git(&repo, &["merge", "--abort"]);
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
    let rr = repo.join(".git").join("rr-cache");
    let id = fs::read_dir(&rr)
        .unwrap_or_else(|e| panic!("no rr-cache ({e}): {}", listing(&repo)))
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .next()
        .unwrap_or_else(|| panic!("rr-cache is empty: {}", listing(&repo)));
    let rel = format!("rr-cache/{id}/postimage");
    assert!(
        repo.join(".git").join(&rel).is_file(),
        "no postimage after the abort: {}",
        listing(&repo)
    );
    let res = scan_with(&root, network_everywhere());
    assert_backed_up(&res, &repo, &rel);
}

#[test]
fn y2_an_edited_stock_sample_hook_and_a_custom_sample_are_backed_up() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let hooks = repo.join(".git").join("hooks");
    write(&hooks.join("pre-commit.sample"), "#!/bin/sh\nmine\n");
    write(&hooks.join("custom.sample"), "#!/bin/sh\nkeep\n");
    let res = scan_with(&root, network_everywhere());
    assert_backed_up(&res, &repo, "hooks/pre-commit.sample");
    assert_backed_up(&res, &repo, "hooks/custom.sample");
}

#[test]
fn y2_a_draft_left_in_commit_editmsg_is_backed_up() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    write(
        &repo.join(".git").join("COMMIT_EDITMSG"),
        "Half-written explanation of a fix\n\nnever committed\n",
    );
    let res = scan_with(&root, network_everywhere());
    assert_backed_up(&res, &repo, "COMMIT_EDITMSG");
}

#[test]
fn y2_a_hidden_file_under_logs_refs_is_backed_up() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    write(
        &repo.join(".git").join("logs").join("refs").join(".notes"),
        "unique notes kept next to the reflogs\n",
    );
    let res = scan_with(&root, network_everywhere());
    let rel = "logs/refs/.notes";
    let files = admin_files(&res, &repo);
    assert!(files.iter().any(|f| f == rel), "{files:?}");
    let p = plan_of(&res);
    let it = item(&p, &repo);
    // The reflog check may not read it; either way it is never removed unsaved.
    match it.action {
        Action::Ask => assert_eq!(it.reclaimable_bytes, 0),
        _ => {
            assert_eq!(it.action, Action::BackupThenRemove, "{}", it.reason);
            assert_preserves(&res, it, &repo, &files);
        }
    }
}

// ---------------------------------------------------------------- Y2: the invariant

/// xorshift64: a fixed seed gives the same names on every system.
struct Rng(u64);

impl Rng {
    fn step(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn hex(&mut self, n: usize) -> String {
        const HEX: &[u8] = b"0123456789abcdef";
        (0..n)
            .map(|_| HEX[(self.step() as usize) % HEX.len()] as char)
            .collect()
    }

    fn word(&mut self, min: usize, max: usize) -> String {
        const ABC: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let n = min + (self.step() as usize) % (max - min + 1);
        (0..n)
            .map(|_| ABC[(self.step() as usize) % ABC.len()] as char)
            .collect()
    }
}

#[test]
fn y2_any_file_dropped_in_the_git_directory_is_asked_about_or_backed_up() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let gd = repo.join(".git");
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let base = scan_with(&root, network_everywhere());
    assert_eq!(action_for(&base, &repo), Action::BackupThenRemove);
    let rr_id = "0123456789abcdef0123456789abcdef01234567";
    let rr_loc = format!("rr-cache/{rr_id}");
    let locations: Vec<&str> = vec![
        "",
        "hooks",
        "info",
        "logs",
        "logs/refs/heads",
        "refs/heads",
        rr_loc.as_str(),
        "objects",
        "objects/ab",
        "objects/pack",
        "objects/info",
        "branches",
        "NEWDIR",
    ];
    let mut rng = Rng(0x5eed_0016_2026_1119);
    let (mut removed, mut asked) = (0u32, 0u32);
    let mut cases: Vec<String> = Vec::new();
    for loc in &locations {
        for k in 0..3 {
            let dot = if rng.step().is_multiple_of(4) {
                "."
            } else {
                ""
            };
            let mut name = format!("{dot}{}", rng.word(4, 10));
            let mut content = format!("{}\n", rng.word(8, 60));
            // Every third name is one the earlier checks accept, so the case can reach a
            // removing action and the obligation is what is tested.
            if k == 0 {
                match *loc {
                    "hooks" => name = format!("{}.sample", rng.word(4, 10)),
                    "refs/heads" => {
                        name = rng.word(4, 10);
                        content = format!("{head}\n");
                    }
                    l if l == rr_loc => name = format!("postimage.{}", rng.step() % 10),
                    // Names of object-store files: never exempt, whatever they hold.
                    "objects/ab" => name = rng.hex(38),
                    "objects/pack" => name = format!("pack-{}.pack", rng.hex(40)),
                    "objects/info" => name = "commit-graph".to_string(),
                    _ => {}
                }
            }
            let loc = if *loc == "NEWDIR" {
                format!("new{}", rng.word(4, 8))
            } else {
                loc.to_string()
            };
            let rel = if loc.is_empty() {
                name.clone()
            } else {
                format!("{loc}/{name}")
            };
            // Remember the first directory that has to be created, to clean up after.
            let mut created: Option<PathBuf> = None;
            let mut dir = gd.clone();
            for part in loc.split('/').filter(|p| !p.is_empty()) {
                dir = dir.join(part);
                if created.is_none() && !dir.exists() {
                    created = Some(dir.clone());
                }
            }
            let path = gd.join(&rel);
            write(&path, &content);

            // Asked about, or in exactly one of the two backup obligations.
            let res = scan_with(&root, network_everywhere());
            let files = admin_files(&res, &repo);
            let store = store_files(&res, &repo);
            assert!(
                files.contains(&rel) != store.contains(&rel),
                "{rel} must be in exactly one class: {files:?} {store:?}"
            );
            let p = plan_of(&res);
            let it = item(&p, &repo);
            match it.action {
                Action::Ask => {
                    assert_eq!(it.reclaimable_bytes, 0, "{rel}");
                    asked += 1;
                }
                Action::BackupThenRemove | Action::RecoverBackupThenRemove => {
                    assert_preserves(&res, it, &repo, &files);
                    removed += 1;
                }
                other => panic!("{rel}: unexpected {other:?}: {}", it.reason),
            }
            cases.push(format!("{rel} -> {}", it.action.as_str()));

            match created {
                Some(d) => fs::remove_dir_all(&d).unwrap(),
                None => fs::remove_file(&path).unwrap(),
            }
        }
    }
    let n = removed + asked;
    assert!(n >= 30, "only {n} cases");
    // Not vacuous: names the earlier checks accept reach a removing action, so the obligation
    // was what kept them safe.
    assert!(removed >= 6, "only {removed} removed: {cases:#?}");
}

// ---------------------------------------------------------------- Y4: object files by content

/// The single pack of `repo` after `git repack -a -d`: (pack, idx).
fn only_pack(repo: &Path) -> (PathBuf, PathBuf) {
    git(repo, &["repack", "-a", "-d", "-q"]);
    let dir = repo.join(".git").join("objects").join("pack");
    let mut packs: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().map(|x| x == "pack").unwrap_or(false))
        .collect();
    assert_eq!(packs.len(), 1, "{}", listing(repo));
    let pack = packs.remove(0);
    let idx = pack.with_extension("idx");
    assert!(idx.is_file());
    (pack, idx)
}

/// A pack, its index, reverse index or bitmap (`objects/pack/pack-<id>.<ext>`).
fn is_pack_object(f: &str) -> bool {
    let ext = f.rsplit('.').next().unwrap_or("");
    f.starts_with("objects/pack/pack-") && matches!(ext, "pack" | "idx" | "rev" | "bitmap")
}

/// A file named like an object file is never exempt: the scan counts it under the object-store
/// backup obligation, or the repository asks.
fn assert_never_exempt(root: &Path, repo: &Path, rel: &str) {
    let res = scan_with(root, network_everywhere());
    let store = store_files(&res, repo);
    assert!(store.iter().any(|f| f == rel), "{rel} not in {store:?}");
    assert!(!admin_files(&res, repo).iter().any(|f| f == rel));
    let p = plan_of(&res);
    let it = item(&p, repo);
    if it.action == Action::Ask {
        assert_eq!(it.reclaimable_bytes, 0);
    } else {
        assert_eq!(it.action, Action::BackupThenRemove, "{}", it.reason);
        assert_preserves(&res, it, repo, &admin_files(&res, repo));
    }
}

#[test]
fn y4_a_text_file_named_like_a_loose_object_is_never_exempt() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let name = "cdef0123456789abcdef0123456789abcdef01";
    write(
        &repo.join(".git").join("objects").join("ab").join(name),
        "not a zlib stream, just my notes\n",
    );
    assert_never_exempt(&root, &repo, &format!("objects/ab/{name}"));
}

#[test]
fn y4_a_pack_holding_text_is_never_exempt() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let (_, idx) = only_pack(&repo);
    // A real index beside it, so only the pack's own content is wrong.
    let fake = "pack-1111111111111111111111111111111111111111";
    let dir = repo.join(".git").join("objects").join("pack");
    fs::copy(&idx, dir.join(format!("{fake}.idx"))).unwrap();
    write(&dir.join(format!("{fake}.pack")), "not a pack\n");
    assert_never_exempt(&root, &repo, &format!("objects/pack/{fake}.pack"));
}

#[test]
fn y4_an_index_without_its_pack_is_never_exempt() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let (_, idx) = only_pack(&repo);
    let fake = "pack-2222222222222222222222222222222222222222";
    let dir = repo.join(".git").join("objects").join("pack");
    fs::copy(&idx, dir.join(format!("{fake}.idx"))).unwrap();
    assert_never_exempt(&root, &repo, &format!("objects/pack/{fake}.idx"));
}

#[test]
fn y4_control_gc_and_repack_keep_backup_then_remove_and_leave_objects_out() {
    let (tmp, root, remotes) = workspace();
    let gc = clone_of(tmp.path(), &root, &remotes, "gc");
    git(&gc, &["gc", "-q"]);
    let repack = clone_of(tmp.path(), &root, &remotes, "repack");
    git(&repack, &["repack", "-a", "-d", "-q"]);
    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    for repo in [&gc, &repack] {
        let it = item(&p, repo);
        assert_eq!(
            it.action,
            Action::BackupThenRemove,
            "{}\n{}",
            it.reason,
            listing(repo)
        );
        let files = admin_files(&res, repo);
        assert_preserves(&res, it, repo, &files);
        for f in &files {
            assert!(!is_pack_object(f), "listed as the git directory's own: {f}");
        }
    }
}

#[test]
fn y4_control_loose_objects_at_every_compression_level_are_objects() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    // Stored (level 0), fixed and dynamic Huffman blocks (levels 1 and 9), with a few KB of
    // text so the dynamic block has real tables.
    let body: String = (0..400)
        .map(|i| format!("line {i} of some text\n"))
        .collect();
    for level in ["0", "1", "9"] {
        let name = format!("file{level}.txt");
        write(&repo.join(&name), &format!("{level}\n{body}"));
        let c = format!("core.compression={level}");
        git(&repo, &["-c", &c, "add", &name]);
        git(&repo, &["-c", &c, "commit", "-q", "-m", &name]);
    }
    git(&repo, &["push", "-q", "origin", "main"]);
    let res = scan_with(&root, network_everywhere());
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(
        it.action,
        Action::BackupThenRemove,
        "{}\n{}",
        it.reason,
        listing(&repo)
    );
    let files = admin_files(&res, &repo);
    assert_preserves(&res, it, &repo, &files);
    let loose = files
        .iter()
        .filter(|f| f.starts_with("objects/") && f.len() == "objects/ab/".len() + 38)
        .count();
    assert_eq!(loose, 0, "loose objects listed: {files:?}");
}
