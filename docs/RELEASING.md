# Releasing

How a version of the command-line program reaches the releases page. The workflow is
`.github/workflows/release.yml`; its guards are `.github/scripts/release_meta.py`, tested by
`.github/scripts/test_release_meta.py`; the text of the release notes is `.github/release-notes.md`.
The binaries are **not signed**; the notes say so and say how to check a download.

## What actually controls a release

In order of strength:

1. **The maintainer's publish step.** The workflow only ever creates a **draft**. Nobody but people
   with write access sees a draft, and only a person publishes it (step 4 below).
2. **Repository settings** (in force, see below): only a repository admin can create a `v*` tag, no
   one can move or delete one, `main` changes only through a pull request, and a published release
   is immutable.
3. **The workflow's guards** (`release_meta.py`). They are hygiene, not a security boundary: GitHub
   runs the `release.yml` of the tagged commit, so whoever can create a tag could tag a commit whose
   workflow leaves the guards out. The settings above are what limit who can do that.

One limit of (2): the repository has one collaborator, the owner, and agents working on it act with
the owner's credentials. A ruleset cannot tell those apart, so "only an admin creates a tag" means
"only the owner's credentials". The publish step (1) is where a person looks.

## Repository settings in force

In force, as read back from the API on 2026-11-21:

| Id | Name | Target | Rules | Bypass |
|---|---|---|---|---|
| 27783904 | `release tags` | tags `refs/tags/v*` | `update`, `deletion` | none |
| 27783905 | `release tags: creation` | tags `refs/tags/v*` | `creation` | repository admin role (`RepositoryRole` 5, always) |
| 27783906 | `main` | default branch | `deletion`, `non_fast_forward`, `pull_request` (0 approvals) | none |

Immutable releases: enabled.

Read them back:

```
gh api repos/ericmacdougall/disk-cleaner/rulesets --jq '.[] | {id, name, target, enforcement}'
gh api repos/ericmacdougall/disk-cleaner/rulesets/27783904 --jq '{name, conditions, rules, bypass_actors}'
gh api repos/ericmacdougall/disk-cleaner/rulesets/27783905 --jq '{name, conditions, rules, bypass_actors}'
gh api repos/ericmacdougall/disk-cleaner/rulesets/27783906 --jq '{name, conditions, rules, bypass_actors}'
gh api repos/ericmacdougall/disk-cleaner/immutable-releases
```

Undo (owner only; each weakens the controls above):

```
gh api -X DELETE repos/ericmacdougall/disk-cleaner/rulesets/<id>
gh api -X DELETE repos/ericmacdougall/disk-cleaner/immutable-releases
```

What these settings mean for the steps below: a pushed `v*` tag is permanent; once a release is
published its tag and assets are locked (GitHub's immutable releases page, read 2026-11-21: the title,
notes and pre-release status can still be edited, and a deleted immutable release's tag name cannot
be reused); `main` cannot be pushed to directly. The
`main` ruleset does not require status checks, so "CI passed" is enforced only by the workflow guard.

## Rehearse (no release is made)

Actions tab, "release", "Run workflow", on a branch. (GitHub offers "Run workflow" only once
`release.yml` is on the default branch; before that, a rehearsal needs a temporary `push:` trigger for
the branch, removed again afterwards.) It builds the four targets, packages them, writes
`SHA256SUMS`, attests every file it lists, and keeps everything as artifacts named `release-*` and
`release-files` for 14 days. It never creates a tag or a release, and it refuses to run from a tag
ref. The archives are named `-rehearsal.<run id>`.

The rehearsal also proves the published verification command rejects it: for every file, the
command with `--source-ref refs/tags/<rehearsal name>` must fail while the same flags with the run's
own branch ref must pass (step "Check the published verification command"). Its `meta` step prints
what a tag on that commit would be refused for.

Never push a `v*` tag here to rehearse: the tag rules make it permanent. The tag-to-draft path is
rehearsed only in a separate, disposable repository with the same rules (its slug differs, so its
attestations say nothing about this repository). **In this repository the first real tag is the
first execution of the draft step**, and that step only creates a draft. If it fails, nothing is
public; fix it and release the next patch version.

The `crates-dry-run` job in the same run is the crates.io check (below). It does not block.

## Cut a release

1. Bump `version` in `[workspace.package]` in `Cargo.toml` (and `Cargo.lock`) through a pull request
   to `main`; wait until `ci.yml` has passed on the merged commit (a run started by the push to
   `main`; a pull request run does not count).
2. `git tag vX.Y.Z <commit on main>` and `git push origin vX.Y.Z`. Use the version exactly: the tag
   must equal `v` plus the workspace version. A pre-release is a version with a suffix in
   `Cargo.toml` itself (for example `0.2.0-rc.1`, tag `v0.2.0-rc.1`); the suffix may hold only ASCII
   letters, digits and dots. A tag that is pushed stays forever, so check the commit first.
3. The `meta` job refuses, and no build, package or draft job runs (only the separate, read-only
   `crates-dry-run` job does), unless: the tag matches
   `^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$` (checked before anything else reads it); it equals `v`
   plus the workspace version; the commit is on `main`; a `ci.yml` run started by a push of exactly
   that commit succeeded; and the tag on GitHub still points at that commit (lightweight and
   annotated tags are both peeled to the commit). Every later job checks out that commit, and the
   draft job checks again, just before and just after creating the draft, that the tag still points
   at it. If it moved, the job fails, and it deletes only the release it created, found by the id it
   recorded, and only while that release is still an unchanged draft. A release that was published,
   edited or replaced meanwhile is never deleted; the job fails and says so.
4. The workflow creates a **draft** release with the archives, `SHA256SUMS` and the notes, titled
   `PruneDisk vX.Y.Z`. Nothing is public yet. Read it: the notes name the commit; check it is the one
   you meant (`git rev-parse vX.Y.Z^{commit}`). Download one archive and verify it:

   ```
   gh attestation verify <archive file name> --repo ericmacdougall/disk-cleaner --signer-workflow ericmacdougall/disk-cleaner/.github/workflows/release.yml --source-ref refs/tags/vX.Y.Z --deny-self-hosted-runners
   ```

   Then publish: `gh release edit vX.Y.Z --draft=false`. From then on its tag and assets are locked.
5. Update the status wording in `README.md` and `site/index.html` only if it no longer reads true
   (the download text is written to be true with or without a release).

Rollback: before publishing, first check it is still a draft
(`gh release view vX.Y.Z --json isDraft` must say `true`), then delete it
(`gh release delete vX.Y.Z`, without `--cleanup-tag`; the tag cannot be deleted anyway). Never
delete a published release to roll back. After publishing, the assets and the tag cannot be changed or
removed: supersede the release with a new patch version (fix on `main`, bump, tag `vX.Y.(Z+1)`), say
in the new notes what was wrong with the old one, and edit the old release's notes to point at the
new one (or mark it a pre-release). Attestations stay; they are history.

## What the attestation proves, and what it does not

`gh attestation verify` with the flags above passes only for a file whose SHA-256 has a Sigstore-signed
build provenance record made by `.github/workflows/release.yml` of this repository, running on a
GitHub-hosted runner, for the ref `refs/tags/vX.Y.Z`. A build from a branch (a rehearsal, or a
workflow run on an unmerged branch) has `refs/heads/...` as its source ref and fails it; each
rehearsal shows that. The `--repo`-only form, which an earlier draft of these docs printed, accepts
branch builds and must not be used.

It does not prove that the code was reviewed, that the commit is on `main`, or that the workflow's
own guards ran unchanged: the tagged commit's workflow file is what ran. Those rest on the settings
and the publish step above. `SHA256SUMS` itself is not attested; every file it lists is.

## Desktop bundles (extension point)

The desktop app has its own bundle jobs on its branch. Add them to `release.yml` next to the `build`
job: each checks out `needs.meta.outputs.sha` and uploads its installers as an artifact named
`release-desktop-<target>` containing only the files to publish. The `package` job writes every file
of every `release-*` artifact into `SHA256SUMS`, and the attest step reads its subjects from
`SHA256SUMS` (`subject-checksums`), so `.dmg`, `.msi`, `.AppImage` and `.deb` files are checksummed and
attested with no change there. The release notes' file table and the site's platform list must be
updated by hand. Installers need signing decisions (macOS notarization, Windows signing) that are not
made yet.

## crates.io (preparation only)

Checked 2026-11-20 against the crates.io API: `prunedisk`, `prunedisk-core`, `prunedisk-cli` and
`prunedisk-rules` are free. `disk-cleaner` is already taken by someone else; `disk-cleaner-core` and
`disk-cleaner-cli` are free. Publish under the new names (`prunedisk` for the binary package,
`prunedisk-core` for the library) after the rename, and re-check right before publishing, because a
name is first come, first served.

What the manifests need before a real publish:

- Every published crate: `description`, `license`, `repository` (present); add `readme`, `keywords`
  (at most five), `categories`, and `rust-version`. A `README.md` and the `LICENSE` file inside each
  crate directory so the package carries them.
- The CLI's dependency on the core crate has `path = "../core"` only. A published crate needs a
  version as well: `prunedisk-core = { version = "0.1.0", path = "../core" }`. Publish the core crate
  first, then the CLI.
- **Blocker found by this check:** `crates/core/src/rules.rs` embeds `../../../rules/artifacts.toml`,
  which is outside the crate directory. `cargo package` leaves that file out of the tarball, so the
  published core would not compile. The rules file must live inside `crates/core` (for example
  `crates/core/rules/artifacts.toml`, with docs and tests updated), or the rules be copied in at
  build time. This touches `crates/**` and `rules/**`, which other streams are editing, so it is left
  for the integration step.
- Use `include` or `exclude` so tests, fixtures and docs the crate does not need are not published,
  and check with `cargo package --list -p <name>`.

The `crates-dry-run` job runs `cargo package --list` per crate, reports whether each name is free and
runs `cargo publish --dry-run --workspace --locked`. It is expected to fail until the points above
are done. It uploads nothing and uses no registry token.

The real publish needs a crates.io credential. crates.io supports trusted publishing from GitHub
Actions (OpenID Connect; the `rust-lang/crates-io-auth-action` action exchanges the job's identity
token for a short-lived crates.io token, with `id-token: write`). It is configured per crate on
crates.io with the repository owner and name and the workflow file name, and a crate must already
exist before that can be set up, so the first publish of each crate needs an API token created by the
account owner (kept as a repository secret or used once from a terminal, never committed). Confidence
medium: read the current crates.io documentation when setting it up. After the repository rename,
update the trusted publisher entry to the new repository name.

## Repository rename: what hard-codes `ericmacdougall/disk-cleaner`

One `sed` of `ericmacdougall/disk-cleaner` to `ericmacdougall/prunedisk` covers these; the command and
package renames (`disk-cleaner` to `prunedisk`) are separate and listed after.

| Where | What |
|---|---|
| `Cargo.toml` | `repository` (workspace table; the crates inherit it) |
| `README.md` | releases link, the `gh attestation verify` command (`--repo` and `--signer-workflow`), and other links |
| `site/index.html` | about eight links to the repository, its releases page, LICENSE, README and design doc; the `gh attestation verify` example |
| `docs/RELEASING.md` | the `gh attestation verify` and `gh api` commands in this file |
| `site/brand/BRAND.md`, `site/README.md` | the "not yet renamed" paragraphs (rewrite, not sed) |
| `site/tests/test_site.py` | `ALLOWED_LINK_PREFIX`, the repository link checks, the `/disk-cleaner` base path assertion |
| `.github/scripts/test_release_meta.py` | the repository name the published-command test expects |
| `.github/workflows/site.yml`, `site/tools/smoke.py` | the `/disk-cleaner/` Pages base path (project sites are served under the repository name) |
| `.github/workflows/release.yml` | nothing: the repository comes from `github.repository`; the binary and package names are the two `env` lines at the top |
| `.github/release-notes.md` | nothing: `@REPO@` and `@BIN@` are filled in by the workflow |
| `docs/DESIGN.md`, `docs/SCAN-FORMAT.md`, `rules/artifacts.toml`, `crates/**` | the program name in prose, help text and tests (command rename) |
| `.github/workflows/ci.yml` | package and binary names in the test and binary jobs (command rename) |

What GitHub's "Renaming a repository" page (read 2026-11-20) says happens on a rename:

- Redirected: web traffic to the old name (issues, wikis, stars and the rest), and `git clone`,
  `git fetch` and `git push` to the old URL, until someone creates a new repository with the old
  name. Local clones keep working but should have their `origin` updated.
- Not redirected: GitHub Pages project site URLs (the site moves to
  `https://<owner>.github.io/<new name>/` and every link to the old path breaks), and calls to an
  action hosted by the renamed repository (workflows that use it fail with "repository not found").
  This repository hosts no action, so nothing outside depends on that.
- Not stated by that page: whether REST API URLs and release asset download URLs of the old name
  redirect. Check them after the rename rather than relying on it.

Also by hand: the crates.io trusted publisher entry and any other name-matched setting; the rulesets
above stay attached to the repository. Attestations carry the old repository name in already-signed
records, so for an old release `gh attestation verify` needs `--repo` and `--signer-workflow` with
the name the file was built under.

## Known gaps (to be ticketed, not fixed in this change)

- `actions/checkout` v4.2.2 and `actions/upload-artifact` v4.6.2 are older majors than the latest
  (v7.x seen with `git ls-remote`); their runtime support on future runners was not checked.
- The `ubuntu-22.04` runner's retirement date was not checked.
- A re-run of the `package` job downloads `release-*`, which on a re-run can include the earlier
  attempt's `release-files` artifact (a hypothesis, untested). If it does, `release_meta.py package`
  refuses (a directory in `dist/`) rather than publishing stray files; the fix is a re-run of the
  whole workflow.
- The `main` ruleset does not require status checks; "CI passed" is checked only by the workflow
  guard. (The guard does reject pull_request-only runs.)
