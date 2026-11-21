# disk-cleaner design

Status: draft. Phase 1 (core library and CLI, dry-run only) is in progress. Nothing here deletes data yet.

## 1. What it is

A desktop disk analyzer and cleaner for Windows, macOS and Linux. It shows where space went the way
TreeSize and Disk Space Analyzer do, and adds what those tools lack: it understands what each
directory *is* (a git repo, a linked worktree, a build output, a package cache, an agent session log,
a produced video) and therefore what can be removed, what must be backed up first, and what must be
left alone.

It is built for developers and people who run coding agents, whose disks fill with hundreds of
clones, worktrees, `node_modules` trees and session logs. It stays useful for everyone else: the
analyzer, the biggest-files view and the cache rules work on any machine.

## 2. Principles

1. **Never lose unique data.** An item is removed only after the tool has proven that every byte of
   it is regenerable or exists somewhere else, and has re-checked that proof at the moment of removal.
2. **Unknown means keep.** A failed or timed-out check never reads as "clean". Every fact has three
   states: yes, no, unknown.
3. **Plan, then apply.** Scanning and planning are read-only. A plan is a file the user can read,
   edit and approve. Apply executes exactly that plan and nothing else.
4. **Everything removed is findable.** Each apply appends to a manifest: what was removed, why it was
   safe, where its backup lives and how to restore it.
5. **Reuse, don't rebuild.** Storage backends, git plumbing, directory walking and artifact rules
   come from established open-source projects (section 9).
6. **Secrets stay local.** Ignored files such as `.env` are never pushed to a git remote. Credentials
   live in the OS keychain or in rclone's own config, never in the manifest, logs or plans.

## 3. Classification

Every directory gets one primary class. Size is attributed to the innermost classified ancestor.

| Class | Examples | Default disposition |
|---|---|---|
| `repo` | directory with a `.git` directory | keep; eligible for backup-then-remove when idle and fully remote |
| `worktree` | directory with a `.git` file (linked worktree) | as `repo`, checked against its own HEAD |
| `bare-repo` | `*.git` with `HEAD`, `objects`, `refs` | never auto-removed while any worktree or clone depends on it |
| `regenerable` | `node_modules`, `target`, `.venv`, `.next`, `DerivedData`, `Pods`, `__pycache__` | remove when the owning project is not active |
| `cache` | npm, pnpm, cargo registry, Gradle, pip, Playwright browsers, Homebrew, Xcode caches | remove by age or size policy |
| `agent-session` | Claude Code, Codex and similar transcripts, write-ahead logs, work directories | back up continuously; remove from disk only after a verified backup and past the age threshold |
| `output` | video, audio, images, archives, model weights, datasets, VM disks | never to git; back up to storage, then remove by policy |
| `loose-code` | source files or a project manifest outside any repo | ask: connect an origin, back up raw, remove, or leave |
| `system` / `user-data` | OS files, documents, photos, application data | analyze only |

Rules that follow from the principles:

- A build directory (`dist`, `build`, `out`, `target`) is regenerable only inside a project, and is
  scanned for `output` files first. A rendered video inside `out/` is an output, not an artifact.
  The same holds for every artifact: a model of any size in `.venv` or `node_modules` outside an
  installed package's own directory, or above the output threshold anywhere in `.venv` or
  `node_modules`, makes it ask, naming the file.
- Artifact rules are data (a rules file), not code, so the community can extend them. Each rule
  names the marker that proves the project type (for example `Cargo.toml` next to `target`).

### Repo safety facts

For each repo and each worktree the scanner records, each as yes / no / unknown:

- has at least one **network** remote (a local path or a bundle file is not a backup);
- HEAD is contained in a remote branch, checked for the worktree's own HEAD, including detached HEADs;
- every local branch and tag is contained in a remote ref;
- stash is empty;
- the working tree is clean (tracked changes and untracked files);
- ignored-but-unique files exist (`.env`, local databases, anything ignored that is not a
  regenerable directory);
- another clone or worktree depends on this object store (alternates, linked worktrees). Such
  dependencies are found from the references themselves during the walk (symlinks and alternates
  anywhere, `.git` entries inside dependency directories), not only from the repositories that are
  classified, so a clone hidden inside `node_modules` still protects its provider. A repository
  that sets `extensions.preciousObjects = true` declares such dependents itself. Hard links and
  references from outside the scanned roots cannot be seen (docs/SCAN-FORMAT.md, known limits).
  Folders the scan could not read and mount points it did not enter are recorded as the scan's
  coverage, and by default they make every repository's `no_dependents` unknown, so it asks. A
  clone or link in a place the scan could not read may be the only thing that needs the
  repository, and disclosing the gap does not make removing it safe. A user who knows what those
  places hold opts in with `accept_incomplete_coverage` (`--accept-incomplete-coverage`), and each
  removal decision then says what was not read.

At apply time the remote facts are confirmed against the live remote (`git ls-remote`), because
remote-tracking refs can be stale.

A repo is **safe to remove** only when every fact is the safe value. Anything else is **ambiguous**
and goes through recovery.

### The preserve obligation

Safe to remove never means removed without a backup. The facts show that every commit is on a
remote; they cannot show that nothing else in the git directory is unique (a custom hook, a
recorded rerere resolution, a draft in `COMMIT_EDITMSG`, a note under `logs/`), and judging those
files one kind at a time never converged. So every plan action that removes a git node is named for
its contract (`backup-then-remove`, `recover-backup-then-remove`) and carries `preserve`, which
splits every file of the git directory, by name alone, into exactly one of two obligations:

- `preserve.git_admin`, every file that does not have an object-store name: always backed up
  (saved with the manifest, the restore record an apply step writes, and read back) before
  anything is removed;
- `preserve.object_store`, files under `objects/` named like loose objects, packs and their
  indexes and sidecars, multi-pack-indexes and commit-graphs: the object data, backed up too
  before anything is removed. It is counted apart only because it can be large.

No file is exempt because of its name, its first bytes or any check this tool runs: earlier
versions tried content checks, then an opt-in git verification of the object store, and review
showed each could pass a file holding someone's notes. This phase defines no verification of
object data; a later apply phase may define one against the live remote, and `plan` refuses a scan
file that claims one. A git directory whose own files exceed `max_git_admin_bytes` (64 MiB by default) asks instead, because that
much is the user's data, not git's settings. No apply phase exists yet; the rule is part of the plan format
(docs/SCAN-FORMAT.md, "The preserve obligation"), which also lists the existing asks the backup
makes redundant and keeps them for now.

### Activity

"Active" is a user decision informed by evidence, never a hard-coded age. Evidence gathered: newest
modification time of non-regenerable files, last commit time, running processes whose working
directory is inside the tree, and agent sessions that reference it.

The **active window** is a setting (`active_window_days`, default 7). A project is active when a
process is running inside it, or when its non-regenerable files were modified or its last commit
falls within the window. Files under `.git` do not count, and a regenerable directory such as
`node_modules` takes its project's activity, so a write only inside it does not make the project
active. The setting lives in
the config file, is editable in the app's settings and can be overridden per run on the CLI
(`--active-window-days`). The tool proposes the resulting active set; the user can pin a project as
always active or mark one inactive. Active projects and everything under them are left untouched,
including their regenerable directories.

## 4. Recovery for ambiguous repos

Modes: `ask` (per item), `auto` (apply the default action), `report` (list only).

Default action for a repo with unsaved work:

1. Commit nothing on the user's branches. Create `recovery/<host>/<date>/<branch>` refs that capture
   local commits, and a snapshot commit of tracked changes plus untracked files (built with a
   temporary index so the working tree and the real index are untouched).
2. Push the recovery refs to the network remote. If there is no remote, or the push is refused,
   write a `git bundle` of all refs and send it to backup storage.
3. Ignored-but-unique files go to backup storage as a private archive. They are never committed.
4. Stashes are converted to recovery refs.
5. Record everything in the manifest, then re-run the safety facts. Only a repo that now passes is
   eligible for removal.

Linked worktrees are removed with `git worktree remove` semantics so the parent stays consistent.

## 5. Backup connectors

Storage is delegated to [rclone](https://rclone.org) (MIT), driven through its remote-control API.
That gives Dropbox, S3 and Glacier classes, Backblaze B2, Google Drive, OneDrive, SFTP, a local or
external disk and many more without writing a client for each. The app helps the user create and
test a remote; rclone owns the credentials.

- **Cold storage** for outputs, raw loose code, bundles and old agent sessions.
- **Verification**: an upload counts only after a checksum comparison (`rclone check`, or a size and
  hash listing where the backend has no common hash). Rate-limit responses pause and resume; a
  partial run is never recorded as a backup.
- **Agent session backup** runs on a schedule, is incremental, and reports its own freshness. A
  stale backup blocks removal of any session data.

## 6. Policies and thresholds

A policy is a rule set the user enables, for example:

- regenerable directories in projects idle for N days: remove;
- package caches not accessed for N days, or above N GB: remove;
- repos idle for N days: recover if ambiguous, then back up the git directory's own files and remove;
- agent sessions older than N days and not tied to an active project: back up, verify, remove;
- outputs older than N days: back up, verify, remove.

Policies produce a plan. A scheduled run can apply the parts the user has marked automatic and queue
the rest for review. No default policy is automatic.

## 7. Manifest

An append-only JSON Lines file per machine, mirrored to backup storage. One record per removed or
backed-up item: path, class, size, hashes or git object ids, the safety facts and when they were
verified, backup location (for a git node, where its git directory's own files were saved), and
the exact restore command. `disk-cleaner find <text>` searches it;
`disk-cleaner restore <id>` puts an item back (re-clone at the recorded commit, fetch a bundle,
download an archive, or note that it is regenerable and how).

## 8. Interface

Informed by a walk through of Disk Space Analyzer and TreeSize.

- **Sidebar**: volumes, favourite folders, connected machines, then task views (Analyze, Projects,
  Reclaim, Backups, Manifest).
- **Analyze**: column outline with size, percentage bar, file count and last modified; sunburst and
  treemap, switchable, with hover details and click to drill; breadcrumb bar; biggest files with a
  detail pane; grouping of small and hidden items. Colour encodes class, so regenerable and cache
  space is visible at a glance.
- **Projects**: every repo and worktree with its safety facts, idle time, reclaimable size and state
  (active, safe, ambiguous). Filter by remote, age and state.
- **Reclaim**: the plan as a basket. Every line says what will happen and why it is safe. Total
  space to be freed is shown before anything runs.
- **Backups**: connectors, last verified run, what is waiting.
- **Manifest**: search and restore.
- Keyboard access, focus order, labels and contrast are part of the definition of done for each view.
- A CLI exposes the same operations and produces JSON, so remote and headless machines are first
  class. The desktop app can show a scan produced by the CLI on another machine.

## 9. Building blocks

| Need | Choice | Licence | Note |
|---|---|---|---|
| App shell | Tauri 2 | MIT or Apache-2.0 | small binaries, Rust core shared with the CLI |
| Parallel walk | `jwalk` | MIT | used by dua-cli; `ignore` for gitignore-aware passes |
| Git | `git` CLI first, `gix` where it pays | MIT or Apache-2.0 | the CLI is the reference for worktrees and pushes |
| Storage | rclone (rc API) | MIT | bundled or discovered on PATH |
| Artifact rules | modelled on kondo, npkill, cargo-sweep | MIT | rules kept as data |
| Recycle bin | `trash` crate | MIT | reversible removal where the user wants it |
| Charts | d3-hierarchy | ISC | sunburst and treemap layout |
| Volumes, processes | `sysinfo` | MIT | free space, process working directories |

Versions are pinned in lockfiles. Each dependency's licence is checked in CI (`cargo deny`).

Faster scan paths that read filesystem metadata directly (NTFS MFT on Windows, `getattrlistbulk` on
macOS) are later optimisations behind the same scanner interface.

## 10. Architecture

```
crates/
  core/      scan, classify, facts, plan, manifest (no UI, no network)
  git/       repo facts and recovery, wrapping the git CLI
  storage/   rclone rc client, verification
  cli/       disk-cleaner binary
app/         Tauri shell and web UI
rules/       artifact and cache rules (data)
```

The core never deletes. Removal lives in one module with one entry point that takes an approved plan
record and re-verifies it.

## 11. Phases

1. Core and CLI: `scan`, `classify`, `plan`, all read-only, JSON output, synthetic fixtures.
2. Recovery, storage connectors, apply with manifest and restore.
3. Desktop app.
4. Scheduled policies, multi-machine view, fast platform scanners.

## 12. Out of scope for now

Duplicate file detection (czkawka does this well), uninstalling applications, and registry or system
"optimisation".
