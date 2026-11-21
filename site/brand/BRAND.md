# PruneDisk brand

Status: the project owner chose the name PruneDisk. The rename of the repository and the command is planned, not done (see section 1).

## 1. Name

- **Display name: PruneDisk.** One word, capital P and capital D, in prose and headings, including at
  the start of a sentence. Never "Prune Disk", "Prunedisk", "prune-disk" or "PRUNEDISK".
- **Command, package and domain forms: `prunedisk`**, lower case, in code font. This is the planned
  name of the command-line binary, crate, package and any domain.
- **Today (not yet renamed):** the repository is `github.com/ericmacdougall/disk-cleaner` and the
  command-line binary is still `disk-cleaner`. Renaming the repository and the binary to `prunedisk`
  is planned. Until it happens, anything a reader can copy (links, commands, examples) must use the
  current `disk-cleaner` names, with one short line saying the command will be renamed. Do not print
  `prunedisk.com` or `prunedisk.app` as live addresses or links: they are not registered.
- **Wordmark:** the three-block mark followed by "PruneDisk" drawn as strokes (section 6). Minimum
  clear space: the height of one small block on every side. Minimum size: 16 px for the mark alone
  (`favicon.svg`), 120 px wide for the full logo.
- **Do not** put the name next to the logo of another product or imitate another product's look.

## 2. Tagline

Recommended (one): **Know what is safe to remove.**
Why: it says what the tool does that others do not (classify before removing), makes no size or speed
claim, and stays true while only the read-only scanner exists.

Alternates:

1. Disk space for machines full of repos and agents.
2. Remove nothing that exists only here.

Do not use: "fastest", "one click", "clean your disk", "reclaim gigabytes", or any number that has not
been measured and published with its method.

## 3. Voice

Write in American English (analyzer, color, license, judgment). Plain, specific, calm. Say what a thing is, then what it does. Prefer the concrete noun
(`node_modules`, "an unpushed branch") to the abstract one ("junk", "bloat"). Say "early" and "planned"
where they are true; never write a future feature in the present tense. State limits next to claims.
No exclamation marks, no emoji, no fear ("your disk is dying"), no superlatives. Examples invented for
the site use obviously synthetic paths such as `~/projects/example-app`.

| Instead of | Write |
|---|---|
| Supercharge your dev machine | Shows what fills a developer's disk and what is safe to remove. |
| Cleans your disk in one click | Plans removals you can read first. In the current version nothing is removed. |
| Safe and secure | Unknown never counts as yes. A check that fails or times out leaves its fact unknown, and the plan then asks you about an idle repo instead of proposing to remove it. |

## 4. Color tokens

Text pairs are WCAG 2.2 AA at normal size (4.5:1) or better. Non-text parts (borders that mark a
control, chart tiles) are 3:1 or better. Ratios were computed with the WCAG relative-luminance formula
from the hex values below.

| Token | Light | Dark | Use |
|---|---|---|---|
| `--bg` | `#fbfaf6` | `#0f1513` | page |
| `--surface` | `#f1eee6` | `#18201d` | cards, code |
| `--text` | `#1b2420` | `#e8eeea` | body text |
| `--muted` | `#485650` | `#a8b6af` | secondary text |
| `--accent` | `#0b5d56` | `#5ed6c8` | links, buttons, logo main block |
| `--border` | `#78837d` | `#6f7f78` | borders of controls and callouts |
| `--rule` | `#d9d5c9` | `#2a3531` | decorative dividers only |

Contrast, light: text on bg 15.2, text on surface 13.7, muted on bg 7.4, muted on surface 6.7,
accent on bg 7.4, accent on surface 6.7, white on accent (button) 7.7, border on bg 3.8, border on
surface 3.4.
Contrast, dark: text on bg 15.7, text on surface 14.1, muted on bg 8.8, muted on surface 7.9, accent
on bg 10.5, accent on surface 9.4, bg on accent (button) 10.5, border on bg 4.4, border on surface 3.9.

Class colors (one per file class; always paired with a text label, never the only signal):

| Class | Light | Dark | Label text on tile, light / dark |
|---|---|---|---|
| repo, worktree | `#2f5fb3`, `#6d49b8` | `#7aa7f0`, `#b79cf0` | 6.2, 6.4 / 7.6, 7.9 |
| regenerable | `#b45309` | `#f0a54a` | 5.0 / 9.0 |
| cache | `#9a3f1b` | `#e8855a` | 6.8 / 7.0 |
| agent-session | `#a3306f` | `#e879b4` | 6.5 / 6.9 |
| output | `#1f7a4d` | `#5fcf93` | 5.3 / 9.5 |
| loose-code | `#5f6b66` | `#9aa7a1` | 5.6 / 7.4 |

Label text is white on the light tiles and `#0f1513` on the dark tiles. Each class color also
clears 4.3:1 or more against both page backgrounds in its own scheme, so it is usable as a swatch or
a thin line.

The three logo colors are `--accent`, the regenerable color and the repo color of the active scheme.

## 5. Type

System font stacks only. Nothing is downloaded, embedded or licensed.

- Text and the site header name: `system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif`
- Code and command names: `ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace`

These are the operating system's own fonts, used by name. No font file is distributed with the
project, so there is no font license to carry. If the project later bundles a typeface it must be
open-licensed (SIL OFL or Apache-2.0), self-hosted, and its license added to the repository.

Scale: base 16 px, line height 1.6; headings clamp from 1.4 rem to 3 rem. Code never goes below 13 px.

## 6. Logo

- **Mark** (`logo-mark.svg`): a square cut into one large block and two small ones, a treemap in three
  tiles. It says what the tool shows (where space went, by kind) and the large block is the part that
  stays. Three flat tiles with generous gaps stay distinguishable at 16 px; `favicon.svg` is the same
  mark redrawn on a 32-unit grid with wider relative gaps.
- **Light and dark files.** The standalone files do not switch by themselves: `logo.svg` and
  `logo-mark.svg` are for light backgrounds, `logo-dark.svg` and `logo-mark-dark.svg` for dark ones. The
  caller picks, for example with
  `<picture><source srcset="logo-dark.svg" media="(prefers-color-scheme: dark)"><img src="logo.svg" alt="PruneDisk"></picture>`.
  The one exception is `favicon.svg`, which follows the browser's color scheme with `prefers-color-scheme`
  because a tab strip can be light or dark independently of the page.
- **Logo** (`logo.svg`): the mark plus the wordmark. The letters of "PruneDisk" are hand-drawn as
  rounded strokes (nine paths, no font), so the logo renders identically on every system and carries no
  font license. They are geometric and plain by design; they are not an outlined copy of any typeface.
  The website header sets the name in the system sans-serif at weight 700 instead of using the logo image.
- Drawn by hand as plain rectangles. No stock icon, no generated image, no shape borrowed from another
  product. Do not rotate, outline, add shadows or recolor outside the tokens above.

## 7. Name decision and availability

**Decision (project owner): the product is called PruneDisk.** The earlier generic name `disk-cleaner`
was crowded: the crates.io name was already taken, and a Snap and a Mac App Store product used it.

Availability note, dated **2026-11-18**: when checked on that date, the names `prunedisk.com` and
`prunedisk.app` and the names `prunedisk` / PruneDisk on crates.io, npm, GitHub, the Snap Store and the
Mac App Store were unclaimed. Not checked: the Microsoft Store and Flathub. This is **not a trademark
search**; a trademark search in the target jurisdictions is still needed before the name is relied on
publicly. The domains are not registered, so nothing on the site links to them.

Planned follow-up (not done): register the domains and reserve the package names; rename the repository
and the binary to `prunedisk`; update the site's links and example commands at the same time.
