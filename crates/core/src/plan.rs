//! Dry-run plan from a scan (design sections 3, 4 and 6). Nothing here changes anything on disk.
//!
//! The plan is advice: per classified directory an action, a plain-language reason and the
//! bytes that would be reclaimed. There is no built-in age: the active window and the active
//! paths are inputs. A project is active when an `--active` path covers it, when its newest
//! activity is inside the window, or when a running process has its working directory inside it.
//! A repo whose facts are not all `Yes` never gets a removing action.

use crate::model::*;
use crate::procs::ProcessCheck;
use crate::words::{agree, count, kib, size, some_of};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What the plan proposes for one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// Leave it alone.
    Keep,
    /// Delete a regenerable or cache directory.
    RemoveRegenerable,
    /// A git node with unsaved work: save it to recovery refs or a bundle, re-check, back up
    /// the git directory's own files and its object store ([`PlanItem::preserve`]), verify that
    /// backup, then remove.
    RecoverBackupThenRemove,
    /// A git node whose commits are all on a network remote: back up the git directory's own
    /// files and its object store ([`PlanItem::preserve`]), verify that backup, then remove.
    BackupThenRemove,
    /// A person decides.
    Ask,
}

impl Action {
    /// JSON and display name.
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Keep => "keep",
            Action::RemoveRegenerable => "remove-regenerable",
            Action::RecoverBackupThenRemove => "recover-backup-then-remove",
            Action::BackupThenRemove => "backup-then-remove",
            Action::Ask => "ask",
        }
    }

    /// Does the action end with something being removed from disk?
    pub fn removes(self) -> bool {
        matches!(
            self,
            Action::RemoveRegenerable | Action::RecoverBackupThenRemove | Action::BackupThenRemove
        )
    }
}

/// Whether a project was judged active, idle, or could not be judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectState {
    Active,
    Idle,
    Unknown,
}

/// Inputs of the plan. All three are the user's.
#[derive(Debug, Clone)]
pub struct PlanOptions {
    /// Paths the user says are active. Anything under one, or enclosing one, is kept.
    pub active: Vec<PathBuf>,
    /// A project is active when its newest activity is at most this many days old.
    pub active_window_days: u64,
    /// Where the window came from (`default`, a config file path, `--active-window-days`).
    pub window_source: String,
    /// Current time, unix seconds (an input so plans are reproducible).
    pub now: i64,
    /// Working directories of running processes.
    pub process: ProcessCheck,
    /// The user accepts a partial process check (some processes could not be inspected) as
    /// enough to remove things. Off by default: an unreadable process may be working inside the
    /// candidate.
    pub trust_partial_process_check: bool,
    /// The user accepts a scan that could not look everywhere (unreadable folders, mount points
    /// not entered) as enough to remove repositories: each such reason then says what was not
    /// read. Off by default: with incomplete coverage every repository's `no_dependents = yes`
    /// becomes `unknown`, so it asks.
    pub accept_incomplete_coverage: bool,
    /// Most allocated bytes of its own files ([`PlanItem::preserve`]) one git directory may hold
    /// and still be removed after a backup; more asks, naming the largest files (a big file
    /// dropped into `.git` is the user's data, not git's settings).
    pub max_git_admin_bytes: u64,
}

impl PlanOptions {
    /// No active paths, no running processes known, window from the caller.
    pub fn new(active_window_days: u64, now: i64) -> PlanOptions {
        PlanOptions {
            active: Vec::new(),
            active_window_days,
            window_source: "input".to_string(),
            now,
            process: ProcessCheck::none_running(),
            trust_partial_process_check: false,
            accept_incomplete_coverage: false,
            max_git_admin_bytes: crate::config::DEFAULT_MAX_GIT_ADMIN_BYTES,
        }
    }
}

/// What must be backed up, and the backup verified, before a removing action removes anything.
/// Set on every removing item that removes a git node (repo, worktree or bare repo), whether its
/// own or one nested inside it, and only there.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preserve {
    pub git_admin: PreserveGitAdmin,
    #[serde(default)]
    pub object_store: PreserveObjectStore,
}

/// The git directories' object-store files: the object data (see [`crate::preserve`]). They are
/// counted apart from [`PreserveGitAdmin`] because they can be very large, but the obligation is
/// the same: a consumer that applies a plan must refuse to remove the item unless a verified
/// backup of all of them exists. This format defines no verification of object data that could
/// stand in for that backup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreserveObjectStore {
    /// How many files, summed over the item's git directories.
    pub files: u64,
    /// Their allocated bytes.
    pub bytes: u64,
}

/// The git directories' own files: everything in each git directory that is not an object file
/// of its object store (see [`crate::preserve`]). A consumer that applies a plan must refuse to
/// remove the item unless a verified backup of all of them exists.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreserveGitAdmin {
    /// The git directories whose files must be saved: the item's own and those of git nodes
    /// inside it, as the scan spelled them.
    pub git_dirs: Vec<String>,
    /// How many files (regular files, symlinks and other non-directory entries).
    pub files: u64,
    /// Their allocated bytes.
    pub bytes: u64,
    /// Up to five of them, sorted, whose names are not ones every clone has, relative to the
    /// item's directory with `/` separators (`.git/COMMIT_EDITMSG`, `.git/hooks/custom`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
}

/// One line of the plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanItem {
    /// Index into the scan's `nodes`.
    pub node: usize,
    pub path: String,
    pub class: Class,
    pub action: Action,
    /// Was the project judged active, idle or unknown, and why.
    pub state: ProjectState,
    pub state_reason: String,
    /// Plain-language reason for the action.
    pub reason: String,
    /// Bytes freed if the action completes (0 for keep and ask).
    pub reclaimable_bytes: u64,
    pub bytes_total: u64,
    /// Whole days since the newest activity evidence, when there is any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days_since_activity: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<Facts>,
    /// What must be saved before this item is removed (removing git items only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserve: Option<Preserve>,
}

/// Count and bytes for one action.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionTotal {
    pub items: u64,
    pub reclaimable_bytes: u64,
}

/// Plan totals. Nested items are not double counted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanTotals {
    /// Bytes freed if every removing action completed.
    pub reclaimable_bytes: u64,
    /// Bytes freed by `remove-regenerable` (needs no recovery or remote verification step).
    pub ready_bytes: u64,
    pub by_action: BTreeMap<String, ActionTotal>,
}

/// A dry-run plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub schema_version: u32,
    /// Always true in this phase: a plan changes nothing.
    pub dry_run: bool,
    pub generated_at: i64,
    pub scan_generated_at: i64,
    /// The window used to decide activity.
    pub active_window_days: u64,
    /// Where the window came from.
    pub window_source: String,
    /// `ok`, `partial` or `unknown`: could running processes be inspected?
    pub process_check: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_check_note: Option<String>,
    /// The user chose to treat a partial process check as sufficient
    /// (`trust_partial_process_check` / `--trust-partial-process-check`).
    #[serde(default)]
    pub trusted_partial_process_check: bool,
    /// Always false in this phase: remote facts come from local remote-tracking refs, which can
    /// be stale. Apply must re-check them with `git ls-remote` before removing anything.
    pub remote_facts_verified: bool,
    pub remote_facts_note: String,
    /// Scan roots inside a repository the scan did not cover: that repository was not assessed.
    #[serde(default)]
    pub enclosing_repos: Vec<EnclosingRepo>,
    /// `volume` when every scanned root is a mount point, else `subtree`. `no_dependents` means
    /// no dependents were found within the scanned roots; a subtree scan cannot see clones
    /// elsewhere.
    #[serde(default = "crate::model::default_scope")]
    pub scope: String,
    /// The scan read every folder and entered every mount point (its `coverage.complete`).
    pub coverage_complete: bool,
    /// When coverage is incomplete: one sentence saying what was not read and that links or
    /// clones there were not checked. With `accepted_incomplete_coverage` every
    /// backup-then-remove and recover-backup-then-remove reason repeats it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_note: Option<String>,
    /// The user accepted incomplete coverage (`accept_incomplete_coverage` /
    /// `--accept-incomplete-coverage`). Without it, incomplete coverage made every repository's
    /// `no_dependents = yes` `unknown`, so none is removed.
    #[serde(default)]
    pub accepted_incomplete_coverage: bool,
    pub active: Vec<String>,
    pub totals: PlanTotals,
    pub items: Vec<PlanItem>,
}

/// Real path of `p`, without the `\\?\` prefix Windows canonicalization adds (process working
/// directories are reported without it).
fn canon(p: &Path) -> PathBuf {
    let real = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let text = real.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => real,
    }
}

fn under_active(path: &Path, active: &[PathBuf]) -> Option<String> {
    if active.is_empty() {
        return None;
    }
    // Compare real paths as well as the stored spelling: a scan taken through a symlink or
    // relative path must still match an `--active` path spelled another way.
    let real = canon(path);
    active
        .iter()
        .find(|a| {
            let real_a = canon(a);
            path.starts_with(a)
                || a.starts_with(path)
                || real.starts_with(&real_a)
                || real_a.starts_with(&real)
        })
        .map(|a| a.to_string_lossy().into_owned())
}

/// Resolve the user's `--active` paths: each must exist, is made absolute and symlink-free, and
/// must lie inside or around something the scan covered. A mistyped path must not silently
/// protect nothing.
pub fn resolve_active(
    scan: &ScanResult,
    raw: &[PathBuf],
) -> std::result::Result<Vec<PathBuf>, crate::Error> {
    let covered: Vec<PathBuf> = scan.roots.iter().map(|r| canon(Path::new(r))).collect();
    let mut out = Vec::new();
    for p in raw {
        let real = std::fs::canonicalize(p).map_err(|e| crate::Error::Active {
            path: p.clone(),
            message: format!("cannot use this --active path: {e}"),
        })?;
        let real = canon(&real);
        let related = covered
            .iter()
            .any(|r| real.starts_with(r) || r.starts_with(&real))
            || scan
                .nodes
                .iter()
                .any(|n| canon(Path::new(&n.path)).starts_with(&real));
        if !related {
            return Err(crate::Error::Active {
                path: p.clone(),
                message: "this --active path is not inside or around anything the scan covered"
                    .to_string(),
            });
        }
        out.push(real);
    }
    Ok(out)
}

/// A running process's working directory inside this node's directory.
fn process_hit(node: &Node, process: &ProcessCheck) -> Option<String> {
    let raw = Path::new(&node.path);
    let real = canon(raw);
    process
        .inside(raw)
        .or_else(|| process.inside(&real))
        .map(|c| format!("{} (working directory of a running process)", c.display()))
}

/// What the plan needs to know about every node, computed once with no recursion (a scan file can
/// describe a tree of any depth): parents first, then per-node results inherited down the tree.
///
/// Parent links that are out of range or form a cycle are cut (the scan loader rejects them; this
/// keeps `plan` total when a caller builds a scan by hand).
struct Ctx {
    /// Parent of each node, cycles and bad indexes removed.
    parent: Vec<Option<usize>>,
    /// Node indexes, every node after its parent.
    order: Vec<usize>,
    /// Active path covering the node or a node enclosing it.
    active: Vec<Option<String>>,
    /// Running process inside the node or a node enclosing it.
    process: Vec<Option<String>>,
    /// Newest activity evidence of the project the node belongs to.
    activity: Vec<Option<i64>>,
}

fn build_ctx(scan: &ScanResult, opts: &PlanOptions) -> Ctx {
    let n = scan.nodes.len();
    let mut parent: Vec<Option<usize>> = scan
        .nodes
        .iter()
        .enumerate()
        .map(|(i, nd)| nd.parent.filter(|p| *p < n && *p != i))
        .collect();
    let mut depth = vec![usize::MAX; n];
    let mut on_path = vec![false; n];
    for start in 0..n {
        if depth[start] != usize::MAX {
            continue;
        }
        let mut path: Vec<usize> = Vec::new();
        let mut cur = Some(start);
        let mut base = 0usize;
        while let Some(c) = cur {
            if depth[c] != usize::MAX {
                base = depth[c] + 1;
                break;
            }
            if on_path[c] {
                // A cycle: cut the link that closed it.
                if let Some(&last) = path.last() {
                    parent[last] = None;
                }
                base = 0;
                break;
            }
            on_path[c] = true;
            path.push(c);
            cur = parent[c];
        }
        for (off, &c) in path.iter().rev().enumerate() {
            depth[c] = base + off;
            on_path[c] = false;
        }
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| depth[i]);

    let mut active: Vec<Option<String>> = vec![None; n];
    let mut process: Vec<Option<String>> = vec![None; n];
    let mut activity: Vec<Option<i64>> = vec![None; n];
    for &i in &order {
        let node = &scan.nodes[i];
        let par = parent[i];
        active[i] = under_active(Path::new(&node.path), &opts.active)
            .or_else(|| par.and_then(|p| active[p].clone()));
        process[i] =
            process_hit(node, &opts.process).or_else(|| par.and_then(|p| process[p].clone()));
        // Regenerable directories use their owning project.
        activity[i] = match par {
            Some(p) if node.class.is_artifact() => activity[p],
            _ => {
                let commit = node.repo.as_ref().and_then(|r| r.last_commit_time);
                max_opt(node.newest_mtime, commit)
            }
        };
    }
    Ctx {
        parent,
        order,
        active,
        process,
        activity,
    }
}

fn max_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// Judge one node: active (path, process or recent activity), idle or unknown, with the reason.
fn judge(ctx: &Ctx, idx: usize, opts: &PlanOptions) -> (ProjectState, String, Option<u64>) {
    let last = ctx.activity[idx];
    let days = last.map(|t| (opts.now.saturating_sub(t).max(0) / 86_400) as u64);
    if let Some(a) = ctx.active[idx].as_deref() {
        return (
            ProjectState::Active,
            format!("covered by the active path {a}"),
            days,
        );
    }
    if let Some(p) = ctx.process[idx].as_deref() {
        return (ProjectState::Active, format!("in use: {p}"), days);
    }
    let window = opts.active_window_days;
    match (last, days) {
        (Some(t), Some(d)) => {
            let age = opts.now.saturating_sub(t).max(0);
            // An absurdly large window saturates instead of wrapping negative.
            let window_secs = i64::try_from(window)
                .unwrap_or(i64::MAX)
                .saturating_mul(86_400);
            if age <= window_secs {
                (
                    ProjectState::Active,
                    format!(
                        "newest activity {} ago, within the {window}-day active window",
                        count(d, "day", "days")
                    ),
                    days,
                )
            } else {
                (
                    ProjectState::Idle,
                    format!(
                        "newest activity {} ago, outside the {window}-day active window",
                        count(d, "day", "days")
                    ),
                    days,
                )
            }
        }
        _ => (
            ProjectState::Unknown,
            "no activity evidence (no files and no commits found)".to_string(),
            None,
        ),
    }
}

/// A submodule git directory under this repo's `.git/modules` that no scanned git node inside
/// the repo uses (matched by identity, not by count). Its stash and branches are deleted with
/// the repo and nobody has looked at them.
fn orphan_submodule_gitdir(scan: &ScanResult, idx: usize) -> Option<String> {
    let dirs = &scan.nodes[idx].repo.as_ref()?.submodule_gitdirs;
    if dirs.is_empty() {
        return None;
    }
    let inside = |j: usize| -> bool {
        let mut cur = scan.nodes[j].parent.filter(|p| *p < scan.nodes.len());
        let mut steps = 0usize;
        while let Some(p) = cur {
            if p == idx {
                return true;
            }
            steps += 1;
            if steps > scan.nodes.len() {
                return false;
            }
            cur = scan.nodes[p].parent.filter(|q| *q < scan.nodes.len());
        }
        false
    };
    let owned: Vec<PathBuf> = scan
        .nodes
        .iter()
        .enumerate()
        .filter(|(j, n)| n.class.is_git() && inside(*j))
        .filter_map(|(_, n)| {
            n.repo
                .as_ref()?
                .git_dir
                .as_ref()
                .map(|g| canon(Path::new(g)))
        })
        .collect();
    dirs.iter()
        .find(|d| !owned.iter().any(|o| *o == canon(Path::new(d.as_str()))))
        .cloned()
}

/// Build a dry-run plan from a scan.
pub fn plan(scan: &ScanResult, opts: &PlanOptions) -> Plan {
    let ctx = build_ctx(scan, opts);
    let complete = scan.coverage.complete;
    // Strict coverage, the default: what was not read, unless the user accepted that it does not
    // block removal.
    let strict: Option<String> = if !opts.accept_incomplete_coverage && !complete {
        Some(coverage_gaps(&scan.coverage))
    } else {
        None
    };
    let mut items: Vec<PlanItem> = Vec::new();
    // Per item: its action from its own facts and contents, before the process check (which
    // downgrades every removing item alike, so it is no reason for one item to veto another).
    let mut own_action: Vec<Action> = Vec::new();
    for (idx, node) in scan.nodes.iter().enumerate() {
        if matches!(node.class, Class::Output | Class::Other) {
            continue;
        }
        let (state, state_reason, days) = judge(&ctx, idx, opts);
        // Decided first on the scan's own facts, so a reason about this node itself (an
        // unreadable entry or a skipped mount inside it) is the one given; otherwise, under
        // strict coverage, it is decided again with the unproven `no_dependents` as unknown, so
        // the reason says what was not read whenever the facts show it.
        let (mut action, mut reason, mut reclaim) = decide(scan, idx, state, &state_reason, None);
        let removing = action.removes();
        if removing {
            if let Some(why) = node.ask_reason.as_deref() {
                reason = format!("{reason}; but it {why}");
                action = Action::Ask;
                reclaim = 0;
            } else if node.unreadable > 0 {
                reason = format!(
                    "{reason}; but {} inside could not be read, so the scan of it is incomplete",
                    count(node.unreadable, "entry", "entries")
                );
                action = Action::Ask;
                reclaim = 0;
            } else if node.skipped_mounts > 0 {
                let n = node.skipped_mounts;
                reason = format!(
                    "{reason}; but {} inside {} on another file system and {} not scanned \
                     (--one-file-system), so what is there was never looked at",
                    count(n, "directory", "directories"),
                    agree(n, "is", "are"),
                    agree(n, "was", "were")
                );
                action = Action::Ask;
                reclaim = 0;
            } else if let Some(orphan) = orphan_submodule_gitdir(scan, idx) {
                reason = format!(
                    "{reason}; but the submodule git directory {orphan} under .git/modules has \
                     no checked-out, scanned worktree, so its stash or local branches would be \
                     deleted unseen"
                );
                action = Action::Ask;
                reclaim = 0;
            }
        }
        if action.removes() && node.class.is_git() {
            // Nothing removes a git node unless its git directory's own files are backed up
            // first: they must have been counted, and be few enough to be git's own.
            if let Some(why) = preserve_veto(node, opts.max_git_admin_bytes) {
                reason = format!("{reason}; but {why}");
                action = Action::Ask;
                reclaim = 0;
            }
        }
        let vetoed = removing && action == Action::Ask;
        if let Some(gap) = strict.as_deref() {
            if !vetoed {
                // A repository that would be removed now asks; one that asks for another reason
                // also names what was not read and the opt-in. Other nodes are unaffected.
                (action, reason, reclaim) = decide(scan, idx, state, &state_reason, Some(gap));
            }
        }
        own_action.push(action);
        if action.removes() {
            match &opts.process {
                ProcessCheck::Unknown { reason: why } => {
                    // Cannot prove nothing is using it: a person decides.
                    reason =
                        format!("{reason}; but running processes could not be inspected ({why})");
                    action = Action::Ask;
                    reclaim = 0;
                }
                ProcessCheck::Partial { unreadable, .. } if !opts.trust_partial_process_check => {
                    // One of the processes nobody could look at may be working inside it.
                    let n = *unreadable as u64;
                    reason = format!(
                        "{reason}; but {} could not be inspected, so {} may be using it \
                         (--trust-partial-process-check accepts that risk)",
                        count(n, "running process", "running processes"),
                        agree(n, "it", "one of them")
                    );
                    action = Action::Ask;
                    reclaim = 0;
                }
                _ => {}
            }
        }
        items.push(PlanItem {
            node: idx,
            path: node.path.clone(),
            class: node.class,
            action,
            state,
            state_reason,
            reason,
            reclaimable_bytes: reclaim,
            bytes_total: node.bytes_total,
            days_since_activity: days,
            facts: node
                .repo
                .as_ref()
                .map(|r| effective_facts(r.facts, strict.is_some())),
            preserve: None,
        });
    }

    // An enclosing directory goes with everything inside it. When a nested node is kept, asked
    // about, needs recovery, or holds something that cannot be removed, the enclosing node can
    // not be removed either: it becomes `ask`, and its reason leads with that veto (whatever else
    // also downgraded it). Deepest nodes first, so a downgraded node blocks its own parent in
    // turn (no assumption about the order of `nodes`).
    let mut vetoes: Vec<Vec<String>> = vec![Vec::new(); items.len()];
    let mut pos: Vec<Option<usize>> = vec![None; scan.nodes.len()];
    for (k, it) in items.iter().enumerate() {
        pos[it.node] = Some(k);
    }
    for &node_idx in ctx.order.iter().rev() {
        let k = match pos[node_idx] {
            Some(k) => k,
            None => continue,
        };
        let action = own_action[k];
        if !matches!(
            action,
            Action::Keep | Action::Ask | Action::RecoverBackupThenRemove
        ) {
            continue;
        }
        let parent = match ctx.parent[node_idx].and_then(|p| pos[p]) {
            Some(pk) => pk,
            None => continue,
        };
        if own_action[parent].removes() {
            let why = match action {
                Action::Keep => "is kept",
                Action::Ask => "needs a person's decision",
                _ => "has unsaved work to recover first",
            };
            let veto = format!("the nested {} {why}", items[k].path);
            vetoes[parent].push(veto);
            own_action[parent] = Action::Ask;
            items[parent].action = Action::Ask;
            items[parent].reclaimable_bytes = 0;
        }
    }
    for (it, v) in items.iter_mut().zip(vetoes) {
        if !v.is_empty() {
            it.reason = format!(
                "{}, and everything inside goes with this one; on its own: {}",
                v.join("; "),
                it.reason
            );
        }
    }

    // Every item that removes a git node, its own or one inside it, carries what must be backed
    // up first and says so; one that cannot asks. No removing action is left without it.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); scan.nodes.len()];
    for (i, p) in ctx.parent.iter().enumerate() {
        if let Some(p) = *p {
            children[p].push(i);
        }
    }
    for it in items.iter_mut() {
        if !it.action.removes() {
            continue;
        }
        let why = match preserve_for(scan, &children, it.node) {
            Ok(None) => continue,
            Ok(Some(pre)) if it.class.is_git() => {
                let g = &pre.git_admin;
                let s = &pre.object_store;
                let inside = g.git_dirs.len().saturating_sub(1) as u64;
                let also = if inside > 0 {
                    format!(
                        ", with those of {} inside it",
                        count(inside, "git repository", "git repositories")
                    )
                } else {
                    String::new()
                };
                it.reason = format!(
                    "{}; its {} ({}{also}: settings, hooks, logs and the like) {} saved with the \
                     manifest first; its object store ({}, {}) holds object data that must be \
                     backed up too before removal, then it is removed",
                    it.reason,
                    count(g.files, "local git file", "local git files"),
                    kib(g.bytes),
                    agree(g.files, "is", "are"),
                    count(s.files, "file", "files"),
                    size(s.bytes)
                );
                it.preserve = Some(pre);
                continue;
            }
            // The scan makes a directory that holds a git repository ask, so a removal that is
            // not of a git node never reaches here; this keeps the rule total.
            Ok(Some(_)) => "it holds a git repository whose git directory would be removed \
                            without a backup"
                .to_string(),
            Err(path) => format!(
                "the git directory of {path} inside it was not counted, so it could not be \
                 backed up first"
            ),
        };
        it.reason = format!("{}; but {why}", it.reason);
        it.action = Action::Ask;
        it.reclaimable_bytes = 0;
    }

    // Every decision that rests on "nothing depends on it" says where the scan could not look.
    // Last, so it is only added to items that are still removed or recovered.
    if let Some(gap) = coverage_reason(&scan.coverage, complete) {
        for it in items.iter_mut() {
            if matches!(
                it.action,
                Action::BackupThenRemove | Action::RecoverBackupThenRemove
            ) {
                it.reason.push_str(&gap);
            }
        }
    }

    // Totals: an item inside another removing item is already counted by the outer one.
    let mut removing = vec![false; scan.nodes.len()];
    for it in &items {
        if it.action.removes() {
            removing[it.node] = true;
        }
    }
    // Is any enclosing node removing? Parents come first in `order`, so one pass is enough.
    let mut nested_in_removing = vec![false; scan.nodes.len()];
    for &i in &ctx.order {
        nested_in_removing[i] = ctx.parent[i]
            .map(|p| removing[p] || nested_in_removing[p])
            .unwrap_or(false);
    }
    let mut totals = PlanTotals::default();
    for it in &items {
        let slot = totals
            .by_action
            .entry(it.action.as_str().to_string())
            .or_default();
        slot.items += 1;
        if it.action.removes() && !nested_in_removing[it.node] {
            slot.reclaimable_bytes = slot.reclaimable_bytes.saturating_add(it.reclaimable_bytes);
            totals.reclaimable_bytes = totals
                .reclaimable_bytes
                .saturating_add(it.reclaimable_bytes);
            // Only regenerable directories are ready without a recovery or verification step:
            // a backup-then-remove rests on remote facts that stay unverified until apply.
            if it.action == Action::RemoveRegenerable {
                totals.ready_bytes = totals.ready_bytes.saturating_add(it.reclaimable_bytes);
            }
        }
    }

    Plan {
        schema_version: SCHEMA_VERSION,
        dry_run: true,
        generated_at: opts.now,
        scan_generated_at: scan.generated_at,
        active_window_days: opts.active_window_days,
        window_source: opts.window_source.clone(),
        process_check: opts.process.status().to_string(),
        process_check_note: opts.process.note(),
        trusted_partial_process_check: opts.trust_partial_process_check,
        remote_facts_verified: false,
        remote_facts_note:
            "remote facts come from local remote-tracking refs, which can be stale; \
                            they are unverified until apply re-checks them with git ls-remote"
                .to_string(),
        enclosing_repos: scan.enclosing_repos.clone(),
        scope: scan.scope.clone(),
        coverage_complete: complete,
        coverage_note: coverage_note(&scan.coverage, complete),
        accepted_incomplete_coverage: opts.accept_incomplete_coverage,
        active: opts
            .active
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
        totals,
        items,
    }
}

/// The places the scan could not look, each as (`"1 folder"` or `"2 folders"`, verb phrase), and
/// how many there are in all.
fn gaps(c: &Coverage) -> (Vec<(String, &'static str)>, u64) {
    let kinds = [
        (
            c.unreadable_dirs,
            "folder",
            "folders",
            "could not be read",
            "could not be read",
        ),
        (
            c.other_unreadable_entries,
            "other entry",
            "other entries",
            "could not be read",
            "could not be read",
        ),
        (
            c.mount_points_skipped,
            "mount point",
            "mount points",
            "was not entered (--one-file-system)",
            "were not entered (--one-file-system)",
        ),
    ];
    let mut out = Vec::new();
    let mut total = 0u64;
    for (n, one, many, verb_one, verb_many) in kinds {
        if n > 0 {
            total = total.saturating_add(n);
            out.push((count(n, one, many), agree(n, verb_one, verb_many)));
        }
    }
    (out, total)
}

/// `1 folder could not be read and 2 mount points were not entered (--one-file-system)`.
fn coverage_gaps(c: &Coverage) -> String {
    let (parts, _) = gaps(c);
    if parts.is_empty() {
        return "the scan did not record where it could not look".to_string();
    }
    parts
        .iter()
        .map(|(noun, verb)| format!("{noun} {verb}"))
        .collect::<Vec<_>>()
        .join(" and ")
}

/// The plan header's sentence for an incomplete scan (also printed by the scan report).
pub fn coverage_note(c: &Coverage, complete: bool) -> Option<String> {
    if complete {
        return None;
    }
    let (_, total) = gaps(c);
    Some(format!(
        "Coverage is incomplete: {}, so links or clones inside {} were not checked.",
        coverage_gaps(c),
        agree(total, "it", "them")
    ))
}

/// The ending of every backup-then-remove and recover-backup-then-remove reason when coverage is
/// incomplete: `; 1 folder in this scan could not be read; dependents inside it were not checked`.
fn coverage_reason(c: &Coverage, complete: bool) -> Option<String> {
    if complete {
        return None;
    }
    let (parts, total) = gaps(c);
    if parts.is_empty() {
        return Some(
            "; the scan did not record where it could not look, so dependents may have been \
             missed"
                .to_string(),
        );
    }
    let text = parts
        .iter()
        .enumerate()
        .map(|(k, (noun, verb))| {
            if k == 0 {
                format!("{noun} in this scan {verb}")
            } else {
                format!("{noun} {verb}")
            }
        })
        .collect::<Vec<_>>()
        .join(" and ");
    Some(format!(
        "; {text}; dependents inside {} were not checked",
        agree(total, "it", "them")
    ))
}

/// A repository's facts as the plan uses them: under strict coverage (the default when coverage
/// is incomplete) an unproven `no_dependents = yes` is `unknown`.
fn effective_facts(mut facts: Facts, strict: bool) -> Facts {
    if strict && facts.no_dependents == Tri::Yes {
        facts.no_dependents = Tri::Unknown;
    }
    facts
}

/// Each fact said in words: what `no` means (a noun phrase: what is not saved elsewhere) and what
/// `unknown` means (what could not be checked and what would settle it). Every reason that
/// rests on a fact says it in these words and then names the fact as a trailing tag,
/// `[tree_clean]`, so the words never depend on knowing the field names.
static FACT_WORDS: [(&str, &str, &str); 11] = [
    (
        "has_network_remote",
        "no remote that points to another machine",
        "the remotes could not be read (git failed); run the scan again with git working",
    ),
    (
        "head_on_remote",
        "a checked-out commit that no network remote has",
        "HEAD could not be compared with the remote-tracking refs (git failed, or there is no \
         commit yet); run the scan again",
    ),
    (
        "all_branches_on_remote",
        "local branches, tags or other refs that no network remote has",
        "the local branches could not be compared with the remote-tracking refs (git failed); \
         run the scan again",
    ),
    (
        "stash_empty",
        "stashed changes",
        "the stash could not be read (git failed); run the scan again",
    ),
    (
        "tree_clean",
        "uncommitted changes or untracked files",
        "whether the working tree is clean could not be established (git failed, or a submodule \
         could not be read); run the scan again",
    ),
    (
        "no_unique_ignored",
        "ignored files outside regenerable directories (such as .env files or local databases)",
        "the ignored files could not be listed (git failed); run the scan again",
    ),
    (
        "no_dependents",
        "other checkouts, clones or links that use this repository's objects or refs",
        "whether other repositories use this one's objects could not be established (an \
         alternates file, symlink or git directory could not be resolved or lies outside the \
         scanned folders); scan the folders that hold those repositories too",
    ),
    (
        "reflog_saved",
        "commits that only the reflog or a pseudoref such as ORIG_HEAD still reaches",
        "commits kept only by the reflog or a pseudoref such as ORIG_HEAD could not be compared \
         with the remote (git failed or timed out, or there is no commit yet); run the scan again",
    ),
    (
        "no_pending_state",
        "an unfinished operation or local-only LFS or annex content",
        "an unfinished rebase, merge, cherry-pick, revert or bisect, or local LFS or annex \
         content, is present or could not be checked; finish or abandon it",
    ),
    (
        "plain_layout",
        "a layout check failed",
        "the layout could not be checked",
    ),
    (
        "no_local_admin_content",
        "files of its own in its git directory (a custom hook, a note in info, an edited \
         description, an unsent message or a setting only this repository has) that exist \
         nowhere else",
        "the git directory's own files (hooks, info, description, messages) could not all be \
         listed or read, or there are too many to check; look at the git directory and run the \
         scan again",
    ),
];

/// The words for fact `name` in state `t` (`No` or `Unknown`), without the tag.
pub fn fact_sentence(name: &str, t: Tri) -> &'static str {
    let found = FACT_WORDS.iter().find(|(n, _, _)| *n == name);
    match (found, t) {
        (Some(&(_, no, _)), Tri::No) => no,
        (Some(&(_, _, unknown)), Tri::Unknown) => unknown,
        (None, Tri::No) => "a safety check failed",
        (None, Tri::Unknown) => "a safety check could not be made; run the scan again",
        (_, Tri::Yes) => "the check passed",
    }
}

/// [`fact_sentence`] followed by the fact's name as a tag: `stashed changes [stash_empty]`.
pub fn fact_words(name: &str, t: Tri) -> String {
    format!("{} [{name}]", fact_sentence(name, t))
}

/// Why a repository that others depend on asks: up to three of them are named.
fn dependents_reason(node: &Node) -> String {
    const TAG: &str = " [no_dependents]";
    let mut who: Vec<String> = Vec::new();
    let mut links: Vec<String> = Vec::new();
    let mut precious = false;
    if let Some(r) = node.repo.as_ref() {
        who.extend(r.dependents.iter().cloned());
        for w in &r.worktrees {
            if !who.contains(w) {
                who.push(w.clone());
            }
        }
        links.extend(r.links_into_git_dir.iter().cloned());
        precious = r.precious_objects;
    }
    if precious {
        let known: Vec<String> = who.iter().chain(links.iter()).cloned().collect();
        let named = if known.is_empty() {
            String::new()
        } else {
            format!(" (known: {})", some_of(&known, 3))
        };
        return format!(
            "this repository sets extensions.preciousObjects, which declares that other \
             repositories borrow its objects{named}; deal with those first{TAG}"
        );
    }
    let mut parts: Vec<String> = Vec::new();
    if !who.is_empty() {
        let n = who.len() as u64;
        let one = "other checkout or clone uses";
        let many = "other checkouts or clones use";
        parts.push(format!(
            "{} this repository's objects or refs: {}",
            count(n, one, many),
            some_of(&who, 3)
        ));
    }
    match links.len() {
        0 => {}
        1 => parts.push(format!(
            "a link at {} points into this repository's git directory",
            links[0]
        )),
        n => parts.push(format!(
            "{n} links point into this repository's git directory: {}",
            some_of(&links, 3)
        )),
    }
    let n = (who.len() + links.len()) as u64;
    if parts.is_empty() {
        let what = fact_sentence("no_dependents", Tri::No);
        parts.push(format!("{what} were found"));
    }
    format!(
        "{}; deal with {} first{TAG}",
        parts.join("; "),
        agree(n, "it", "those")
    )
}

/// Why a git node may not be removed even after a backup of its git directory's own files: they
/// were not counted, or there are more of them than `max` bytes (the largest are named).
fn preserve_veto(node: &Node, max: u64) -> Option<String> {
    let Some(g) = node.repo.as_ref().and_then(|r| r.git_admin.as_ref()) else {
        return Some(
            "its git directory's own files were not counted (see the notes), so they could not \
             be backed up first"
                .to_string(),
        );
    };
    if node
        .repo
        .as_ref()
        .and_then(|r| r.object_store.as_ref())
        .is_none()
    {
        return Some(
            "its object store's files were not counted (see the notes), so they could not be \
             backed up first"
                .to_string(),
        );
    }
    if g.bytes <= max {
        return None;
    }
    let largest: Vec<String> = g
        .largest
        .iter()
        .map(|f| format!("{} ({})", f.path, kib(f.bytes)))
        .collect();
    Some(format!(
        "its git directory holds {} of files of its own besides git objects, more than \
         max_git_admin_bytes ({}): that is the user's data, not git's settings; the largest: {}; \
         move them out or raise max_git_admin_bytes",
        kib(g.bytes),
        kib(max),
        largest.join(", ")
    ))
}

/// A preserved file at least this large is named first among the examples (up to three), so a
/// large file kept inside `.git` (possible when `max_git_admin_bytes` is raised) is visible.
const LARGE_PRESERVED: u64 = 1024 * 1024;

/// What removing `idx` must back up first: the own files and the object store of every git
/// directory it removes (its own and those of git nodes inside it). `Err` names a git node inside
/// it whose files were not counted.
fn preserve_for(
    scan: &ScanResult,
    children: &[Vec<usize>],
    idx: usize,
) -> std::result::Result<Option<Preserve>, String> {
    let base = Path::new(&scan.nodes[idx].path);
    let mut stack = vec![idx];
    let mut found: Vec<usize> = Vec::new();
    while let Some(i) = stack.pop() {
        if scan.nodes[i].class.is_git() {
            found.push(i);
        }
        stack.extend(children[i].iter().copied());
    }
    if found.is_empty() {
        return Ok(None);
    }
    found.sort_unstable();
    let mut out = PreserveGitAdmin::default();
    let mut store = PreserveObjectStore::default();
    let mut examples: Vec<String> = Vec::new();
    let mut large: Vec<(u64, String)> = Vec::new();
    for i in found {
        let n = &scan.nodes[i];
        let (g, s) = n
            .repo
            .as_ref()
            .and_then(|r| Some((r.git_admin.as_ref()?, r.object_store.as_ref()?)))
            .ok_or_else(|| n.path.clone())?;
        store.files = store.files.saturating_add(s.files);
        store.bytes = store.bytes.saturating_add(s.bytes);
        let gd = if n.class == Class::BareRepo {
            PathBuf::from(&n.path)
        } else {
            Path::new(&n.path).join(".git")
        };
        // Examples relative to the removed directory, `/`-separated.
        let prefix = gd
            .strip_prefix(base)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let under = |e: &str| {
            if prefix.is_empty() {
                e.to_string()
            } else {
                format!("{prefix}/{e}")
            }
        };
        examples.extend(g.examples.iter().map(|e| under(e)));
        large.extend(
            g.largest
                .iter()
                .filter(|f| f.bytes >= LARGE_PRESERVED)
                .map(|f| (f.bytes, under(&f.path))),
        );
        out.git_dirs.push(gd.to_string_lossy().into_owned());
        out.files = out.files.saturating_add(g.files);
        out.bytes = out.bytes.saturating_add(g.bytes);
    }
    // The largest preserved files first, then the unusual names.
    large.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut named: Vec<String> = large.into_iter().take(3).map(|(_, p)| p).collect();
    examples.sort();
    for e in examples {
        if !named.contains(&e) {
            named.push(e);
        }
    }
    named.truncate(5);
    out.examples = named;
    Ok(Some(Preserve {
        git_admin: out,
        object_store: store,
    }))
}

/// Why a repository whose git directory holds files of its own asks: up to five are named.
fn local_admin_reason(node: &Node) -> String {
    let (mut found, total) = node
        .repo
        .as_ref()
        .map(|r| (r.local_admin_content.clone(), r.local_admin_content_count))
        .unwrap_or_default();
    let rest = total.saturating_sub(found.len() as u64);
    if rest > 0 {
        found.push(format!("{rest} more"));
    }
    let list = if found.is_empty() {
        String::new()
    } else {
        format!(": {}", found.join(", "))
    };
    format!(
        "this repository has files of its own in its git directory that exist nowhere \
         else{list}; copy them out or decide they can go [no_local_admin_content]"
    )
}

/// For each unknown fact: what could not be checked and what would settle it. `strict` is what
/// the scan could not read, when incomplete coverage was not accepted.
fn explain_unknown(node: &Node, facts: &Facts, strict: Option<&str>) -> String {
    let notes = node
        .repo
        .as_ref()
        .map(|r| r.notes.join(" "))
        .unwrap_or_default();
    // Was `no_dependents` itself unknown in the scan, or only made so by strict coverage?
    let scanned_unknown = node
        .repo
        .as_ref()
        .map(|r| r.facts.no_dependents == Tri::Unknown)
        .unwrap_or(true);
    let unchecked = node
        .repo
        .as_ref()
        .map(|r| r.unchecked_pseudorefs.clone())
        .unwrap_or_default();
    let parts: Vec<String> = facts
        .unknown()
        .into_iter()
        .map(|name| match (strict, name) {
            (Some(gap), "no_dependents") if !scanned_unknown => format!(
                "the scan was incomplete ({gap}), so links or clones there may depend on this \
                 repository; make them readable and scan again, or accept incomplete coverage \
                 with --accept-incomplete-coverage [{name}]"
            ),
            // Each entry is a whole sentence naming the pseudoref and what is wrong with it.
            (_, "reflog_saved") if !unchecked.is_empty() => format!(
                "{}, so what {} cannot be shown to be on a network remote; look at {} and run \
                 the scan again [{name}]",
                some_of(&unchecked, 3),
                agree(unchecked.len() as u64, "it keeps", "they keep"),
                agree(unchecked.len() as u64, "it", "them")
            ),
            (_, "all_branches_on_remote") if notes.contains("has tags") => format!(
                "this repository has local tags and tag names cannot be checked against the \
                 remote without contacting it; they will be verified when a removal is applied \
                 [{name}]"
            ),
            _ => fact_words(name, Tri::Unknown),
        })
        .collect();
    parts.join("; ")
}

/// Disclosure for a scan that covered only folders, not whole volumes.
fn scope_note(scan: &ScanResult) -> &'static str {
    if scan.scope == "volume" {
        ""
    } else {
        "; other clones outside the scanned folders were not checked for dependents"
    }
}

/// `strict` is set when the scan's coverage was incomplete and the user did not accept that:
/// what was not read.
fn decide(
    scan: &ScanResult,
    idx: usize,
    state: ProjectState,
    why: &str,
    strict: Option<&str>,
) -> (Action, String, u64) {
    let node = &scan.nodes[idx];
    if state == ProjectState::Active {
        return (Action::Keep, format!("active: {why}"), 0);
    }
    match node.class {
        Class::Regenerable | Class::Cache => {
            let what = node.rule.as_deref().unwrap_or("artifact");
            if node.contains_outputs {
                // Named relative to the directory, so a person can find them.
                let files: Vec<String> = scan
                    .outputs
                    .iter()
                    .filter(|o| o.node == Some(idx))
                    .map(|o| {
                        Path::new(&o.path)
                            .strip_prefix(&node.path)
                            .map(|r| r.to_string_lossy().replace('\\', "/"))
                            .unwrap_or_else(|_| o.path.clone())
                    })
                    .collect();
                // Small ones beyond the first few per directory are counted, not listed.
                let unlisted = node.outputs_unlisted;
                let n = files.len() as u64 + unlisted;
                let named = if files.is_empty() {
                    String::new()
                } else if unlisted == 0 {
                    format!(": {}", some_of(&files, 3))
                } else {
                    let shown = files.len().min(3);
                    format!(
                        ": {} and {} more",
                        files[..shown].join(", "),
                        n - shown as u64
                    )
                };
                return (
                    Action::Ask,
                    format!(
                        "{what} directory holds {} (video, archive, model, dataset...) that {} \
                         not regenerable{named}; not offered for removal until {} backed up or \
                         moved",
                        count(n, "output file", "output files"),
                        agree(n, "is", "are"),
                        agree(n, "it is", "they are")
                    ),
                    0,
                );
            }
            match state {
                ProjectState::Unknown => (
                    Action::Ask,
                    format!("cannot judge the owning project: {why}"),
                    0,
                ),
                _ => (
                    Action::RemoveRegenerable,
                    format!(
                        "{what} is rebuilt by the project's tooling; owning project idle: {why}"
                    ),
                    node.bytes_total,
                ),
            }
        }
        Class::Repo | Class::Worktree => {
            let facts = match node.repo.as_ref() {
                Some(r) => effective_facts(r.facts, strict.is_some()),
                None => {
                    return (
                        Action::Ask,
                        "repo facts are missing from the scan".to_string(),
                        0,
                    )
                }
            };
            if state == ProjectState::Unknown {
                return (Action::Ask, format!("cannot judge activity: {why}"), 0);
            }
            if facts.plain_layout != Tri::Yes {
                // Only a repository with the plain layout is ever removed: anything else may
                // keep data in places this tool does not look.
                let issue = node
                    .repo
                    .as_ref()
                    .and_then(|r| r.layout_issue.clone())
                    .unwrap_or_else(|| fact_sentence("plain_layout", facts.plain_layout).into());
                return (
                    Action::Ask,
                    format!("unusual repository layout: {issue} [plain_layout]"),
                    0,
                );
            }
            if facts.no_dependents == Tri::No {
                // Recovery saves this repository's own refs; it cannot make the repositories
                // that use its objects or refs safe, so it must not be removed first.
                return (Action::Ask, dependents_reason(node), 0);
            }
            if facts.no_local_admin_content == Tri::No {
                // Recovery saves commits and working-tree files, not the git directory's own
                // files, so only a person can keep them.
                return (Action::Ask, local_admin_reason(node), 0);
            }
            let hidden = node
                .repo
                .as_ref()
                .map(|r| r.hidden_index_entries)
                .unwrap_or(0);
            if hidden > 0 {
                // Recovery captures changes with git's own commands, which do not see edits to
                // these files: only a person can save them.
                let also = if facts.any_unknown() {
                    format!("; also, {}", explain_unknown(node, &facts, strict))
                } else {
                    String::new()
                };
                (
                    Action::Ask,
                    format!(
                        "idle ({why}), but {} marked skip-worktree or assume-unchanged, so git \
                         hides edits to {} and {} be saved automatically; review {} first \
                         [tree_clean]{also}",
                        count(hidden, "file is", "files are"),
                        agree(hidden, "it", "them"),
                        agree(hidden, "it cannot", "they cannot"),
                        agree(hidden, "it", "them")
                    ),
                    0,
                )
            } else if facts.any_unknown() {
                (
                    Action::Ask,
                    format!(
                        "idle ({why}) but cannot prove it is safe: {}",
                        explain_unknown(node, &facts, strict)
                    ),
                    0,
                )
            } else if !facts.all_yes() {
                let pseudorefs = node
                    .repo
                    .as_ref()
                    .map(|r| r.unsaved_pseudorefs.clone())
                    .unwrap_or_default();
                let unsaved = facts
                    .failing()
                    .into_iter()
                    .map(|name| match name {
                        "reflog_saved" if !pseudorefs.is_empty() => format!(
                            "commits kept only by {} or the reflog [{name}]",
                            some_of(&pseudorefs, 3)
                        ),
                        _ => fact_words(name, Tri::No),
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                (
                    Action::RecoverBackupThenRemove,
                    format!(
                        "idle ({why}), but not fully saved elsewhere: {unsaved}; recover first, \
                         then back up and remove{}",
                        scope_note(scan)
                    ),
                    node.bytes_total,
                )
            } else {
                (
                    Action::BackupThenRemove,
                    format!(
                        "idle ({why}); everything is on the remote: local remote-tracking refs \
                         say every commit (reflog entries and pseudorefs such as ORIG_HEAD \
                         included) is on a network remote, nothing is unsaved, no operation is \
                         half done and no dependents were found{} (unverified until apply \
                         re-checks the remote with git ls-remote)",
                        scope_note(scan)
                    ),
                    node.bytes_total,
                )
            }
        }
        Class::BareRepo => {
            let dependents = node
                .repo
                .as_ref()
                .map(|r| r.facts.no_dependents)
                .unwrap_or(Tri::Unknown);
            if dependents == Tri::No {
                (
                    Action::Keep,
                    "linked worktrees or clones depend on this object store".to_string(),
                    0,
                )
            } else {
                (
                    Action::Ask,
                    "bare repos are never removed automatically".to_string(),
                    0,
                )
            }
        }
        Class::LooseCode => {
            let inside = scan
                .enclosing_repos
                .iter()
                .any(|e| Path::new(&node.path).starts_with(&e.root));
            let why = if inside {
                "source inside a repository this scan did not cover (see the header): assess \
                 that repository, or leave it"
            } else {
                "source outside any repo: connect an origin, back it up raw, remove it, or \
                 leave it"
            };
            (Action::Ask, why.to_string(), 0)
        }
        Class::Output | Class::Other => (Action::Keep, "analysis only".to_string(), 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fact_has_words_for_no_and_for_unknown() {
        let facts = Facts::default();
        let names: Vec<&str> = facts.entries().iter().map(|(n, _)| *n).collect();
        assert_eq!(FACT_WORDS.len(), names.len());
        for name in names {
            assert!(
                FACT_WORDS.iter().any(|(n, _, _)| *n == name),
                "{name} has no row in FACT_WORDS"
            );
            for t in [Tri::No, Tri::Unknown] {
                let words = fact_sentence(name, t);
                assert!(
                    !words.is_empty() && !words.starts_with("a safety check"),
                    "{name} as {} has no sentence of its own",
                    t.as_str()
                );
                assert!(!words.contains(name), "{name}: {words}");
                assert_eq!(fact_words(name, t), format!("{words} [{name}]"));
            }
        }
    }
}
