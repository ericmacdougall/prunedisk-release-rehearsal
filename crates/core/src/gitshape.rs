//! The positive shape of the directories inside a git directory: what git itself writes in
//! `objects/`, `refs/`, `logs/`, `rr-cache/`, `lfs/` and `fsmonitor--daemon/`.
//!
//! The scan's walk already visits every entry below a `.git` directory (it sizes it and looks
//! for symlinks). It judges each one here, by its path below the git directory and its kind;
//! only a regular file under `refs/` is read, at most [`MAX_REF_BYTES`] of it. An entry that is
//! not what git writes is the repository's own content, which recovery (commits and working-tree
//! files) does not save, so the repository asks (`no_local_admin_content`). The entries directly
//! inside the git directory are judged by `facts` (`TOP_LEVEL_KNOWN`), and the directories whose
//! contents other checks own (`hooks`, `info`, `branches`, `remotes` by `facts`; `worktrees` and
//! `modules`, never plain when not empty; an unfinished operation's directory, which
//! `no_pending_state` reports; an unknown entry, which makes the layout not plain) are not
//! judged again here.

use std::path::Path;

/// What an entry is, by `lstat` (never followed).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Dir,
    File,
    /// A symlink, socket, FIFO or device.
    Other,
}

/// What [`judge`] says about one entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Verdict {
    /// What git writes there, or not judged here.
    Fine,
    /// Not what git writes there: the repository's own content.
    Local,
    /// A regular file under `refs/`: fine only when it is at most [`MAX_REF_BYTES`] long and
    /// [`ref_content_ok`] accepts it.
    RefFile,
}

/// Most bytes a loose ref may hold: one object id or `ref: refs/...` line.
pub(crate) const MAX_REF_BYTES: u64 = 512;

fn hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A full object id: 40 (SHA-1) or 64 (SHA-256) lowercase hex digits.
pub(crate) fn oid(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && hex(s)
}

/// `<prefix><object id>.<ext>` with `ext` one of `exts`.
pub(crate) fn named(name: &str, prefix: &str, exts: &[&str]) -> bool {
    match name.strip_prefix(prefix).and_then(|r| r.split_once('.')) {
        Some((id, ext)) => oid(id) && exts.contains(&ext),
        None => false,
    }
}

/// The files git writes beside a pack: index, reverse index, bitmap, keep, cruft mtimes and
/// promisor marker.
const PACK_EXTS: [&str; 7] = ["pack", "idx", "rev", "bitmap", "keep", "mtimes", "promisor"];

/// Judge one entry below a git directory. `rel` is its path below the git directory, one item
/// per component; `kind` is what `lstat` says it is.
///
/// - `objects/`: two-hex-digit fanout directories of loose objects (38 or 62 hex digits),
///   `pack/` (`pack-<id>.{pack,idx,rev,bitmap,keep,mtimes,promisor}`, `multi-pack-index`,
///   `multi-pack-index-<id>.{bitmap,rev}`, and `multi-pack-index.d/` with
///   `multi-pack-index-chain` and `multi-pack-index-<id>.{midx,bitmap,rev}`) and `info/`
///   (`packs`, `commit-graph`, `commit-graphs/` with `commit-graph-chain` and
///   `graph-<id>.graph`, and the alternates files the layout check refuses). git's `tmp_*` and
///   `.tmp-*` leftovers are not here: they are an interrupted operation, which a person looks at.
/// - `refs/`: directories at any depth; every regular file must parse as a ref and not be a
///   `*.lock`.
/// - `logs/`: `HEAD` and anything under `logs/refs/` (the reflog check reads them).
/// - `rr-cache/`: directories named by an object id holding `preimage*`, `postimage*` and
///   `thisimage*` files.
/// - `lfs/`: `objects/` (local-only LFS content, which `no_pending_state` reports) and an empty
///   `tmp/`.
/// - `fsmonitor--daemon/`: `cookies/` holding regular files.
pub(crate) fn judge(rel: &[&str], kind: Kind) -> Verdict {
    use Kind::{Dir, File};
    match rel {
        [] | [_] => Verdict::Fine,
        ["objects", d] => ok(kind == Dir && (fanout(d) || matches!(*d, "pack" | "info"))),
        ["objects", "pack", f] => ok(pack_entry(f, kind)),
        ["objects", "pack", "multi-pack-index.d", f] => ok(kind == File && midx_layer(f)),
        ["objects", "info", f] => ok(info_entry(f, kind)),
        ["objects", "info", "commit-graphs", f] => ok(kind == File && graph_file(f)),
        ["objects", d, f] if fanout(d) => ok(kind == File && loose_object(f)),
        ["objects", ..] => Verdict::Local,
        // A lock left by an interrupted update holds a valid id, but is not a ref.
        ["refs", .., last] if last.ends_with(".lock") => Verdict::Local,
        ["refs", ..] => match kind {
            Dir => Verdict::Fine,
            File => Verdict::RefFile,
            Kind::Other => Verdict::Local,
        },
        ["logs", "HEAD"] => ok(kind == File),
        ["logs", "refs", ..] => ok(kind == Dir || kind == File),
        ["logs", ..] => Verdict::Local,
        ["rr-cache", id] => ok(kind == Dir && oid(id)),
        ["rr-cache", id, f] if oid(id) => ok(kind == File && rerere_image(f)),
        ["rr-cache", ..] => Verdict::Local,
        ["lfs", "objects", ..] => Verdict::Fine,
        ["lfs", "tmp"] => ok(kind == Dir),
        ["lfs", ..] => Verdict::Local,
        ["fsmonitor--daemon", "cookies"] => ok(kind == Dir),
        ["fsmonitor--daemon", "cookies", _] => ok(kind == File),
        ["fsmonitor--daemon", ..] => Verdict::Local,
        _ => Verdict::Fine,
    }
}

fn ok(fine: bool) -> Verdict {
    if fine {
        Verdict::Fine
    } else {
        Verdict::Local
    }
}

/// A loose-object fanout directory: two lowercase hex digits.
pub(crate) fn fanout(d: &str) -> bool {
    d.len() == 2 && hex(d)
}

/// A loose object's file name: the rest of a SHA-1 or SHA-256 id.
pub(crate) fn loose_object(f: &str) -> bool {
    (f.len() == 38 || f.len() == 62) && hex(f)
}

fn pack_entry(f: &str, kind: Kind) -> bool {
    match kind {
        Kind::File => {
            f == "multi-pack-index"
                || named(f, "pack-", &PACK_EXTS)
                || named(f, "multi-pack-index-", &["bitmap", "rev"])
        }
        Kind::Dir => f == "multi-pack-index.d",
        Kind::Other => false,
    }
}

/// A file of an incremental multi-pack index.
fn midx_layer(f: &str) -> bool {
    f == "multi-pack-index-chain" || named(f, "multi-pack-index-", &["midx", "bitmap", "rev"])
}

fn info_entry(f: &str, kind: Kind) -> bool {
    match kind {
        Kind::File => matches!(
            f,
            "packs" | "commit-graph" | "alternates" | "http-alternates"
        ),
        Kind::Dir => f == "commit-graphs",
        Kind::Other => false,
    }
}

/// A file of a split commit-graph.
fn graph_file(f: &str) -> bool {
    f == "commit-graph-chain" || named(f, "graph-", &["graph"])
}

/// A recorded conflict: `preimage`, `postimage` or `thisimage`, possibly with a variant suffix.
fn rerere_image(f: &str) -> bool {
    ["preimage", "postimage", "thisimage"]
        .iter()
        .any(|p| f.starts_with(p))
}

/// Does a loose ref file hold one object id or `ref: refs/...` line (and nothing else)?
pub(crate) fn ref_content_ok(bytes: &[u8]) -> bool {
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let line = text.strip_suffix('\n').unwrap_or(text);
    if line.contains('\n') {
        return false;
    }
    if oid(line) {
        return true;
    }
    match line.strip_prefix("ref: refs/") {
        Some(t) => !t.is_empty() && !t.bytes().any(|b| b.is_ascii_whitespace()),
        None => false,
    }
}

/// The components of `path` below its innermost `.git` component (lossy, for display and
/// matching), or `None` when no component is `.git` or `path` is that directory itself.
pub(crate) fn below_git_dir(path: &Path) -> Option<Vec<String>> {
    // Every walked entry passes here: nothing is allocated unless a component is `.git`.
    let mut last_git = None;
    for (i, c) in path.components().enumerate() {
        if c.as_os_str() == ".git" {
            last_git = Some(i);
        }
    }
    let at = last_git?;
    let rest: Vec<String> = path
        .components()
        .skip(at + 1)
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    (!rest.is_empty()).then_some(rest)
}

/// Read at most [`MAX_REF_BYTES`] of a regular file under `refs/` and judge it. An entry that is
/// no longer a regular file when it is opened is an error ([`crate::regular::open_regular`]).
pub(crate) fn ref_file_ok(path: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut buf = Vec::new();
    crate::regular::open_regular(path)?
        .take(MAX_REF_BYTES + 1)
        .read_to_end(&mut buf)?;
    Ok(buf.len() as u64 <= MAX_REF_BYTES && ref_content_ok(&buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Kind::{Dir, File, Other};
    use Verdict::{Fine, Local, RefFile};

    fn j(rel: &str, kind: Kind) -> Verdict {
        let parts: Vec<&str> = rel.split('/').collect();
        judge(&parts, kind)
    }

    #[test]
    fn what_git_writes_is_fine() {
        let id = "0123456789abcdef0123456789abcdef01234567";
        let loose = &id[2..];
        let fine = [
            ("objects/ab".to_string(), Dir),
            (format!("objects/01/{loose}"), File),
            ("objects/pack".to_string(), Dir),
            ("objects/info".to_string(), Dir),
            (format!("objects/pack/pack-{id}.pack"), File),
            (format!("objects/pack/pack-{id}.idx"), File),
            (format!("objects/pack/pack-{id}.rev"), File),
            (format!("objects/pack/pack-{id}.bitmap"), File),
            (format!("objects/pack/pack-{id}.keep"), File),
            (format!("objects/pack/pack-{id}.mtimes"), File),
            (format!("objects/pack/pack-{id}.promisor"), File),
            ("objects/pack/multi-pack-index".to_string(), File),
            (format!("objects/pack/multi-pack-index-{id}.bitmap"), File),
            ("objects/pack/multi-pack-index.d".to_string(), Dir),
            (
                "objects/pack/multi-pack-index.d/multi-pack-index-chain".to_string(),
                File,
            ),
            (
                format!("objects/pack/multi-pack-index.d/multi-pack-index-{id}.midx"),
                File,
            ),
            ("objects/info/packs".to_string(), File),
            ("objects/info/commit-graph".to_string(), File),
            ("objects/info/commit-graphs".to_string(), Dir),
            (
                "objects/info/commit-graphs/commit-graph-chain".to_string(),
                File,
            ),
            (format!("objects/info/commit-graphs/graph-{id}.graph"), File),
            ("refs/heads/feature/x".to_string(), Dir),
            ("logs/HEAD".to_string(), File),
            ("logs/refs/heads/main".to_string(), File),
            ("logs/refs/remotes".to_string(), Dir),
            (format!("rr-cache/{id}"), Dir),
            (format!("rr-cache/{id}/preimage"), File),
            (format!("rr-cache/{id}/postimage.1"), File),
            ("lfs/objects/ab/cd/x".to_string(), File),
            ("lfs/tmp".to_string(), Dir),
            ("fsmonitor--daemon/cookies".to_string(), Dir),
            ("fsmonitor--daemon/cookies/123-0".to_string(), File),
            ("hooks/pre-commit".to_string(), File),
            ("modules/sub/notes.txt".to_string(), File),
            ("HEAD".to_string(), File),
        ];
        for (rel, kind) in fine {
            assert_eq!(j(&rel, kind), Fine, "{rel}");
        }
        assert_eq!(j("refs/heads/main", File), RefFile);
    }

    #[test]
    fn anything_else_is_local() {
        let id = "0123456789abcdef0123456789abcdef01234567";
        let local = [
            ("objects/notes.txt".to_string(), File),
            ("objects/ab/README".to_string(), File),
            ("objects/AB".to_string(), Dir),
            ("objects/ab/sub".to_string(), Dir),
            ("objects/pack/backup.tar".to_string(), File),
            ("objects/pack/tmp_pack_abc123".to_string(), File),
            (format!("objects/pack/.tmp-1-pack-{id}.pack"), File),
            (format!("objects/pack/pack-{id}.tar"), File),
            (format!("objects/pack/pack-{id}.pack"), Other),
            ("objects/pack/old".to_string(), Dir),
            ("objects/info/notes".to_string(), File),
            ("objects/info/commit-graphs/tmp_graph_x".to_string(), File),
            ("objects/pack/multi-pack-index.d/notes".to_string(), File),
            ("refs/heads/link".to_string(), Other),
            ("refs/heads/main.lock".to_string(), File),
            ("logs/notes.txt".to_string(), File),
            ("logs/old".to_string(), Dir),
            ("rr-cache/notes.txt".to_string(), File),
            ("rr-cache/abc".to_string(), Dir),
            (format!("rr-cache/{id}/notes"), File),
            ("lfs/notes".to_string(), File),
            ("lfs/tmp/partial".to_string(), File),
            ("fsmonitor--daemon/notes".to_string(), File),
        ];
        for (rel, kind) in local {
            assert_eq!(j(&rel, kind), Local, "{rel}");
        }
    }

    #[test]
    fn ref_files_must_parse() {
        let id = "0123456789abcdef0123456789abcdef01234567";
        assert!(ref_content_ok(format!("{id}\n").as_bytes()));
        assert!(ref_content_ok(b"ref: refs/remotes/origin/main\n"));
        for bad in [
            &b"todo: rebase later\n"[..],
            b"",
            b"ref: HEAD\n",
            b"ref: refs/\n",
            b"\xff\n",
        ] {
            assert!(!ref_content_ok(bad), "{bad:?}");
        }
        assert!(!ref_content_ok(format!("{id}\n{id}\n").as_bytes()));
    }

    #[test]
    fn paths_are_taken_below_the_innermost_git_directory() {
        let p = Path::new("w").join(".git").join("objects").join("ab");
        assert_eq!(
            below_git_dir(&p),
            Some(vec!["objects".to_string(), "ab".to_string()])
        );
        let nested = Path::new("w")
            .join(".git")
            .join("x")
            .join(".git")
            .join("refs");
        assert_eq!(below_git_dir(&nested), Some(vec!["refs".to_string()]));
        assert_eq!(below_git_dir(&Path::new("w").join(".git")), None);
        assert_eq!(below_git_dir(Path::new("w/src")), None);
    }

    /// Review M5: a ref file the walk saw as regular can be replaced by a FIFO before it is read.
    #[cfg(unix)]
    #[test]
    fn a_ref_file_swapped_for_a_fifo_is_unread_without_blocking() {
        use crate::test_fifo::{mkfifo, must_not_block};
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("main");
        mkfifo(&fifo);
        let p = fifo.clone();
        let got = must_not_block(&fifo, move || ref_file_ok(&p).map_err(|e| e.to_string()));
        assert_eq!(got, Err("not a regular file".to_string()));
    }
}
