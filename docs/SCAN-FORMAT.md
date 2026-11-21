# Scan and plan JSON format

Schema version: **15** (`schema_version` in every file). Any incompatible change bumps it; `plan`
refuses a scan with a different version. Version 2 added the facts `reflog_saved` and
`no_pending_state` to `repo.facts` (a version 1 scan has neither, and a plan built from it would
have read a repo as saved without them, so it is refused: scan again). Version 4 added the fact
`plain_layout`, `repo.layout_issue` and the scan field `scope`. Version 5 changed no field but
made `no_dependents` and `plain_layout` stricter (inbound references, symlinks anywhere in a git
directory, see below): a version 4 scan can say `yes` where version 5 says `no`, so it is refused.
Version 6 added the required scan field `coverage`, `repo.precious_objects` and the plan fields
`coverage_complete`, `coverage_note` and `required_full_coverage`; a version 5 scan does not say
where it could not look, so it is refused. Version 7 added `repo.hidden_index_entries` and
`repo.links_into_git_dir`; a version 6 scan cannot tell an edit git hides from an ordinary
untracked file, so it is refused. Version 8 reads the file names git prints byte for byte (a
version 7 scan could miss a hidden edit to a file whose name is not valid UTF-8, so it is
refused) and replaced the plan field `required_full_coverage` with `accepted_incomplete_coverage`
(incomplete coverage now makes repositories ask unless the user accepts it). Version 9 added
`repo.unsaved_pseudorefs` and `repo.unchecked_pseudorefs`: `reflog_saved` now also covers
pseudorefs such as `ORIG_HEAD`, and an unknown entry at the top of the git directory makes
`plain_layout` `no`; a version 8 scan could read a repository as saved while `ORIG_HEAD` alone kept
an unpushed commit, so it is refused. Version 10 added the fact `no_local_admin_content` and
`repo.local_admin_content` and `repo.local_admin_content_count`, made a pseudoref that names a tag
object `unknown`, and made `repo.unchecked_pseudorefs` whole sentences; a version 9 scan could
archive a repository whose custom hook or note existed nowhere else, so it is refused. Version 11
added `repo.git_admin` (the survey of the git directory's own files), the plan item field
`preserve` and the config key `max_git_admin_bytes`, renamed the plan actions `archive` to
`backup-then-remove` and `recover-then-archive` to `recover-backup-then-remove`, and made the
preserve obligation a rule of the plan format (see "The preserve obligation"); a version 10 scan
has no survey to back up from, so it is refused. Version 12 split every git directory into two
classes by name, added `repo.object_store`, the plan field `preserve.object_store`, the scan option
`--verify-objects` and the config key `verify_objects`, and dropped the content checks that
exempted files whose first bytes looked like object files (see "The preserve obligation"); a
version 11 scan left such files out of every obligation, so it is refused. Version 13 added
`repo.object_store.checked` and, when the scan verified the store, counts the kinds no git command
reads (a cruft pack's `.mtimes`, a multi-pack-index's `.rev`, the incremental multi-pack-index
layers) with the git directory's own files; a version 12 scan could mark a store verified while
such files were in it, so it is refused. Version 14 removed object verification: the scan option
`--verify-objects` (with `scan --config`, which only carried its setting), the config key
`verify_objects` (now refused with a message) and the fields `repo.object_store.verified`,
`repo.object_store.checked` and `preserve.object_store.verified`. This format defines no
verification of object data, both classes of a git directory are backed up, and a scan file that
claims a verification is refused as invalid. Version 14 also made every listed output file make
the artifact holding it ask, whatever the artifact's rule, and listed an output of any size directly
in an artifact's top directory. A version 13 scan could mark a store verified, or leave an output
in a dependency directory such as `.venv` unflagged, so it is refused. Version 15 lists an output
of any size anywhere inside an artifact unless it sits in an installed package's directory (see
"Outputs"), with the node field `outputs_unlisted`; a version 14 scan could leave a small nested
output (a `.venv/data/model.pt`) unlisted and its directory offered for removal, so it is refused.
All sizes are bytes. All times are unix seconds (UTC).
Paths are strings; a path that is not valid UTF-8 is converted lossily, so it will not round-trip.
Such a node therefore gets an `ask_reason` and is never offered for removal (see "File names git
prints").

Produce a scan with `disk-cleaner scan <path>... --out scan.json` (or `--json` for stdout) and a
plan with `disk-cleaner plan --scan scan.json [--active-window-days N] [--config FILE] [--active PATH]... [--trust-partial-process-check] [--accept-incomplete-coverage] [--json]`. Both are
read-only. Nothing in this phase deletes, moves or modifies anything; `--out` writes only the file
you name.

## Scan file

| Field | Meaning |
|---|---|
| `schema_version` | integer, currently 15 (3 added `enclosing_repos`, 4 the `plain_layout` fact, `layout_issue` and `scope`, 5 the inbound-reference checks, 6 `coverage` and `precious_objects`, 7 `hidden_index_entries` and `links_into_git_dir`, 8 byte-exact file names from git, 9 pseudorefs and the top-level layout rule, 10 the git directory's own files, 11 the `git_admin` survey behind the preserve obligation, 12 the object-store class and its verification, 13 `checked` and the files git never reads, 14 no object verification and outputs in every artifact, 15 small outputs anywhere in an artifact outside installed packages; an older scan lacks the checks that fill them, so it is refused) |
| `tool_version` | version of the tool that wrote the file |
| `generated_at` | when the scan finished |
| `roots` | absolute scan roots that were walked, as spelled (a kept inner root under the spelling of the root that encloses it); overlapping roots are normalised first (see "Overlapping scan roots") |
| `options` | `one_file_system`, `top_files`, `min_output_bytes`, `git_facts` |
| `totals` | see below |
| `nodes` | classified directories (below) |
| `outputs` | output files listed individually (below) |
| `big_files` | the biggest `top_files` files: `path`, `bytes`, `node` |
| `issues` | up to 1000 unreadable entries: `path`, `message`; `totals.unreadable` has the full count |
| `warnings` | notes about limits that applied to this run |
| `scope` | `volume` when every scan root is a mount point (its device differs from its parent's) or a file system root, otherwise `subtree`. A subtree scan cannot see clones that live elsewhere, so `no_dependents` means "no dependents were found within the scanned roots"; the plan header repeats the scope and, for `subtree`, every `backup-then-remove` reason says other clones outside the scanned folders were not checked (disclosure only: the apply phase re-verifies, and the plan is not downgraded to `ask` on scope alone) |
| `coverage` | where the walk could not look, so links, clones and alternates files there were not seen: `complete` (no entry was unreadable and no mount point was skipped), `unreadable_dirs` (directories whose contents could not be listed, each counted once) with up to 20 `unreadable_dir_examples`, `other_unreadable_entries` (entries whose metadata could not be read, and walk errors), and `mount_points_skipped` with up to 20 `mount_point_examples`. See "Incomplete coverage" below |
| `enclosing_repos` | scan roots that lie inside a git repository the scan did not cover: `root`, `repo`, `note`. Before the walk each root is looked at upward for a `.git` directory or file in any ancestor (the real path first, then the spelled one; symlinks are not followed). That repository is not a node and never gets a plan item, but the regenerable and cache directories below the root that no scanned repo encloses are decided by its git exactly as in a full scan: the config allowlist and work-tree checks run on it, and a directory is removable only if git lists it as wholly ignored and untracked. A refused config, a git failure or timeout, `--no-git`, or an ancestor `.git` that is not a plain directory or file makes them ask. The plan header and the human outputs say the repository was not assessed |

A `.git` entry that is neither a regular directory nor a regular file (a symlink, socket, fifo or
device) is ambiguous evidence of a repository. It is never followed and git is never run through it.
Inside a regenerable or cache directory it gives that directory an `ask_reason`; anywhere else the
directory becomes a `repo` node whose facts are all `unknown` (note: the `.git` entry was not
followed), so it and the artifacts under it ask and are not classified as plain loose code.

Every file in a scanned tree that the scan reads itself (a `.git` file, `alternates`, pseudorefs,
`info/` entries, ref files) is read only after `lstat` shows a regular file, and the open checks
again (the tool's own config file, from `--config` or its default location, and the scan file
`plan` reads are not in a scanned tree and are read as ordinary files): on unix it does
not follow a symlink (`O_NOFOLLOW`) or wait for a writer (`O_NONBLOCK`), and the open file must
still be a regular file, so a symlink or FIFO put there in between is refused, not followed or
waited on. Windows has no FIFOs in the file system (opening a named pipe never waits); there the
open file must be a regular file too, and a symlink put there in between is followed as before.

A git directory that another directory reaches through a `.git` symlink has dependents: removing it
would break that checkout's history. The link is read as a path only (`readlink`, then a
canonicalisation of the path itself; nothing is opened and no git runs through it). When the
target is, or lies inside, a git node of the scan, that node's `no_dependents` becomes `no` with a
note and an entry in `dependents` naming the checkout. When a `.git` entry is not a symlink, cannot
be resolved, or leads outside every git directory in the scan, which repository it depends on is
unknown: every git node that had `no_dependents = yes` gets `unknown` (a known `no` stays) and a
warning says why.

**The scan document is validated on load** (`plan` reads it through `read_scan`) before anything
plans from it: every `nodes[].parent`, `nodes[].repo.parent_node`, `big_files[].node` and
`outputs[].node` must be in range, a node cannot be its own parent, the parent links must form no
cycle (one iterative pass, whatever the order of `nodes`), and the sums of the byte and file counts
must not overflow 64 bits. Negative counts are rejected by the parser. A violation is a clean error
naming the field and index (`invalid scan file: nodes[3].parent: index 999999 is out of range`),
exit code 1, no partial plan. The planner itself has no recursion, so a deep valid tree cannot
overflow the stack; for a scan built by hand, bad parent links are ignored.

### `totals`

`files`, `dirs`, `symlinks`, `allocated_bytes`, `logical_bytes`, `hardlink_duplicates_skipped`,
`unreadable`, `mount_points_skipped`, `output_bytes`, and `bytes_by_class`, a map from class name to
the sum of `bytes_self` of nodes of that class, with `other` for space outside any classified
directory. Output files are not a separate slice of `bytes_by_class` (they stay in the class of
the directory that holds them); `output_bytes` counts them separately.

### How size is counted

- **Allocated size** is what the filesystem reserved: `st_blocks * 512` on Unix. On Windows it is the
  logical length (allocated and compressed size are not read in this phase).
- A file with more than one hard link is counted once per `(device, inode)`. On Windows hard links
  are not detected and are counted per name.
- Symlinks are never followed; a symlink counts as the small link object.
- `--one-file-system` skips directories on another device than their scan root. It is ignored on
  Windows and a warning says so. A skipped directory's contents are never seen, so the node that
  holds it (or the mount point itself, when a rule matches it) records it in `skipped_mounts` and is
  never offered for removal; enclosing nodes then ask through the plan's enclosing rule.
- Unreadable entries are counted and listed in `issues`; the scan continues.

### Overlapping scan roots

Every directory is walked once, however the roots overlap. Before the walk each root is made
absolute and compared with the others by its real path (`canonicalize`); the walk itself and every
reported path keep the spelling the user gave, except for a kept inner root (below).

- A root that is the same directory as an earlier root (`/projects` and `/projects/app/..`, or a
  symlinked spelling of it) is dropped, with a warning.
- A root inside another root (`/projects` and `/projects/app`) is dropped, with a warning, when the
  enclosing root's walk provably reaches it: every directory from the enclosing root down to it can
  be listed and, with `--one-file-system`, is on the enclosing root's device. The result is then the
  same as scanning the enclosing root alone: the same totals, nodes, outputs, biggest files and
  plan bytes.
- Otherwise the inner root is kept and walked on its own, and the enclosing walk stops at it (it
  counts that directory's own entry once and does not enter it). This is the `--one-file-system`
  case: a child root on another device than its ancestor is not covered by the ancestor's walk, so
  it is still walked, it is counted once, and it is not reported as a skipped mount point. It also
  covers a directory on the way that cannot be listed. A kept inner root belongs to the directories
  that physically enclose it, so it is walked and reported under the spelling of the closest root
  that encloses it plus its real path below that root, with a warning when this differs from what
  the user typed (`/scan` and `/alias/proj/node_modules/mnt`, where `/alias` links to `/scan`, walk
  `/scan/proj/node_modules/mnt`). A repository found there is then inside that `node_modules` for
  every rule: the artifact asks, a veto propagates to enclosing nodes, and dependency discovery
  sees it.
- A root whose real path cannot be established (`canonicalize` fails, for example a symlink whose
  target is missing) cannot be placed: when more than one root is walked, every artifact and
  repository in the scan gets an `ask_reason` saying so, since any of them might enclose it, and a
  warning names the root.

Dropping a covered inner root, rather than refusing the combination, was chosen because the
user's intent is unambiguous (the inner root is already part of the scan) and the warning says what
happened. A root spelled differently only by letter case on a case-insensitive file system may not
be recognised as the same directory.

### `nodes`

One entry per classified directory. A directory belongs to its innermost classified ancestor.

| Field | Meaning |
|---|---|
| `path`, `class` | `repo`, `worktree`, `bare-repo`, `regenerable`, `cache`, `loose-code` |
| `parent` | index of the enclosing classified node, if any |
| `rule` | the rule that matched (regenerable and cache) |
| `bytes_self` | bytes attributed to this node, excluding nested nodes |
| `bytes_total` | `bytes_self` plus all nested nodes |
| `files_total` | files in the subtree |
| `output_bytes` | allocated bytes of output-extension files in the subtree |
| `contains_outputs` | regenerable or cache directory that holds a listed output file (see "Outputs"); never offered for removal |
| `outputs_unlisted` | output files that make this directory ask but are not in `outputs` one by one: small ones the scan did not list after the first 16 of one directory (see "Outputs"; omitted when 0) |
| `newest_mtime` | newest modification time of files attributed to this node, excluding `.git` and nested nodes |
| `unreadable` | entries inside this node that could not be read (omitted when 0) |
| `skipped_mounts` | directories inside this node on another file system that `--one-file-system` did not enter (omitted when 0) |
| `ask_reason` | why this node is never offered for removal (a regenerable or cache node for the reasons below; any node whose path is not valid UTF-8) |
| `repo` | present for `repo`, `worktree` and `bare-repo` |

Classification rules (design section 3): a directory with a `.git` directory is a `repo`; with a
`.git` file a `worktree`; a directory with `HEAD`, `objects` and `refs` and no `.git` is a `bare-repo`.
A directory whose name matches a rule in `rules/artifacts.toml` is `regenerable` or `cache` when
the rule's marker file or directory exists in the directory beside it, that is, its parent (a marker
further up proves nothing; a rule without markers matches by name alone; ambiguous names such as
`build` always have markers). Regenerable and cache directories are opaque: nothing inside them is
classified, so a repo vendored inside `node_modules` is not reported as a node; instead the
directory gets an `ask_reason`. Opaque does not mean unchecked: while the scan is inside one, the
rules that carry `ask` or `ask_if_contains` are still evaluated for the directories nested in it,
and file names are compared with the `[[state_file]]` list (below); any hit gives the enclosing
artifact an `ask_reason`, and the plan's enclosing rule then makes every repo and directory around
it ask. A directory outside any repo that holds a project manifest or
source files is `loose-code` (a scan root counts only if it holds a manifest).

A regenerable or cache node carries an `ask_reason`, and is never offered for removal, when: a git
repository lives inside it; it is a build directory (`scan_outputs`) in loose code, where no git
history proves it holds no hand-written source; or, inside a repo, git does not show it as wholly
ignored and untracked (a tracked or partly tracked `build/` may hold source); or its rule says
it can hold content that exists nowhere else (`ask` in the rules file: the Maven local repository,
which also holds artifacts installed with `mvn install`, and `.terraform`, which records backend
configuration), or the directory holds an entry its rule says a generated directory never has
(`ask_if_contains`: a `.gradle` beside a build file that holds `gradle.properties` or `init.d` is the
Gradle user home, not a project cache), or a directory nested anywhere inside it matches such a rule
(`build/.terraform`, a `.m2/repository` inside `node_modules`), or a file inside it has a
unique-state name. `unreadable` counts entries inside a node that could not
be read and `skipped_mounts` the mount points inside it that were not entered; such a node is
incomplete and is never offered for removal.

**Outputs.** Output files are recognised by extension (video, audio, images, documents,
archives, datasets, model weights, VM disks, database files and dumps; the list is in
`rules/artifacts.toml`). Recognised outputs are protected in every artifact class unless there is
specific evidence that they are installed package contents. One is listed in `outputs` when:

- it is at least `min_output_bytes` (scan option `--min-output-bytes`, default 100 MiB), anywhere;
- it is inside a regenerable or cache directory, whatever its size, unless it sits in an installed
  package's directory: the same **evidence of belonging to an installed package** that the
  `fixture_prone` exemption uses (below: a `package.json` above it inside `node_modules`, a package
  directory directly below `site-packages`, a `Cargo.toml` or `*.nuspec` above it in those
  caches). A file directly in the artifact's top directory never has that evidence;
- it is inside a build directory (rule `scan_outputs = true`), whatever its size, even inside a
  package directory.

So `--min-output-bytes` narrows only two cases: outputs inside an installed package's directory,
and outputs outside every regenerable or cache directory. The size it is compared with is the
file's size as the rest of the scan counts it: on Unix the **allocated** bytes (`st_blocks` × 512),
so a sparse file counts only the blocks it has written and a file on a compressing file system
(APFS, Btrfs, ZFS) counts its compressed size, and either can stay below the threshold however
large its length; on Windows, where no allocated size is read cheaply, the **logical** length,
so a sparse or NTFS-compressed file counts at its full length. A 50 MiB `.venv/data/model.pt` or a
1 KiB `node_modules/.stuff/clip.mp4` is listed at the default threshold; a 1 KiB
`node_modules/pkg/test/fixture.zip` beside `node_modules/pkg/package.json` is not, because packages
routinely ship fixture archives, while a 200 MiB `node_modules/pkg/weights.bin` is. To bound
memory, the scan may list only the first 16 small outputs of one directory one by one (a
directory with no build-directory name in its path); any it does not list are counted in
the node's `outputs_unlisted`, so the plan's count is complete. Every listed output inside a regenerable or cache directory flags that
directory `contains_outputs`, whatever its rule, so it asks and its reason counts the files and
names up to three; the rule says what is regenerable, never that an output file is. A flagged
directory is also left out of its repository's regenerable set, so its ignored files count as
unique (`no_unique_ignored` `no`) and the plan's enclosing rule keeps the repository from being
removed with it.

**Known over-asking.** Output extensions are names, not proof, and some tools keep files with
those names in their own generated directories. Such a directory asks every time: the safe side,
at the cost of reclaiming less. Expected cases, from how these tools lay out their files (not
measured on every tool):

- Python environments with a `.pth` file straight in `site-packages` (`.pth` is also a model
  weight extension): `distutils-precedence.pth` from the setuptools that older `venv` versions
  install, `_virtualenv.pth` from `virtualenv` (and so `tox`, Poetry and Hatch environments) and
  `uv venv`, and the `.pth` file of an editable install (`pip install -e`). A plain `venv` from a
  Python that installs only pip has none and stays regenerable.
- Coverage's `htmlcov/` (`*.png`), `.parcel-cache` (`*.mdb`), `.cache/pre-commit` (`db.db`),
  Unity's `Library`, `.gradle` and `.gradle/caches` (`*.bin`), `.turbo` (cached `*.tar.zst`),
  `Library/Caches/Homebrew` (downloaded `*.tar.gz`), `.cache/uv` and `.tox` (`*.tar.gz`),
  browser caches such as `.cache/ms-playwright` and `.cache/puppeteer`, `Pods` (images in pod
  resources), and caches kept inside `node_modules` outside any package (`node_modules/.cache`).

**Unique-state file names** (`[[state_file]]` in `rules/artifacts.toml`, matched on the file name
only, ignoring case; no file is opened). Found anywhere under a regenerable or cache directory they
make it ask, whatever the rule: Terraform state (`*.tfstate`, `terraform.tfstate.*`) and variable
values (`*.tfvars`), `.env` and `.env.*` (not `.env.example`, `.sample`, `.template`, `.dist`),
`id_rsa`, `id_ed25519`, local databases (`*.sqlite`, `*.sqlite3`, `*.db`) and key material
(`*.pem`, `*.key`, `*.keystore`, `*.jks`, `*.p12`). The reason names the kind and the path inside
the directory, never contents. Trade-off: the databases and key material are `fixture_prone`:
installed packages ship them as test fixtures (a `test/fixtures/cert.pem`), and every JavaScript
project would otherwise ask. They are exempt only for a file that shows **evidence of belonging to
an installed package**, defined per rule in the rules file:

- `node_modules` (`fixture_files = ["package.json"]`): some directory between the file and the
  `node_modules` root, root excluded, holds a `package.json`. Scoped packages
  (`node_modules/@scope/pkg/package.json`) and nested `node_modules` qualify; a file in
  `node_modules/local/` with no `package.json` above it, or straight in the root, does not.
- Python environments (`.venv`, `venv`, `.tox`, `.cache/pre-commit`:
  `fixture_dirs = ["site-packages", "dist-packages"]`): the file sits in a package directory
  directly below a `site-packages` or `dist-packages` directory. A file in the environment root, or
  straight in `site-packages`, does not qualify.
- Cargo registry and git checkouts (a `Cargo.toml`), NuGet packages (a `*.nuspec`): same idea.
- Every other rule (`Pods`, `deps`, `.npm`, `.pnpm-store`, `.cache/pip`, `.cache/uv`,
  `.gradle/caches`, all build output directories) defines no evidence, so those names always ask.

**Known limit: files placed by hand inside an installed package.** A state-like file (a database,
key or certificate name marked `fixture_prone`) inside a directory that carries package evidence
is assumed to be package content. So is an output file below the output threshold there. A file
a person placed there by hand (`node_modules/pkg/prod.key`, `site-packages/pkg/local.db`, a
50 MiB `node_modules/pkg/model.pt` at the default threshold) looks exactly like a file the
package shipped and is not detected; the directory is offered for removal as if it were not there.
This scan reads names only and keeps no record of what each package installed; a later phase can
compare the files against the package records (`package-lock.json` integrity, a wheel's
`RECORD`) to tell them apart. Terraform state, `.env` and the SSH key names ask everywhere, with
or without evidence, and so does an output at or above the threshold.

### `repo`

| Field | Meaning |
|---|---|
| `facts` | the eleven safety facts, each `"yes"`, `"no"` or `"unknown"` |
| `notes` | why facts are not `yes` (git errors, ignored paths found); every note passes one redaction function (see below) |
| `remotes` | `name`, `url` (credentials stripped), `network` |
| `head_commit`, `last_commit_time` | HEAD id; newest commit on any ref |
| `worktrees` | live linked worktrees reported by git (may be outside the scan) |
| `parent_repo`, `parent_node` | for a linked worktree: its repo as git reports it, and its index in `nodes` if scanned |
| `precious_objects` | present and `true` when the repository sets `extensions.preciousObjects = true` (see "Declared dependents") |
| `dependents` | clones in the scan that borrow objects from this repo (alternates; the file is read only if it is a regular file under 64 KiB, otherwise a warning is recorded and `no_dependents` of the other git nodes becomes `unknown`, see below) |
| `links_into_git_dir` | symlinks in the scan, outside every git directory and checkout, that point into this repository's git directory (a convenience link such as `hooks -> A/.git/hooks`); they make `no_dependents` `no` and the plan names each as "a link at <path>", not as a checkout |
| `hidden_index_entries` | present when not 0: index entries marked skip-worktree or assume-unchanged whose file is present in the working tree. Git hides edits to them from `status`, `diff`, `add` and `stash`, so recovery cannot save them; `tree_clean` is `no` and the plan asks (see "Hidden index entries") |
| `submodule_gitdirs`, `git_dir` | submodule git directories under `.git/modules`; this node's own git directory |
| `unsaved_pseudorefs` | present when not empty: the exact names of the pseudorefs at the top of this worktree's git directory (`ORIG_HEAD` and the like) whose commit no network remote's tracking refs contain (`reflog_saved` is `no`). A name that would not be shown exactly (a control character, text the redaction rewrites) is never listed here: such a pseudoref is unchecked instead, so recovery is never told a name that is not the file's |
| `unchecked_pseudorefs` | present when not empty: one sentence per pseudoref that could not be read, parsed or compared, naming it and what is wrong (`ORIG_HEAD at the top of the git directory names a tag object, not a commit`); `reflog_saved` is `unknown` |
| `local_admin_content`, `local_admin_content_count` | present when not empty: up to five examples, sorted, of what the git directory holds of its own (`hooks/pre-commit`, `info/notes.txt`, `description`, `config: user.email`), and how many were found in all; `no_local_admin_content` is `no` (see "Local content in the git directory") |
| `git_admin` | present when the node's own git directory could be enumerated: `files` and `bytes` (allocated) of every file in it that is not of the object-store class (see "The preserve obligation"), `examples` (up to five, sorted, of those whose names are not ones every clone has) and `largest` (the three largest, `path` and `bytes`). Absent when there is no own git directory (a linked worktree) or the walk failed (a note says why); the plan then asks |
| `object_store` | present with `git_admin`: `files` and `bytes` (allocated) of the git directory's object-store class, all of which a removal backs up; there is no verification field, and a scan file whose repository block, `object_store` or `git_admin` carries one (`verified`, `checked`, any name containing `verif`) or any other unknown field is refused (see "The preserve obligation") |
| `sizes` | `total`, `git` (the `.git` directory), `regenerable`, `output` |

Facts (the safe value is always `yes`):

| Fact | `yes` means |
|---|---|
| `has_network_remote` | at least one remote that is a network location; a local path, `file://` URL or `.bundle` is not |
| `head_on_remote` | the worktree's own HEAD, including a detached HEAD, is contained in a remote-tracking ref of a network remote |
| `all_branches_on_remote` | every local ref (branches, notes, replace refs...; not the stash or HEAD) is contained in a network remote ref, and there are no local tags: tag names are not visible in tracking refs, so a repo with tags is `unknown` until apply runs `git ls-remote` |
| `stash_empty` | the stash is empty |
| `tree_clean` | no tracked changes and no untracked files in this repo's own tree (submodules are their own nodes), no present file marked skip-worktree or assume-unchanged (those hide local edits; counted in `hidden_index_entries`, and a sparse checkout's absent files are not counted), and every submodule gitlink agrees: the index matches HEAD, and each checked-out submodule's HEAD equals the commit the index records (`status --ignore-submodules=all` hides both). A recorded submodule with a missing or empty directory is fine; a directory with files and no `.git` at a gitlink path, an unmerged gitlink or a differing commit is `no`; a submodule whose config fails the allowlist, whose HEAD cannot be read, or more than 200 submodules is `unknown`. The commits are compared with `ls-files -s`, `ls-tree -r HEAD` and `rev-parse --verify HEAD` run in the submodule through the hardened runner; no status runs inside it |
| `no_unique_ignored` | no ignored files outside regenerable and cache directories (`.env`, local databases...) |
| `no_dependents` | no linked worktree and, within the scanned roots, no alternates clone, shared object directory or other reference into this repository's git directory was found (see "Inbound references"), and the repository does not declare `extensions.preciousObjects = true` (see "Declared dependents"). Places the scan could not read are not covered: see "Incomplete coverage" |
| `reflog_saved` | every commit reachable from any reflog entry (HEAD of this worktree and every ref's reflog), and every commit a pseudoref at the top of this worktree's git directory names (`ORIG_HEAD`, `MERGE_AUTOSTASH`, `FETCH_HEAD`, `AUTO_MERGE`, any file there that names a commit: see "Plain layout", check 4a), is contained in a network remote's tracking refs. An unpushed commit after `git reset --hard`, an amended or rebased-away commit and a stash's commits are `no`, also when reflogs are disabled or expired and only `ORIG_HEAD` still names the commit (`repo.unsaved_pseudorefs` lists such pseudorefs and the plan reason names them). No network remote is `no`; an unborn HEAD, any git failure or a timeout is `unknown`, and so is a pseudoref that cannot be read, does not parse, or names something that is not a commit (a tag object, whose annotation or signature may exist nowhere else even when its commit is pushed; a tree, as `AUTO_MERGE` does; or an object the repository does not have), or whose name cannot be shown exactly: `repo.unchecked_pseudorefs` lists them and the plan reason names them. `unknown` wins over `no` |
| `plain_layout` | the repository has the plain layout described under "Plain layout" below. `no` or `unknown` makes the plan `ask` with the reason `unusual repository layout: <first failing check>` (kept in `repo.layout_issue`); it is never `recover-backup-then-remove` or `backup-then-remove` |
| `no_local_admin_content` | the git directory holds nothing of its own outside the object store, refs and logs: only the stock `*.sample` hooks, a comment-only `info/exclude`, the stock or an empty `description`, no unsent tag, merge or squash message, no legacy `branches/` or `remotes/` file and no config value only this repository has (see "Local content in the git directory"). `no` makes the plan `ask`, naming what was found, with nothing reclaimable; `unknown` when a bounded read or listing failed or overflowed, or when the git directory is not the node's own `.git` directory (a linked worktree or submodule checkout, whose layout already asks) |
| `no_pending_state` | no unfinished rebase, merge, cherry-pick, revert or bisect in this worktree's git directory (`rebase-merge`, `rebase-apply`, `sequencer`, `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `BISECT_*`) and no local-only content under `.git/lfs/objects` or `.git/annex/objects`. Found by name only. Never `no`: a finding is `unknown` (state that no ref records cannot be recovered into a ref, so a person decides) |

`reflog_saved` is computed with `git rev-list -n 1 --reflog --not --remotes=<network remote>...`, which
only reads refs, reflogs and objects (see git-rev-list(1); it takes no lock and writes nothing) and
stops at the first commit no tracking ref contains; the command timeout bounds it. In a linked
worktree it sees that worktree's HEAD reflog and the shared repo's other reflogs; the main repo may
not see a linked worktree's HEAD reflog, but a repo with linked worktrees is already
`no_dependents = no` and never removed. A bare repo keeps no reflog unless `core.logAllRefUpdates` is set. A zero-commit
repo is never `yes`. Reflogs expire (90 days, 30 for unreachable entries), so the fact errs toward
`no` for recent history: a branch squash-merged remotely keeps its original commits in its reflog and
reads `no` until they expire. Other state git keeps outside refs: `refs/original` and
`refs/rewritten` are refs and are covered by `all_branches_on_remote`; a non-empty `.git/rr-cache`
only records conflict resolutions that can be derived again and is not work.

Each pseudoref's object id is read from the file itself (bounded, no git command). Its own object
type is read first with `git cat-file -t <id>` (no peeling): only a commit goes on to
`git rev-list -n 1 <id> --not --remotes=<network remote>...`. A tag object (whose annotation or
signature may exist nowhere else once its tag ref is deleted, even when the commit it points to is
pushed), a tag of a tag, a tree, a blob, a missing object or any git failure makes the fact
`unknown`, and the sentence in `repo.unchecked_pseudorefs` says which (`names a tag object, not a
commit`). A `FETCH_HEAD` written by `git fetch --tags` lists annotated tags, so it asks too. A
pseudoref whose name would be changed by display redaction is not checked at all: it is `unknown`
with the sentence `... has a name that cannot be shown exactly`, so `repo.unsaved_pseudorefs` only
ever holds exact names. A `git pull` or a reset to a
pushed commit leaves `ORIG_HEAD` and `FETCH_HEAD` at commits a tracking ref contains, so they do not
change the fact. Without a network remote nothing is subtracted (`--not` is left out): every
pseudoref commit is unsaved, so `reflog_saved` is `no` and `repo.unsaved_pseudorefs` tells recovery
which pseudorefs to capture, and one that cannot be checked still makes it `unknown`. A pseudoref
that names an object the repository no longer has (likely after `git gc` pruned a commit that
only `ORIG_HEAD` named; not verified here) cannot be checked, so the repository asks until the
pseudoref is removed or moved.

**Known limit: unreferenced objects.** Objects that no ref, reflog or pseudoref points at
(dangling commits awaiting garbage collection, including those `git fsck --lost-found` would list)
are not examined, because git itself treats them as discardable. A `lost-found` directory in the
git directory makes the layout not plain (check 4a).

Any git failure, timeout or missing binary makes the affected fact `unknown`, never `yes`. A bare
repo has no working tree, so its `tree_clean` and `no_unique_ignored` are `unknown`. Remote-tracking
refs can be stale; a later phase confirms them against the live remote with `git ls-remote` at apply
time. `no_dependents` cannot see clones outside the scanned roots. An alternates file that was not
read (a symlink, larger than 64 KiB, unreadable) leaves its provider unidentified, so every other
git node in the scan that had `no_dependents = yes` gets `unknown` instead (a known `no` stays).

**Plain layout.** Closing every unusual git layout by enumerating the bad ones kept missing one,
so the rule is positive: only a repository whose shape is known may be backed up then removed, or recovered,
backed up then removed. `plain_layout` is `yes` only if all of these hold, checked in this order by `lstat`,
directory listing, names, bounded reads and one `for-each-ref` through the hardened runner (nothing
is followed):

1. `.git` is a real directory. A `.git` file (a linked worktree, a submodule checkout, a
   `--separate-git-dir` repository) or a symlink is never plain. This is a deliberate Phase 1
   limit: those repositories, and the main repository of a worktree, always ask.
2. `HEAD` and `config` are regular files and `refs`, `objects`, `objects/info` and `objects/pack` are
   directories, all present and none a symlink. Beyond that, no entry anywhere inside the git
   directory, at any depth, is a symbolic link: a loose-object fanout directory, a pack or index
   file, a ref, `packed-refs`, a hook or an `info` file that is a symlink puts part of the
   repository somewhere else (`the git directory contains a symbolic link (objects/ab)`). This is
   found by the scan's own walk, which already sizes the git directory, not by a second traversal;
   when the checks above fail too, their finding is reported first. A symlinked hook therefore makes
   a repository ask.
3. None of `objects/info/alternates`, `objects/info/http-alternates`, `commondir`, `info/grafts`
   or `annex` exists.
4. `worktrees/` and `modules/` are absent or empty (so a repository with submodules, checked out
   or not, always asks). For a non-empty `worktrees/`, `layout_issue` adds up to three of the
   linked worktree paths git reports (`worktrees is not empty: /w/a-feature, ...`).
4a. Every entry directly inside the git directory is known or is a pseudoref (a positive model of
   the top level; nothing below it is read here). Known, with the kind each must be: the files
   `HEAD`, `config`, `config.worktree`, `description`, `index`, `packed-refs`, `shallow`,
   `COMMIT_EDITMSG`, `TAG_EDITMSG`, `MERGE_MSG`, `SQUASH_MSG`, `MERGE_MODE`, `MERGE_RR`,
   `gitk.cache`, `gc.log`, `gc.pid` and the split index's `sharedindex.*`; the directories `hooks`,
   `info`, `logs`, `objects`, `refs`, `branches`, `remotes`, `rr-cache`, `lfs`, `fsmonitor--daemon`;
   `fsmonitor--daemon.ipc` (a socket); the names checked elsewhere (`worktrees`, `modules`, and the
   unfinished-operation names of `no_pending_state`). Each holds settings, text, caches, refs or
   reflogs that other checks read, or the index, whose staged content `tree_clean` sees; none keeps
   a commit that no other check looks at. A pseudoref is a regular file whose name ends in `_HEAD`
   or is all capitals, or whose content (at most 64 KiB, opened only after `lstat` shows a regular
   file) parses as one: one line holding an object id or `ref: refs/...`, or for `FETCH_HEAD` one
   line per fetched ref starting with an object id and a tab. Pseudorefs are checked by
   `reflog_saved`. Any other entry (a file that is not a pseudoref, a directory such as `lost-found`
   or `reftable`, a lock file, a symlink, a special file) makes `plain_layout` `no`, naming up to
   five such entries (`the git directory holds an entry this tool does not know (stuff), ...`).
   `FETCH_HEAD` is checked, not trusted: the commits it names were fetched, but possibly from a
   remote this tool does not count as saved (a local path).
   What each known entry may hold and still be nothing unique is a separate rule, read by
   `no_local_admin_content` (see "Local content in the git directory").
5. No unfinished rebase, merge, cherry-pick, revert or bisect and no local-only LFS or annex
   content (`no_pending_state`).
6. The config passed the allowlist (a refused config makes every fact `unknown` before this).
7. Every ref is `refs/stash` or under `refs/heads/`, `refs/tags/` or `refs/remotes/` (so
   `refs/backup/*`, `refs/replace/*`, `refs/notes/*`, `refs/original/*`, `refs/bisect/*` or a
   fetched `refs/pull/*` make it not plain), and its target is a commit or an annotated tag whose
   target is a commit (`for-each-ref --format='%(refname) %(objecttype) %(type) %(*objecttype)'`:
   `%(type)` is a tag object's direct target type, so a tag whose target is another tag is not plain
   even though `%(*objecttype)` peels it all the way to a commit; a blob or a tree is not plain).

The structural checks (1 to 4) run first, by `lstat` alone. If an entry git itself reads (`HEAD`,
`config`, `refs`, `objects`, `objects/info`, `objects/pack`) is missing, a symlink or the wrong kind, or a
`commondir` exists, no git command is run at all in that repo: its facts are `unknown`,
`plain_layout` is `no` and the reason names the entry (`HEAD is a symlink`). A repo that is merely
unusual but healthy (a `.git` file, alternates, worktrees, modules) is still read with the hardened,
read-only commands and asks for its layout.

**Local content in the git directory.** Recovery, as defined so far, saves commits and
working-tree files; it does not save the git directory's own small files. So every allowlisted
entry at the top of the git directory (check 4a) also has a rule for what it may hold and still be
nothing unique (`Holds`, next to the allowlist in `crates/core/src/facts.rs`), read by `lstat`,
directory listings capped at 1000 entries and reads capped at 64 KiB per file, nothing followed:

| Entry | Nothing unique when | Otherwise |
|---|---|---|
| `hooks/` | every entry is a regular file whose name ends in `.sample` (the templates `git init` installs) | a custom hook, a hook manager's shim, a subdirectory or a symlink is local content (`hooks/pre-commit`) |
| `info/` | `exclude` holds only blank lines and lines starting with `#` (the stock template; a trailing CR is allowed); `refs` is absent or holds only `<object id>`, tab, `refs/...` lines (the listing `git update-server-info` writes, which `git gc` and `git repack` run by default, so an empty-only rule would make every garbage-collected repository ask); `attributes` is absent or empty | any other file (`info/notes.txt`, `info/sparse-checkout`), a real exclude pattern or other text in `info/refs` is local content |
| `description` | its text, trimmed, is empty or exactly `Unnamed repository; edit this file 'description' to name the repository.` | an edited description is local content |
| `TAG_EDITMSG`, `MERGE_MSG`, `SQUASH_MSG` | absent or empty | a message written and not used by a tag or commit: small, but it exists nowhere else |
| `branches/`, `remotes/` | empty | any file: a legacy remote shorthand that no clone recreates |
| `COMMIT_EDITMSG` | always | the last commit's message, which that commit holds; git overwrites it at every commit, so counting it would make every repository ask. A draft left by a commit that a hook or an empty message aborted is not lost: the file is in `preserve.git_admin` and is backed up before any removal |
| `HEAD`, `index`, `packed-refs`, `shallow` | always | covered by the ref, reflog, `tree_clean` and hidden-index checks; `shallow` lists boundary commits that came from the remote |
| `refs/`, `logs/`, `objects/`, `rr-cache/`, `lfs/`, `fsmonitor--daemon/` | what git writes inside them (see "Inside the git directory's own subdirectories" below) | anything else inside them is local content |
| `MERGE_MODE`, `MERGE_RR` | always | merge-mode text and rerere's state for a conflict in progress, which `no_pending_state` sees through `MERGE_HEAD`; recorded resolutions in `rr-cache/` (including one recorded before a merge was aborted, which no merge commit holds) are in `preserve.git_admin` and backed up before any removal |
| `gitk.cache`, `gc.log`, `gc.pid`, `fsmonitor--daemon.ipc` | always | caches, logs, a lock-holder's pid, the file-system monitor's socket |
| `worktrees/`, `modules/` | always | not plain when not empty (check 4) |
| `config`, `config.worktree` | every key's family is one the next table marks not local | see below |

Config keys first pass the allowlist (anything that could run a program is refused before this).
Each allowed family is then decided once:

| Family | Local content? | Why |
|---|---|---|
| `core.*` | no | format and platform values that `init` and `clone` write again, and behaviour preferences, not work |
| `remote.*` | no | the URL is recorded in `repo.remotes`; refspecs are set by clone |
| `branch.<b>.description` | yes | text a person wrote, kept nowhere else |
| other `branch.<b>.*` | no | tracking is set again by clone and `push -u` |
| `user.*` | yes | a per-repository identity; without it later commits would carry another identity |
| `url.<base>.*` | yes | URL rewrites may be what makes the remote reachable |
| `submodule.*` | no | derived from `.gitmodules` again (and `modules/` is never plain) |
| `extensions.*` | no | format extensions written by `init` and `clone` (`preciousObjects` is read as a dependency) |
| `init`, `gc`, `pack`, `receive`, `transfer`, `fetch`, `pull`, `push`, `color`, `advice`, `gui`, `commit.gpgsign`, `tag.gpgsign` | no | preferences |
| `include.*`, `includeIf.*` | no | an included file lives where it is named; one inside the git directory is an unknown top-level entry, so the layout is not plain |
| any family without a row | yes | it asks rather than loses |

Anything found makes `no_local_admin_content` `no`, and the plan asks ("this repository has files
of its own in its git directory that exist nowhere else: hooks/pre-commit, info/notes.txt; copy
them out or decide they can go") with nothing reclaimable. This ask predates the preserve
obligation (see "The preserve obligation"), which now backs up every one of these files before any
removal; it is kept unchanged since schema 11 so that no repository moves toward removal in the
change that introduced the backup. The stock templates were checked on git 2.56 on Linux, macOS and Windows: all
three install the same fourteen `*.sample` hooks, the same comment-only `info/exclude` and the same
`description`.

Linked worktrees: their private git directory (`worktrees/<name>/` in the main repository) has no
hooks of its own but can hold `info/sparse-checkout`, `logs/HEAD`, pseudorefs and message files.
It is not inventoried: a linked worktree's `.git` is a file, so its layout is not plain and it asks
(`no_local_admin_content` is `unknown` there), and the main repository asks because `worktrees/`
is not empty. Submodule git directories under `modules/` likewise make the layout not plain.

**Inside the git directory's own subdirectories.** The scan's walk already visits every entry
below a `.git` directory (it sizes it and looks for symlinks); it also judges each one against
the shape git itself writes there (`crates/core/src/gitshape.rs`), by its path below the git
directory and its kind (`lstat`, never followed). Only a regular file under `refs/` is read, at
most 512 bytes of it.

| Below the git directory | What git writes, and so nothing unique |
|---|---|
| `objects/` | two-hex-digit fanout directories, `pack/` and `info/`; nothing else |
| `objects/<xx>/` | regular files named by the rest of an object id (38 or 62 lowercase hex digits) |
| `objects/pack/` | regular files `pack-<id>.{pack,idx,rev,bitmap,keep,mtimes,promisor}`, `multi-pack-index`, `multi-pack-index-<id>.{bitmap,rev}`, and the directory `multi-pack-index.d/` holding `multi-pack-index-chain` and `multi-pack-index-<id>.{midx,bitmap,rev}` |
| `objects/info/` | regular files `packs`, `commit-graph` (and `alternates`, `http-alternates`, which the layout check refuses first), and the directory `commit-graphs/` holding `commit-graph-chain` and `graph-<id>.graph` |
| `refs/` | directories at any depth; regular files of at most 512 bytes holding one object id or `ref: refs/...` line |
| `logs/` | `HEAD` and anything under `logs/refs/` (files and directories; the reflog check reads them) |
| `rr-cache/` | directories named by an object id holding `preimage*`, `postimage*` and `thisimage*` files |
| `lfs/` | `objects/` (local-only LFS content, which `no_pending_state` already reports) and an empty `tmp/` |
| `fsmonitor--daemon/` | `cookies/` holding regular files |

Anything else there, of any kind, is the repository's own content (a note in `rr-cache/` or
`logs/`, a README in a fanout directory, a backup in `objects/pack/`, a file in `refs/heads/` that
is not a ref, a symlink): it makes `no_local_admin_content` `no` and is named with the other
findings (`objects/pack/backup.tar`), so the plan asks. A ref file that cannot be read makes the
fact `unknown`. git's own leftovers of an interrupted operation (`tmp_pack_*`, `tmp_obj_*`,
`.tmp-*-pack-*`, a stale `*.lock`) are not in the shape either: they mean an operation was cut
short, and a person looks before anything is removed. The same holds for an operation still
running: after commands such as commit, merge or fetch git starts its automatic maintenance in
the background by default, and it holds `objects/maintenance.lock` until it ends. A repository
scanned in that moment asks; a later scan judges it without the lock, so two scans can differ
only by the earlier one asking. The fact is used rather than
`plain_layout` because git itself does not depend on these entries (the layout is still one git
reads correctly); what is at stake is content of the repository's own that recovery does not
save. `annex/` (check 3), `reftable/`, `lost-found/` and any other unknown top-level entry
(check 4a) already make the layout not plain, and `worktrees/` and `modules/` do when not empty.
The shapes were checked on git 2.56 on Linux, macOS and Windows against a fresh clone, `git gc`,
`git repack -a -d --write-bitmap-index`, `git commit-graph write --reachable` (single and
`--split`), `git multi-pack-index write` (plain and `--bitmap`), a recorded rerere resolution and
`git pack-refs --all`: each stays `backup-then-remove`. Whatever the shape check finds, every file in
the git directory is also under one of the two backup obligations in "The preserve obligation", so
a file the shape rules miss is never removed unsaved.

A `shallow` file does not make a repository unplain (a shallow clone is complete for what it
holds); the shape check ignores it. A failure is `no`; a check that cannot be made is `unknown`;
both ask.

Object stores reached from repos that are not plain: a `.git`-file repo's effective git directory
is read (`gitdir:` line, that directory's `commondir`, then `objects`, all canonicalised) and a
symlinked `objects` or `commondir` is resolved as a path. If the resulting store is not a scanned
repo's own object directory it is inspected once for `info/alternates` (providers inside the scan
get `no_dependents = no`), and if it lies outside the scanned roots, or cannot be read, the repo is
an unresolved borrower: every plain repo's `no_dependents = yes` becomes `unknown`.

**Object sharing.** Every user of each canonical object directory is kept, so repos that share
one (a symlinked `.git/objects`, or a `commondir` file in a repository's `.git` or in a bare
repository pointing into another repo) all get
`no_dependents = no`, each naming the others in `dependents` and `notes`: an unpushed commit in one
lives in the other's directory. `objects/info` and `objects/pack` must be plain directories; a
symlink there is read as a path (never followed to run git) and is a dependency if it lands inside
a scanned repo's object directory. A symlinked `objects`, `objects/info` or `objects/pack` or a
`commondir` that leaves the scan, or a non-plain entry, is unresolved like an unread alternates
file (every other git node's `yes` becomes `unknown`). A linked worktree makes the repo git
reports as its parent a dependency (`no`), so a forged `commondir` cannot hide it. No allowlisted
config key can relocate objects: only `objects/info/alternates`, a symlink, or `commondir` can, and
the environment variables that could are cleared.

**Alternates syntax.** The file is parsed as git does: blank lines and lines starting with `#` are
skipped; an entry starting with `"` is C-style quoted (`\\`, `\"`, `\a \b \f \n \r \t \v`, `\ooo`
octal with a first digit of 0 to 3; anything else, a missing closing quote or text after it is
malformed); other entries are paths as written; a relative entry is resolved against the borrower's
`objects` directory. Each entry is canonicalised and must equal the object directory of a scanned
git node; an entry that is malformed, cannot be canonicalised, or names a place outside the scan
is never dropped: it is unresolved (see above) and a warning says why. Chains are followed
through scanned repos only: if A borrows from B and B from C, C lists both A and B as dependents.

**Inbound references.** Repositories that depend on a scanned repository are not always nodes
themselves: a clone inside `node_modules` or another regenerable directory is never classified, and
a repository can borrow objects through a symlink anywhere in its object store. Enumerating such
layouts kept missing one, so dependencies are also found from the references themselves, whatever
holds them. While walking every directory under the scan roots (inside regenerable and cache
directories and inside `.git` directories too) the scan records:

- every symbolic link: `readlink`, then the target resolved as a path (`canonicalize`; for a
  dangling link, the real path of its deepest existing ancestor plus the rest). Nothing is opened or
  traversed through the link and no program is run. Links whose target lies outside every scan root
  cannot point into a scanned repository and are not kept; at most 200,000 are kept;
- every `objects/info/alternates` and `objects/info/http-alternates` entry, wherever it is
  (inside `node_modules`, inside `.git/modules`, in a repository that is not a node); at most
  20,000;
- every entry named `commondir`, wherever it is; after the walk one counts when the directory
  holding it is named `.git` or also holds an entry named `HEAD` that is not a directory (a file
  or a symbolic link, which git also accepts), that is, when it belongs to a git directory of any
  kind: a repository's `.git`, a bare repository, a linked worktree's or submodule's git
  directory inside another one, a directory with only `HEAD` and `commondir` (which git accepts
  and the scan classifies as nothing), any of them inside an artifact or in a separately walked
  root; at most 20,000;
- every `.git` file that is not itself a scanned checkout (inside a regenerable or cache
  directory, inside a git directory). A scanned checkout's `.git` file is resolved with its
  repository for the object store (see "Object sharing"), and the git directory it names and the
  store it leads to are checked here too: either one inside another repository's git directory
  (`git init --separate-git-dir A/.git/sub`, or a `commondir` into `A/.git/sub`) makes that
  repository `no`, unless the checkout lies inside that repository's own folder (a submodule,
  which goes with it).

Each kind of reference above is checked for every git directory the walk recognises, whatever
the borrower was classified as. A git directory is recognised only by the entries named above,
and one the walk does not reach (outside every scan root, or below a directory it cannot read)
is not found by this pass.

After the walk:

- A reference whose target lies inside the git directory of a scanned repository N (its `.git`
  directory, or a bare repository), made from outside that git directory, gives N
  `no_dependents = no`, with a note naming the path that refers in and an entry in `dependents`:
  the repository whose git directory holds the reference, else the checkout whose `.git` holds it,
  else the link itself. This covers a symlinked `objects`, a symlinked fanout directory or pack file,
  a hidden checkout's `.git` symlink, and a convenience link such as `hooks -> A/.git/hooks`. The
  last is deliberately conservative: a link into a git directory may be how another checkout
  reaches its objects, and the scan cannot tell the difference, so the repository asks. A link
  that names only itself (outside every git directory and checkout) is recorded in
  `links_into_git_dir` instead of `dependents`, and the reason says "a link at <path> points into
  this repository's git directory".
- A reference that is git data (an alternates entry, a `.git` file's `gitdir:`, the `commondir` and
  object store it leads to, or a symlink inside a git directory or object store) whose target lies
  in another node is a dependency on that node: a repository there gets `no_dependents = no`, a
  regenerable or cache directory there gets an `ask_reason`. A symlink outside every git directory
  that points into a repository's working tree, or into an artifact directory (`node_modules/x ->
  ../../other-project`), is not a dependency and blocks nothing.
- The alternates files are parsed with the syntax below; an entry that is malformed, cannot be
  resolved or lies outside the scan, an unreadable alternates file, a `.git` file or `commondir`
  that cannot be read (not a plain file, no permission) or resolved or leads outside the scan, a
  symlink that cannot be read, and more links, alternates or `commondir` files than the bounds are
  unresolved: every git node that had
  `no_dependents = yes` gets `unknown` and a warning says why. (`http-alternates` holds URLs, which
  never resolve to a scanned directory, so any entry there is unresolved.)
- A regenerable or cache directory that holds such a repository still asks, as before.

**Known limits of dependency discovery.**

- Hard links cannot be detected as references. Two object stores that share pack or object files
  through hard links (`git clone` of a local path without `--no-hardlinks` does this) look like two
  independent repositories. Their objects survive the removal of either (a hard-linked file stays
  until its last name is removed), but nothing records that one was created from the other.
- References from outside the scanned roots are invisible: a clone, symlink or alternates file that
  lives in a folder the scan did not cover is not seen. A subtree scan says so in its `scope`.
- References inside a directory the scan could not read, or a mount point `--one-file-system` did
  not enter, are not seen either. The scan records those places in `coverage`, and by default
  they make every repository ask (see "Incomplete coverage"); with `accept_incomplete_coverage`
  the plan offers them for removal after a backup anyway and discloses them on every decision they could affect.
- More than 200,000 kept symlinks, 20,000 alternates files or 20,000 `commondir` entries makes every repository's
  `no_dependents` `unknown` (safe, but no repository is removed); a large pnpm or git-annex tree can reach
  it.
- Resolving a symlink's target as a path touches each directory on the way (as `realpath` does), so
  a link to an automounted or stale network path can slow the scan; nothing is read through it.

**Declared dependents.** A repository whose config sets `extensions.preciousObjects = true` declares
that other repositories borrow its objects (git then never deletes any of them). The key is on the
config allowlist, so the other facts are still computed; its value is read with
`git config --type=bool --get extensions.preciousObjects`. `true` makes `no_dependents` `no`, sets
`repo.precious_objects` and adds a note, and the plan asks with a reason naming the setting; a
value git cannot read makes `no_dependents` `unknown`; `false` changes nothing. A linked worktree
shares the setting of its repository and is not affected.

**Incomplete coverage.** Links, clones and alternates files inside a directory the scan could not
read, or inside a mount point `--one-file-system` did not enter, are never seen, so a repository
they depend on can look free of dependents. So by default incomplete coverage blocks removing repositories:
a dependent the scan could not see may be the only thing that needs the repository, and saying
so does not make its removal safe; a person who knows what those places hold can accept the gap
explicitly. What the plan does:

- the scan's `coverage` block counts the unreadable directories, other unreadable entries and
  skipped mount points (tracking overflows already make every repository's dependents unknown,
  see the known limits), with up to 20 example paths of each kind of directory;
- the plan records `coverage_complete`, and when it is `false` a one-sentence `coverage_note`
  that the human header prints (`Coverage is incomplete: 3 folders could not be read and 1 mount
  point was not entered (--one-file-system), so links or clones inside them were not checked.`);
- by default (strict coverage) every repository, worktree and bare repository whose
  `no_dependents` was `yes` gets `unknown` in the plan (`items[].facts` shows the value used), so
  none is removed or recovered: it asks, and the reason names what was not read and the opt-in
  (`the scan was incomplete (3 folders could not be read), so links or clones there may depend on
  this repository; make them readable and scan again, or accept incomplete coverage with
  --accept-incomplete-coverage [no_dependents]`). The human header says that no repository is
  offered for removal because coverage is incomplete.

**Accepting incomplete coverage.** On a real home folder some directories are almost always
unreadable (privacy-protected folders, other users' files). The config setting
`accept_incomplete_coverage = true` or `plan --accept-incomplete-coverage` (the same kind of
opt-in as `--trust-partial-process-check`) restores removal after a backup: `no_dependents` keeps the scan's
value, and every `backup-then-remove` and `recover-backup-then-remove` reason ends with what was not read, for
example `; 3 folders in this scan could not be read; dependents inside them were not checked`
(after the subtree-scope sentence when that applies). The plan records
`accepted_incomplete_coverage: true` and the human header says the user accepted incomplete
coverage. With complete coverage the setting changes nothing. The setting
`require_full_coverage` and the flag `--require-full-coverage` of earlier versions are refused
with an error naming the new ones.

Either way, an unreadable entry or skipped mount point inside a node makes that node (and the
nodes around it) ask, as described under `unreadable` and `skipped_mounts`, and its reason says
so. `remove-regenerable` and `totals.ready_bytes` are not affected: regenerable data has no
dependents.

**File names git prints.** Git prints file names as bytes. Every listing whose names are used is
requested NUL-separated (`ls-files -z`, `ls-tree -z`, `status --porcelain -z`,
`worktree list --porcelain -z`, falling back to the line format only on a git without it, where
a quoted or unexpected line makes the fact `unknown`), and every path git prints for the scanner
to look at (`rev-parse --show-toplevel`, `--absolute-git-dir`, `--git-common-dir`,
`core.worktree`, the hidden index entries, submodule paths, ignored entries, worktree paths) is
used byte for byte on Unix. On Windows, where git prints UTF-8, a name that does not decode is a
failed check (`unknown`). Other git output (refs, remotes, config keys) must be valid UTF-8 or
the check fails. The repository files the scanner reads itself (`.git` files, `commondir`,
alternates) must be valid UTF-8 or they are unreadable, which makes dependents `unknown`. A
lossily decoded name is only ever shown, never looked up or compared. A node whose own path is
not valid UTF-8 cannot be written exactly to the scan file, so it gets the `ask_reason`
`has a path that is not valid UTF-8, ...` and is never offered for removal.

Remote URLs have embedded credentials removed (`https://user:token@host/x` is stored as
`https://host/x`; everything up to the last `@` counts as userinfo, and query strings and fragments are
dropped) before they are stored, and all git error text is redacted the same way.

Everything stored from text a repository controls goes through one function (`redact::diag`):
notes, remote names, ask reasons built from file names, git error output, warnings. It strips URL
credentials and turns control characters into spaces. A rejected config key is never printed in
full: a key's subsection is free text and can be a credential URL
(`[credential "https://user:TOKEN@host"]`), so only the section and variable name are shown
(`credential.<redacted>.helper`). Paths of scanned directories are not redacted; they are the
data being reported.

**The repo's own config is untrusted.** A scanned folder may be an unpacked download, and its
`.git/config` can name programs that git would run (`filter.*.clean`, `gpg.program`, `core.fsmonitor`,
`core.sshCommand`, ...). Before any other git command the tool lists the config keys
(`git config --local --includes --name-only --list`, plus `--worktree` when
`extensions.worktreeConfig` is set and the git directory's `config.worktree` exists; a missing one
has no keys if git's own scoped listing agrees, and one that is not a regular file is refused). The
git directory is found by `lstat`, not by git: a `.git` directory, the directory a `.git` file
names, or the bare repo itself; a `.git` that is neither a directory nor a regular file (a symlink,
as in a submodule checkout the superproject compares) is refused, which makes the superproject's
`tree_clean` `unknown` (current git refuses a FIFO or socket `.git` itself, in the first listing,
with the same result). The scoped listing is
`git config --list --show-scope --includes --name-only`, which needs git 2.26 or newer; when it
fails the repo is refused and the note says so in those words, followed by git's error. If
any key is outside a small allowlist of plain-data keys (`core.*` format and line-ending settings,
`remote.<n>.url|pushurl|fetch|push|tagopt|prune|promisor|partialclonefilter`,
`branch.<n>.remote|merge|rebase|description|pushremote`, `user.name|email|signingkey`,
`submodule.*.url|path|active|branch|ignore`, `url.*.insteadOf`, `extensions.*`, `commit.gpgsign`,
`tag.gpgsign`, and simple `init`, `gc`, `pack`, `receive`, `transfer`, `fetch`, `pull`, `push`,
`color`, `advice`, `gui`, `include` settings) then every fact is `unknown`, a note names the
section and variable of the key (not its subsection), and no further git command is run in that repo. The same check is applied to every
`<gitdir>/modules/**/config` (bounded walk; a symlink, unreadable file or too many entries also
counts as unsafe), because git runs a child status inside submodules using their own config; the
outer status uses `--ignore-submodules=all` since each checked-out submodule is its own node. A
`core.worktree` whose effective value (the shared config, then `config.worktree` when
`extensions.worktreeConfig` is set, the later winning) does not resolve to the node's own directory
is refused, and independently `git rev-parse --show-toplevel` must equal the node's real path (for
a bare repo, `git rev-parse --absolute-git-dir` must); any mismatch or failure makes every fact `unknown`. Repos using Git LFS
(`filter.lfs.*`, `lfs.*`) or custom merge or diff drivers are therefore reported `unknown`: no `lfs`
key is allowed. Git runs with `core.fsmonitor=false`,
`log.showSignature=false`, `core.ignoreStat=false`, `core.hooksPath` set to the null device,
`protocol.allow=never`, `GIT_NO_LAZY_FETCH=1`, in its own process group that is killed on timeout.

**Configuration from outside the repo is not read either.** Git also reads the system config, the
user's global config (`~/.gitconfig`, `$XDG_CONFIG_HOME/git/config`) and configuration passed in the
environment (`GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n`, `GIT_CONFIG_PARAMETERS`).
None of it is checked, and a globally configured filter driver (Git LFS installs one) selected by
a scanned repo's `.gitattributes` would run during `git status`. So every git command runs with
`GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_SYSTEM`, `GIT_CONFIG_GLOBAL`, `HOME` and `XDG_CONFIG_HOME` set
to the null device (older git that ignores `GIT_CONFIG_GLOBAL` still finds no file under `HOME`),
and with every `GIT_*` variable of the caller removed (`GIT_DIR`, `GIT_WORK_TREE`,
`GIT_INDEX_FILE`, `GIT_CONFIG_*`, `GIT_EXTERNAL_DIFF`, `GIT_PAGER`, `GIT_ASKPASS`, `GIT_SSH*`,
`GIT_EXEC_PATH`, `GIT_TRACE*`, ...), plus `PAGER` and `SSH_ASKPASS`. What that changes:

- A `.gitattributes` `filter`, `diff` or `merge` attribute whose driver is defined only
  globally has no driver, so git compares the raw content. An LFS-tracked file that was touched
  therefore shows as modified rather than clean (`tree_clean = no`, not `unknown`).
- Global ignore rules (`core.excludesFile`, the default `$XDG_CONFIG_HOME/git/ignore`) no longer
  apply. A file only they ignored is now untracked instead of ignored: `tree_clean` becomes `no`
  while `no_unique_ignored` may become `yes`, so the repo still fails a fact and gets
  `recover-backup-then-remove`, never `backup-then-remove`. Recovery (a later phase) must treat such a file as
  possibly secret: it was ignored on this machine even though the repo does not ignore it.
- A global or system `core.autocrlf` no longer applies, which can only make files look modified
  (`tree_clean` toward `no`). It has a cost on Windows, where Git for Windows sets
  `core.autocrlf=true` system-wide:
  in a CRLF checkout, a file whose stat data is stale (touched, copied) reads as modified, so an
  otherwise saved repo gets `recover-backup-then-remove` (whose recovery step re-checks every fact)
  instead of `backup-then-remove`.
- `safe.directory` is honoured only from global and system config, so a repository owned by
  another user is refused by git and every fact of it is `unknown`, never clean.
- A global `url.<base>.insteadOf` rewrite is not applied, so a remote is classified by the URL
  written in the repo's own config. This can move remote facts toward `yes` with nothing else
  failing: a remote whose written URL is a network address but which a global rewrite sends to a
  local mirror now counts as a network remote. Remote facts are labelled unverified until apply
  re-checks them against the live remote, which must resolve the effective URL.

Remote facts come from local remote-tracking refs, which can be stale (a remote changed with
`set-url` keeps its old refs). The plan therefore labels them unverified; apply (a later phase) must
re-check them with `git ls-remote`.

A repo's submodule git data under `.git/modules` (with its stash and local branches) is deleted
with the repo: `submodule_gitdirs` lists those directories (real paths) and `git_dir` is each git
node's own git directory. The plan asks unless every listed directory is the `git_dir` of a scanned git
node inside the repo (matched by identity, not by count); the reason names the orphaned directory.

## Plan file

`schema_version`, `dry_run` (always `true`), `generated_at`, `scan_generated_at`,
`active_window_days`, `window_source`, `process_check` (`ok`, `partial` or `unknown`),
`process_check_note`, `trusted_partial_process_check`, `remote_facts_verified` (always `false` in this phase) and
`remote_facts_note`, `enclosing_repos`, `scope`, `coverage_complete`, `coverage_note` (only when
coverage is incomplete), `accepted_incomplete_coverage` (see "Incomplete coverage" and
"Accepting incomplete coverage"), `active`, `totals` and `items`.

Each item: `node` (index into the scan's `nodes`), `path`, `class`, `action`, `state` (`active`,
`idle` or `unknown`), `state_reason` (why it was judged so), `reason` (plain language),
`reclaimable_bytes`, `bytes_total`, `days_since_activity` (if there is any evidence), `facts`
and, on every item whose action removes a git node, `preserve` (see "The preserve obligation").

### The active window

One setting decides activity: `active_window_days`, default **7**. It is read from
`<platform config dir>/disk-cleaner/config.toml` (`$XDG_CONFIG_HOME` or `~/.config` on Linux,
`~/Library/Application Support` on macOS, `%APPDATA%` on Windows), or from the file named by
`--config`, and overridden per run by `plan --active-window-days N`. The file is TOML:

```toml
active_window_days = 7
# trust_partial_process_check = true   # optional, see below; default false
# accept_incomplete_coverage = true    # optional, see "Accepting incomplete coverage"; default false
# max_git_admin_bytes = 67108864       # optional, see "The preserve obligation"; default 64 MiB
```

The key `verify_objects` of schemas 12 and 13 is refused with a message: there is no object
verification (see "The preserve obligation").

A missing default file means the default. A file that exists but is malformed, has an unknown key
or a non-integer or negative value is an error; there is no fallback to a shorter window. A
`--config` file must exist. The config is validated even when the flag overrides it.

A project is **active** when any of these holds:

1. an `--active PATH` covers it, or an active path lies inside it (or inside the project that owns a
   regenerable directory). `--active` paths must exist and must lie inside or around something the
   scan covered, otherwise `plan` fails; both sides are compared as real (symlink-free) paths;
2. its newest activity, the later of `newest_mtime` (non-regenerable, non-`.git` files) and
   `last_commit_time`, is at most `active_window_days` old (exactly at the edge counts as active);
3. a running process has its working directory inside it.

Process information comes from `sysinfo`. `process_check` is `ok` when every process's working
directory was readable, `partial` when some (usually other users') were not, and `unknown` when none
could be read. When `unknown`, a removing action is never proposed: it becomes `ask` and says so.
When `partial`, the same holds: an unreadable process may be working inside the candidate, so every
removing action becomes `ask` (reason: some running processes could not be inspected) and
`totals.ready_bytes` is 0. The user can accept the risk with the setting
`trust_partial_process_check = true` in the config file or `plan --trust-partial-process-check`;
the plan then records `trusted_partial_process_check: true` and the human header says the user
accepted it. A readable process inside a candidate still keeps it. With no evidence at all the
state is `unknown`.

`action` is one of `keep`, `remove-regenerable`, `recover-backup-then-remove`,
`backup-then-remove` and `ask` (schema 10 called the two git actions `recover-then-archive` and
`archive`). The names carry the contract: no action removes a git node without first backing up
its git directory's own files (see "The preserve obligation"). The plan has no age of its own beyond the window and the
`--active` paths, which are the user's. Rules, in order:

1. Active: `keep`.
2. Regenerable and cache: contains a listed output file (the reason names up to three), has an `ask_reason`, is incomplete (`unreadable`
   or `skipped_mounts`), or state unknown: `ask`; otherwise `remove-regenerable`.
3. Repo and worktree: state unknown: `ask`; `plain_layout` not `yes`: `ask` (unusual repository layout); `no_dependents` `no`: `ask`, never a removing action, naming up to three dependents and links and the count of the rest (recovery saves this repo's own refs and cannot make the repos that use its objects safe); `no_local_admin_content` `no`: `ask` with nothing reclaimable, naming up to five of the git directory's own files and the count of the rest (recovery does not save them); `hidden_index_entries` above 0: `ask` ("N files are marked skip-worktree or assume-unchanged, so git hides edits to them and they cannot be saved automatically; review them first"); any fact `unknown`: `ask`, with each unknown fact explained in words (what could not be checked and what would settle it; for local tags, that tag names cannot be checked against the remote without contacting it and will be verified when a removal is applied); any other fact `no`:
   `recover-backup-then-remove`, naming in words what is not saved elsewhere; every fact `yes`: `backup-then-remove` (labelled unverified, see
   `remote_facts_verified`; its reason says "no dependents were found", never that nothing depends on it). Both then carry
   `preserve` and say what is backed up ("its N local git files (X KiB: settings, hooks, logs and
   the like) are saved with the manifest first; its object store (M files, Y GiB) holds object data that must be backed up
   too before removal, then it is removed"); a git node whose files were not counted, or whose own files hold more than
   `max_git_admin_bytes`, asks instead. A repo whose `.git/modules` hold a submodule git directory that no scanned
   git node inside it uses, or that is incomplete (`unreadable` or `skipped_mounts`), asks.
   When the scan's coverage is incomplete and the user did not accept that, `no_dependents`
   `yes` is read as `unknown` (see "Incomplete coverage"), so no repo is removed or recovered;
   its reason names what was not read unless the node's own incompleteness above is the reason.
   Any node, of any class, with an `ask_reason` (such as a path that is not valid UTF-8) is never
   given a removing action.
4. Bare repo: `keep` when something depends on it, otherwise `ask`.
5. Loose code: `ask`.
6. Enclosing: a node that would be removed becomes `ask` when any node inside it is `keep`, `ask` or
   `recover-backup-then-remove` (a nested video, a submodule with a stash, a recently active nested
   project), because everything inside goes with it. A removable regenerable child does not block.
   The reason leads with that veto (`the nested <path> needs a person's decision, and everything
   inside goes with this one; on its own: ...`), even when another check also made it ask.

**Reasons in words.** Every reason that rests on a fact says what the fact means in words and then
names the fact as a trailing tag, for example `stashed changes [stash_empty]` or `the stash could
not be read (git failed); run the scan again [stash_empty]`. One table in `plan.rs` holds the words
for every fact as `no` and as `unknown`. Counts agree with their number (`1 file is`, `2 files
are`) in reasons, notes and report headers.

**Hidden index entries.** `git update-index --skip-worktree` and `--assume-unchanged` tell git to
stop looking at a file, so an edit to it is invisible to `status`, `diff`, `add -A` and `stash`,
the commands recovery would use. The scan runs `git ls-files -v -z` and counts the entries tagged
`S` (skip-worktree) or with a lowercase tag (assume-unchanged) whose file is present (`lstat`; a
file that cannot be checked counts). A sparse checkout marks every file outside its cone
skip-worktree and removes it from the working tree; an absent file holds no edit, so a sparse
checkout with everything pushed is still `backup-then-remove`. A present file is not hashed: comparing its
bytes with the index blob would need the repository's clean filters and line-ending rules, which
this tool never runs, so any present entry asks.

Activity evidence is the newest of `newest_mtime` and `last_commit_time`; a regenerable directory
uses its owning project. `totals.reclaimable_bytes` does not count an item nested inside another
removing item twice; `totals.ready_bytes` is only `remove-regenerable` bytes: a
`backup-then-remove` rests on remote facts that are unverified until apply re-checks them and on a
backup that has not been made, so it is never "ready". An absurdly large
`--active-window-days` saturates (everything with evidence is active) instead of wrapping.


### The preserve obligation

Earlier schemas tried to prove that a git directory held nothing unique by judging its files one
kind at a time (hooks, `info/`, message files, `rr-cache/`, `logs/`, ...), and schema 11 then
exempted object files whose first bytes looked right. Every round of review found another kind, or
a file whose prefix passed and whose rest was someone's notes. Schemas 12 and 13 then let an
opt-in `scan --verify-objects` mark an object store verified by git's own checks, and review found
that a check of a file's checksum does not show its content is git's (a bitmap with an arbitrary
payload and a recomputed checksum would pass) and that the checks git runs differ between
versions.
Schema 14 removes that verification: no file is exempt because of its name, its first bytes or
any check this tool runs.

**This phase defines no verification procedure for object data.** Every file in a git directory
is under a backup obligation before removal: `preserve.git_admin` and `preserve.object_store`;
both MUST be backed up. A later apply phase may define a verification against the live remote;
this format does not, and a field that claims one is not part of it: `plan` refuses (exit 1,
"invalid scan file") a scan file whose document, any node, repository block, `repo.object_store`
or `repo.git_admin` carries `verified`, `checked`, `verification`, `verify_objects` or any other
field name containing `verif`, compared ignoring case (`Verified`, `VERIFY`), and the three
repository blocks refuse any field this format does not define.

**Two classes, by name alone.** Every non-directory entry of a git directory (a symlink or other
non-directory entry included, never followed) is in exactly one of two classes, decided by its
path below the git directory. No content is read to decide. The classes differ in how they are
counted and capped, not in the obligation.

| Class | Plan field | Which files | Obligation |
|---|---|---|---|
| (b) the object store | `preserve.object_store` | in `objects/`, a name of an object-store file: `<xx>/<38 or 62 hex>` (loose object); `pack/pack-<id>.{pack,idx,rev,bitmap,mtimes}`; `pack/multi-pack-index`, `pack/multi-pack-index-<id>.{bitmap,rev}`, `pack/multi-pack-index.d/multi-pack-index-<id>.{midx,bitmap,rev}`; `info/commit-graph`, `info/commit-graphs/graph-<id>.graph` | backed up before removal |
| (a) the git directory's own files | `preserve.git_admin` | every other entry, of any name or kind | backed up before removal |

Class (a) therefore includes `config`, `HEAD`, `index`, `packed-refs`, every file under `refs/` and
`logs/` (`logs/refs/.notes` included), every hook whatever its name or suffix (an edited
`pre-commit.sample`, a `custom.sample`), `info/*`, `description`, `COMMIT_EDITMSG` and the other
message files, `rr-cache/` (recorded resolutions, including one recorded before a merge was
aborted), `.keep` and `.promisor` markers, `objects/info/packs`, `objects/info/alternates`, the
chain files `multi-pack-index-chain` and `commit-graph-chain`, `lfs/`, `modules/`, `worktrees/`,
and any stray file anywhere, including one inside `objects/` whose name is not an object-store
name. An entry with an object-store name that is not a regular file (a symlink) is class (a) too,
and makes `no_local_admin_content` `no`, so the repository asks. A file with an object-store name
is class (b) whatever it holds (text named like a loose object, a `.pack` of text, an `.idx`
without its pack, a `.bitmap` with any payload, a multi-pack-index `.rev`): it is backed up like
any other. `disk_cleaner_core::git_admin_files` and `disk_cleaner_core::object_store_files` list
the two classes; together they list every file exactly once.

**The rule.** Every plan item whose action removes a git node (repository, worktree or bare
repository) carries `preserve`, covering its own git directory and those of the git nodes inside
it. A removing item that is not a git node but holds one (the scan already makes such a directory
ask) asks rather than remove a git directory without the obligation. A consumer that applies a
plan MUST NOT remove such a node unless, first, every file of both classes has been backed up and
the backup verified (each file read back and compared). If the backup cannot be made or verified,
it MUST NOT remove anything. Both lists are enumerated again at apply time with the two functions
above (the same classifier the scan used) rather than trusted from the plan, and an enumeration
error means nothing is removed. **No apply phase exists in this version**: `scan` and `plan` are
read-only, and this rule binds whatever applies a plan later.

**The cap.** Class (a) is meant for small files. When one git directory's class-(a) files take
more than `max_git_admin_bytes` (allocated, default 64 MiB, config key `max_git_admin_bytes`), the
node asks, naming the largest of them: a large file put inside `.git` is the user's data, not git's
settings, and a person decides where it goes. Class (b) has no cap: its size is the repository's
object data, reported in `preserve.object_store.bytes` and in the reason, so a person sees how much
the backup holds.

**The plan fields.** `preserve.git_admin` holds `git_dirs` (the git directories covered:
`<path>/.git` for a repository or worktree, the path itself for a bare repository, and those of git
nodes nested inside a removed directory), `files` and `bytes` (allocated, summed over `git_dirs`)
and `examples` (up to five paths relative to the item's directory: first the three largest covered
files of at least 1 MiB, so a large file kept after raising `max_git_admin_bytes` is named, then
files whose names not every clone has, such as `.git/COMMIT_EDITMSG` or `.git/hooks/pre-commit`).
`preserve.object_store` holds `files` and `bytes`, summed over the same git directories; it has no
verification field. The reason says both: "its N local git files (X KiB: settings, hooks, logs and
the like) are saved with the manifest first; its object store (M files, Y GiB) holds object data
that must be backed up too before removal, then it is removed". A git node whose files the scan
could not count (`repo.git_admin` or `repo.object_store` absent) asks. The human plan defines the
manifest once in its header ("Before a repository is removed, its own git files are saved with the
manifest (the manifest is the restore record a later apply step writes), and its object store is
backed up too.") and prints a `back up first:` line and an `object store: M files (Y GiB) of object
data, to be backed up before removal` line under each such item.

**Asks the backup makes redundant.** With the obligation in place, some existing asks guard content
that the backup now saves. They are kept unchanged, so that no repository moves toward removal in
the change that introduced the backup; relaxing them is a separate decision:

- `no_local_admin_content` `no` from a custom hook, a non-stock `info/` file, an edited
  `description`, a pending `TAG_EDITMSG`, `MERGE_MSG` or `SQUASH_MSG`, legacy `branches/` or
  `remotes/` files, or a config key of a local family (`user.*`, `url.*`, `branch.<b>.description`,
  unknown families): all of these files are in `preserve.git_admin`;
- `no_local_admin_content` `no` from a stray file found by the shape check under `refs/`, `logs/`,
  `rr-cache/`, `objects/`, `lfs/` or `fsmonitor--daemon/`;
- the part of `plain_layout` that refuses an unknown top-level file (not directory) in the git
  directory: such a file is backed up too, although `plain_layout` also guards how git reads the
  repository, so only that part would be redundant.

One ask of schema 11 is gone, by design: a file with an object-store name whose first bytes did not
look like an object file made the repository ask. Schema 12 and later read no content to classify,
so such a file is under the object-store backup obligation instead.

Not redundant, and not touched by the backup: dependents, hidden index entries, pseudorefs (they
are about commits, which the remote check covers), coverage, the process check, alternates, linked
worktrees and submodule git directories.
