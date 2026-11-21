//! Repo safety facts (design section 3), gathered with read-only git commands.
//!
//! Every fact is `Yes`, `No` or `Unknown`. Any git failure, timeout or unparseable output is
//! `Unknown`; it is never promoted to the safe value. Remote URLs lose embedded credentials
//! before they are stored.

use crate::git::{path_from_git, rel_git_bytes, single_path, Git, GitError};
use crate::model::{Class, Facts, Remote, RepoInfo, Tri};
use crate::redact::{config_key_label, diag, strip_url_credentials};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Decides whether a (credential-free) remote URL is a network location.
pub type NetworkPredicate = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// How facts are gathered. The network predicate is injectable so tests can treat a local
/// bare repository as "the network" without changing production behaviour.
#[derive(Clone)]
pub struct FactsConfig {
    pub git: Git,
    pub is_network: NetworkPredicate,
}

impl Default for FactsConfig {
    fn default() -> Self {
        FactsConfig {
            git: Git::default(),
            is_network: Arc::new(is_network_url),
        }
    }
}

impl FactsConfig {
    /// Use a different git binary or timeout.
    pub fn with_git(mut self, git: Git) -> Self {
        self.git = git;
        self
    }

    /// Replace the network predicate (tests only; production uses [`is_network_url`]).
    pub fn with_network_predicate(
        mut self,
        f: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.is_network = Arc::new(f);
        self
    }
}

/// Production rule: a local path, a `file://` URL or a `.bundle` file is not a network remote
/// (design section 3: it is not a backup); `scheme://host`, `user@host:path` and `host:path`
/// are.
pub fn is_network_url(url: &str) -> bool {
    let u = url.trim();
    if u.is_empty() {
        return false;
    }
    let lower = u.to_ascii_lowercase();
    if lower.ends_with(".bundle") || lower.starts_with("file:") {
        return false;
    }
    if let Some(idx) = lower.find("://") {
        return idx > 0;
    }
    if u == "." || u == ".." || u.starts_with(['/', '\\', '~']) {
        return false;
    }
    if u.starts_with("./") || u.starts_with("../") || u.starts_with(".\\") || u.starts_with("..\\")
    {
        return false;
    }
    let b = u.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\')
    {
        return false; // Windows drive path
    }
    match (u.find(':'), u.find('/')) {
        (None, _) => false, // plain relative path
        (Some(c), Some(s)) if s < c => false,
        _ => true, // scp-style host:path
    }
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn note(what: &str, e: &GitError) -> String {
    diag(&format!("{what}: {e}"))
}

/// Diagnostic lines of one repo. Every line passes through [`diag`] on the way in, so nothing a
/// repository controls (a file name, a git error) reaches a stored note unredacted.
#[derive(Default)]
struct Notes(Vec<String>);

impl Notes {
    fn push(&mut self, text: impl AsRef<str>) {
        self.0.push(diag(text.as_ref()));
    }
}

fn parse_remotes(out: &str, cfg: &FactsConfig) -> Vec<Remote> {
    let mut remotes = Vec::new();
    for line in out.lines() {
        let (name, rest) = match line.split_once('\t') {
            Some(x) => x,
            None => continue,
        };
        let url = match rest.strip_suffix(" (fetch)") {
            Some(u) => strip_url_credentials(u.trim()),
            None => continue,
        };
        let network = (cfg.is_network)(&url);
        remotes.push(Remote {
            name: diag(name),
            url,
            network,
        });
    }
    remotes
}

/// Is the `/`-separated path `p` the directory `dir` or inside it? Both are git's bytes.
fn under(p: &[u8], dir: &[u8]) -> bool {
    p == dir || (p.starts_with(dir) && p.get(dir.len()) == Some(&b'/'))
}

/// Text for a message from bytes git printed (a file name): lossy, for display only. Every note
/// passes through [`diag`] before it is stored.
fn shown(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A path for a stored message or list (lossy, redacted); never used to look anything up.
fn path_label(p: &Path) -> String {
    diag(&p.to_string_lossy())
}

/// The worktrees `git worktree list --porcelain` names, as exact paths, main worktree first.
///
/// `-z` (git 2.36 and later) separates fields with NUL, so any name is read as it is. An older git
/// refuses `-z`; then the line format is read, and anything it could have quoted or split (a
/// quoted path, a line that is not a known attribute) is an error rather than a guess.
fn worktree_paths(git: &Git, path: &Path) -> Result<Vec<PathBuf>, String> {
    let field = |f: &[u8]| -> Result<Option<PathBuf>, String> {
        match f.strip_prefix(b"worktree ") {
            Some(p) => path_from_git(p)
                .map(Some)
                .ok_or_else(|| "a worktree path is not valid on this system".to_string()),
            None => Ok(None),
        }
    };
    let z = ["worktree", "list", "--porcelain", "-z"];
    if let Ok(out) = git.run_bytes(path, &z) {
        let mut found = Vec::new();
        for f in out.split(|b| *b == 0) {
            if let Some(p) = field(f)? {
                found.push(p);
            }
        }
        return Ok(found);
    }
    let out = git
        .run_bytes(path, &["worktree", "list", "--porcelain"])
        .map_err(|e| note("worktree list", &e))?;
    const KNOWN: [&[u8]; 6] = [
        b"HEAD ",
        b"branch ",
        b"bare",
        b"detached",
        b"locked",
        b"prunable",
    ];
    let mut found = Vec::new();
    for line in out.split(|b| *b == b'\n') {
        if line.starts_with(b"worktree \"") {
            return Err("git quoted a worktree path in its line format".to_string());
        }
        if let Some(p) = field(line)? {
            found.push(p);
            continue;
        }
        let known = line.is_empty() || KNOWN.iter().any(|k| line.starts_with(k));
        if !known {
            return Err("git printed a worktree list line that is not understood".to_string());
        }
    }
    Ok(found)
}

/// The path a command such as `rev-parse --show-toplevel` prints, exactly (see
/// [`single_path`]), or why it could not be read; a relative one is taken from `path`.
fn printed_path(git: &Git, path: &Path, args: &[&str], what: &str) -> Result<PathBuf, String> {
    let out = git.run_bytes(path, args).map_err(|e| note(what, &e))?;
    let p = single_path(&out)
        .ok_or_else(|| format!("{what}: git printed a path that is not valid on this system"))?;
    Ok(if p.is_absolute() { p } else { path.join(p) })
}

/// What an entry is, by `lstat`: never followed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lk {
    Missing,
    Dir,
    File,
    Link,
    Other,
}

fn lk(p: &Path) -> Result<Lk, String> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_symlink() => Ok(Lk::Link),
        Ok(m) if m.is_dir() => Ok(Lk::Dir),
        Ok(m) if m.is_file() => Ok(Lk::File),
        Ok(_) => Ok(Lk::Other),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Lk::Missing),
        Err(e) => Err(e.to_string()),
    }
}

/// Refs a plain repository may have: branches, tags, remote-tracking refs and the stash.
fn plain_ref_name(name: &str) -> bool {
    name == "refs/stash"
        || name.starts_with("refs/heads/")
        || name.starts_with("refs/tags/")
        || name.starts_with("refs/remotes/")
}

/// Does this repository have the plain layout? `Yes` only when every check holds; `No` (with the
/// first failing check) when one fails; `Unknown` when a check could not be made. All checks are
/// `lstat`, directory listings, names and bounded reads, plus one `for-each-ref` through the
/// hardened runner; nothing is followed.
///
/// Checks, in order: `.git` is a real directory; `HEAD`, `config`, `refs`, `objects`,
/// `objects/info` and `objects/pack` exist as the right kind of entry and none is a symlink; none
/// of `objects/info/alternates`, `objects/info/http-alternates`, `commondir`, `info/grafts` or
/// `annex` exists; `worktrees` and `modules` are absent or empty; no unfinished operation and no
/// local-only LFS or annex content ([`pending_state`]); every ref lies under `refs/heads/`,
/// `refs/tags/`, `refs/remotes/` or is `refs/stash`, and points at a commit or at an annotated tag
/// whose target is a commit. (The config allowlist runs before any of this.)
/// Outcome of the structural part of the layout check.
enum Shape {
    Fine,
    /// A check failed. `skip_git` is true when git must not be run in this repo at all: an
    /// entry git itself reads (`HEAD`, `config`, `refs`, `objects`) is missing, a symlink or the
    /// wrong kind, or a `commondir` would send git to another repository. It is false for a
    /// layout that is merely unusual but healthy (a `.git` file, alternates, worktrees, modules).
    No {
        issue: String,
        skip_git: bool,
    },
    Unknown(String),
}

/// The layout issue of a repository with linked worktrees; [`collect_repo_info`] adds the paths
/// git reports.
const WORKTREES_NOT_EMPTY: &str = "worktrees is not empty";

/// The structural checks of [`layout_check`] (the real `.git` directory, the expected entries, the
/// forbidden entries, empty `worktrees` and `modules`). They are `lstat` and directory listings
/// only, so they run before any git command and decide whether git may be run at all.
fn shape(path: &Path, class: Class) -> Shape {
    macro_rules! no {
        ($($a:tt)*) => {
            return Shape::No {
                issue: format!($($a)*),
                skip_git: true,
            }
        };
    }
    macro_rules! no_soft {
        ($($a:tt)*) => {
            return Shape::No {
                issue: format!($($a)*),
                skip_git: false,
            }
        };
    }
    let gd = if class == Class::BareRepo {
        path.to_path_buf()
    } else {
        let g = path.join(".git");
        match lk(&g) {
            Ok(Lk::Dir) => g,
            Ok(Lk::Missing) => no!(".git is missing"),
            Ok(Lk::File) => {
                return Shape::No {
                    issue: ".git is a file, not a real directory (a linked worktree, submodule \
                            checkout or --separate-git-dir repository: not handled in this phase)"
                        .to_string(),
                    skip_git: false,
                }
            }
            Ok(_) => no!(
                ".git is not a real directory (a linked worktree, submodule checkout, \
                 --separate-git-dir repository or a symlink: not handled in this phase)"
            ),
            Err(e) => return Shape::Unknown(format!("cannot read .git ({e})")),
        }
    };
    let mut unknown: Option<String> = None;
    for (name, want_dir) in [
        ("HEAD", false),
        ("config", false),
        ("refs", true),
        ("objects", true),
        ("objects/info", true),
        ("objects/pack", true),
    ] {
        match lk(&gd.join(name)) {
            Ok(Lk::Missing) => no!("{name} is missing from the git directory"),
            Ok(Lk::Link) => no!("{name} is a symlink"),
            Ok(Lk::Dir) if want_dir => {}
            Ok(Lk::File) if !want_dir => {}
            Ok(_) => no!("{name} is not the expected kind of entry"),
            Err(e) => {
                unknown.get_or_insert(format!("cannot read {name} ({e})"));
            }
        }
    }
    for (name, skip_git) in [
        ("objects/info/alternates", false),
        ("objects/info/http-alternates", false),
        ("commondir", true),
        ("info/grafts", false),
        ("annex", false),
    ] {
        match lk(&gd.join(name)) {
            Ok(Lk::Missing) => {}
            Ok(_) => {
                return Shape::No {
                    issue: format!(
                        "{name} exists (shared objects, a moved git directory or other local \
                         state)"
                    ),
                    skip_git,
                }
            }
            Err(e) => {
                unknown.get_or_insert(format!("cannot read {name} ({e})"));
            }
        }
    }
    for name in ["worktrees", "modules"] {
        match lk(&gd.join(name)) {
            Ok(Lk::Missing) => {}
            Ok(Lk::Dir) => match std::fs::read_dir(gd.join(name)) {
                Ok(mut rd) => {
                    if rd.next().is_some() {
                        if name == "worktrees" {
                            no_soft!("{WORKTREES_NOT_EMPTY}");
                        }
                        no_soft!("{name} is not empty");
                    }
                }
                Err(e) => {
                    unknown.get_or_insert(format!("cannot read {name} ({e})"));
                }
            },
            Ok(_) => no_soft!("{name} is not a plain directory"),
            Err(e) => {
                unknown.get_or_insert(format!("cannot read {name} ({e})"));
            }
        }
    }
    // A positive model of the top level: every entry is known, or a pseudoref whose commit is
    // checked later. Anything else may keep data where this tool does not look.
    match top_level(&gd) {
        Ok(top) if !top.unknown.is_empty() => {
            let n = top.unknown.len();
            let shown_names: Vec<&str> = top.unknown.iter().take(5).map(|s| s.as_str()).collect();
            let more = if n > 5 {
                format!(" and {} more", n - 5)
            } else {
                String::new()
            };
            no_soft!(
                "the git directory holds {} this tool does not know ({}{more}), which may keep \
                 data where it does not look",
                if n == 1 { "an entry" } else { "entries" },
                shown_names.join(", ")
            );
        }
        Ok(_) => {}
        Err(why) => {
            unknown.get_or_insert(why);
        }
    }
    match unknown {
        Some(why) => Shape::Unknown(why),
        None => Shape::Fine,
    }
}

fn layout_check(git: &Git, path: &Path, class: Class) -> (Tri, Option<String>) {
    macro_rules! no {
        ($($a:tt)*) => {
            return (Tri::No, Some(format!($($a)*)))
        };
    }
    let mut unknown: Option<String> = None;
    match shape(path, class) {
        Shape::Fine => {}
        Shape::No { issue, .. } => return (Tri::No, Some(issue)),
        Shape::Unknown(why) => unknown = Some(why),
    }
    match pending_state(git, path) {
        Ok(found) if found.is_empty() => {}
        Ok(found) => no!(
            "an unfinished operation or local-only content is present ({})",
            found.join(", ")
        ),
        Err(why) => {
            unknown.get_or_insert(why);
        }
    }
    match git.run(
        path,
        &[
            "for-each-ref",
            // `%(type)` is a tag object's direct target type; `%(*objecttype)` would peel a
            // tag of a tag all the way to the commit and hide it.
            "--format=%(refname) %(objecttype) %(type) %(*objecttype)",
        ],
    ) {
        Ok(out) => {
            for line in out.lines() {
                let mut it = line.split(' ');
                let name = it.next().unwrap_or("");
                let ty = it.next().unwrap_or("");
                let direct = it.next().unwrap_or("");
                let peeled = it.next().unwrap_or("");
                if !plain_ref_name(name) {
                    no!("the ref {name} is outside refs/heads, refs/tags, refs/remotes and refs/stash");
                }
                match (ty, direct) {
                    ("commit", _) | ("tag", "commit") => {}
                    _ => no!(
                        "the ref {name} does not point at a commit ({})",
                        ref_kind(ty, direct, peeled)
                    ),
                }
            }
        }
        Err(e) => {
            unknown.get_or_insert(note("refs", &e));
        }
    }
    match unknown {
        Some(why) => (Tri::Unknown, Some(why)),
        None => (Tri::Yes, None),
    }
}

/// What a ref that is not a commit points at, naming only the parts `for-each-ref` printed:
/// `it is a blob`, `it is a tag whose target is a tree`, `... peeling to a blob`.
fn ref_kind(ty: &str, direct: &str, peeled: &str) -> String {
    if ty.is_empty() {
        return "its object type could not be read".to_string();
    }
    let target = if direct.is_empty() {
        String::new()
    } else {
        format!(" whose target is a {direct}")
    };
    let peel = if peeled.is_empty() || peeled == direct {
        String::new()
    } else {
        format!(", peeling to a {peeled}")
    };
    format!("it is a {ty}{target}{peel}")
}

/// Names that, present in a worktree's own git directory, show an operation that is not finished.
/// Its state (a rebase todo list, conflict resolutions, the commits a sequence still has to apply)
/// lives in these files and not in any ref, and is deleted with the repo.
const IN_PROGRESS: [&str; 9] = [
    "rebase-merge",
    "rebase-apply",
    "sequencer",
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "BISECT_LOG",
    "BISECT_START",
    "BISECT_TERMS",
];

/// Directories of the shared git directory that hold content not recorded in any ref: Git LFS
/// objects and git-annex content that may never have been uploaded.
const LOCAL_CONTENT: [&str; 2] = ["lfs/objects", "annex/objects"];

/// What an allowlisted entry at the top of the git directory may hold and still be nothing
/// unique; [`local_admin_content`] checks it. Anything else there is the repository's own
/// content, which recovery (commits and working-tree files) does not save, so the plan asks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Holds {
    /// Nothing more to read: another fact covers it, or it is a cache, log or lock that git
    /// writes again.
    Covered,
    /// Config: each key is judged by its family ([`CONFIG_FAMILIES`]).
    ConfigKeys,
    /// Only regular files whose names end in `.sample`: the templates `git init` installs.
    SampleHooks,
    /// `exclude` with blank and comment lines only (the stock template); `refs` holding only
    /// `<object id>\t<refname>` lines (what `git gc` writes through `update-server-info`);
    /// `attributes` empty. Any other entry is local content.
    Info,
    /// [`STOCK_DESCRIPTION`] or nothing.
    StockDescription,
    /// Local content when it holds anything at all.
    LocalWhenNonEmpty,
    /// Every entry inside is local content (an empty directory is fine).
    LocalEntries,
}

/// Entries at the top of a git directory that are known not to hold history of their own, with
/// the kind each must be (`None`: any kind) and what each may hold ([`Holds`]). Everything else
/// there is either a pseudoref, whose commit is checked ([`pseudoref_check`]), or unknown, which
/// makes the layout not plain.
///
/// - `HEAD`, `config`, `refs`, `objects`, `worktrees`, `modules`: checked by [`shape`] and the
///   ref and object checks. `packed-refs` holds refs, which `for-each-ref` and `rev-list` read;
///   `logs` holds the reflogs, which `reflog_saved` reads; `index` holds staged content, which
///   `status` (`tree_clean`) and the hidden-index count report.
/// - `config`, `config.worktree`: settings, judged key by key ([`CONFIG_FAMILIES`]).
/// - `hooks`, `info`, `description`: the stock templates are nothing unique; anything else is
///   (a custom hook, a note, a real exclude pattern, a sparse-checkout pattern list, an edited
///   description). `info/grafts` and `info/alternates` are refused by [`shape`] first.
/// - `branches`, `remotes`: legacy remote shorthands, which no clone recreates: any file there
///   is local content.
/// - `shallow`: the ids of the commits where a shallow clone's history was cut, which came from
///   the remote.
/// - `TAG_EDITMSG`, `MERGE_MSG`, `SQUASH_MSG`: a message that was written and not (yet) used by a
///   commit or tag; small, but it exists nowhere else, so one that is not empty is local content.
///   A squash merge's changes are in the index and working tree, which `tree_clean` covers.
/// - `COMMIT_EDITMSG`: the message of the last commit, which that commit holds; git overwrites
///   it at every commit, so counting it would make every repository with a commit ask. What is
///   lost is only the message of a commit that a hook or an empty message aborted.
/// - `MERGE_MODE`, `MERGE_RR`, `rr-cache`: merge-mode text and recorded conflict resolutions,
///   which `rerere` can learn again from the merge commits that hold the resolved results.
/// - What lies inside `objects`, `refs`, `logs`, `rr-cache`, `lfs` and `fsmonitor--daemon` is
///   judged by the scan's walk against the shape git writes there ([`crate::gitshape`]); the
///   scan folds its findings into the same fact.
/// - `gitk.cache`, `gc.log`, `gc.pid`, `fsmonitor--daemon`, `fsmonitor--daemon.ipc`, and the
///   split index's `sharedindex.*` files: caches, logs, a lock-holder's pid, the file-system
///   monitor's socket and cookies, and part of the index. None keeps an object alive.
/// - `lfs`: its `objects` are checked by [`pending_state`]; the rest is a download cache.
/// - The names of [`IN_PROGRESS`]: an unfinished operation, which [`pending_state`] reports.
///
/// `FETCH_HEAD` is not here: the commits it names were fetched, but from a remote this tool may
/// not count as saved (a local path), so they are checked like any pseudoref.
const TOP_LEVEL_KNOWN: [(&str, Option<Lk>, Holds); 29] = [
    ("HEAD", Some(Lk::File), Holds::Covered),
    ("config", Some(Lk::File), Holds::ConfigKeys),
    ("config.worktree", Some(Lk::File), Holds::ConfigKeys),
    ("description", Some(Lk::File), Holds::StockDescription),
    ("index", Some(Lk::File), Holds::Covered),
    ("packed-refs", Some(Lk::File), Holds::Covered),
    ("shallow", Some(Lk::File), Holds::Covered),
    ("COMMIT_EDITMSG", Some(Lk::File), Holds::Covered),
    ("TAG_EDITMSG", Some(Lk::File), Holds::LocalWhenNonEmpty),
    ("MERGE_MSG", Some(Lk::File), Holds::LocalWhenNonEmpty),
    ("SQUASH_MSG", Some(Lk::File), Holds::LocalWhenNonEmpty),
    ("MERGE_MODE", Some(Lk::File), Holds::Covered),
    ("MERGE_RR", Some(Lk::File), Holds::Covered),
    ("gitk.cache", Some(Lk::File), Holds::Covered),
    ("gc.log", Some(Lk::File), Holds::Covered),
    ("gc.pid", Some(Lk::File), Holds::Covered),
    ("fsmonitor--daemon.ipc", None, Holds::Covered),
    ("fsmonitor--daemon", Some(Lk::Dir), Holds::Covered),
    ("hooks", Some(Lk::Dir), Holds::SampleHooks),
    ("info", Some(Lk::Dir), Holds::Info),
    ("logs", Some(Lk::Dir), Holds::Covered),
    ("objects", Some(Lk::Dir), Holds::Covered),
    ("refs", Some(Lk::Dir), Holds::Covered),
    ("branches", Some(Lk::Dir), Holds::LocalEntries),
    ("remotes", Some(Lk::Dir), Holds::LocalEntries),
    ("rr-cache", Some(Lk::Dir), Holds::Covered),
    ("lfs", Some(Lk::Dir), Holds::Covered),
    ("worktrees", Some(Lk::Dir), Holds::Covered),
    ("modules", Some(Lk::Dir), Holds::Covered),
];

/// The text `git init` writes to `description` (trailing newline aside), on every system the
/// tests run on (git 2.56 on Linux, macOS and Windows).
const STOCK_DESCRIPTION: &str =
    "Unnamed repository; edit this file 'description' to name the repository.";

/// Most entries listed from one directory of the git directory; more makes
/// `no_local_admin_content` unknown.
const MAX_ADMIN_ENTRIES: usize = 1000;

/// Config key families: `(section, variable or "" for any, local)`. `local` is true when a
/// value there exists only in this repository and its loss is concrete; false when the value is
/// recorded in the scan output (remote URLs), set again by cloning (format, tracking), or a
/// preference rather than work. The first matching row wins; a key that matches no row is
/// local, so a family added to [`config_key_is_safe`] without a row here asks rather than loses.
const CONFIG_FAMILIES: [(&str, &str, bool); 23] = [
    // `core.*`: format and platform values that `init` and `clone` write again, and behaviour
    // preferences (line endings, sparse checkout on or off), not work.
    ("core", "", false),
    // Remote URLs are recorded in the scan output (`remotes`); fetch refspecs are set by clone.
    ("remote", "", false),
    // A branch description is text a person wrote, kept nowhere else.
    ("branch", "description", true),
    // Tracking (`remote`, `merge`, `rebase`, `pushremote`) is set again by clone and `push -u`.
    ("branch", "", false),
    // A per-repository identity: without it, later commits would carry another identity.
    ("user", "", true),
    // Submodule settings are derived from `.gitmodules` again (`modules/` is never plain).
    ("submodule", "", false),
    // URL rewrites may be what makes the remote reachable at all.
    ("url", "", true),
    // Repository format extensions are written by `init` and `clone` (`preciousObjects` is
    // read separately, as a dependency).
    ("extensions", "", false),
    // Behaviour preferences without subsections.
    ("init", "", false),
    ("gc", "", false),
    ("pack", "", false),
    ("receive", "", false),
    ("transfer", "", false),
    ("fetch", "", false),
    ("pull", "", false),
    ("push", "", false),
    ("color", "", false),
    ("advice", "", false),
    ("gui", "", false),
    // An included file lives where it is named; one inside the git directory is an unknown
    // top-level entry, which makes the layout not plain.
    ("include", "", false),
    ("includeif", "", false),
    // `commit.gpgsign`, `tag.gpgsign`: a signing preference.
    ("commit", "", false),
    ("tag", "", false),
];

/// The decision of the first [`CONFIG_FAMILIES`] row that matches config `key`, if any.
fn config_family(key: &str) -> Option<bool> {
    let key = key.to_ascii_lowercase();
    let (section, rest) = key.split_once('.').unwrap_or((key.as_str(), ""));
    let name = rest.rsplit('.').next().unwrap_or(rest);
    CONFIG_FAMILIES
        .iter()
        .find(|(s, n, _)| *s == section && (n.is_empty() || *n == name))
        .map(|(_, _, local)| *local)
}

/// Is the value of config `key` local content? A key no family matches is.
fn config_key_is_local(key: &str) -> bool {
    config_family(key).unwrap_or(true)
}

/// The entries of directory `dir` (shown in messages as `label`), with their kind; a name that
/// is not valid Unicode is spelt lossily and given the kind `Other`, so no rule accepts it.
fn list_bounded(dir: &Path, label: &str) -> Result<Vec<(String, Lk)>, String> {
    let list_err = |e: std::io::Error| format!("cannot list {label} ({e})");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(list_err)? {
        let entry = entry.map_err(list_err)?;
        if out.len() == MAX_ADMIN_ENTRIES {
            return Err(format!(
                "{label} holds more than {MAX_ADMIN_ENTRIES} entries"
            ));
        }
        let os_name = entry.file_name();
        let item = match os_name.to_str() {
            Some(n) => {
                let read_err = |e: String| format!("cannot read {label}/{n} ({e})");
                let kind = lk(&entry.path()).map_err(read_err)?;
                (n.to_string(), kind)
            }
            None => (os_name.to_string_lossy().into_owned(), Lk::Other),
        };
        out.push(item);
    }
    Ok(out)
}

/// Does `info/<name>` (of kind `kind`) hold only what `git init` puts there?
fn info_entry_is_stock(info: &Path, name: &str, kind: Lk) -> Result<bool, String> {
    if kind != Lk::File {
        return Ok(false);
    }
    let p = info.join(name);
    let read_err = |e: String| format!("cannot read info/{name} ({e})");
    match name {
        "exclude" => {
            let bytes = read_bounded(&p).map_err(read_err)?;
            let text = String::from_utf8_lossy(&bytes);
            Ok(text.lines().all(|l| {
                let l = l.strip_suffix('\r').unwrap_or(l);
                l.trim().is_empty() || l.starts_with('#')
            }))
        }
        // `git update-server-info`, which `git gc` and `git repack` run by default, writes one
        // `<object id>\t<refname>` line per ref: a listing the ref checks already cover.
        "refs" => {
            let bytes = read_bounded(&p).map_err(read_err)?;
            let lines = match std::str::from_utf8(&bytes) {
                Ok(text) => text,
                Err(_) => return Ok(false),
            };
            Ok(lines.lines().all(|l| match l.split_once('\t') {
                Some((id, r)) => is_object_id(id) && r.starts_with("refs/"),
                None => false,
            }))
        }
        "attributes" => match std::fs::symlink_metadata(&p) {
            Ok(m) => Ok(m.len() == 0),
            Err(e) => Err(read_err(e.to_string())),
        },
        _ => Ok(false),
    }
}

/// What the git directory `gd` holds of its own outside the object store, refs and logs, by the
/// rules of [`TOP_LEVEL_KNOWN`] and [`CONFIG_FAMILIES`]: names such as `hooks/pre-commit`,
/// `info/notes.txt` or `config: user.email`, sorted (lossy, for display), or why the inventory
/// could not be made (an entry that cannot be listed or read, a file over the read bound, a
/// directory over [`MAX_ADMIN_ENTRIES`]). Only `lstat`, directory listings and bounded reads;
/// nothing is followed. `config_keys` are the keys the allowlist already read.
fn local_admin_content(gd: &Path, config_keys: &[String]) -> Result<Vec<String>, String> {
    let mut found: Vec<String> = Vec::new();
    for (name, _, holds) in TOP_LEVEL_KNOWN {
        let p = gd.join(name);
        let kind = lk(&p).map_err(|e| format!("cannot read {name} ({e})"))?;
        if kind == Lk::Missing {
            continue;
        }
        let read_err = |e: String| format!("cannot read {name} ({e})");
        match holds {
            Holds::Covered | Holds::ConfigKeys => {}
            Holds::LocalWhenNonEmpty => {
                let len = match std::fs::symlink_metadata(&p) {
                    Ok(m) => m.len(),
                    Err(e) => return Err(read_err(e.to_string())),
                };
                if kind != Lk::File || len > 0 {
                    found.push(name.to_string());
                }
            }
            Holds::StockDescription => {
                let stock = kind == Lk::File && {
                    let bytes = read_bounded(&p).map_err(read_err)?;
                    let text = String::from_utf8_lossy(&bytes);
                    let t = text.trim();
                    t.is_empty() || t == STOCK_DESCRIPTION
                };
                if !stock {
                    found.push(name.to_string());
                }
            }
            Holds::SampleHooks | Holds::Info | Holds::LocalEntries => {
                if kind != Lk::Dir {
                    found.push(name.to_string());
                    continue;
                }
                for (entry, ekind) in list_bounded(&p, name)? {
                    let stock = match holds {
                        Holds::SampleHooks => ekind == Lk::File && entry.ends_with(".sample"),
                        Holds::Info => info_entry_is_stock(&p, &entry, ekind)?,
                        _ => false,
                    };
                    if !stock {
                        found.push(format!("{name}/{entry}"));
                    }
                }
            }
        }
    }
    for key in config_keys {
        if config_key_is_local(key) {
            found.push(format!("config: {}", config_key_label(key)));
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

/// Most bytes read from one pseudoref: git writes one line (or, for `FETCH_HEAD`, one line per
/// fetched ref), so anything longer is not a pseudoref this tool understands.
const MAX_PSEUDOREF_BYTES: u64 = 64 * 1024;

/// What a pseudoref file holds.
#[derive(Debug, PartialEq, Eq)]
enum Pseudo {
    /// Object ids (one, or one per line of `FETCH_HEAD`).
    Ids(Vec<String>),
    /// `ref: refs/...`: it names a ref, which the ref checks cover.
    Symbolic,
    /// It could not be read or parsed (the reason).
    Unreadable(String),
}

/// The entries at the top of a git directory that matter here.
#[derive(Default)]
struct TopLevel {
    /// Entries that are neither known nor a pseudoref (names, lossily).
    unknown: Vec<String>,
    /// Pseudorefs, by name.
    pseudorefs: Vec<(String, Pseudo)>,
}

/// A full object id: 40 (SHA-1) or 64 (SHA-256) lowercase hex digits.
fn is_object_id(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A name git gives a pseudoref: one ending in `_HEAD`, or one in capitals (`ORIG_HEAD`,
/// `AUTO_MERGE`, `MERGE_AUTOSTASH`, `BISECT_EXPECTED_REV`, and any git adds later).
fn pseudoref_name(name: &str) -> bool {
    name.ends_with("_HEAD")
        || (name.bytes().any(|b| b.is_ascii_uppercase())
            && name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'))
}

/// Parse a pseudoref's bytes. `FETCH_HEAD` has one line per fetched ref, each starting with an
/// object id and a tab; every other pseudoref is one line: an object id or `ref: refs/...`.
fn parse_pseudoref(name: &str, bytes: &[u8]) -> Pseudo {
    let bad = || Pseudo::Unreadable("does not hold an object id or a ref: line".to_string());
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return bad(),
    };
    if name == "FETCH_HEAD" {
        let mut ids = Vec::new();
        for line in text.lines().filter(|l| !l.is_empty()) {
            match line.split_once('\t') {
                Some((id, _)) if is_object_id(id) => ids.push(id.to_string()),
                _ => return bad(),
            }
        }
        return Pseudo::Ids(ids);
    }
    let line = text.strip_suffix('\n').unwrap_or(text);
    if line.contains('\n') {
        return bad();
    }
    if is_object_id(line) {
        return Pseudo::Ids(vec![line.to_string()]);
    }
    let target = line.strip_prefix("ref: ").unwrap_or("");
    if target.starts_with("refs/") && !target.bytes().any(|b| b.is_ascii_whitespace()) {
        Pseudo::Symbolic
    } else {
        bad()
    }
}

/// Read at most [`MAX_PSEUDOREF_BYTES`] of a regular file. Anything else, a FIFO or symlink
/// swapped in after the caller's `lstat` included, is refused without blocking
/// ([`crate::regular::open_regular`]).
fn read_bounded(p: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let f = crate::regular::open_regular(p).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    f.take(MAX_PSEUDOREF_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    if buf.len() as u64 > MAX_PSEUDOREF_BYTES {
        return Err(format!("longer than {MAX_PSEUDOREF_BYTES} bytes"));
    }
    Ok(buf)
}

/// Sort every entry directly inside the git directory `gd`: known ([`TOP_LEVEL_KNOWN`]), a
/// pseudoref (a regular file whose name looks like one or whose content parses as one), or
/// unknown. Only regular files are opened; a directory, symlink or special file that is not
/// known is unknown. Nothing below the top level is read.
fn top_level(gd: &Path) -> Result<TopLevel, String> {
    let list_err = |e: std::io::Error| format!("cannot list the git directory ({e})");
    let mut out = TopLevel::default();
    for entry in std::fs::read_dir(gd).map_err(list_err)? {
        let entry = entry.map_err(list_err)?;
        let os_name = entry.file_name();
        let name = match os_name.to_str() {
            Some(n) => n.to_string(),
            None => {
                out.unknown.push(os_name.to_string_lossy().into_owned());
                continue;
            }
        };
        let kind = lk(&entry.path()).map_err(|e| format!("cannot read {name} ({e})"))?;
        if let Some((_, want, _)) = TOP_LEVEL_KNOWN.iter().find(|(n, _, _)| *n == name) {
            if want.is_none() || *want == Some(kind) {
                continue;
            }
            out.unknown.push(name);
            continue;
        }
        if IN_PROGRESS.contains(&name.as_str())
            || (name.starts_with("sharedindex.") && kind == Lk::File)
        {
            continue;
        }
        if kind != Lk::File {
            out.unknown.push(name);
            continue;
        }
        match read_bounded(&entry.path()) {
            Ok(bytes) => {
                let parsed = parse_pseudoref(&name, &bytes);
                if pseudoref_name(&name) || !matches!(parsed, Pseudo::Unreadable(_)) {
                    out.pseudorefs.push((name, parsed));
                } else {
                    out.unknown.push(name);
                }
            }
            Err(e) if pseudoref_name(&name) => {
                let why = format!("could not be read ({e})");
                out.pseudorefs.push((name, Pseudo::Unreadable(why)));
            }
            Err(_) => out.unknown.push(name),
        }
    }
    out.unknown.sort();
    out.pseudorefs.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Pseudorefs of one git directory compared with the network remotes.
#[derive(Debug, Default, PartialEq, Eq)]
struct PseudorefCheck {
    /// Exact names whose commit no network remote's tracking refs contain.
    unsaved: Vec<String>,
    /// Pseudorefs that could not be read, parsed or compared: a sentence naming each (its
    /// shown name) and why, with git's own error when there is one.
    unchecked: Vec<(String, Option<String>)>,
}

/// The sentence for a pseudoref that cannot be checked.
fn unchecked_sentence(shown: &str, clause: &str) -> String {
    format!("{shown} at the top of the git directory {clause}")
}

/// The word for an object type `git cat-file -t` printed, from a fixed list (git's output is
/// never repeated as is).
fn object_kind(printed: &str) -> &'static str {
    match printed.trim() {
        "commit" => "commit",
        "tag" => "tag",
        "tree" => "tree",
        "blob" => "blob",
        _ => "unexpected",
    }
}

impl PseudorefCheck {
    fn tri(&self) -> Tri {
        if !self.unchecked.is_empty() {
            Tri::Unknown
        } else if !self.unsaved.is_empty() {
            Tri::No
        } else {
            Tri::Yes
        }
    }
}

/// Check every pseudoref at the top of `gitdir` (this worktree's own git directory): the commit
/// each names must be contained in a network remote's tracking refs (`names`; when it is empty,
/// every commit is unsaved), as for the reflogs.
///
/// The object's own type is read first (`git cat-file -t <id>`): only a commit goes on to the
/// containment check. A tag object (whose annotation or signature may exist nowhere else even
/// when the commit it points to is pushed), a tree (as `AUTO_MERGE` holds), a blob, a missing
/// object or any failure is unchecked, and the sentence says which. A pseudoref whose name would
/// not be shown exactly (a control character, text the redaction rewrites) is unchecked too, so
/// [`PseudorefCheck::unsaved`] only ever holds exact names.
fn pseudoref_check(git: &Git, path: &Path, gitdir: &Path, names: &[String]) -> PseudorefCheck {
    let mut out = PseudorefCheck::default();
    let top = match top_level(gitdir) {
        Ok(t) => t,
        Err(why) => {
            let s = "the pseudorefs at the top of the git directory could not be listed";
            out.unchecked.push((s.to_string(), Some(why)));
            return out;
        }
    };
    let pats: Vec<String> = names.iter().map(|n| format!("--remotes={n}")).collect();
    for (name, content) in top.pseudorefs {
        let shown = diag(&name);
        if shown != name {
            let s = unchecked_sentence(&shown, "has a name that cannot be shown exactly");
            out.unchecked.push((s, None));
            continue;
        }
        let ids = match content {
            Pseudo::Ids(ids) => ids,
            Pseudo::Symbolic => continue,
            Pseudo::Unreadable(why) => {
                let s = unchecked_sentence(&name, &why);
                out.unchecked.push((s, None));
                continue;
            }
        };
        for id in ids {
            let kind = match git.run(path, &["cat-file", "-t", id.as_str()]) {
                Ok(t) => object_kind(&t),
                Err(e) => {
                    let clause = "names an object git could not read (missing or damaged)";
                    let s = unchecked_sentence(&name, clause);
                    out.unchecked.push((s, Some(e.to_string())));
                    break;
                }
            };
            if kind != "commit" {
                let s = unchecked_sentence(&name, &format!("names a {kind} object, not a commit"));
                out.unchecked.push((s, None));
                break;
            }
            let mut args: Vec<&str> = vec!["rev-list", "-n", "1", id.as_str()];
            if !pats.is_empty() {
                // Without a network remote nothing is subtracted: every commit is unsaved.
                args.push("--not");
                args.extend(pats.iter().map(|s| s.as_str()));
            }
            match git.run(path, &args) {
                Ok(o) if o.trim().is_empty() => {}
                Ok(_) => {
                    out.unsaved.push(name.clone());
                    break;
                }
                Err(e) => {
                    let clause = "could not be compared with the remote (git failed)";
                    let s = unchecked_sentence(&name, clause);
                    out.unchecked.push((s, Some(e.to_string())));
                    break;
                }
            }
        }
    }
    out
}

/// Count the index entries whose edits git hides: those marked assume-unchanged (a lowercase tag
/// in `git ls-files -v`) or skip-worktree (`S`), whose file is present in the working tree.
///
/// A sparse checkout marks every file outside its cone skip-worktree and leaves it out of the
/// working tree; such an absent file holds no edit (its content is in the index), so it is not
/// counted. A present file is counted without reading it: comparing its bytes with the index
/// blob would need git's clean filters and line-ending rules, which this crate never runs, so a
/// raw hash would give false alarms and a filtered one would run repository-chosen programs. A
/// file that cannot be `lstat`ed is counted too. `listing` is the `-z` output.
///
/// The names are git's bytes, looked up exactly ([`path_from_git`]); a name that cannot be a path
/// here is an error, so the check fails instead of missing the file.
fn hidden_index_entries(root: &Path, listing: &[u8]) -> Result<u64, String> {
    let mut n = 0u64;
    for e in listing.split(|b| *b == 0) {
        let (tag, rest) = match e.split_first() {
            Some(x) => x,
            None => continue,
        };
        let rel = match rest.strip_prefix(b" ") {
            Some(r) if !r.is_empty() => r,
            _ => continue,
        };
        let hidden_tag = *tag == b'S' || tag.is_ascii_lowercase();
        if !hidden_tag {
            continue;
        }
        let rel = path_from_git(rel).ok_or_else(|| {
            format!(
                "tree_clean: the index lists a file whose name is not valid on this system ({})",
                shown(rel)
            )
        })?;
        if !matches!(lk(&root.join(rel)), Ok(Lk::Missing)) {
            n += 1;
        }
    }
    Ok(n)
}

/// Does `dir` exist and hold at least one entry? `Err` for anything but a plain directory or a
/// missing one: a symlink or an unreadable path is not looked through.
fn non_empty_dir(dir: &Path) -> Result<bool, String> {
    use std::io::ErrorKind::NotFound;
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => std::fs::read_dir(dir)
            .map(|mut rd| rd.next().is_some())
            .map_err(|e| format!("cannot read {}: {e}", dir.display())),
        Ok(_) => Err(format!("{} is not a plain directory", dir.display())),
        Err(e) if e.kind() == NotFound => Ok(false),
        Err(e) => Err(format!("cannot read {}: {e}", dir.display())),
    }
}

/// Unfinished operations of this worktree and local-only LFS or annex content, found by name
/// only (no file is read). `rr-cache` is not listed: it records conflict resolutions that can be
/// derived again, not work.
fn pending_state(git: &Git, path: &Path) -> Result<Vec<String>, String> {
    let gitdir = printed_path(git, path, &["rev-parse", "--absolute-git-dir"], "git dir")?;
    let common = printed_path(git, path, &["rev-parse", "--git-common-dir"], "git dir")?;
    let mut found: Vec<String> = Vec::new();
    for name in IN_PROGRESS {
        // `symlink_metadata` succeeds for any kind of entry, including a symlink or a FIFO.
        match std::fs::symlink_metadata(gitdir.join(name)) {
            Ok(_) => found.push(name.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot check for {name}: {e}")),
        }
    }
    for rel in LOCAL_CONTENT {
        let dir = rel.split('/').fold(common.clone(), |p, part| p.join(part));
        if non_empty_dir(&dir)? {
            found.push(rel.to_string());
        }
    }
    Ok(found)
}

/// Is a local config key one that cannot make git run a program of the repo's choosing?
///
/// This is an allowlist: a repository we scan may be an unpacked download, and its `.git/config`
/// is attacker-controlled. Keys (`section.subsection.name`, lowercased section and name) that are
/// not known to be plain data are refused, and the repo is then not inspected at all.
pub fn config_key_is_safe(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    let (section, rest) = match key.split_once('.') {
        Some(x) => x,
        None => return false,
    };
    let name = rest.rsplit('.').next().unwrap_or(rest);
    let has_subsection = rest.contains('.');
    match section {
        "core" => matches!(
            name,
            "repositoryformatversion"
                | "filemode"
                | "bare"
                | "logallrefupdates"
                | "ignorecase"
                | "precomposeunicode"
                | "symlinks"
                | "autocrlf"
                | "eol"
                | "safecrlf"
                | "sparsecheckout"
                | "sparsecheckoutcone"
                | "worktree"
                | "longpaths"
                | "protectntfs"
                | "protecthfs"
                | "untrackedcache"
                | "splitindex"
                | "commentchar"
                | "abbrev"
                | "excludesfile"
                | "attributesfile"
                | "quotepath"
                | "trustctime"
                | "checkstat"
                | "preloadindex"
                | "fscache"
        ),
        "remote" => {
            has_subsection
                && matches!(
                    name,
                    "url"
                        | "pushurl"
                        | "fetch"
                        | "push"
                        | "tagopt"
                        | "prune"
                        | "promisor"
                        | "partialclonefilter"
                )
        }
        "branch" => {
            has_subsection
                && matches!(
                    name,
                    "remote" | "merge" | "rebase" | "description" | "pushremote"
                )
        }
        "user" => matches!(name, "name" | "email" | "signingkey" | "useconfigonly"),
        "submodule" => matches!(name, "url" | "path" | "active" | "branch" | "ignore"),
        "url" => has_subsection && matches!(name, "insteadof" | "pushinsteadof"),
        "extensions" => true,
        "init" | "gc" | "pack" | "receive" | "transfer" | "fetch" | "pull" | "push" | "color"
        | "advice" | "gui" | "include" | "includeif" => !has_subsection,
        "commit" | "tag" => name == "gpgsign",
        _ => false,
    }
}

/// Keys of the repo-local config (including included files), or why they could not be read.
fn local_config_keys(git: &crate::git::Git, path: &Path) -> Result<Vec<String>, String> {
    let list = |extra: &[&str]| -> Result<Vec<String>, String> {
        let mut args = vec!["config"];
        args.extend_from_slice(extra);
        args.extend_from_slice(&["--includes", "--name-only", "--list"]);
        git.run(path, &args)
            .map(|out| out.lines().map(|l| l.trim().to_string()).collect())
            .map_err(|e| note("config", &e))
    };
    let mut keys = list(&["--local"])?;
    if keys
        .iter()
        .any(|k| k.eq_ignore_ascii_case("extensions.worktreeconfig"))
    {
        if worktree_config_exists(path)? {
            keys.extend(list(&["--worktree"])?);
        } else {
            // git writes `config.worktree` only when a value is first set with `--worktree`, and
            // `--worktree` fails when the file is missing. Missing means no keys, if git agrees:
            // its own listing must show no worktree scope. (When git rejects this git directory
            // and uses one above it, that one's file was not judged, so the repo is refused.)
            // `--show-scope` is git 2.26; any failure of this listing is said in those words,
            // with git's own error after them (its text is not matched).
            let all = git
                .run(
                    path,
                    &[
                        "config",
                        "--list",
                        "--show-scope",
                        "--includes",
                        "--name-only",
                    ],
                )
                .map_err(|e| {
                    format!(
                        "git config --show-scope failed (it needs git 2.26 or newer), so whether \
                         git reads worktree-scoped config that was not judged is unknown: {}",
                        note("config", &e)
                    )
                })?;
            for line in all.lines() {
                match line.split_once('\t') {
                    Some((scope, _)) if scope != "worktree" => {}
                    _ => {
                        return Err(
                            "git would read worktree-scoped config that was not judged (not \
                             this git directory's config.worktree); no other git command was \
                             run in this repo"
                                .into(),
                        );
                    }
                }
            }
        }
    }
    Ok(keys)
}

/// Does this repo's per-worktree config file exist? Found by `lstat` and a bounded read of a
/// `.git` file, never by running git: a `.git` directory holds it, a `.git` file names the git
/// directory that does (a linked worktree's own `<common>/worktrees/<name>`), and with no `.git`
/// (a bare repo) the directory itself does. A missing file is `false`; one that is not a regular
/// file (a symlink, a directory) or cannot be looked at is refused, as `.git/modules` configs are
/// ([`check_module_configs`]).
fn worktree_config_exists(path: &Path) -> Result<bool, String> {
    use std::io::ErrorKind::NotFound;
    let dotgit = path.join(".git");
    let gitdir = match std::fs::symlink_metadata(&dotgit) {
        Ok(m) if m.is_dir() => dotgit,
        Ok(m) if m.is_file() => gitfile_dir(path, &dotgit)?,
        Ok(_) => return Err(".git is neither a directory nor a regular file".into()),
        Err(e) if e.kind() == NotFound => path.to_path_buf(),
        Err(e) => return Err(format!("cannot read .git: {e}")),
    };
    match std::fs::symlink_metadata(gitdir.join("config.worktree")) {
        Ok(m) if m.is_file() => Ok(true),
        Ok(_) => Err("config.worktree is not a regular file".into()),
        Err(e) if e.kind() == NotFound => Ok(false),
        Err(e) => Err(format!("cannot read config.worktree: {e}")),
    }
}

/// The git directory a `.git` file names, as git reads it: everything after `gitdir: ` without
/// the trailing line ending, relative to the directory holding the file. Anything git would read
/// differently (more than one line, a NUL, no path) is refused.
fn gitfile_dir(path: &Path, file: &Path) -> Result<PathBuf, String> {
    let bad = |why: &str| format!("the .git file {why}, so its config.worktree was not found");
    let bytes = read_bounded(file).map_err(|e| bad(&format!("was not read ({e})")))?;
    let rest = bytes
        .strip_prefix(b"gitdir: ")
        .ok_or_else(|| bad("does not name a git directory"))?;
    let mut end = rest.len();
    while end > 0 && matches!(rest[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    let target = &rest[..end];
    if target.iter().any(|&b| matches!(b, b'\n' | b'\r')) {
        return Err(bad("holds more than one line"));
    }
    let dir = path_from_git(target).ok_or_else(|| bad("names no path valid on this system"))?;
    Ok(if dir.is_absolute() {
        dir
    } else {
        path.join(dir)
    })
}

fn unsafe_key(keys: &[String], whose: &str) -> Result<(), String> {
    match keys.iter().find(|k| !config_key_is_safe(k)) {
        Some(bad) => {
            // Only the section and variable name: the subsection can be a URL carrying a token.
            let shown = config_key_label(bad);
            Err(format!(
                "{whose} can run programs or is not understood ({shown}); \
                 no git command was run in this repo"
            ))
        }
        None => Ok(()),
    }
}

/// Is every config in the repo, and in every submodule git directory under `modules`, free of
/// keys that could run a program? `git status` recurses into submodules and honours their config.
fn check_module_configs(git: &crate::git::Git, cwd: &Path, modules: &Path) -> Result<(), String> {
    use std::io::ErrorKind::NotFound;
    match std::fs::symlink_metadata(modules) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err("the submodule directory .git/modules is not a directory".into()),
        Err(e) if e.kind() == NotFound => return Ok(()),
        Err(e) => return Err(format!("cannot read .git/modules: {e}")),
    }
    let mut stack = vec![modules.to_path_buf()];
    let mut visited = 0usize;
    while let Some(dir) = stack.pop() {
        visited += 1;
        if visited > 2000 {
            return Err("too many submodule directories under .git/modules to check".into());
        }
        let is_gitdir = std::fs::symlink_metadata(dir.join("HEAD"))
            .map(|m| m.is_file())
            .unwrap_or(false);
        if is_gitdir {
            for name in ["config", "config.worktree"] {
                let file = dir.join(name);
                match std::fs::symlink_metadata(&file) {
                    Ok(m) if m.is_file() => {
                        // The file's own path, exactly (never a lossy spelling of it).
                        let args: [&std::ffi::OsStr; 6] = [
                            "config".as_ref(),
                            "--file".as_ref(),
                            file.as_os_str(),
                            "--includes".as_ref(),
                            "--name-only".as_ref(),
                            "--list".as_ref(),
                        ];
                        let out = git
                            .run_bytes(cwd, &args)
                            .map_err(|e| note("submodule config", &e))?;
                        let out = String::from_utf8(out)
                            .map_err(|_| note("submodule config", &GitError::NotUtf8))?;
                        let keys: Vec<String> = out.lines().map(|l| l.trim().to_string()).collect();
                        unsafe_key(&keys, "a submodule's config")?;
                    }
                    Ok(_) => return Err("a submodule config is not a regular file".into()),
                    Err(e) if e.kind() == NotFound => {}
                    Err(e) => return Err(format!("cannot read a submodule config: {e}")),
                }
            }
            stack.push(dir.join("modules"));
        } else {
            let rd = match std::fs::read_dir(&dir) {
                Ok(r) => r,
                Err(e) if e.kind() == NotFound => continue,
                Err(e) => return Err(format!("cannot read {}: {e}", dir.display())),
            };
            for entry in rd {
                let entry = entry.map_err(|e| format!("cannot read .git/modules: {e}"))?;
                let ft = entry
                    .file_type()
                    .map_err(|e| format!("cannot read .git/modules: {e}"))?;
                if ft.is_symlink() {
                    return Err("a symlink under .git/modules".into());
                }
                if ft.is_dir() {
                    stack.push(entry.path());
                }
            }
        }
    }
    Ok(())
}

/// Refuse to run git in a repo whose config, or whose submodules' configs, could execute
/// programs, or whose `core.worktree` points at some other directory. Returns the repo's local
/// config keys.
fn check_repo_config(
    git: &crate::git::Git,
    path: &Path,
    class: Class,
) -> Result<Vec<String>, String> {
    let keys = local_config_keys(git, path)?;
    unsafe_key(&keys, "repo config")?;
    if keys.iter().any(|k| k.eq_ignore_ascii_case("core.worktree")) {
        // The effective value: with `extensions.worktreeConfig` a value in `config.worktree`
        // overrides the shared one, and `--local` would not see it. System and global config
        // are switched off, so the only scopes left are the two the allowlist already read.
        // Both read exactly: the value is a path, and a lossy spelling of it could name another
        // directory or fail to match this one.
        let value = git
            .run_bytes(path, &["config", "--get", "core.worktree"])
            .map_err(|e| note("config", &e))?;
        let v = single_path(&value).ok_or_else(|| {
            "core.worktree is not a path this system can use; facts would describe another tree"
                .to_string()
        })?;
        let gitdir = printed_path(git, path, &["rev-parse", "--absolute-git-dir"], "git dir")?;
        let resolved = if v.is_absolute() { v } else { gitdir.join(v) };
        if canon(&resolved) != canon(path) {
            return Err(
                "core.worktree points to a different directory than this one; facts would \
                 describe another tree"
                    .into(),
            );
        }
    }
    // Whatever the config says, what git inspects must be the scanned directory: a directory git
    // does not accept as a repository makes it walk up to the next one. Both run only after the
    // config git reads (the `--local` listing is of the git directory git found, and the
    // worktree scope is git's own) passed the allowlist.
    if class != Class::BareRepo {
        let top = printed_path(git, path, &["rev-parse", "--show-toplevel"], "work tree")?;
        if canon(&top) != canon(path) {
            return Err(
                "git's work tree is not the scanned directory; facts would describe another tree"
                    .into(),
            );
        }
    } else {
        let gitdir = printed_path(git, path, &["rev-parse", "--absolute-git-dir"], "git dir")?;
        if canon(&gitdir) != canon(path) {
            return Err(
                "git's git directory is not the scanned directory; facts would describe another \
                 repository"
                    .into(),
            );
        }
    }
    let common = printed_path(git, path, &["rev-parse", "--git-common-dir"], "git dir")?;
    check_module_configs(git, path, &common.join("modules"))?;
    Ok(keys)
}

/// One entry of a `-z` listing whose metadata (ASCII) ends at the first tab and whose path (git's
/// bytes) follows: the metadata fields and the path. `None` for a malformed entry.
fn meta_and_path(e: &[u8]) -> Option<(Vec<&str>, &[u8])> {
    let tab = e.iter().position(|b| *b == b'\t')?;
    let meta = std::str::from_utf8(&e[..tab]).ok()?;
    Some((meta.split(' ').collect(), &e[tab + 1..]))
}

/// Gitlinks (submodule entries, mode 160000) recorded in an `ls-files -s -z` listing:
/// `(commit, stage, path bytes)`.
fn index_gitlinks(out: &[u8]) -> Vec<(String, String, Vec<u8>)> {
    out.split(|b| *b == 0)
        .filter_map(|e| {
            let (meta, p) = meta_and_path(e)?;
            if meta.first() != Some(&"160000") {
                return None;
            }
            let sha = meta.get(1)?.to_string();
            let stage = meta.get(2)?.to_string();
            Some((sha, stage, p.to_vec()))
        })
        .collect()
}

/// Gitlinks in an `ls-tree -r -z` listing: path bytes to commit.
fn tree_gitlinks(out: &[u8]) -> std::collections::BTreeMap<Vec<u8>, String> {
    out.split(|b| *b == 0)
        .filter_map(|e| {
            let (meta, p) = meta_and_path(e)?;
            if meta.first() != Some(&"160000") {
                return None;
            }
            Some((p.to_vec(), meta.get(2)?.to_string()))
        })
        .collect()
}

/// Does each checked-out submodule sit at the commit the parent's index records, and does the
/// index agree with HEAD about every gitlink? `status --ignore-submodules=all` hides both.
///
/// `Ok(None)` is clean, `Ok(Some(why))` is a difference, `Err` means it could not be decided.
/// A recorded submodule with no checkout (missing or empty directory) is fine; a directory with
/// files but no `.git` is not. The submodule's HEAD is read with the hardened runner after its own
/// config passed the same allowlist as any repo; no status is run inside it.
fn gitlink_state(git: &Git, path: &Path, has_head: bool) -> Result<Option<String>, String> {
    let listing = git
        .run_bytes(path, &["ls-files", "-s", "-z"])
        .map_err(|e| note("gitlinks", &e))?;
    let index = index_gitlinks(&listing);
    let head = if has_head {
        let tree = git
            .run_bytes(path, &["ls-tree", "-r", "-z", "HEAD"])
            .map_err(|e| note("gitlinks", &e))?;
        tree_gitlinks(&tree)
    } else {
        Default::default()
    };
    if index.len() > 200 {
        return Err("more than 200 submodules; their commits were not compared".into());
    }
    // Paths are compared as git's bytes; `sub` is their spelling for messages only.
    for (sha, stage, sub_bytes) in &index {
        let sub = shown(sub_bytes);
        if stage != "0" {
            return Ok(Some(format!("submodule {sub} has an unmerged index entry")));
        }
        if head.get(sub_bytes) != Some(sha) {
            return Ok(Some(format!(
                "the index records submodule {sub} differently from HEAD (a staged change)"
            )));
        }
        let rel = path_from_git(sub_bytes)
            .ok_or_else(|| format!("submodule {sub}: its path is not valid on this system"))?;
        let dir = path.join(rel);
        match std::fs::symlink_metadata(&dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("cannot read submodule {sub}: {e}")),
            Ok(m) if !m.is_dir() => {
                return Ok(Some(format!("submodule {sub} is not a directory")));
            }
            Ok(_) => {}
        }
        let empty = std::fs::read_dir(&dir)
            .map_err(|e| format!("cannot read submodule {sub}: {e}"))?
            .next()
            .is_none();
        if empty {
            continue; // recorded, not checked out
        }
        match std::fs::symlink_metadata(dir.join(".git")) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Some(format!(
                    "the path of submodule {sub} is a plain directory with files, not a checkout"
                )));
            }
            Err(e) => return Err(format!("cannot read submodule {sub}: {e}")),
            Ok(_) => {}
        }
        check_repo_config(git, &dir, Class::Worktree)
            .map_err(|why| format!("submodule {sub}: {why}"))?;
        let at = git
            .run(&dir, &["rev-parse", "--verify", "HEAD"])
            .map_err(|e| note("submodule HEAD", &e))?;
        if at.trim() != sha {
            return Ok(Some(format!(
                "submodule {sub} is checked out at another commit than the index records"
            )));
        }
    }
    if let Some(gone) = head.keys().find(|p| !index.iter().any(|(_, _, q)| q == *p)) {
        return Ok(Some(format!(
            "the index no longer records submodule {} (a staged removal)",
            shown(gone)
        )));
    }
    Ok(None)
}

/// Result of inspecting one repo.
pub struct RepoCheck {
    pub info: RepoInfo,
    /// Regenerable directories inside the repo that git does not prove are ignored, untracked
    /// build output: `(path, reason)`.
    pub unverified_artifacts: Vec<(PathBuf, String)>,
    /// For a linked worktree: the main worktree git names, as an exact path. (`info.parent_repo`
    /// is its redacted spelling for the scan file and is never looked up.)
    pub parent_path: Option<PathBuf>,
}

fn all_unknown(reason: String, unverified: &[PathBuf]) -> RepoCheck {
    let reason = diag(&reason);
    let why = reason.clone();
    RepoCheck {
        parent_path: None,
        info: RepoInfo {
            layout_issue: Some(reason.clone()),
            notes: vec![reason],
            ..RepoInfo::default()
        },
        unverified_artifacts: unverified
            .iter()
            .map(|p| (p.clone(), format!("was not checked against git: {why}")))
            .collect(),
    }
}

/// Gather facts for one repo, worktree or bare repo at `path`.
///
/// `regen_paths` are the regenerable and cache directories inside it that the scan found
/// removable; ignored files under them do not count as unique, and each is checked to be ignored
/// by git.
pub fn collect_repo_info(
    cfg: &FactsConfig,
    path: &Path,
    class: Class,
    regen_paths: &[PathBuf],
) -> RepoCheck {
    let git = &cfg.git;
    // A damaged or unusual structure is found by `lstat` alone and git is not run in it at all
    // (a repo that is only unusual, such as a `.git` file or shared objects, is still read and
    // asks for its layout).
    if let Shape::No {
        issue,
        skip_git: true,
    } = shape(path, class)
    {
        let mut rc = all_unknown(
            format!("layout: {issue}; no git command was run in this repo"),
            regen_paths,
        );
        rc.info.facts.plain_layout = Tri::No;
        rc.info.layout_issue = Some(diag(&issue));
        return rc;
    }
    // The repo's own config is untrusted. Refuse to run anything else if it could make git
    // execute a program (filter drivers, gpg.program, ...).
    let config_keys = match check_repo_config(git, path, class) {
        Ok(keys) => keys,
        Err(why) => return all_unknown(why, regen_paths),
    };
    let mut unverified: Vec<(PathBuf, String)> = Vec::new();
    let mut info = RepoInfo::default();
    let mut facts = Facts::default();
    let mut notes = Notes::default();

    // Remotes and the network predicate.
    let remotes: Option<Vec<Remote>> = match git.run(path, &["remote", "-v"]) {
        Ok(out) => Some(parse_remotes(&out, cfg)),
        Err(e) => {
            notes.push(note("remotes", &e));
            None
        }
    };
    let network_names: Option<Vec<String>> = remotes.as_ref().map(|rs| {
        rs.iter()
            .filter(|r| r.network)
            .map(|r| r.name.clone())
            .collect()
    });
    facts.has_network_remote = match &network_names {
        Some(names) => Tri::from_bool(!names.is_empty()),
        None => Tri::Unknown,
    };

    // The worktree's own HEAD, including a detached HEAD.
    let head: Option<String> = match git.run(path, &["rev-parse", "--verify", "HEAD"]) {
        Ok(out) => Some(out.trim().to_string()),
        Err(e) => {
            notes.push(note("HEAD", &e));
            None
        }
    };
    facts.head_on_remote = match (&head, &network_names) {
        (Some(h), Some(names)) => {
            if names.is_empty() {
                Tri::No
            } else {
                let args = [
                    "for-each-ref",
                    "--contains",
                    h.as_str(),
                    "--format=%(refname)",
                    "refs/remotes",
                ];
                match git.run(path, &args) {
                    Ok(out) => Tri::from_bool(out.lines().any(|l| {
                        names
                            .iter()
                            .any(|n| l.starts_with(&format!("refs/remotes/{n}/")))
                    })),
                    Err(e) => {
                        notes.push(note("head_on_remote", &e));
                        Tri::Unknown
                    }
                }
            }
        }
        _ => Tri::Unknown,
    };
    info.head_commit = head;

    // Every local branch and tag is contained in a network remote ref.
    facts.all_branches_on_remote = match &network_names {
        Some(names) if names.is_empty() => Tri::No,
        Some(names) => {
            // Every local ref (branches, tags, notes, replace refs, ...) except the stash, which
            // has its own fact. HEAD is judged by `head_on_remote`.
            let pats: Vec<String> = names.iter().map(|n| format!("--remotes={n}")).collect();
            let mut args: Vec<&str> = vec![
                "rev-list",
                "-n",
                "1",
                "--exclude=refs/stash",
                "--glob=refs/*",
                "--not",
            ];
            args.extend(pats.iter().map(|s| s.as_str()));
            match git.run(path, &args) {
                Ok(out) if !out.trim().is_empty() => Tri::No,
                Ok(_) => {
                    // Tag names (and tag objects) are not visible in remote-tracking refs, so a
                    // local tag cannot be shown to exist on the remote without `ls-remote`.
                    match git.run(
                        path,
                        &[
                            "for-each-ref",
                            "--count=1",
                            "--format=%(refname)",
                            "refs/tags",
                        ],
                    ) {
                        Ok(tags) if tags.trim().is_empty() => Tri::Yes,
                        Ok(_) => {
                            notes.push(
                                "all_branches_on_remote: the repo has tags, whose presence on the \
                                 remote cannot be verified from local refs",
                            );
                            Tri::Unknown
                        }
                        Err(e) => {
                            notes.push(note("all_branches_on_remote", &e));
                            Tri::Unknown
                        }
                    }
                }
                Err(e) => {
                    notes.push(note("all_branches_on_remote", &e));
                    Tri::Unknown
                }
            }
        }
        None => Tri::Unknown,
    };

    // Commits that only a reflog still reaches (an unpushed commit after `reset --hard`, an amended
    // or rebased-away commit). No ref names them, so every check above can read as saved while
    // the work is recoverable only from the reflog, which goes with the repo.
    facts.reflog_saved = match &network_names {
        Some(names) => {
            let reflogs = if names.is_empty() {
                // No network remote can hold anything.
                Tri::No
            } else if info.head_commit.is_none() {
                // An unborn HEAD (no commit yet) is not shown to be saved: HEAD is only unknown
                // when `rev-parse` failed, and an empty reflog would otherwise read as vacuously
                // saved.
                Tri::Unknown
            } else {
                // `rev-list` only reads refs, reflogs and objects (git-rev-list(1)): it takes no
                // lock and writes nothing. `--reflog` adds every reflog entry (HEAD of this
                // worktree and all refs of the shared repo) to the revisions; `--not
                // --remotes=<n>` removes whatever a network remote's tracking refs contain;
                // `-n 1` stops at the first survivor, and the command timeout bounds the walk (a
                // timeout is `Unknown`).
                let pats: Vec<String> = names.iter().map(|n| format!("--remotes={n}")).collect();
                let mut args: Vec<&str> = vec!["rev-list", "-n", "1", "--reflog", "--not"];
                args.extend(pats.iter().map(|s| s.as_str()));
                match git.run(path, &args) {
                    Ok(out) => Tri::from_bool(out.trim().is_empty()),
                    Err(e) => {
                        notes.push(note("reflog_saved", &e));
                        Tri::Unknown
                    }
                }
            };
            // Pseudorefs (`ORIG_HEAD` after a reset, `MERGE_AUTOSTASH`, any other file at the
            // top of the git directory that names a commit) keep a commit alive after every ref
            // and reflog has let go of it. They are checked with or without a network remote,
            // so that recovery is told which ones to capture and one that cannot be checked
            // asks.
            let gitdir_args = ["rev-parse", "--absolute-git-dir"];
            let pseudo = match printed_path(git, path, &gitdir_args, "git dir") {
                Ok(gitdir) => pseudoref_check(git, path, &gitdir, names),
                Err(why) => {
                    let s = "the pseudorefs at the top of the git directory could not be listed";
                    PseudorefCheck {
                        unchecked: vec![(s.to_string(), Some(why))],
                        ..PseudorefCheck::default()
                    }
                }
            };
            for name in &pseudo.unsaved {
                notes.push(format!(
                    "reflog_saved: {name} names a commit that no network remote has"
                ));
            }
            for (sentence, detail) in &pseudo.unchecked {
                notes.push(match detail {
                    Some(d) => format!("reflog_saved: {sentence} ({})", diag(d)),
                    None => format!("reflog_saved: {sentence}"),
                });
            }
            let tri = pseudo.tri();
            // Exact names: `pseudoref_check` refuses any name `diag` would change.
            info.unsaved_pseudorefs = pseudo.unsaved.clone();
            info.unchecked_pseudorefs = pseudo.unchecked.iter().map(|(s, _)| s.clone()).collect();
            // Unknown wins: something that could not be checked is never recovered on trust.
            match (reflogs, tri) {
                (Tri::Unknown, _) | (_, Tri::Unknown) => Tri::Unknown,
                (Tri::No, _) | (_, Tri::No) => Tri::No,
                _ => Tri::Yes,
            }
        }
        None => Tri::Unknown,
    };

    // Unfinished operations and local-only content that no ref records.
    facts.no_pending_state = match pending_state(git, path) {
        Ok(found) if found.is_empty() => Tri::Yes,
        Ok(found) => {
            notes.push(format!(
                "no_pending_state: found {} in the git directory (an unfinished rebase, \
                 cherry-pick, revert or bisect, or Git LFS or git-annex content); it holds work \
                 that no ref records and cannot be shown to be saved",
                found.join(", ")
            ));
            Tri::Unknown
        }
        Err(why) => {
            notes.push(format!("no_pending_state: {why}"));
            Tri::Unknown
        }
    };

    // Stash.
    // Only emptiness is read, as bytes: a stash message need not be valid UTF-8.
    facts.stash_empty = match git.run_bytes(path, &["stash", "list"]) {
        Ok(out) => Tri::from_bool(out.iter().all(u8::is_ascii_whitespace)),
        Err(e) => {
            notes.push(note("stash_empty", &e));
            Tri::Unknown
        }
    };

    // Working tree facts need a working tree.
    if class == Class::BareRepo {
        notes.push("tree_clean and no_unique_ignored: a bare repo has no working tree");
    } else {
        // Only emptiness is read, as bytes; `-z` so no name is quoted or split.
        let status_args = [
            "status",
            "--porcelain",
            "-z",
            "--untracked-files=all",
            // Each checked-out submodule is its own node with its own config check; a recursive
            // status would run inside it using its config.
            "--ignore-submodules=all",
        ];
        // `status` hides edits to files marked skip-worktree or assume-unchanged, and so do
        // `diff`, `add -A` and `stash`: those entries are counted on their own, whatever
        // `status` says, because recovery cannot capture them.
        let hidden = match git.run_bytes(path, &["ls-files", "-v", "-z"]) {
            Ok(listing) => hidden_index_entries(path, &listing),
            Err(e) => Err(note("tree_clean", &e)),
        };
        facts.tree_clean = match (git.run_bytes(path, &status_args), hidden) {
            (Err(e), _) => {
                notes.push(note("tree_clean", &e));
                Tri::Unknown
            }
            (Ok(_), Err(why)) => {
                notes.push(why);
                Tri::Unknown
            }
            (Ok(out), Ok(hidden)) => {
                if hidden > 0 {
                    info.hidden_index_entries = hidden;
                    notes.push(format!(
                        "tree_clean: {} marked skip-worktree or assume-unchanged and present in \
                         the working tree, so git hides any edits to {}",
                        crate::words::count(hidden, "file is", "files are"),
                        crate::words::agree(hidden, "it", "them")
                    ));
                }
                if !out.iter().all(u8::is_ascii_whitespace) || hidden > 0 {
                    Tri::No
                } else {
                    // `status` runs with --ignore-submodules=all, which also hides a submodule
                    // checked out at another commit than the index records.
                    match gitlink_state(git, path, info.head_commit.is_some()) {
                        Ok(None) => Tri::Yes,
                        Ok(Some(why)) => {
                            notes.push(format!("tree_clean: {why}"));
                            Tri::No
                        }
                        Err(why) => {
                            notes.push(format!("tree_clean: {why}"));
                            Tri::Unknown
                        }
                    }
                }
            }
        };
        let ignored_args = [
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ];
        // Entries and regenerable paths are compared as git's bytes, never as decoded text.
        facts.no_unique_ignored = match git.run_bytes(path, &ignored_args) {
            Ok(out) => {
                let entries: Vec<&[u8]> = out
                    .split(|b| *b == 0)
                    .map(|mut e| {
                        while let Some(rest) = e.strip_suffix(b"/") {
                            e = rest;
                        }
                        e
                    })
                    .filter(|e| !e.is_empty())
                    .collect();
                let mut regen: Vec<Vec<u8>> = Vec::new();
                for p in regen_paths {
                    let rel = match rel_git_bytes(p, path) {
                        Some(r) => r,
                        None => {
                            // Not below the repo as spelled, or not nameable to git here: git
                            // cannot be asked about it.
                            unverified.push((
                                p.clone(),
                                "could not be related to the repository path, so it is not \
                                 verified to be ignored"
                                    .to_string(),
                            ));
                            continue;
                        }
                    };
                    // A generated directory is wholly ignored and untracked, so git lists it
                    // as one ignored entry (or inside one). Anything else may hold source.
                    let ignored = entries.iter().any(|e| under(&rel, e));
                    if ignored {
                        regen.push(rel);
                    } else {
                        unverified.push((
                            p.clone(),
                            "is not shown by git as wholly ignored: it is tracked, partly \
                             tracked or not ignored, so it may hold source"
                                .to_string(),
                        ));
                    }
                }
                let unique: Vec<&[u8]> = entries
                    .iter()
                    .copied()
                    .filter(|e| !regen.iter().any(|r| under(e, r)))
                    .collect();
                if unique.is_empty() {
                    Tri::Yes
                } else {
                    let examples: Vec<String> = unique.iter().take(5).map(|e| shown(e)).collect();
                    notes.push(format!(
                        "no_unique_ignored: {} outside regenerable directories, e.g. {}",
                        crate::words::count(unique.len() as u64, "ignored path", "ignored paths"),
                        examples.join(", ")
                    ));
                    Tri::No
                }
            }
            Err(e) => {
                notes.push(note("no_unique_ignored", &e));
                for p in regen_paths {
                    unverified.push((
                        p.clone(),
                        "could not be checked against git to be ignored".to_string(),
                    ));
                }
                Tri::Unknown
            }
        };
    }

    // Dependents: linked worktrees. (Alternates clones are linked by the scanner.)
    let mut parent_path: Option<PathBuf> = None;
    facts.no_dependents = match worktree_paths(git, path) {
        Ok(entries) => {
            let me = canon(path);
            let others: Vec<&PathBuf> = entries.iter().filter(|e| canon(e) != me).collect();
            let main_is_me = entries.first().map(|e| canon(e) == me).unwrap_or(false);
            // Paths are compared exactly; their lossy spelling is for messages only.
            if class == Class::Worktree && !main_is_me {
                // A linked worktree: it belongs to the first listed worktree, and nothing
                // can depend on it.
                info.parent_repo = entries.first().map(|e| path_label(e));
                parent_path = entries.first().cloned();
                Tri::Yes
            } else {
                let has_none = others.is_empty();
                info.worktrees = others.iter().map(|o| path_label(o)).collect();
                Tri::from_bool(has_none)
            }
        }
        Err(why) => {
            notes.push(format!("no_dependents: {why}"));
            Tri::Unknown
        }
    };

    // `extensions.preciousObjects = true` is the repository saying that others borrow its
    // objects (it stops git from ever deleting them). A linked worktree only shares the setting
    // of the repository it belongs to; nothing depends on the worktree itself.
    let declared = config_keys
        .iter()
        .any(|k| k.eq_ignore_ascii_case("extensions.preciousobjects"));
    if declared && info.parent_repo.is_none() {
        match git.run(
            path,
            &[
                "config",
                "--type=bool",
                "--get",
                "extensions.preciousObjects",
            ],
        ) {
            Ok(v) if v.trim() == "true" => {
                info.precious_objects = true;
                facts.no_dependents = Tri::No;
                notes.push(
                    "no_dependents: this repository sets extensions.preciousObjects, which \
                     declares that other repositories borrow its objects",
                );
            }
            Ok(_) => {}
            Err(e) => {
                if facts.no_dependents == Tri::Yes {
                    facts.no_dependents = Tri::Unknown;
                }
                notes.push(note("no_dependents (extensions.preciousObjects)", &e));
            }
        }
    }

    // Activity evidence: newest commit on any ref.
    info.last_commit_time = git
        .run(path, &["log", "-1", "--all", "--format=%ct"])
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok());

    // Files the git directory keeps of its own outside the object store, refs and logs: a
    // custom hook, a note in `info/`, an edited description, an unsent message, a setting only
    // this repository has. Recovery saves commits and working-tree files, not these.
    let own_git_dir = if class == Class::BareRepo {
        Some(path.to_path_buf())
    } else {
        let g = path.join(".git");
        matches!(lk(&g), Ok(Lk::Dir)).then_some(g)
    };
    facts.no_local_admin_content = match own_git_dir.as_deref() {
        Some(gd) => match local_admin_content(gd, &config_keys) {
            Ok(found) if found.is_empty() => Tri::Yes,
            Ok(found) => {
                let shown: Vec<String> = found.iter().map(|n| diag(n)).collect();
                notes.push(format!(
                    "no_local_admin_content: the git directory holds files of its own that \
                     exist nowhere else: {}",
                    crate::words::some_of(&shown, 5)
                ));
                info.local_admin_content_count = shown.len() as u64;
                info.local_admin_content = shown.into_iter().take(5).collect();
                Tri::No
            }
            Err(why) => {
                notes.push(format!("no_local_admin_content: {why}"));
                Tri::Unknown
            }
        },
        // A `.git` file (a linked worktree, a submodule checkout, a separate git directory):
        // the layout is not plain, so the plan asks for that first.
        None => {
            notes.push(
                "no_local_admin_content: the git directory is not this directory's own .git, \
                 so its files were not listed",
            );
            Tri::Unknown
        }
    };

    // The two backup obligations of a removal (`crate::preserve`): the git directory's own files
    // and the object store's files. The classes go by name; nothing is verified.
    if let Some(gd) = own_git_dir.as_deref() {
        match crate::preserve::survey(gd) {
            Ok(found) => {
                let mut own_content: Vec<String> = Vec::new();
                if !found.not_objects.is_empty() {
                    let shown: Vec<String> = found.not_objects.iter().map(|n| diag(n)).collect();
                    notes.push(format!(
                        "no_local_admin_content: entries with the names of object files are not \
                         regular files: {}",
                        crate::words::some_of(&shown, 5)
                    ));
                    own_content.extend(shown);
                }
                if !own_content.is_empty() {
                    let mut all = info.local_admin_content.clone();
                    all.extend(own_content.iter().cloned());
                    all.sort();
                    all.dedup();
                    info.local_admin_content_count += own_content.len() as u64;
                    info.local_admin_content = all.into_iter().take(5).collect();
                    facts.no_local_admin_content = Tri::No;
                }
                info.git_admin = Some(found.admin);
                info.object_store = Some(found.store);
            }
            Err(e) => {
                notes.push(format!(
                    "no_local_admin_content: the git directory's files could not all be listed \
                     ({e}), so they could not be counted for a backup"
                ));
                if facts.no_local_admin_content == Tri::Yes {
                    facts.no_local_admin_content = Tri::Unknown;
                }
            }
        }
    }

    let (plain, issue) = layout_check(git, path, class);
    facts.plain_layout = plain;
    info.layout_issue = issue.map(|i| {
        let i = diag(&i);
        // Name the linked worktrees git reports, so a person can find them.
        if i == WORKTREES_NOT_EMPTY && !info.worktrees.is_empty() {
            format!("{i}: {}", crate::words::some_of(&info.worktrees, 3))
        } else {
            i
        }
    });
    info.facts = facts;
    info.notes = notes.0;
    info.remotes = remotes.unwrap_or_default();
    RepoCheck {
        info,
        unverified_artifacts: unverified,
        parent_path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pseudoref_names_and_contents_are_recognised() {
        for n in [
            "ORIG_HEAD",
            "AUTO_MERGE",
            "FOO_HEAD",
            "BISECT_EXPECTED_REV",
            "X1",
        ] {
            assert!(pseudoref_name(n), "{n}");
        }
        for n in ["notes.txt", "gitk.cache", "Orig_head", "_", "123"] {
            assert!(!pseudoref_name(n), "{n}");
        }
        let id = "0123456789abcdef0123456789abcdef01234567";
        let ids = |v: &[&str]| Pseudo::Ids(v.iter().map(|s| s.to_string()).collect());
        assert_eq!(
            parse_pseudoref("ORIG_HEAD", format!("{id}\n").as_bytes()),
            ids(&[id])
        );
        assert_eq!(parse_pseudoref("ORIG_HEAD", id.as_bytes()), ids(&[id]));
        let long = "ab".repeat(32);
        assert_eq!(parse_pseudoref("X", long.as_bytes()), ids(&[long.as_str()]));
        assert_eq!(
            parse_pseudoref("ORIG_HEAD", b"ref: refs/heads/main\n"),
            Pseudo::Symbolic
        );
        let fetch = format!("{id}\t\tbranch 'main' of x\n{id}\tnot-for-merge\tbranch 'b' of x\n");
        assert_eq!(
            parse_pseudoref("FETCH_HEAD", fetch.as_bytes()),
            ids(&[id, id])
        );
        assert_eq!(parse_pseudoref("FETCH_HEAD", b""), ids(&[]));
        for bad in [
            &b"not a commit id\n"[..],
            b"",
            b"0123\n",
            b"ref: HEAD\n",
            b"ref: refs/heads/a b\n",
            b"\xff\n",
        ] {
            assert!(
                matches!(parse_pseudoref("ORIG_HEAD", bad), Pseudo::Unreadable(_)),
                "{bad:?}"
            );
        }
        let upper = id.to_ascii_uppercase();
        assert!(matches!(
            parse_pseudoref("ORIG_HEAD", upper.as_bytes()),
            Pseudo::Unreadable(_)
        ));
        let two = format!("{id}\n{id}\n");
        assert!(matches!(
            parse_pseudoref("ORIG_HEAD", two.as_bytes()),
            Pseudo::Unreadable(_)
        ));
    }

    #[test]
    fn standard_pseudoref_names_are_shown_exactly() {
        // `pseudoref_check` refuses a name `diag` would change; git's own names must pass.
        for n in [
            "ORIG_HEAD",
            "FETCH_HEAD",
            "AUTO_MERGE",
            "MERGE_AUTOSTASH",
            "REBASE_HEAD",
        ] {
            assert_eq!(diag(n), n);
        }
        assert_ne!(diag("ODD\u{1b}_HEAD"), "ODD\u{1b}_HEAD");
    }

    #[test]
    fn every_config_family_the_allowlist_accepts_has_a_decision() {
        // One key or more for each arm of `config_key_is_safe`.
        let local = [
            "branch.main.description",
            "user.name",
            "user.email",
            "user.signingkey",
            "url.https://example.invalid/.insteadof",
        ];
        let not_local = [
            "core.repositoryformatversion",
            "core.bare",
            "core.sparsecheckout",
            "remote.origin.url",
            "remote.origin.fetch",
            "branch.main.remote",
            "branch.main.merge",
            "submodule.sub.url",
            "extensions.objectformat",
            "init.defaultbranch",
            "gc.auto",
            "pack.threads",
            "receive.fsckobjects",
            "transfer.fsckobjects",
            "fetch.prune",
            "pull.rebase",
            "push.default",
            "color.ui",
            "advice.detachedhead",
            "gui.encoding",
            "include.path",
            "includeif.path",
            "commit.gpgsign",
            "tag.gpgsign",
        ];
        for k in local.iter().chain(not_local.iter()) {
            assert!(config_key_is_safe(k), "{k} must pass the allowlist");
            assert!(config_family(k).is_some(), "{k} has no CONFIG_FAMILIES row");
        }
        for k in local {
            assert!(config_key_is_local(k), "{k}");
        }
        for k in not_local {
            assert!(!config_key_is_local(k), "{k}");
        }
        // A family without a row is local: it asks rather than loses.
        assert!(config_key_is_local("newsection.key"));
    }

    fn put(gd: &Path, rel: &str, text: &str) {
        let p = gd.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn a_stock_git_directory_holds_nothing_of_its_own_and_each_rule_bites() {
        let tmp = tempfile::tempdir().unwrap();
        let gd = tmp.path();
        let exclude = "# git ls-files --others --exclude-from=.git/info/exclude\n\
                       # Lines that start with '#' are comments.\n\n";
        put(gd, "hooks/pre-commit.sample", "#!/bin/sh\n");
        put(gd, "hooks/update.sample", "#!/bin/sh\n");
        put(gd, "info/exclude", exclude);
        let listing = format!(
            "{}\trefs/heads/main\n{}\trefs/tags/v1^{{}}\n",
            "a".repeat(40),
            "b".repeat(40)
        );
        put(gd, "info/refs", &listing);
        put(gd, "description", &format!("{STOCK_DESCRIPTION}\n"));
        put(gd, "COMMIT_EDITMSG", "the last commit's message\n");
        put(gd, "TAG_EDITMSG", "");
        std::fs::create_dir_all(gd.join("branches")).unwrap();
        let keys = [
            "core.bare".to_string(),
            "remote.origin.url".to_string(),
            "branch.main.merge".to_string(),
        ];
        assert_eq!(local_admin_content(gd, &keys), Ok(vec![]));
        // The same template with Windows line endings, and an empty description.
        put(gd, "info/exclude", &exclude.replace('\n', "\r\n"));
        put(gd, "description", "");
        assert_eq!(local_admin_content(gd, &keys), Ok(vec![]));

        put(gd, "info/exclude", "# comment\n*.log\n");
        put(gd, "info/attributes", "* text\n");
        put(gd, "info/refs", "a note\n");
        put(gd, "info/notes.txt", "x");
        put(gd, "hooks/pre-commit", "#!/bin/sh\n");
        std::fs::create_dir_all(gd.join("hooks").join("dir.sample")).unwrap();
        put(gd, "description", "my project\n");
        put(gd, "TAG_EDITMSG", "release notes\n");
        put(gd, "branches/origin", "https://example.invalid/x\n");
        let found = local_admin_content(gd, &["user.email".to_string()]).unwrap();
        assert_eq!(
            found,
            vec![
                "TAG_EDITMSG",
                "branches/origin",
                "config: user.email",
                "description",
                "hooks/dir.sample",
                "hooks/pre-commit",
                "info/attributes",
                "info/exclude",
                "info/notes.txt",
                "info/refs",
            ]
        );
    }

    #[test]
    fn an_oversized_listing_or_file_is_unknown_not_stock() {
        let tmp = tempfile::tempdir().unwrap();
        let many = tmp.path().join("many");
        for i in 0..=MAX_ADMIN_ENTRIES {
            put(&many, &format!("hooks/h{i}.sample"), "");
        }
        assert!(local_admin_content(&many, &[]).is_err());
        let big = tmp.path().join("big");
        let text = "#".repeat(MAX_PSEUDOREF_BYTES as usize + 1);
        put(&big, "description", &text);
        assert!(local_admin_content(&big, &[]).is_err());
    }

    #[test]
    fn lfs_keys_are_not_allowed() {
        // An LFS repo defines filter.lfs.* (a program) and is reported unknown; lfs.* itself
        // is not allowlisted either.
        for k in [
            "lfs.url",
            "lfs.concurrenttransfers",
            "filter.lfs.clean",
            "lfs.x.url",
        ] {
            assert!(!config_key_is_safe(k), "{k}");
        }
        assert!(config_key_is_safe("remote.origin.url"));
    }

    #[test]
    fn network_predicate() {
        for u in [
            "https://example.invalid/x.git",
            "ssh://git@example.invalid/x.git",
            "git@example.invalid:o/r.git",
            "example.invalid:o/r.git",
            "git://example.invalid/r.git",
        ] {
            assert!(is_network_url(u), "{u} should be network");
        }
        for u in [
            "/srv/git/r.git",
            "../r.git",
            "./r.git",
            "r.git",
            "~/r.git",
            "file:///srv/r.git",
            "C:\\repos\\r.git",
            "C:/repos/r.git",
            "/srv/backup.bundle",
            "https://example.invalid/backup.bundle",
            "",
        ] {
            assert!(!is_network_url(u), "{u:?} should not be network");
        }
    }

    /// Review M5: the callers `lstat` an entry before reading it, but the entry can be replaced
    /// in between. A FIFO swapped in must be refused, not waited on (an open for reading blocks
    /// until a writer comes), and a symlink swapped in must not be followed.
    #[cfg(unix)]
    mod swapped_in {
        use super::*;
        use crate::test_fifo::{mkfifo, must_not_block};

        #[test]
        fn read_bounded_refuses_a_fifo_without_blocking() {
            let tmp = tempfile::tempdir().unwrap();
            let fifo = tmp.path().join("ORIG_HEAD");
            mkfifo(&fifo);
            let p = fifo.clone();
            let got = must_not_block(&fifo, move || read_bounded(&p));
            assert_eq!(got, Err("not a regular file".to_string()));
        }

        #[test]
        fn a_git_file_that_is_a_fifo_is_refused_without_blocking() {
            let tmp = tempfile::tempdir().unwrap();
            let wt = tmp.path().to_path_buf();
            let fifo = wt.join(".git");
            mkfifo(&fifo);
            let (w, p) = (wt.clone(), fifo.clone());
            let got = must_not_block(&fifo, move || gitfile_dir(&w, &p));
            assert_eq!(
                got,
                Err(
                    "the .git file was not read (not a regular file), so its config.worktree was \
                     not found"
                        .to_string()
                )
            );
        }

        #[test]
        fn read_bounded_does_not_follow_a_symlink() {
            let tmp = tempfile::tempdir().unwrap();
            let target = tmp.path().join("elsewhere");
            std::fs::write(&target, "gitdir: /somewhere\n").unwrap();
            let link = tmp.path().join(".git");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(read_bounded(&link).is_err(), "a symlink was followed");
        }

        #[test]
        fn control_read_bounded_reads_a_regular_file() {
            let tmp = tempfile::tempdir().unwrap();
            let file = tmp.path().join("ORIG_HEAD");
            std::fs::write(&file, "abc\n").unwrap();
            assert_eq!(read_bounded(&file), Ok(b"abc\n".to_vec()));
        }
    }
}
