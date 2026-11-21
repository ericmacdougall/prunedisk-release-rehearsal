//! Data model shared by scan, plan and the JSON files. See `docs/SCAN-FORMAT.md`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Version of the scan and plan JSON schemas. Bumped on any incompatible change.
pub const SCHEMA_VERSION: u32 = 15;

/// Primary class of a directory (design section 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    Repo,
    Worktree,
    BareRepo,
    Regenerable,
    Cache,
    Output,
    LooseCode,
    Other,
}

impl Class {
    /// The name used in JSON and in reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Repo => "repo",
            Class::Worktree => "worktree",
            Class::BareRepo => "bare-repo",
            Class::Regenerable => "regenerable",
            Class::Cache => "cache",
            Class::Output => "output",
            Class::LooseCode => "loose-code",
            Class::Other => "other",
        }
    }

    /// repo, worktree or bare-repo.
    pub fn is_git(self) -> bool {
        matches!(self, Class::Repo | Class::Worktree | Class::BareRepo)
    }

    /// regenerable or cache.
    pub fn is_artifact(self) -> bool {
        matches!(self, Class::Regenerable | Class::Cache)
    }
}

/// A three-valued fact. `Unknown` is never treated as the safe value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tri {
    Yes,
    No,
    #[default]
    Unknown,
}

impl Tri {
    /// `Yes` for true, `No` for false.
    pub fn from_bool(b: bool) -> Tri {
        if b {
            Tri::Yes
        } else {
            Tri::No
        }
    }

    /// Display word.
    pub fn as_str(self) -> &'static str {
        match self {
            Tri::Yes => "yes",
            Tri::No => "no",
            Tri::Unknown => "unknown",
        }
    }
}

/// The repo safety facts of design section 3. Each fact is phrased so that `Yes` is the safe value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Facts {
    /// At least one remote that is a network location (a local path or bundle is not a backup).
    pub has_network_remote: Tri,
    /// The worktree's own HEAD (including a detached HEAD) is contained in a network remote branch.
    pub head_on_remote: Tri,
    /// Every local branch and tag is contained in a network remote ref.
    pub all_branches_on_remote: Tri,
    /// The stash is empty.
    pub stash_empty: Tri,
    /// No tracked changes and no untracked files.
    pub tree_clean: Tri,
    /// No ignored files outside regenerable rule directories (`.env`, local databases...).
    pub no_unique_ignored: Tri,
    /// No linked worktree or alternates clone depends on this object store.
    pub no_dependents: Tri,
    /// Every commit reachable from any reflog entry (an unpushed commit after `reset --hard`, an
    /// amended or rebased-away commit) or named by a pseudoref at the top of the git directory
    /// (`ORIG_HEAD` and the like) is contained in a network remote's tracking refs.
    pub reflog_saved: Tri,
    /// No unfinished rebase, merge, cherry-pick, revert or bisect, and no local-only Git LFS or
    /// git-annex content: state that no ref records. Never `No`: when something is found the
    /// fact is `Unknown`, because this tool cannot show it is saved.
    pub no_pending_state: Tri,
    /// The repository has the plain layout (see `docs/SCAN-FORMAT.md`): a real `.git` directory
    /// with the expected entries, no alternates, commondir, worktrees, modules, annex or LFS
    /// content, and only refs under `refs/heads`, `refs/tags`, `refs/remotes` and `refs/stash`
    /// that point at commits. Only a plain repo may be backed up then removed.
    pub plain_layout: Tri,
    /// The git directory holds nothing of its own outside the object store, refs and logs: no
    /// custom hook, no file in `info/` beyond the stock comment-only `exclude`, a stock or empty
    /// `description`, no unsent tag, merge or squash message, no legacy `branches/` or
    /// `remotes/` file, no recorded conflict resolution, and no config value that only this
    /// repository holds (see `docs/SCAN-FORMAT.md`). `No` names what was found
    /// ([`RepoInfo::local_admin_content`]); `Unknown` when a bounded read or listing failed.
    #[serde(default)]
    pub no_local_admin_content: Tri,
}

impl Facts {
    /// Every fact paired with its JSON name.
    pub fn entries(&self) -> [(&'static str, Tri); 11] {
        [
            ("has_network_remote", self.has_network_remote),
            ("head_on_remote", self.head_on_remote),
            ("all_branches_on_remote", self.all_branches_on_remote),
            ("stash_empty", self.stash_empty),
            ("tree_clean", self.tree_clean),
            ("no_unique_ignored", self.no_unique_ignored),
            ("no_dependents", self.no_dependents),
            ("reflog_saved", self.reflog_saved),
            ("no_pending_state", self.no_pending_state),
            ("plain_layout", self.plain_layout),
            ("no_local_admin_content", self.no_local_admin_content),
        ]
    }

    /// True only when every fact is `Yes`.
    pub fn all_yes(&self) -> bool {
        self.entries().iter().all(|(_, t)| *t == Tri::Yes)
    }

    /// True when at least one fact is `Unknown`.
    pub fn any_unknown(&self) -> bool {
        self.entries().iter().any(|(_, t)| *t == Tri::Unknown)
    }

    /// Names of the facts that are `No`.
    pub fn failing(&self) -> Vec<&'static str> {
        self.entries()
            .iter()
            .filter(|(_, t)| *t == Tri::No)
            .map(|(n, _)| *n)
            .collect()
    }

    /// Names of the facts that are `Unknown`.
    pub fn unknown(&self) -> Vec<&'static str> {
        self.entries()
            .iter()
            .filter(|(_, t)| *t == Tri::Unknown)
            .map(|(n, _)| *n)
            .collect()
    }
}

/// A git remote with credentials already removed from the URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    pub name: String,
    pub url: String,
    /// Whether the URL counts as a network location.
    pub network: bool,
}

/// Size breakdown of a repo or worktree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSizes {
    /// Everything under the repo directory, including nested classified directories.
    pub total: u64,
    /// The `.git` directory.
    pub git: u64,
    /// Regenerable and cache directories directly inside the repo.
    pub regenerable: u64,
    /// Files with output extensions anywhere inside the repo.
    pub output: u64,
}

/// Git-derived information about a repo, worktree or bare repo. A field this build does not
/// know is refused when a scan file is read (see [`crate::scan_from_json`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoInfo {
    pub facts: Facts,
    /// The first layout check that failed or could not be made, when `plain_layout` is not
    /// `yes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_issue: Option<String>,
    /// Why each non-`Yes` fact is not `Yes` (redacted).
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub remotes: Vec<Remote>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_commit: Option<String>,
    /// Commit time (unix seconds) of the newest commit on any ref.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_commit_time: Option<i64>,
    /// Live linked worktrees of this repo, as reported by git (may lie outside the scan).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worktrees: Vec<String>,
    /// For a linked worktree: the repo it belongs to, as reported by git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_repo: Option<String>,
    /// For a linked worktree whose parent is inside the scan: index into `nodes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_node: Option<usize>,
    /// Clones inside the scan that borrow objects from this repo (alternates).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependents: Vec<String>,
    /// The repository sets `extensions.preciousObjects = true`: it declares that other
    /// repositories borrow its objects (`no_dependents` is then `no`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub precious_objects: bool,
    /// Symlinks inside the scan that point into this repository's git directory and are not
    /// checkouts (a convenience link to `.git/hooks`, say): removing the repository breaks them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links_into_git_dir: Vec<String>,
    /// Index entries marked skip-worktree or assume-unchanged whose file is present in the
    /// working tree. Git hides edits to them from `status`, `diff`, `add` and `stash`, so they
    /// cannot be saved automatically; the plan asks. (Sparse-checkout entries whose file is
    /// absent are not counted.)
    #[serde(default, skip_serializing_if = "is_zero")]
    pub hidden_index_entries: u64,
    /// Git directories of submodules stored under `.git/modules` (their stash and local
    /// branches are deleted with this repo), as real paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub submodule_gitdirs: Vec<String>,
    /// This node's own git directory (a real path): `<dir>/.git`, or what its `.git` file names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_dir: Option<String>,
    /// Pseudorefs at the top of this worktree's git directory (`ORIG_HEAD`, `MERGE_AUTOSTASH`,
    /// any file there that names a commit) whose commit no network remote's tracking refs
    /// contain: `reflog_saved` is then `no`. Each is the file's exact name: a name that would
    /// not be shown exactly is never listed here (it is unchecked instead).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsaved_pseudorefs: Vec<String>,
    /// One sentence per pseudoref that could not be read, parsed or compared with the remote
    /// (one naming a tag, tree or blob object or a missing object, holding text that is not an
    /// object id, or whose name cannot be shown exactly): `reflog_saved` is then `unknown`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unchecked_pseudorefs: Vec<String>,
    /// Up to five examples, sorted, of what the git directory holds of its own
    /// (`hooks/pre-commit`, `info/notes.txt`, `config: user.email`): `no_local_admin_content`
    /// is then `no`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub local_admin_content: Vec<String>,
    /// How many such entries were found in all (the examples are the first five).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub local_admin_content_count: u64,
    /// The git directory's own files, which a removal must back up first (see
    /// [`crate::preserve`]). `None` when they were not counted: the plan then never removes the
    /// node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_admin: Option<GitAdminSurvey>,
    /// The object store's files, which a removal must back up first too (see
    /// [`crate::preserve`]; this format defines no verification of object data). `None` when
    /// they were not counted: the plan then never removes the node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_store: Option<ObjectStoreSurvey>,
    pub sizes: RepoSizes,
}

/// What [`crate::preserve::survey`] counted of one git directory's object store: the files in
/// `objects/` whose names are those of object-store files (loose objects, packs and their
/// indexes and sidecars, multi-pack-indexes, commit-graphs). All of them are to be backed up
/// before a removal: there is no verification field, and a scan file that carries one is
/// refused.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectStoreSurvey {
    /// How many files.
    pub files: u64,
    /// Their allocated bytes.
    pub bytes: u64,
}

/// What [`crate::preserve::survey`] counted of one git directory's own files: everything in it
/// that is not an object file of its object store.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitAdminSurvey {
    /// How many files (regular files, symlinks and other non-directory entries).
    pub files: u64,
    /// Their allocated bytes.
    pub bytes: u64,
    /// Up to five of them, sorted, whose names are not ones every clone has (relative to the git
    /// directory, `/`-separated).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
    /// The largest of them, largest first (up to three).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub largest: Vec<AdminFile>,
}

/// One of a git directory's own files and its allocated bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminFile {
    pub path: String,
    pub bytes: u64,
}

/// A classified directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub path: String,
    pub class: Class,
    /// Index of the innermost classified directory that encloses this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<usize>,
    /// Name of the rule that matched (regenerable and cache only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// Bytes attributed to this node itself, excluding nested classified directories.
    pub bytes_self: u64,
    /// Bytes of this node including nested classified directories.
    pub bytes_total: u64,
    pub files_total: u64,
    /// Files with output extensions in this node's subtree (allocated bytes).
    pub output_bytes: u64,
    /// A build directory that holds output files: never offered for removal.
    pub contains_outputs: bool,
    /// Output files in this directory that are not listed in `outputs` one by one (small ones
    /// beyond the first few per directory), counted toward its ask (omitted when 0).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub outputs_unlisted: u64,
    /// Newest modification time (unix seconds) of files attributed to this node, excluding
    /// `.git` and nested classified directories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest_mtime: Option<i64>,
    /// Entries inside this node (or directly attributed to it) that could not be read. The scan
    /// of this node is incomplete, so it is never offered for removal.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unreadable: u64,
    /// Directories inside this node (or the node itself) on another file system that
    /// `--one-file-system` did not enter. Their contents were never looked at, so the node is
    /// never offered for removal.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub skipped_mounts: u64,
    /// Why this node must not be removed without a person looking (regenerable and cache only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<RepoInfo>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl Node {
    /// A node with zeroed numbers.
    pub fn new(path: impl Into<String>, class: Class) -> Node {
        Node {
            path: path.into(),
            class,
            parent: None,
            rule: None,
            bytes_self: 0,
            bytes_total: 0,
            files_total: 0,
            output_bytes: 0,
            contains_outputs: false,
            outputs_unlisted: 0,
            newest_mtime: None,
            unreadable: 0,
            skipped_mounts: 0,
            ask_reason: None,
            repo: None,
        }
    }
}

/// An output file (video, archive, model weight...) listed individually.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputFile {
    pub path: String,
    pub bytes: u64,
    /// Index of the node that owns the directory it sits in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<usize>,
}

/// One of the biggest files seen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BigFile {
    pub path: String,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<usize>,
}

/// A problem met during the scan (unreadable entry...). Never fatal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanIssue {
    pub path: String,
    pub message: String,
}

/// Totals over the whole scan.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Totals {
    pub files: u64,
    pub dirs: u64,
    pub symlinks: u64,
    /// Allocated bytes (blocks on Unix), each hard-linked inode counted once.
    pub allocated_bytes: u64,
    /// Sum of file lengths, each hard-linked inode counted once.
    pub logical_bytes: u64,
    pub hardlink_duplicates_skipped: u64,
    /// Entries that could not be read; see `issues`.
    pub unreadable: u64,
    pub mount_points_skipped: u64,
    /// Output-extension files anywhere in the scan (allocated bytes).
    pub output_bytes: u64,
    /// `bytes_self` summed per class, plus `other` for unclassified space. Output bytes are
    /// not a separate slice here; see `output_bytes`.
    pub bytes_by_class: BTreeMap<String, u64>,
}

/// Where the walk could not look. Links, clones and alternates files inside these places were not
/// seen, so a repository they depend on can look free of dependents.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    /// Nothing was unreadable and no mount point was skipped.
    pub complete: bool,
    /// Directories whose contents could not be listed (each counted once).
    pub unreadable_dirs: u64,
    /// Up to 20 of them.
    pub unreadable_dir_examples: Vec<String>,
    /// Other entries that could not be read (metadata or walk errors); see `issues`.
    pub other_unreadable_entries: u64,
    /// Directories on another file system that `--one-file-system` did not enter.
    pub mount_points_skipped: u64,
    /// Up to 20 of them.
    pub mount_point_examples: Vec<String>,
}

/// Most example paths kept per kind in [`Coverage`].
pub const COVERAGE_EXAMPLES: usize = 20;

/// Options recorded in the scan file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanOptionsRecord {
    pub one_file_system: bool,
    pub top_files: usize,
    pub min_output_bytes: u64,
    pub git_facts: bool,
}

/// The result of `scan`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub schema_version: u32,
    pub tool_version: String,
    /// Unix seconds when the scan finished.
    pub generated_at: i64,
    pub roots: Vec<String>,
    pub options: ScanOptionsRecord,
    pub totals: Totals,
    pub nodes: Vec<Node>,
    pub outputs: Vec<OutputFile>,
    pub big_files: Vec<BigFile>,
    /// At most the first 1000 issues; `totals.unreadable` has the full count.
    pub issues: Vec<ScanIssue>,
    pub warnings: Vec<String>,
    /// Scan roots that lie inside a git repository the scan did not cover.
    #[serde(default)]
    pub enclosing_repos: Vec<EnclosingRepo>,
    /// `volume` when every root is a mount point (or a file system root), else `subtree`: other
    /// clones outside the scanned folders cannot be seen by a subtree scan.
    #[serde(default = "default_scope")]
    pub scope: String,
    /// Where the walk could not look (required: a scan without it is not trusted as complete).
    pub coverage: Coverage,
}

pub fn default_scope() -> String {
    "subtree".to_string()
}

/// A scan root inside a repository above it. The repository was not walked or assessed; only the
/// artifact directories below the root were checked against its git.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnclosingRepo {
    pub root: String,
    pub repo: String,
    pub note: String,
}
