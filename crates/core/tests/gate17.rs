//! Regression tests for the coordinator's round Z after the gate review of 36f9698 (PR #2), as
//! amended by gate round 15 at 087a1b3.
//!
//! Z1: a file in `objects/` was left out of the backup when its first bytes looked like an
//! object file's (a `RIDX` sidecar holding notes, a pack or index with valid magic and junk
//! after it, a loose object whose header inflates but whose remainder is corrupt). Now every
//! file of a git directory is in exactly one of two backup obligations, decided by name alone:
//! `preserve.git_admin` and `preserve.object_store`. Round 15 removed `scan --verify-objects`:
//! this format defines no verification of object data, so both obligations are backups, and no
//! file, however it was tampered with, is outside them.
//!
//! Every fixture uses real git.

mod common;

use common::*;
use disk_cleaner_core::{
    git_admin_files, object_store_files, plan, Action, Plan, PlanItem, PlanOptions, ScanResult,
};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn plan_of(res: &ScanResult) -> Plan {
    plan(res, &PlanOptions::new(7, far_future()))
}

fn item<'a>(p: &'a Plan, path: &Path) -> &'a PlanItem {
    p.items
        .iter()
        .find(|i| i.path == path.to_string_lossy())
        .unwrap_or_else(|| panic!("no plan item for {}", path.display()))
}

/// The scan (there is no other mode).
fn scan_default(root: &Path) -> ScanResult {
    scan_with(root, network_everywhere())
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

fn admin(res: &ScanResult, repo: &Path) -> Vec<String> {
    git_admin_files(node(res, repo)).expect("the git directory's own files can be listed")
}

fn store(res: &ScanResult, repo: &Path) -> Vec<String> {
    object_store_files(node(res, repo)).expect("the object store's files can be listed")
}

/// Every non-directory entry of the git directory, `/`-separated, sorted.
fn every_file(repo: &Path) -> Vec<String> {
    let gd = repo.join(".git");
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![gd.clone()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if fs::symlink_metadata(&p).unwrap().is_dir() {
                stack.push(p);
                continue;
            }
            let rel = p.strip_prefix(&gd).unwrap().to_string_lossy();
            out.push(rel.replace('\\', "/"));
        }
    }
    out.sort();
    out
}

/// The two obligations partition the git directory: every file is in exactly one.
fn assert_partition(res: &ScanResult, repo: &Path) {
    let a = admin(res, repo);
    let s = store(res, repo);
    for f in &s {
        assert!(!a.contains(f), "{f} is in both classes");
    }
    let mut both: Vec<String> = a.iter().chain(s.iter()).cloned().collect();
    both.sort();
    assert_eq!(
        both,
        every_file(repo),
        "the two classes must cover every file"
    );
}

/// A removing item for `repo` that carries both backup obligations, with counts matching the
/// two public lists, no verification field, and a reason that says all of it.
fn assert_obligations(res: &ScanResult, it: &PlanItem, repo: &Path) {
    assert_eq!(it.action, Action::BackupThenRemove, "{}", it.reason);
    let pre = it
        .preserve
        .as_ref()
        .unwrap_or_else(|| panic!("a removing item has no preserve obligation: {}", it.reason));
    assert_eq!(pre.git_admin.files, admin(res, repo).len() as u64);
    let s = store(res, repo);
    assert!(!s.is_empty(), "a clone has an object store");
    assert_eq!(pre.object_store.files, s.len() as u64, "{s:?}");
    assert!(pre.object_store.bytes > 0);
    let v = serde_json::to_value(pre).unwrap();
    assert!(v["object_store"].get("verified").is_none(), "{v}");
    for want in [
        "local git file",
        "are saved with the manifest first",
        "its object store (",
        "holds object data that must be backed up too before removal",
        "then it is removed",
    ] {
        assert!(it.reason.contains(want), "{want:?} not in {}", it.reason);
    }
    for gone in [
        "verified by this scan",
        "not yet verified",
        "verified against the remote or saved",
    ] {
        assert!(!it.reason.contains(gone), "{gone:?} in {}", it.reason);
    }
}

fn pack_dir(repo: &Path) -> PathBuf {
    repo.join(".git").join("objects").join("pack")
}

/// `rel` (below the git directory) is in one of the two backup sets, the two sets partition the
/// git directory, and the plan either asks (reclaiming nothing) or removes only with both
/// obligations.
fn assert_in_a_backup_set(res: &ScanResult, repo: &Path, rel: &str) {
    assert_partition(res, repo);
    let a = admin(res, repo);
    let s = store(res, repo);
    assert!(
        a.iter().any(|f| f == rel) || s.iter().any(|f| f == rel),
        "{rel} is in neither backup set: {a:?} {s:?}"
    );
    let p = plan_of(res);
    let it = item(&p, repo);
    if it.action == Action::Ask {
        assert_eq!(it.reclaimable_bytes, 0);
    } else {
        assert_obligations(res, it, repo);
    }
}

// ---------------------------------------------------------------- the reviewer's three cases

const MIDX_REV: &str = "multi-pack-index-1111111111111111111111111111111111111111.rev";

#[test]
fn z1_a_midx_sidecar_holding_notes_is_under_the_object_store_obligation() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let mut notes = b"RIDX".to_vec();
    notes.extend_from_slice(b"my only copy of these notes\n");
    fs::write(pack_dir(&repo).join(MIDX_REV), &notes).unwrap();
    let rel = format!("objects/pack/{MIDX_REV}");

    let res = scan_default(&root);
    assert!(
        store(&res, &repo).contains(&rel),
        "{:?}",
        store(&res, &repo)
    );
    assert!(!admin(&res, &repo).contains(&rel));
    assert_partition(&res, &repo);
    let p = plan_of(&res);
    assert_obligations(&res, item(&p, &repo), &repo);
}

const MIDX_BITMAP: &str = "multi-pack-index-1111111111111111111111111111111111111111.bitmap";

/// A multi-pack-index `.bitmap` with an arbitrary payload and no multi-pack-index is in a
/// backup set.
#[test]
fn z1_a_midx_bitmap_with_an_arbitrary_payload_is_in_a_backup_set() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    fs::write(pack_dir(&repo).join(MIDX_BITMAP), notes_after(b"BITM")).unwrap();
    let res = scan_default(&root);
    assert_in_a_backup_set(&res, &repo, &format!("objects/pack/{MIDX_BITMAP}"));
}

const FAKE: &str = "pack-3333333333333333333333333333333333333333";

/// A pack and an index of that name whose magic and version are right and whose rest is not.
fn malformed_pack_pair(repo: &Path) -> (String, String) {
    let dir = pack_dir(repo);
    let mut pack = b"PACK\0\0\0\x02\0\0\0\x01".to_vec();
    pack.extend_from_slice(b"these are notes, not a compressed object\n");
    fs::write(dir.join(format!("{FAKE}.pack")), &pack).unwrap();
    let mut idx = vec![0xff, b't', b'O', b'c', 0, 0, 0, 2];
    idx.extend_from_slice(&[0u8; 1024]);
    idx.extend_from_slice(b"notes again\n");
    fs::write(dir.join(format!("{FAKE}.idx")), &idx).unwrap();
    (
        format!("objects/pack/{FAKE}.pack"),
        format!("objects/pack/{FAKE}.idx"),
    )
}

#[test]
fn z1_a_pack_and_index_malformed_after_their_magic_are_never_exempt() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let (pack, idx) = malformed_pack_pair(&repo);

    let res = scan_default(&root);
    assert_obligated_or_asks(&res, &repo, &[pack.as_str(), idx.as_str()]);
}

/// Write a blob as a loose object with real git and return its path below the git directory.
fn loose_blob(repo: &Path, text: &str) -> (String, PathBuf) {
    let src = repo.join("blob.txt");
    write(&src, text);
    let id = git(repo, &["hash-object", "-w", "blob.txt"]);
    fs::remove_file(&src).unwrap();
    let rel = format!("objects/{}/{}", &id[..2], &id[2..]);
    let path = repo.join(".git").join(&rel);
    assert!(path.is_file(), "{rel} was not written loose");
    make_writable(&path);
    (rel, path)
}

/// Git writes objects read-only.
#[cfg(unix)]
fn make_writable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
}

#[cfg(not(unix))]
fn make_writable(path: &Path) {
    let mut perm = fs::metadata(path).unwrap().permissions();
    perm.set_readonly(false);
    fs::set_permissions(path, perm).unwrap();
}

/// A damaged object file either makes some git command fail (the repo asks) or is counted under
/// the object-store obligation. Never exempt.
fn assert_obligated_or_asks(res: &ScanResult, repo: &Path, rels: &[&str]) {
    let s = store(res, repo);
    for rel in rels {
        assert!(s.iter().any(|f| f == rel), "{rel} not in {s:?}");
    }
    assert_partition(res, repo);
    let p = plan_of(res);
    let it = item(&p, repo);
    if it.action == Action::Ask {
        assert_eq!(it.reclaimable_bytes, 0);
    } else {
        assert_obligations(res, it, repo);
    }
}

fn some_text() -> String {
    (0..300).map(|i| format!("line {i} of a blob\n")).collect()
}

#[test]
fn z1_a_loose_object_with_a_valid_header_and_a_corrupt_remainder() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let (rel, path) = loose_blob(&repo, &some_text());
    let mut bytes = fs::read(&path).unwrap();
    let n = bytes.len();
    assert!(
        n > 64,
        "the object is long enough to keep its header intact"
    );
    for b in &mut bytes[n - 16..] {
        *b = b'X';
    }
    fs::write(&path, &bytes).unwrap();

    let res = scan_default(&root);
    assert_obligated_or_asks(&res, &repo, &[rel.as_str()]);
}

#[test]
fn z1_bytes_after_a_valid_loose_object_are_in_a_backup_set() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let (rel, path) = loose_blob(&repo, &some_text());
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    f.write_all(b"notes hidden after the zlib stream\n")
        .unwrap();
    drop(f);

    let res = scan_default(&root);
    assert_obligated_or_asks(&res, &repo, &[rel.as_str()]);
}

// ---------------------------------------------------------------- controls

#[test]
fn z1_controls_carry_both_backup_obligations() {
    let (tmp, root, remotes) = workspace();
    let fresh = clone_of(tmp.path(), &root, &remotes, "fresh");
    let gc = clone_of(tmp.path(), &root, &remotes, "gc");
    git(&gc, &["gc", "-q"]);
    let bitmap = clone_of(tmp.path(), &root, &remotes, "bitmap");
    git(
        &bitmap,
        &["repack", "-a", "-d", "-q", "--write-bitmap-index"],
    );
    let graph = clone_of(tmp.path(), &root, &remotes, "graph");
    git(&graph, &["commit-graph", "write", "--reachable"]);
    let split = clone_of(tmp.path(), &root, &remotes, "split");
    git(&split, &["commit-graph", "write", "--reachable", "--split"]);
    let midx = clone_of(tmp.path(), &root, &remotes, "midx");
    git(&midx, &["repack", "-a", "-d", "-q"]);
    git(&midx, &["multi-pack-index", "write", "--bitmap"]);
    let rev = clone_of(tmp.path(), &root, &remotes, "rev");
    git(&rev, &REV_REPACK);
    let all = [&fresh, &gc, &bitmap, &graph, &split, &midx, &rev];

    let res = scan_default(&root);
    let p = plan_of(&res);
    for repo in all {
        assert_partition(&res, repo);
        assert_obligations(&res, item(&p, repo), repo);
    }
}

const SPLIT_ID: &str = "4444444444444444444444444444444444444444";

#[test]
fn z1_a_split_graph_file_beside_a_single_commit_graph_is_in_a_backup_set() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    git(&repo, &["commit-graph", "write", "--reachable"]);
    let info = repo.join(".git").join("objects").join("info");
    assert!(info.join("commit-graph").is_file());
    let graphs = info.join("commit-graphs");
    fs::create_dir_all(&graphs).unwrap();
    let name = format!("graph-{SPLIT_ID}.graph");
    write(&graphs.join(&name), "CGPH and then my notes\n");
    let chain = graphs.join("commit-graph-chain");
    write(&chain, &format!("{SPLIT_ID}\n"));
    let rel = format!("objects/info/commit-graphs/{name}");

    let res = scan_default(&root);
    assert_in_a_backup_set(&res, &repo, &rel);
}

/// An object that nothing reaches may exist nowhere else: it is under the object-store backup
/// obligation.
#[test]
fn z1_an_unreachable_object_is_under_the_object_store_obligation() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let (rel, _) = loose_blob(&repo, &some_text());

    let res = scan_default(&root);
    assert!(store(&res, &repo).contains(&rel));
    let p = plan_of(&res);
    assert_obligations(&res, item(&p, &repo), &repo);
}

#[test]
fn z1_a_store_whose_files_were_not_counted_asks() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let mut res = scan_default(&root);
    let i = node_index(&res, &repo);
    res.nodes[i].repo.as_mut().unwrap().object_store = None;
    let p = plan_of(&res);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::Ask, "{}", it.reason);
    assert!(it.preserve.is_none());
    assert!(it.reason.contains("object store"), "{}", it.reason);
}

// ---------------------------------------------------------------- click-test remark

#[test]
fn z1_large_preserved_files_lead_the_examples() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    let hooks = repo.join(".git").join("hooks");
    for k in 1..=5 {
        write(&hooks.join(format!("a{k}.sample")), "#!/bin/sh\n");
    }
    write_bytes(&hooks.join("zz-big.sample"), 2 * 1024 * 1024);
    let res = scan_default(&root);
    let mut opts = PlanOptions::new(7, far_future());
    opts.max_git_admin_bytes = 1 << 30;
    let p = plan(&res, &opts);
    let it = item(&p, &repo);
    assert_eq!(it.action, Action::BackupThenRemove, "{}", it.reason);
    let ex = &it.preserve.as_ref().unwrap().git_admin.examples;
    assert_eq!(
        ex.first().map(String::as_str),
        Some(".git/hooks/zz-big.sample"),
        "{ex:?}"
    );
}

// ---------------------------------------------------------------- Z2: tampered derived files

// Every derived file of the object store, tampered with in any way, is in one of the two backup
// sets (round 15: no verification can take a file out of them).

/// `repack -a -d` writing a reverse index (`pack.writeReverseIndex`).
const REV_REPACK: [&str; 6] = [
    "-c",
    "pack.writeReverseIndex=true",
    "repack",
    "-a",
    "-d",
    "-q",
];

/// The one file in `objects/pack` whose name starts with `prefix` and ends with `suffix`.
fn pack_file(repo: &Path, prefix: &str, suffix: &str) -> PathBuf {
    let dir = pack_dir(repo);
    let mut found: Vec<PathBuf> = Vec::new();
    for e in fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        let n = p.file_name().unwrap().to_string_lossy().into_owned();
        if n.starts_with(prefix) && n.ends_with(suffix) {
            found.push(p);
        }
    }
    assert_eq!(found.len(), 1, "one {prefix}*{suffix} in {}", dir.display());
    found.pop().unwrap()
}

/// `path` below the git directory, `/`-separated.
fn rel_of(repo: &Path, path: &Path) -> String {
    let gd = repo.join(".git");
    let rel = path.strip_prefix(&gd).unwrap();
    rel.to_string_lossy().replace('\\', "/")
}

const NOTES: &[u8] = b"my notes, kept nowhere else\n";

fn append_notes(path: &Path) {
    make_writable(path);
    let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(NOTES).unwrap();
}

/// Overwrite everything between the first `keep` bytes and the trailing checksum.
fn replace_body(path: &Path, keep: usize) {
    make_writable(path);
    let mut bytes = fs::read(path).unwrap();
    let n = bytes.len();
    assert!(n > keep + 40, "{} is long enough", path.display());
    for b in &mut bytes[keep..n - 32] {
        *b = b'n';
    }
    fs::write(path, &bytes).unwrap();
}

/// `magic` followed by notes.
fn notes_after(magic: &[u8]) -> Vec<u8> {
    let mut v = magic.to_vec();
    v.extend_from_slice(NOTES);
    v
}

/// A clone repacked into one pack, with a multi-pack-index and its bitmap.
fn midx_clone(tmp: &Path, root: &Path, remotes: &Path, name: &str) -> PathBuf {
    let repo = clone_of(tmp, root, remotes, name);
    git(&repo, &["repack", "-a", "-d", "-q"]);
    git(&repo, &["multi-pack-index", "write", "--bitmap"]);
    repo
}

/// The SHA-1 that ends the multi-pack-index: the id its sidecars are named by.
fn midx_checksum(repo: &Path) -> String {
    let bytes = fs::read(pack_dir(repo).join("multi-pack-index")).unwrap();
    let n = bytes.len();
    bytes[n - 20..].iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn z2_a_pack_bitmap_appended_to_or_rewritten_is_in_a_backup_set() {
    let (tmp, root, remotes) = workspace();
    let appended = clone_of(tmp.path(), &root, &remotes, "appended");
    let rewritten = clone_of(tmp.path(), &root, &remotes, "rewritten");
    let mut rels = Vec::new();
    for repo in [&appended, &rewritten] {
        git(repo, &["repack", "-a", "-d", "-q", "--write-bitmap-index"]);
        rels.push(rel_of(repo, &pack_file(repo, "pack-", ".bitmap")));
    }
    append_notes(&pack_file(&appended, "pack-", ".bitmap"));
    replace_body(&pack_file(&rewritten, "pack-", ".bitmap"), 4);

    let res = scan_default(&root);
    assert_in_a_backup_set(&res, &appended, &rels[0]);
    assert_in_a_backup_set(&res, &rewritten, &rels[1]);
}

#[test]
fn z2_a_reverse_index_appended_to_or_rewritten_is_in_a_backup_set() {
    let (tmp, root, remotes) = workspace();
    let appended = clone_of(tmp.path(), &root, &remotes, "appended");
    let rewritten = clone_of(tmp.path(), &root, &remotes, "rewritten");
    let mut rels = Vec::new();
    for repo in [&appended, &rewritten] {
        git(repo, &REV_REPACK);
        rels.push(rel_of(repo, &pack_file(repo, "pack-", ".rev")));
    }
    append_notes(&pack_file(&appended, "pack-", ".rev"));
    replace_body(&pack_file(&rewritten, "pack-", ".rev"), 12);

    let res = scan_default(&root);
    assert_in_a_backup_set(&res, &appended, &rels[0]);
    assert_in_a_backup_set(&res, &rewritten, &rels[1]);
}

#[test]
fn z2_a_midx_bitmap_appended_to_is_in_a_backup_set_whatever_the_config() {
    let (tmp, root, remotes) = workspace();
    let on = midx_clone(tmp.path(), &root, &remotes, "on");
    let off = midx_clone(tmp.path(), &root, &remotes, "off");
    git(&off, &["config", "core.multiPackIndex", "false"]);
    let mut rels = Vec::new();
    for repo in [&on, &off] {
        let bitmap = pack_file(repo, "multi-pack-index-", ".bitmap");
        rels.push(rel_of(repo, &bitmap));
        append_notes(&bitmap);
    }

    let res = scan_default(&root);
    assert_in_a_backup_set(&res, &on, &rels[0]);
    assert_in_a_backup_set(&res, &off, &rels[1]);
}

#[test]
fn z2_a_midx_reverse_index_holding_notes_is_in_a_backup_set() {
    let (tmp, root, remotes) = workspace();
    let repo = midx_clone(tmp.path(), &root, &remotes, "app");
    let name = format!("multi-pack-index-{}.rev", midx_checksum(&repo));
    let path = pack_dir(&repo).join(&name);
    if path.exists() {
        append_notes(&path);
    } else {
        fs::write(&path, notes_after(b"RIDX")).unwrap();
    }
    let rel = format!("objects/pack/{name}");

    let res = scan_default(&root);
    assert!(store(&res, &repo).contains(&rel));
    assert_in_a_backup_set(&res, &repo, &rel);
}

#[test]
fn z2_cruft_mtimes_beside_a_pack_are_in_a_backup_set() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    git(&repo, &["repack", "-a", "-d", "-q"]);
    let mtimes = pack_file(&repo, "pack-", ".pack").with_extension("mtimes");
    fs::write(&mtimes, notes_after(b"MTME")).unwrap();
    let rel = rel_of(&repo, &mtimes);

    let res = scan_default(&root);
    assert!(store(&res, &repo).contains(&rel));
    assert_in_a_backup_set(&res, &repo, &rel);
}

#[test]
fn z2_a_commit_graph_with_bytes_appended_is_in_a_backup_set() {
    let (tmp, root, remotes) = workspace();
    let repo = clone_of(tmp.path(), &root, &remotes, "app");
    git(&repo, &["commit-graph", "write", "--reachable"]);
    let info = repo.join(".git").join("objects").join("info");
    append_notes(&info.join("commit-graph"));

    let res = scan_default(&root);
    assert_in_a_backup_set(&res, &repo, "objects/info/commit-graph");
}
