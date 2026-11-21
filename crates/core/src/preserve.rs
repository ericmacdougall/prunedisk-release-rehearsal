//! The preserve obligation: what a removal of a git node must back up first.
//!
//! Every file of a git directory belongs to exactly one of two classes, decided by its path
//! alone (no content is read to decide):
//!
//! - **the object store** ([`object_store_files`]): files in `objects/` whose names are those of
//!   object-store files: loose objects, packs, their indexes, reverse indexes, bitmaps and cruft
//!   mtimes, multi-pack-indexes and their sidecars and layers, commit-graphs. They can be very
//!   large, so they are counted apart, but a removal must back each of them up too. This format
//!   defines no verification of object data: a name confers no exemption, only membership in
//!   this class. (A later apply phase may define a verification against the live remote; this
//!   crate has none.)
//! - **the git directory's own files** ([`git_admin_files`]): every other non-directory entry
//!   (config, hooks, refs, logs, the index, messages, rerere's records, `.keep` and `.promisor`
//!   markers, chain files, `objects/info` lists, and anything a person put anywhere). A removal
//!   always backs these up.
//!
//! [`sort`] is the one classifier: the scan's [`survey`], [`git_admin_files`] and
//! [`object_store_files`] all go through it, so they always agree.

use crate::gitshape::{fanout, loose_object, named};
use crate::model::{AdminFile, Class, GitAdminSurvey, Node, ObjectStoreSurvey};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Most entries walked in one git directory; more cannot be enumerated and is an error.
pub const MAX_ENTRIES: u64 = 2_000_000;
/// Examples kept in a [`GitAdminSurvey`].
const EXAMPLES: usize = 5;
/// Largest files kept in a [`GitAdminSurvey`].
const LARGEST: usize = 3;

/// The hooks `git init` installs from its stock template (git 2.56).
const STOCK_HOOKS: [&str; 14] = [
    "applypatch-msg.sample",
    "commit-msg.sample",
    "fsmonitor-watchman.sample",
    "post-update.sample",
    "pre-applypatch.sample",
    "pre-commit.sample",
    "pre-merge-commit.sample",
    "pre-push.sample",
    "pre-rebase.sample",
    "pre-receive.sample",
    "prepare-commit-msg.sample",
    "push-to-checkout.sample",
    "sendemail-validate.sample",
    "update.sample",
];

/// The pack-side files of the object store (`.keep` and `.promisor` are markers a person or a
/// partial clone set: they are the git directory's own).
const PACK_STORE_EXTS: [&str; 5] = ["pack", "idx", "rev", "bitmap", "mtimes"];

/// One git directory's files, sorted into the two classes.
#[derive(Debug, Default)]
pub struct Sorted {
    /// Its own files: path relative to the git directory with `/` separators, and allocated
    /// bytes. Sorted by path.
    pub own: Vec<(String, u64)>,
    /// The object store's files, the same way.
    pub store: Vec<(String, u64)>,
    /// Entries with an object-store name that are not regular files (a symlink, a socket): they
    /// are not git data, so they are in `own`, and the repository asks.
    pub not_objects: Vec<String>,
}

/// What [`survey`] found: the counts for the plan, and what the scan must report.
#[derive(Debug, Default)]
pub struct Survey {
    pub admin: GitAdminSurvey,
    pub store: ObjectStoreSurvey,
    pub not_objects: Vec<String>,
}

/// The git directory a node's removal deletes: `<path>/.git` for a repo or worktree whose `.git`
/// is a real directory, the directory itself for a bare repo.
pub fn git_dir_of(node: &Node) -> io::Result<PathBuf> {
    let path = Path::new(&node.path);
    match node.class {
        Class::BareRepo => Ok(path.to_path_buf()),
        Class::Repo | Class::Worktree => {
            let gd = path.join(".git");
            if fs::symlink_metadata(&gd)?.is_dir() {
                Ok(gd)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the .git entry is not a directory, so the git directory lies elsewhere",
                ))
            }
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a repo, worktree or bare repo",
        )),
    }
}

/// Every file in `node`'s git directory that is not an object-store file, as paths relative to
/// the git directory with `/` separators, sorted. A removal must back up, and verify the backup
/// of, every one of them before it removes anything. An error (an unreadable directory, a name
/// that is not UTF-8, too many entries) means the list is not complete: nothing may be removed
/// then.
pub fn git_admin_files(node: &Node) -> io::Result<Vec<String>> {
    let gd = git_dir_of(node)?;
    Ok(sort(&gd)?.own.into_iter().map(|(p, _)| p).collect())
}

/// Every file in `node`'s git directory whose name is that of an object-store file, the same
/// way. A removal must back up each of them too before it removes anything (this format defines
/// no verification that could stand in for that). Together with [`git_admin_files`] it lists
/// every non-directory entry of the git directory exactly once.
pub fn object_store_files(node: &Node) -> io::Result<Vec<String>> {
    let gd = git_dir_of(node)?;
    Ok(sort(&gd)?.store.into_iter().map(|(p, _)| p).collect())
}

/// Count `git_dir`'s two classes for the plan (see [`sort`]).
pub fn survey(git_dir: &Path) -> io::Result<Survey> {
    let sorted = sort(git_dir)?;
    let store = ObjectStoreSurvey {
        files: sorted.store.len() as u64,
        bytes: sorted.store.iter().map(|(_, b)| *b).sum(),
    };
    Ok(Survey {
        admin: admin_survey(&sorted.own),
        store,
        not_objects: sorted.not_objects,
    })
}

/// The plan's counts of a git directory's own files.
fn admin_survey(own: &[(String, u64)]) -> GitAdminSurvey {
    let mut admin = GitAdminSurvey {
        files: own.len() as u64,
        bytes: own.iter().map(|(_, b)| *b).sum(),
        ..GitAdminSurvey::default()
    };
    admin.examples = own
        .iter()
        .filter(|(p, _)| !everyday(p))
        .take(EXAMPLES)
        .map(|(p, _)| p.clone())
        .collect();
    let mut by_size: Vec<&(String, u64)> = own.iter().collect();
    by_size.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    admin.largest = by_size
        .into_iter()
        .take(LARGEST)
        .map(|(p, b)| AdminFile {
            path: p.clone(),
            bytes: *b,
        })
        .collect();
    admin
}

/// A name every clone has (or git writes in ordinary use): not worth naming as an example.
/// Examples go by name only; the backup covers every file whatever its name or content.
fn everyday(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.as_slice() {
        [top] => matches!(
            *top,
            "HEAD"
                | "config"
                | "description"
                | "index"
                | "packed-refs"
                | "FETCH_HEAD"
                | "ORIG_HEAD"
                | "shallow"
        ),
        ["info", "exclude" | "refs"] | ["objects", "info", "packs"] | ["logs", "HEAD"] => true,
        ["hooks", name] => STOCK_HOOKS.contains(name),
        ["refs", rest @ ..] | ["logs", "refs", rest @ ..] => {
            !rest.is_empty() && rest.iter().all(|p| !p.starts_with('.'))
        }
        _ => false,
    }
}

/// Is this path (relative to the git directory, split at `/`) the name of an object-store file?
fn store_name(parts: &[&str]) -> bool {
    match parts {
        ["objects", d, f] if fanout(d) && loose_object(f) => true,
        ["objects", "pack", "multi-pack-index"] => true,
        ["objects", "pack", f] => {
            named(f, "pack-", &PACK_STORE_EXTS) || named(f, "multi-pack-index-", &["bitmap", "rev"])
        }
        ["objects", "pack", "multi-pack-index.d", f] => {
            named(f, "multi-pack-index-", &["midx", "bitmap", "rev"])
        }
        ["objects", "info", "commit-graph"] => true,
        ["objects", "info", "commit-graphs", f] => named(f, "graph-", &["graph"]),
        _ => false,
    }
}

/// Allocated bytes of an entry (blocks on Unix, the length elsewhere).
#[cfg(unix)]
fn allocated(md: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    md.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn allocated(md: &fs::Metadata) -> u64 {
    md.len()
}

/// Sort every non-directory entry of `git_dir` into its own files and the object store's files,
/// by path alone. Symlinks are never followed; a symlink or any other non-directory entry is a
/// file of the git directory's own (and, under an object-store name, also in `not_objects`).
/// Bounded: more than [`MAX_ENTRIES`] entries is an error, as is any entry that cannot be listed
/// or named in UTF-8.
pub fn sort(git_dir: &Path) -> io::Result<Sorted> {
    let mut out = Sorted::default();
    let mut seen = 0u64;
    let mut stack: Vec<(PathBuf, String)> = vec![(git_dir.to_path_buf(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        for e in fs::read_dir(&dir)? {
            let e = e?;
            seen += 1;
            if seen > MAX_ENTRIES {
                return Err(io::Error::other(format!(
                    "more than {MAX_ENTRIES} entries in the git directory"
                )));
            }
            let name = e.file_name().into_string().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "an entry in the git directory has a name that is not UTF-8",
                )
            })?;
            let md = e.metadata()?;
            let child = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if md.is_dir() {
                stack.push((dir.join(&name), child));
                continue;
            }
            let parts: Vec<&str> = child.split('/').collect();
            if store_name(&parts) {
                if !md.is_file() {
                    out.not_objects.push(child.clone());
                } else {
                    out.store.push((child, allocated(&md)));
                    continue;
                }
            }
            out.own.push((child, allocated(&md)));
        }
    }
    out.own.sort();
    out.store.sort();
    out.not_objects.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(s: &str) -> Vec<&str> {
        s.split('/').collect()
    }

    #[test]
    fn names_alone_say_which_files_are_the_object_stores() {
        let id = "0123456789abcdef0123456789abcdef01234567";
        let store = |s: String| store_name(&parts(&s));
        assert!(store(format!("objects/ab/{}", &id[2..])));
        assert!(store(format!("objects/pack/pack-{id}.pack")));
        assert!(store(format!("objects/pack/pack-{id}.mtimes")));
        assert!(store(format!("objects/pack/multi-pack-index-{id}.bitmap")));
        assert!(store(format!("objects/pack/multi-pack-index-{id}.rev")));
        assert!(store(format!(
            "objects/pack/multi-pack-index.d/multi-pack-index-{id}.midx"
        )));
        assert!(store(format!(
            "objects/info/commit-graphs/graph-{id}.graph"
        )));
        assert!(store_name(&parts("objects/pack/multi-pack-index")));
        assert!(store_name(&parts("objects/info/commit-graph")));
        // Markers, lists, chains and anything else are the git directory's own.
        assert!(!store(format!("objects/pack/pack-{id}.keep")));
        assert!(!store(format!("objects/pack/pack-{id}.promisor")));
        assert!(!store(format!("objects/pack/pack-{id}.pack.notes")));
        for own in [
            "objects/info/packs",
            "objects/info/alternates",
            "objects/info/commit-graphs/commit-graph-chain",
            "objects/pack/multi-pack-index.d/multi-pack-index-chain",
            "objects/notes.txt",
            "objects/ab/README",
            "objects/pack/pack-x.pack",
            "HEAD",
            "hooks/pre-commit.sample",
            "lfs/objects/ab/cd/abcd",
        ] {
            assert!(!store_name(&parts(own)), "{own}");
        }
    }

    #[test]
    fn every_derived_kind_is_in_the_object_store_whatever_it_holds() {
        let id = "0123456789abcdef0123456789abcdef01234567";
        let files = [
            format!("objects/pack/pack-{id}.mtimes"),
            format!("objects/pack/multi-pack-index-{id}.rev"),
            format!("objects/pack/multi-pack-index.d/multi-pack-index-{id}.midx"),
            format!("objects/pack/pack-{id}.rev"),
            format!("objects/pack/pack-{id}.bitmap"),
            format!("objects/pack/multi-pack-index-{id}.bitmap"),
            "objects/pack/multi-pack-index".to_string(),
            "objects/info/commit-graph".to_string(),
        ];
        let dir = tempfile::tempdir().unwrap();
        for f in &files {
            let p = dir.path().join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"my notes, not git data\n").unwrap();
        }
        let sorted = sort(dir.path()).unwrap();
        assert!(sorted.own.is_empty(), "{:?}", sorted.own);
        let mut store: Vec<String> = sorted.store.into_iter().map(|(p, _)| p).collect();
        store.sort();
        let mut want = files.to_vec();
        want.sort();
        assert_eq!(store, want);
    }

    #[test]
    fn examples_skip_names_every_clone_has() {
        for every in [
            "HEAD",
            "config",
            "index",
            "info/exclude",
            "hooks/pre-commit.sample",
            "refs/heads/main",
            "logs/refs/remotes/origin/HEAD",
            "logs/HEAD",
        ] {
            assert!(everyday(every), "{every}");
        }
        for unusual in [
            "COMMIT_EDITMSG",
            "hooks/custom.sample",
            "hooks/pre-commit",
            "logs/refs/.notes",
            "rr-cache/x/postimage",
            "objects/pack/pack-x.keep",
        ] {
            assert!(!everyday(unusual), "{unusual}");
        }
    }
}
