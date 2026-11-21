"""Checks for the static site in site/. Standard library only.

Run from the repository root:
    python3 -m unittest discover -s site/tests -v
"""
import re
import unittest
import xml.etree.ElementTree as ET
from html.parser import HTMLParser
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SITE = ROOT / "site"
INDEX = SITE / "index.html"
BRAND_MD = SITE / "brand" / "BRAND.md"
README = SITE / "README.md"
ROOT_README = ROOT / "README.md"
CI_YML = ROOT / ".github" / "workflows" / "ci.yml"
PAGES_YML = ROOT / ".github" / "workflows" / "pages.yml"
SITE_YML = ROOT / ".github" / "workflows" / "site.yml"
RELEASE_YML = ROOT / ".github" / "workflows" / "release.yml"
ALLOWED_LINK_PREFIX = "https://github.com/ericmacdougall/"
VOID = {"area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"}
UNIT_CMD = "python3 -m unittest discover -s site/tests -v"


def read(path):
    return Path(path).read_text(encoding="utf-8")


class Collector(HTMLParser):
    """Collects structure from the page in one pass."""

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.stack, self.errors = [], []
        self.ids, self.tags, self.headings = [], [], []
        self.metas, self.links, self.anchors = [], [], []
        self.html_attrs = {}
        self.title, self._in_title = "", False
        self.svgs = []  # dicts: attrs, has_title
        self.imgs = []
        self._svg_stack = []

    def _start(self, tag, attrs):
        d = dict(attrs)
        self.tags.append((tag, d))
        if "id" in d:
            self.ids.append(d["id"])
        if tag == "html":
            self.html_attrs = d
        elif tag == "meta":
            self.metas.append(d)
        elif tag == "link":
            self.links.append(d)
        elif tag == "img":
            self.imgs.append(d)
        elif tag == "a" and d.get("href", "").startswith("#"):
            self.anchors.append(d["href"][1:])
        elif tag in ("h1", "h2", "h3", "h4", "h5", "h6"):
            self.headings.append(int(tag[1]))
        elif tag == "svg":
            self.svgs.append({"attrs": d, "has_title": False})
            self._svg_stack.append(self.svgs[-1])
        elif tag == "title":
            if self._svg_stack:
                self._svg_stack[-1]["has_title"] = True
            else:
                self._in_title = True

    def handle_starttag(self, tag, attrs):
        self._start(tag, attrs)
        if tag not in VOID:
            self.stack.append(tag)

    def handle_startendtag(self, tag, attrs):
        self._start(tag, attrs)
        if tag == "svg":
            self._svg_stack.pop()

    def handle_endtag(self, tag):
        if tag in VOID:
            return
        if self.stack and self.stack[-1] == tag:
            self.stack.pop()
            if tag == "svg" and self._svg_stack:
                self._svg_stack.pop()
            if tag == "title":
                self._in_title = False
        else:
            self.errors.append((tag, list(self.stack[-3:]), self.getpos()))

    def handle_data(self, data):
        if self._in_title:
            self.title += data


def parse_page():
    c = Collector()
    c.feed(read(INDEX))
    c.close()
    return c


def external_refs(markup):
    """References that would make the browser fetch something, or links off the allowed prefix."""
    found = []
    for m in re.finditer(r"""\b(?:src|srcset|poster|data)\s*=\s*["']?\s*(?:https?:)?//[^"'\s>]+""", markup, re.I):
        found.append(m.group(0))
    for m in re.finditer(r"""<link\b[^>]*\bhref\s*=\s*["']?\s*(?:https?:)?//[^"'\s>]+""", markup, re.I):
        found.append(m.group(0))
    for m in re.finditer(r"""@import\b[^;]*""", markup, re.I):
        found.append(m.group(0))
    for m in re.finditer(r"""url\(\s*["']?\s*(?:https?:)?//[^)]*\)""", markup, re.I):
        found.append(m.group(0))
    for m in re.finditer(r"""<a\b[^>]*\bhref\s*=\s*["'](https?://[^"']+)["']""", markup, re.I):
        if not m.group(1).startswith(ALLOWED_LINK_PREFIX):
            found.append(m.group(0))
    return found


def forbidden_hits(text, words):
    low = text.lower()
    return [w for w in words if w.lower() in low]


def top_level_permissions(yml):
    perms, inside = {}, False
    for line in yml.splitlines():
        if re.match(r"^permissions:\s*$", line):
            inside = True
            continue
        if inside:
            m = re.match(r"^\s+([a-z-]+):\s*(\S+)\s*$", line)
            if m:
                perms[m.group(1)] = m.group(2)
            elif line.strip() and not line.startswith(" "):
                break
    return perms


def action_refs(yml):
    return re.findall(r"^\s*-?\s*uses:\s*(\S+)", yml, re.M)


NUMBER_WORDS = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
                "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen",
                "eighteen", "nineteen", "twenty"]
CORE_MODEL = ROOT / "crates" / "core" / "src" / "model.rs"
CORE_PLAN = ROOT / "crates" / "core" / "src" / "plan.rs"
CORE_FACTS = ROOT / "crates" / "core" / "src" / "facts.rs"
CLI_REPORT = ROOT / "crates" / "cli" / "src" / "report.rs"


def core_fact_names(src=None):
    """Fact names in the order `RepoFacts::entries()` in the core crate returns them."""
    src = read(CORE_MODEL) if src is None else src
    body = re.search(r"fn entries\(&self\)[^{]*\{(.*?)\n    \}", src, re.S).group(1)
    return re.findall(r'\("([a-z_]+)",\s*self\.', body)


def core_action_names(src=None):
    """Plan action names from `Action::as_str` in the core crate."""
    src = read(CORE_PLAN) if src is None else src
    return re.findall(r'Action::\w+ => "([a-z-]+)"', src)


def example_text(html):
    return re.search(r'aria-label="Example scanner output"><code>(.*?)</code></pre>', html, re.S).group(1)


def example_fact_groups(text):
    """For each repo, worktree or bare repo in the example scan summary, its fact names in order."""
    groups, cur = [], None
    for line in text.splitlines():
        if re.match(r"^\s+\[(repo|worktree|bare-repo)\] ", line) and "=" not in line:
            cur = []
            groups.append(cur)
        elif cur is not None:
            cur.extend(re.findall(r"\b([a-z_]+)=(?:yes|NO|UNKNOWN)\b", line))
    return [g for g in groups if g]


def example_actions(text):
    """The action word of each item line after "Items:" in the example plan."""
    after = text.split("\nItems:\n", 1)
    if len(after) < 2:
        return []
    return re.findall(r"^  ([a-z][a-z-]+) +\S+ \S+ +\[", after[1], re.M)


UNITS = {"B": 1, "KiB": 1024, "MiB": 1024 ** 2, "GiB": 1024 ** 3, "TiB": 1024 ** 4}
SIZE = r"\d+(?:\.\d)? (?:B|KiB|MiB|GiB|TiB)"


def size(s):
    """A size as printed by the CLI ("1.9 GiB") as (bytes, rounding tolerance in bytes)."""
    num, unit = s.split()
    mult = UNITS[unit]
    return float(num) * mult, (0 if unit == "B" else 0.05 * mult)


def parse_example(text):
    """The parts of the example scan summary and plan that carry numbers, in printed order."""
    lines = text.splitlines()
    ex = {"classes": [], "repos": [], "regen": [], "items": [], "repo_count": None, "regen_count": None,
          "list_gaps": 0, "total": None, "scan_cmd": "", "plan_cmd": ""}
    section, cur = None, None
    for line in lines:
        if line.startswith("$ disk-cleaner scan"):
            ex["scan_cmd"] = line
        elif line.startswith("$ disk-cleaner plan"):
            ex["plan_cmd"] = line
            section = "plan"
            continue
        m = re.search(r", (" + SIZE + r") allocated", line)
        if m:
            ex["total"] = size(m.group(1))
        if line.startswith("Space by class"):
            section = "classes"
            continue
        m = re.match(r"Repos and worktrees \((\d+)\):", line)
        if m:
            section, ex["repo_count"] = "repos", int(m.group(1))
            continue
        m = re.match(r"Regenerable and cache directories \((\d+)\):", line)
        if m:
            section, ex["regen_count"] = "regen", int(m.group(1))
            continue
        if line.startswith("Items:"):
            section = "items"
            continue
        if not line.strip():
            if section in ("classes", "repos", "regen"):
                section = None
            continue
        if section == "classes":
            m = re.match(r"^  ([a-z-]+) +(" + SIZE + r")$", line)
            if m:
                ex["classes"].append((m.group(1), size(m.group(2))))
        elif section == "repos":
            m = re.match(r"^  \[([a-z-]+)\] (\S+)  (" + SIZE + r")$", line)
            if m:
                cur = {"class": m.group(1), "path": m.group(2), "total": size(m.group(3)), "facts": {},
                       "regen": None}
                ex["repos"].append(cur)
            elif line.strip() == "..." and line.startswith("  ") and not line.startswith("   "):
                ex["list_gaps"] += 1
            elif cur is not None:
                cur["facts"].update(re.findall(r"\b([a-z_]+)=(yes|NO|UNKNOWN)\b", line))
                m = re.search(r"regenerable (" + SIZE + r")", line)
                if m:
                    cur["regen"] = size(m.group(1))
        elif section == "regen":
            m = re.match(r"^ +(" + SIZE + r")  (\S+)$", line)
            if m:
                ex["regen"].append((m.group(2), size(m.group(1))))
            elif line.strip() == "...":
                ex["list_gaps"] += 1
        elif section == "items":
            m = re.match(r"^  ([a-z][a-z-]+) +(" + SIZE + r")  \[([a-z-]+)\] (\S+)$", line)
            if m:
                ex["items"].append({"action": m.group(1), "size": size(m.group(2)), "class": m.group(3),
                                    "path": m.group(4)})
    return ex


def close(a, b, extra_tol=0.0):
    """Two (bytes, tolerance) sizes agree within their combined rounding."""
    return abs(a[0] - b[0]) <= a[1] + b[1] + extra_tol


def total(sizes):
    return (sum(s[0] for s in sizes), sum(s[1] for s in sizes))


def descending(values):
    return all(values[i] >= values[i + 1] for i in range(len(values) - 1))


def example_problems(ex):
    """Every way the example's numbers, order or commands disagree with what the CLI would print."""
    p = []
    if not (ex["classes"] and ex["repos"] and ex["regen"] and ex["items"] and ex["total"]):
        return ["example is missing a section"]
    if not descending([s[0] for _, s in ex["classes"]]):
        p.append("Space by class is not sorted by size, descending")
    if not descending([r["total"][0] for r in ex["repos"]]):
        p.append("repos and worktrees are not sorted by size, descending")
    if not descending([s[0] for _, s in ex["regen"]]):
        p.append("regenerable and cache rows are not sorted by size, descending")
    if not descending([i["size"][0] for i in ex["items"]]):
        p.append("plan items are not sorted by reclaimable size, descending")
    if ex["list_gaps"]:
        p.append("a list is cut short with '...', so its rows cannot be checked against the totals")
    if ex["repo_count"] != len(ex["repos"]):
        p.append(f"Repos and worktrees ({ex['repo_count']}) lists {len(ex['repos'])}")
    if ex["regen_count"] != len(ex["regen"]):
        p.append(f"Regenerable and cache directories ({ex['regen_count']}) lists {len(ex['regen'])}")
    classes = dict(ex["classes"])
    if not close(total(classes.values()), ex["total"]):
        p.append("class totals do not add up to the allocated total")
    regen_classes = [classes[c] for c in ("regenerable", "cache") if c in classes]
    if not close(total(regen_classes), total([s for _, s in ex["regen"]])):
        p.append("regenerable + cache class totals differ from the listed directories")
    for cls in ("repo", "worktree", "bare-repo"):
        rows = [r for r in ex["repos"] if r["class"] == cls]
        if not rows and cls not in classes:
            continue
        if any(r["regen"] is None for r in rows):
            p.append(f"a {cls} row has no sizes line")
            continue
        own = (sum(r["total"][0] - r["regen"][0] for r in rows), sum(r["total"][1] + r["regen"][1] for r in rows))
        if not close(own, classes.get(cls, (0, 0))):
            p.append(f"{cls} class total differs from its rows minus their regenerable directories")
    for r in ex["repos"]:
        inside = [s for path, s in ex["regen"] if path.startswith(r["path"] + "/")]
        if r["regen"] is not None and not close(total(inside), r["regen"]):
            p.append(f"{r['path']}: regenerable size differs from the listed directories inside it")
    sizes = {path: s for path, s in ex["regen"]}
    sizes.update({r["path"]: r["total"] for r in ex["repos"]})
    facts = {r["path"]: r["facts"] for r in ex["repos"]}
    for it in ex["items"]:
        if it["path"] not in sizes:
            p.append(f"plan item {it['path']} is not in the scan")
        elif it["action"] != "keep" and it["action"] != "ask" and not close(it["size"], sizes[it["path"]]):
            p.append(f"plan item {it['path']} reclaims a different size than the scan shows")
        f = facts.get(it["path"])
        if f:
            vals = set(f.values())
            want = "ask" if "UNKNOWN" in vals else ("recover-backup-then-remove" if "NO" in vals else "backup-then-remove")
            if it["action"] not in (want, "keep", "ask"):
                p.append(f"plan item {it['path']} is {it['action']}, its facts give {want}")
    m = re.search(r"--out (\S+)", ex["scan_cmd"])
    n = re.search(r"--scan (\S+)", ex["plan_cmd"])
    if not m or not n or m.group(1) != n.group(1):
        p.append("the plan reads a scan file the example scan did not write with --out")
    return p


def class_table_defaults(html):
    """Class name to the text of its Default cell in the class table."""
    out = {}
    for m in re.finditer(r'<th scope="row" role="rowheader"><span class="dot"[^>]*></span>([^<]+)</th>'
                         r'<td[^>]*>.*?</td><td[^>]*data-label="[^"]*"[^>]*>(.*?)</td>', html):
        out[m.group(1).strip()] = re.sub(r"<[^>]+>", "", m.group(2))
    return out


def css_tokens(html):
    """(light, dark) dicts of custom properties from the inline stylesheet."""
    style = re.search(r"<style>(.*?)</style>", html, re.S).group(1)
    root = re.search(r":root\s*\{(.*?)\}", style, re.S).group(1)
    dark_block = re.search(r"@media \(prefers-color-scheme: dark\)\s*\{\s*:root\s*\{(.*?)\}", style, re.S).group(1)
    pat = re.compile(r"(--[a-z-]+):\s*(#[0-9a-fA-F]{6})\s*;")
    return dict(pat.findall(root)), dict(pat.findall(dark_block))


class TestDetectorsAreNotVacuous(unittest.TestCase):
    """The detectors below must fire on known-bad input, so the real tests cannot pass by being blind."""

    def test_external_detector_fires(self):
        bad = [
            '<img src="https://cdn.example.com/a.png">',
            '<link rel="stylesheet" href="https://fonts.example.com/css">',
            "<style>@import url(https://x.example/a.css);</style>",
            "<style>a{background:url(//x.example/a.png)}</style>",
            '<a href="https://example.com/">x</a>',
        ]
        for snippet in bad:
            self.assertTrue(external_refs(snippet), snippet)
        self.assertFalse(external_refs('<a href="https://github.com/ericmacdougall/disk-cleaner">ok</a>'))
        self.assertFalse(external_refs('<img src="brand/logo.svg">'))

    def test_forbidden_detector_fires(self):
        self.assertEqual(forbidden_hits("Get it at PruneDisk.com now, the FASTEST", ["prunedisk.com", "fastest"]),
                         ["prunedisk.com", "fastest"])
        self.assertEqual(forbidden_hits("nothing here", ["fastest"]), [])

    def test_structure_detector_fires(self):
        c = Collector()
        c.feed('<div id="a"><p id="a"></div>')
        self.assertTrue(c.errors or c.stack)
        self.assertEqual(c.ids, ["a", "a"])

    def test_core_and_example_parsers_fire(self):
        model = ('    pub fn entries(&self) -> [(&\'static str, Tri); 2] {\n        [\n'
                 '            ("has_network_remote", self.has_network_remote),\n'
                 '            ("stash_empty", self.stash_empty),\n        ]\n    }\n')
        self.assertEqual(core_fact_names(model), ["has_network_remote", "stash_empty"])
        self.assertEqual(core_action_names('Action::Keep => "keep",\n Action::Ask => "ask",'), ["keep", "ask"])
        self.assertGreaterEqual(len(core_fact_names()), 7, "real model.rs parsed to too few facts")
        self.assertIn("keep", core_action_names(), "real plan.rs parsed to no actions")
        old = ("Repos and worktrees (2):\n  [repo] ~/a  1.0 GiB\n      has_network_remote=yes stash_empty=NO\n"
               "      .git 1 B, regenerable 0 B, outputs 0 B\n  [worktree] ~/b  1 B\n      ...\n"
               "Items:\n  recover-then-archive    0 B     [repo] ~/a\n")
        self.assertEqual(example_fact_groups(old), [["has_network_remote", "stash_empty"]])
        self.assertEqual(example_actions(old), ["recover-then-archive"])

    def test_example_checker_fires(self):
        good = ("$ disk-cleaner scan ~/p --out s.json\nScanned ~/p\n3 files, 2 directories, 3.0 GiB allocated (3.0 GiB logical)\n\n"
                "Space by class (innermost classified ancestor):\n  regenerable     2.0 GiB\n  repo            1.0 GiB\n\n"
                "Repos and worktrees (1):\n  [repo] ~/p/a  3.0 GiB\n      has_network_remote=yes stash_empty=NO\n"
                "      .git 1.0 MiB, regenerable 2.0 GiB, outputs 0 B\n\n"
                "Regenerable and cache directories (1):\n     2.0 GiB  ~/p/a/node_modules\n\n"
                "$ disk-cleaner plan --scan s.json\nItems:\n  recover-backup-then-remove     3.0 GiB  [repo] ~/p/a\n")
        self.assertEqual(example_problems(parse_example(good)), [])
        bad = {
            "unsorted": good.replace("  regenerable     2.0 GiB\n  repo            1.0 GiB",
                                     "  repo            1.0 GiB\n  regenerable     2.0 GiB"),
            "no --out": good.replace(" --out s.json", ""),
            "wrong count": good.replace("Repos and worktrees (1)", "Repos and worktrees (3)"),
            "cut short": good.replace("     2.0 GiB  ~/p/a/node_modules\n", "     2.0 GiB  ~/p/a/node_modules\n  ...\n"),
            "class sum": good.replace("  repo            1.0 GiB", "  repo            1.9 GiB"),
            "wrong action": good.replace("stash_empty=NO", "stash_empty=UNKNOWN"),
            "size mismatch": good.replace("3.0 GiB  [repo]", "88.0 MiB  [repo]"),
        }
        for name, text in bad.items():
            self.assertTrue(example_problems(parse_example(text)), name)

    def test_permissions_and_pin_parsers_fire(self):
        yml = "permissions:\n  contents: read\n  issues: write\njobs:\n  a:\n    steps:\n      - uses: actions/x@v4\n"
        self.assertEqual(top_level_permissions(yml), {"contents": "read", "issues": "write"})
        self.assertFalse(re.fullmatch(r".+@[0-9a-f]{40}", action_refs(yml)[0]))


class TestPageStructure(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.page = parse_page()
        cls.html = read(INDEX)

    def test_tags_balance(self):
        self.assertEqual(self.page.errors, [], "mismatched closing tags")
        self.assertEqual(self.page.stack, [], "unclosed tags")

    def test_no_duplicate_ids(self):
        dupes = sorted({i for i in self.page.ids if self.page.ids.count(i) > 1})
        self.assertEqual(dupes, [])

    def test_exactly_one_h1(self):
        self.assertEqual(self.page.headings.count(1), 1)

    def test_no_skipped_heading_levels(self):
        prev = 0
        for h in self.page.headings:
            self.assertLessEqual(h, prev + 1, f"h{h} follows h{prev}")
            prev = h

    def test_html_lang(self):
        self.assertTrue(self.page.html_attrs.get("lang"), "missing lang on <html>")

    def test_viewport_meta(self):
        v = [m for m in self.page.metas if m.get("name") == "viewport"]
        self.assertEqual(len(v), 1)
        self.assertIn("width=device-width", v[0].get("content", ""))

    def test_title(self):
        self.assertTrue(self.page.title.strip())
        self.assertIn("PruneDisk", self.page.title)

    def test_meta_description(self):
        d = [m for m in self.page.metas if m.get("name") == "description"]
        self.assertEqual(len(d), 1)
        text = d[0].get("content", "")
        self.assertTrue(text)
        self.assertLessEqual(len(text), 155, len(text))
        self.assertTrue(text.startswith("Early"), "description must lead with the status")

    def test_open_graph_tags(self):
        props = {m.get("property"): m.get("content", "") for m in self.page.metas if m.get("property")}
        for key in ("og:type", "og:title", "og:description", "og:site_name"):
            self.assertTrue(props.get(key), f"missing {key}")
        self.assertTrue(props["og:description"].startswith("Early"))
        self.assertNotIn("og:url", props, "no og:url until a real address exists")
        self.assertNotIn("og:image", props, "no og:image until a real image exists")

    def test_landmarks(self):
        names = [t for t, _ in self.page.tags]
        for t in ("header", "nav", "main", "footer"):
            self.assertEqual(names.count(t), 1, t)

    def test_in_page_anchors_resolve(self):
        missing = [a for a in self.page.anchors if a not in self.page.ids]
        self.assertEqual(missing, [])
        self.assertTrue(self.page.anchors, "expected in-page anchors")

    def test_images_and_svgs_have_text_alternatives(self):
        for img in self.page.imgs:
            self.assertIn("alt", img, f"img without alt: {img}")
        for svg in self.page.svgs:
            a = svg["attrs"]
            ok = (a.get("aria-hidden") == "true" or a.get("aria-label") or a.get("aria-labelledby") or svg["has_title"])
            self.assertTrue(ok, f"svg without a text alternative: {a}")
        self.assertTrue(self.page.svgs)

    def test_pre_has_role_and_label(self):
        pres = [d for t, d in self.page.tags if t == "pre"]
        self.assertTrue(pres)
        for d in pres:
            self.assertEqual(d.get("role"), "region")
            self.assertTrue(d.get("aria-label"))
            self.assertIn("tabindex", d)

    def test_reduced_motion_and_dark_scheme_present(self):
        self.assertIn("prefers-reduced-motion", self.html)
        self.assertIn("prefers-color-scheme: dark", self.html)


class TestNoExternalResources(unittest.TestCase):
    def test_no_script_elements(self):
        c = parse_page()
        self.assertEqual([t for t, _ in c.tags if t == "script"], [])

    def test_no_external_resources_in_page(self):
        self.assertEqual(external_refs(read(INDEX)), [])

    def test_no_external_resources_in_svgs(self):
        for f in sorted((SITE / "brand").glob("*.svg")):
            self.assertEqual(external_refs(read(f)), [], f.name)


class TestContent(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.html = read(INDEX)
        cls.text = re.sub(r"<[^>]+>", " ", cls.html)

    def test_forbidden_strings_absent(self):
        bad = ["prunedisk.com", "prunedisk.app", "fastest", "download now", "clean your disk"]
        self.assertEqual(forbidden_hits(self.html, bad), [])

    def test_british_spellings_absent(self):
        bad = ["colour", "licence", "judgement", "analyser", "organisation", "behaviour", "favour", "centre"]
        for f in (INDEX, README, BRAND_MD):
            self.assertEqual(forbidden_hits(read(f), bad), [], f.name)

    def test_status_wording_present(self):
        flat = re.sub(r"\s+", " ", self.text)
        self.assertIn("Early.", flat)
        self.assertIn("It deletes nothing", flat)
        self.assertRegex(flat, r"read-only")
        self.assertIn("merged into the main branch", flat)
        self.assertIn("releases page", flat)

    def test_no_stale_review_claims(self):
        # The scanner and planner were merged into main; copy written while they were on a draft pull
        # request must not come back.
        flat = re.sub(r"\s+", " ", self.text).lower()
        stale = ["in review", "review branch", "open draft pull request, not merged",
                 "open pull request for the scanner", "still in review"]
        self.assertEqual([s for s in stale if s in flat], [])

    def test_unshipped_parts_are_planned(self):
        rows = {}
        for m in re.finditer(r'<th scope="row"[^>]*>([^<]+)</th><td[^>]*data-label="Status"[^>]*>'
                             r'<span class="badge">([^<]+)</span>', self.html):
            rows[m.group(1).strip()] = m.group(2).strip()
        for part in ("Recovery, backup, apply, restore", "Desktop app", "App stores and installers"):
            self.assertEqual(rows.get(part), "Planned", part)
        self.assertNotEqual(rows.get("CLI: scan and plan"), "Planned")
        self.assertNotIn("Released", rows.values(), "nothing is released yet")

    def test_no_download_button_or_install_claim(self):
        self.assertNotRegex(self.html, r'(?i)<a[^>]*class="btn[^"]*"[^>]*>[^<]*download')
        self.assertIn("no installer", re.sub(r"\s+", " ", self.text))

    def test_repo_and_command_still_disk_cleaner(self):
        flat = re.sub(r"\s+", " ", self.text)
        self.assertIn("repository is still named disk-cleaner", flat)
        self.assertIn("command is still called", flat)
        self.assertIn("$ disk-cleaner scan", self.html)
        self.assertIn("https://github.com/ericmacdougall/disk-cleaner", self.html)

    def test_platforms_are_stated_as_targets(self):
        flat = re.sub(r"\s+", " ", self.text)
        self.assertIn("target platforms", flat)
        # Which machines it was run on by hand is not recorded in the repository, so the page cannot
        # vouch for it either way; CI coverage is checked against ci.yml instead.
        self.assertNotIn("tested by hand", flat)
        self.assertIn("automated build and tests run on all three", flat)
        ci = read(CI_YML)
        self.assertIn("os: [ubuntu-latest, macos-latest, windows-latest]", ci)

    def test_status_note_names_the_command(self):
        note = re.search(r'<p class="status-note">(.*?)</p>', self.html, re.S).group(1)
        self.assertIn("disk-cleaner", note, "the first thing a reader sees must give the real command name")

    def test_rename_is_planned_not_promised(self):
        for path in (INDEX, ROOT_README):
            self.assertNotRegex(re.sub(r"\s+", " ", read(path)), r"(?i)will be renamed", path.name)

    def test_plan_is_described_as_printed_today(self):
        flat = re.sub(r"\s+", " ", self.text)
        self.assertNotIn("A plan is a file you can read and edit", flat,
                         "today plan prints to stdout; editing and approving a plan is design")
        card = re.search(r"<h3>Plan, then apply</h3><p>(.*?)</p>", self.html).group(1)
        self.assertIn("--json", card)
        self.assertRegex(card, r"(?i)planned|design|when it exists")

    def test_unknown_is_not_called_keep(self):
        flat = re.sub(r"\s+", " ", self.text)
        self.assertNotRegex(flat, r"(?i)times out means keep",
                            "the plan answers an unknown fact with ask, not with the keep action")
        self.assertNotRegex(re.sub(r"\s+", " ", read(ROOT_README)), r'(?i)counts as "keep"')
        # The brand guide's voice examples are copied into new copy, so they must not teach the old rule.
        old = r'(?i)unknown means keep|(?:fails|times out)[^.|]*(?:means|counts as) "?keep'
        for path in (INDEX, ROOT_README, BRAND_MD):
            self.assertNotRegex(re.sub(r"\s+", " ", read(path)), old, path.name)
        # plan.rs: an idle repo with an unknown fact gets ask; the brand guide says so.
        self.assertRegex(read(CORE_PLAN), r"facts\.any_unknown\(\) \{\s*\(\s*Action::Ask")
        brand = re.sub(r"\s+", " ", read(BRAND_MD))
        self.assertRegex(brand, r"Unknown never counts as yes[^|]*\bask")

    def test_skip_worktree_case_is_named(self):
        para = re.search(r"all eleven are yes.*?</p>", re.sub(r"\s+", " ", self.html)).group(0)
        self.assertIn("skip-worktree", para, "plan.rs asks when files are hidden from git, even if all facts are yes")
        self.assertIn("skip-worktree", read(CORE_PLAN))

    def test_only_the_hidden_files_case_asks_with_every_fact_yes(self):
        # Unknown facts, dependents, an unusual layout and the git directory's own files are each a
        # fact that is not yes, so only the skip-worktree/assume-unchanged case can ask with all yes.
        para = re.sub(r"<[^>]+>", "", re.search(r"all eleven are yes.*?</p>", re.sub(r"\s+", " ", self.html)).group(0))
        sentences = [s for s in re.split(r"(?<=[.])\s", para) if re.search(r"(?i)every fact is yes|all facts are yes", s)]
        self.assertEqual(len(sentences), 1, sentences)
        s = sentences[0]
        self.assertIn("skip-worktree", s)
        for other in ("unknown", "dependents", "layout", "of its own"):
            self.assertNotIn(other, s, f"{other!r} cannot occur with every fact yes")

    def test_pending_state_bullet_names_what_the_check_covers(self):
        facts = read(CORE_FACTS)
        for needle in ('"REVERT_HEAD"', '"lfs/objects"', '"annex/objects"'):
            self.assertIn(needle, facts)
        bullet = re.search(r"<li>No rebase[^<]*</li>", self.html).group(0)
        for word in ("rebase", "merge", "cherry-pick", "revert", "bisect", "Git LFS", "git-annex"):
            self.assertIn(word, bullet, word)

    def test_example_intro_says_what_is_left_out(self):
        self.assertIn('"\\nBiggest files:"', read(CLI_REPORT), "report.rs prints a Biggest files section")
        self.assertIn('facts.join(" ")', read(CLI_REPORT), "report.rs prints the facts on one line")
        intro = re.sub(r"\s+", " ", re.search(r'<h2 id="h-example">.*?</h2>\s*<p>(.*?)</p>', self.html, re.S).group(1))
        self.assertRegex(intro, r"(?i)biggest files")
        self.assertRegex(intro, r"(?i)wrapped")
        self.assertNotIn("Biggest files:", example_text(self.html), "if the example printed the section, say so")

    def test_example_repo_with_a_linked_worktree_has_dependents(self):
        # facts.rs: a repository whose `git worktree list` shows other worktrees gets no_dependents=NO.
        self.assertRegex(read(CORE_FACTS), r"info\.worktrees = others.*\n\s*Tri::from_bool\(has_none\)")
        intro = re.sub(r"\s+", " ", re.search(r'<h2 id="h-example">.*?</h2>\s*<p>(.*?)</p>', self.html, re.S).group(1))
        intro = re.sub(r"<[^>]+>", "", intro)
        pairs = re.findall(r"(\S+) is a linked worktree of (\S+?)[,.;]", intro)
        self.assertTrue(pairs, "the intro must say which repo the example's worktree belongs to")
        ex = parse_example(example_text(self.html))
        names = {r["path"].rsplit("/", 1)[-1]: r for r in ex["repos"]}
        for wt, parent in pairs:
            self.assertEqual(names[wt]["class"], "worktree", wt)
            self.assertEqual(names[parent]["class"], "repo", parent)
            self.assertEqual(names[parent]["facts"].get("no_dependents"), "NO",
                             f"{parent} has the linked worktree {wt}, so the scanner prints no_dependents=NO")
        for name, r in names.items():
            if r["class"] == "repo" and r["facts"].get("no_dependents") == "yes":
                self.assertFalse([w for w, p in pairs if p == name], name)

    def test_download_wording_matches_ci(self):
        flat = re.sub(r"\s+", " ", self.text)
        # Written to be true before and after the first release exists: it points at the releases page
        # and says what to do when that page lists nothing.
        self.assertNotIn("no release or download page", flat)
        self.assertNotIn("Merged, not released", flat)
        self.assertIn("lists no release yet", flat)
        for needed in ("not code-signed or notarized", "SHA256SUMS", "gh attestation verify",
                       "Gatekeeper", "SmartScreen", "build from source"):
            self.assertIn(needed, flat)
        self.assertIn("https://github.com/ericmacdougall/disk-cleaner/releases", self.html)
        days = re.search(r"retention-days:\s*(\d+)", read(CI_YML)).group(1)
        readme = re.sub(r"\s+", " ", read(ROOT_README))
        self.assertNotIn("no tagged release, installer or download yet", readme)
        self.assertIn(f"{days} days", readme, "README must state how long CI keeps its binaries")

    def test_fact_count_matches_core(self):
        n = len(core_fact_names())
        flat = re.sub(r"\s+", " ", self.text)
        said = re.findall(r"records (\w+) facts", flat) + re.findall(r"all (\w+) are yes", flat)
        self.assertEqual(len(said), 2, said)
        for word in said:
            self.assertEqual(NUMBER_WORDS.index(word), n, f"page says {word}, core has {n} facts")

    def test_example_lists_the_core_facts(self):
        groups = example_fact_groups(example_text(self.html))
        self.assertTrue(groups, "no repo with facts in the example")
        for g in groups:
            self.assertEqual(g, core_fact_names())

    def test_example_uses_current_action_names(self):
        actions = core_action_names()
        used = example_actions(example_text(self.html))
        self.assertTrue(used, "no plan items in the example")
        self.assertEqual([a for a in used if a not in actions], [])

    def test_example_adds_up_and_is_ordered_like_the_cli(self):
        self.assertEqual(example_problems(parse_example(example_text(self.html))), [])

    def test_removal_rule_is_stated_precisely(self):
        flat = re.sub(r"\s+", " ", self.text)
        self.assertNotRegex(flat, r"(?i)only when all \w+ (?:facts )?are yes",
                            "a repo with some facts no can still be proposed for recover-backup-then-remove")
        para = re.search(r"all eleven are yes.*?</p>", re.sub(r"\s+", " ", self.html)).group(0)
        for needle in ("backup-then-remove", "recover-backup-then-remove", "unknown", "ask"):
            self.assertIn(needle, para)

    def test_class_defaults_say_what_todays_plan_does(self):
        d = class_table_defaults(self.html)
        self.assertEqual(len(d), 9, sorted(d))
        self.assertIn("Today's plan", self.html)
        self.assertIn("remove-regenerable", d["cache"])
        self.assertIn("Planned", d["cache"])
        self.assertIn("keep", d["output"].lower())
        self.assertIn("Planned", d["output"])
        self.assertIn("Planned", d["agent-session"])
        for cls, text in d.items():
            self.assertNotRegex(text, r"(?i)^remove by age", f"{cls}: an age or size policy is not built")
        for cls in ("regenerable", "cache"):
            # An unknown activity state is "not active" too, and plan.rs asks then.
            self.assertNotIn("not active", d[cls], cls)
            self.assertIn("idle", d[cls], cls)
        self.assertRegex(d["regenerable"], r"ask if [^.]*cannot be judged")

    def test_card_headings_do_not_state_planned_features_as_fact(self):
        self.assertNotIn("Everything removed is findable", self.html)
        self.assertIn("Everything removed will be findable", self.html)

    def test_brand_link_goes_to_the_document_top(self):
        page = parse_page()
        tops = [t for t, d in page.tags if d.get("id") == "top"]
        self.assertEqual(tops, ["body"])

    def test_phone_legend_does_not_rely_on_color_alone(self):
        svg = re.search(r'<figure>.*?</figure>', self.html, re.S).group(0)
        tiles = re.findall(r'<rect class="tile-([a-z]+)"', svg)
        keys = re.findall(r'<text class="tile-key"[^>]*>(\d+)</text>', svg)
        self.assertEqual(len(keys), len(tiles), "every tile needs a visible key at phone width")
        legend = re.findall(r'<li><span class="dot"[^>]*></span><span class="key">(\d+)</span>', svg)
        self.assertEqual(sorted(set(keys)), sorted(legend))
        self.assertRegex(self.html, r"@media \(max-width: 44rem\) \{(?:[^{}]*\{[^{}]*\})*?\s*\.tile-key \{ display: inline; \}")
        self.assertRegex(self.html, r"\.tile-key \{ display: none;", "keys only replace the labels on a phone")

    def test_page_weight_under_300_kb(self):
        total = INDEX.stat().st_size + sum(f.stat().st_size for f in (SITE / "brand").glob("*.svg"))
        self.assertLess(total, 300 * 1024, total)


class TestBrand(unittest.TestCase):
    def test_svgs_parse_and_have_viewbox(self):
        svgs = sorted((SITE / "brand").glob("*.svg"))
        self.assertGreaterEqual(len(svgs), 5)
        for f in svgs:
            root = ET.fromstring(read(f))
            self.assertTrue(root.tag.endswith("svg"), f.name)
            self.assertTrue(root.get("viewBox"), f"{f.name} lacks viewBox")

    def test_standalone_logos_do_not_switch_by_scheme(self):
        for name in ("logo.svg", "logo-dark.svg", "logo-mark.svg", "logo-mark-dark.svg"):
            self.assertNotIn("prefers-color-scheme", read(SITE / "brand" / name), name)

    def test_brand_tokens_match_css(self):
        light, dark = css_tokens(read(INDEX))
        rows = {}
        for m in re.finditer(r"^\|\s*`(--[a-z-]+)`\s*\|\s*`(#[0-9a-fA-F]{6})`\s*\|\s*`(#[0-9a-fA-F]{6})`", read(BRAND_MD), re.M):
            rows[m.group(1)] = (m.group(2).lower(), m.group(3).lower())
        for tok in ("--bg", "--surface", "--text", "--muted", "--accent", "--border", "--rule"):
            self.assertIn(tok, rows, f"{tok} missing from BRAND.md")
            self.assertEqual(rows[tok], (light[tok].lower(), dark[tok].lower()), tok)

    def test_class_colors_in_brand_doc_match_css(self):
        light, dark = css_tokens(read(INDEX))
        doc = read(BRAND_MD).lower()
        for tok in [k for k in light if k.startswith("--c-")]:
            self.assertIn(light[tok].lower(), doc, f"light {tok}")
            self.assertIn(dark[tok].lower(), doc, f"dark {tok}")

    def test_logo_colors_use_tokens(self):
        light, dark = css_tokens(read(INDEX))
        logo = read(SITE / "brand" / "logo.svg").lower()
        logo_dark = read(SITE / "brand" / "logo-dark.svg").lower()
        for tok in ("--accent", "--c-regen", "--c-repo"):
            self.assertIn(light[tok].lower(), logo)
            self.assertIn(dark[tok].lower(), logo_dark)


class TestWorkflows(unittest.TestCase):
    def check_pinned(self, path):
        refs = action_refs(read(path))
        self.assertTrue(refs, f"{path.name} uses no actions?")
        for r in refs:
            self.assertRegex(r, r"^[\w./-]+@[0-9a-f]{40}$", f"{path.name}: {r} is not pinned to a 40-character SHA")

    def test_pages_actions_pinned(self):
        self.check_pinned(PAGES_YML)

    def test_site_ci_actions_pinned(self):
        self.check_pinned(SITE_YML)

    def test_pages_permissions_exact(self):
        self.assertEqual(top_level_permissions(read(PAGES_YML)),
                         {"contents": "read", "pages": "write", "id-token": "write"})

    def test_site_ci_permissions_read_only(self):
        self.assertEqual(top_level_permissions(read(SITE_YML)), {"contents": "read"})

    def test_site_ci_triggers_and_command(self):
        y = read(SITE_YML)
        for needle in ("push:", "pull_request:", '"site/**"', ".github/workflows/site.yml",
                       ".github/workflows/pages.yml", "ubuntu-latest", UNIT_CMD):
            self.assertIn(needle, y, needle)

    def test_site_ci_runs_when_a_checked_source_changes(self):
        # These tests read the root README, ci.yml and the core's model and plan sources, so a change to
        # any of them must run them, on push and on pull requests alike.
        y = read(SITE_YML)
        for trigger in ("push", "pull_request"):
            block = re.search(rf"^  {trigger}:\s*\n((?:    .*\n)+)", y, re.M).group(1)
            for path in ("README.md", ".github/workflows/ci.yml", "crates/core/src/model.rs",
                         "crates/core/src/plan.rs", "crates/core/src/facts.rs", "crates/cli/src/report.rs"):
                self.assertIn(f'- "{path}"', block, f"{trigger}: {path}")

    def test_pages_runs_tests_before_upload(self):
        y = read(PAGES_YML)
        self.assertIn(UNIT_CMD, y)
        self.assertLess(y.index(UNIT_CMD), y.index("upload-pages-artifact"))

    def test_both_workflows_call_the_prepare_script(self):
        for path in (PAGES_YML, SITE_YML):
            self.assertIn("python3 site/tools/prepare.py _site", read(path), path.name)
        y = read(PAGES_YML)
        self.assertLess(y.index("site/tools/prepare.py"), y.index("upload-pages-artifact"))
        self.assertNotRegex(y, r"\brm\b", "pages.yml must not prune files itself; prepare.py does")
        self.assertRegex(y, r"upload-pages-artifact@[0-9a-f]{40}\s*\n\s*with:\s*\n\s*path: _site")

    def test_prepare_script_excludes_contributor_files(self):
        import importlib.util
        import tempfile
        spec = importlib.util.spec_from_file_location("prepare", SITE / "tools" / "prepare.py")
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "out"
            mod.prepare(out)
            for gone in ("README.md", "brand/BRAND.md", "tests", "tools"):
                self.assertFalse((out / gone).exists(), gone)
            for kept in ("index.html", "brand/logo.svg", "brand/logo-dark.svg", "brand/favicon.svg"):
                self.assertTrue((out / kept).is_file(), kept)

    def test_rehearsal_job_does_not_deploy(self):
        y = read(SITE_YML)
        self.assertIn("pages-rehearsal:", y)
        self.assertRegex(y, r"pages-rehearsal:\s*\n\s*needs: test")
        self.assertIn("/disk-cleaner", y)
        self.assertIn("site/tools/smoke.py", y)
        self.assertNotIn("deploy-pages", y)
        self.assertNotIn("configure-pages", y)
        for path in ("/README.md", "/brand/BRAND.md", "/tests/test_site.py"):
            self.assertRegex(y, r"expect 404 " + re.escape(path))
        for path in ("/brand/favicon.svg", "/brand/logo.svg", "/brand/logo-dark.svg"):
            self.assertRegex(y, r"expect 200 " + re.escape(path) + r" image/svg\+xml")

    def test_smoke_reference_parser(self):
        import importlib.util
        spec = importlib.util.spec_from_file_location("smoke", SITE / "tools" / "smoke.py")
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        bad = '<link href="brand/x.svg"><a href="#a">a</a><a href="https://e.example/">e</a><img src="/root.png">'
        self.assertEqual(mod.relative_refs(bad), ["/root.png", "brand/x.svg"])
        real = mod.relative_refs(read(INDEX))
        self.assertIn("brand/favicon.svg", real)
        self.assertFalse([r for r in real if r.startswith("/")], "root-relative reference breaks a project subpath")

    def test_pages_does_not_deploy_on_pull_requests(self):
        y = read(PAGES_YML)
        self.assertNotIn("pull_request", y)
        self.assertIn("branches: [main]", y)


GATE_CMD = "python3 -I site/tools/pages_gate.py"


def job_block(yml, name):
    """Text of one job under `jobs:` (two-space indent), up to the next job."""
    m = re.search(rf"^  {re.escape(name)}:\s*\n(.*?)(?=^  \S|^\S|\Z)", yml, re.M | re.S)
    return m.group(1) if m else ""


def job_permissions(block):
    perms, inside = {}, False
    for line in block.splitlines():
        if re.match(r"^    permissions:\s*$", line):
            inside = True
            continue
        if inside:
            m = re.match(r"^      ([a-z-]+):\s*(\S+)\s*$", line)
            if m:
                perms[m.group(1)] = m.group(2)
            else:
                break
    return perms


def load_gate():
    import importlib.util
    spec = importlib.util.spec_from_file_location("pages_gate", SITE / "tools" / "pages_gate.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def load_release_meta():
    import importlib.util
    spec = importlib.util.spec_from_file_location("release_meta", ROOT / ".github" / "scripts" / "release_meta.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


class TestReleaseWorkflow(unittest.TestCase):
    """release.yml: pinned, least privilege, never publishes by itself, one place for the binary name."""

    @classmethod
    def setUpClass(cls):
        cls.y = read(RELEASE_YML)
        cls.notes = read(ROOT / ".github" / "release-notes.md")

    def test_actions_pinned(self):
        refs = action_refs(self.y)
        self.assertTrue(refs)
        for r in refs:
            self.assertRegex(r, r"^[\w./-]+@[0-9a-f]{40}$", f"release.yml: {r} is not pinned to a SHA")

    def test_top_level_permissions_read_only(self):
        self.assertEqual(top_level_permissions(self.y), {"contents": "read"})

    def test_only_the_draft_job_can_write_contents(self):
        writers = re.findall(r"^\s+contents: write", self.y, re.M)
        self.assertEqual(len(writers), 1)
        draft = self.y[self.y.index("  draft-release:"):self.y.index("  crates-dry-run:")]
        self.assertIn("contents: write", draft)
        self.assertIn("needs.meta.outputs.publish == 'true'", draft)
        # The release is made only by release_meta.py create-draft, which always posts draft: true
        # (tested in .github/scripts/test_release_meta.py, TestCreateDraft).
        self.assertIn("release_meta.py create-draft", draft)
        self.assertNotRegex(self.y, r"\bgh release (create|delete|edit)\b")

    def test_rehearsal_never_publishes(self):
        self.assertIn("workflow_dispatch:", self.y)
        self.assertNotIn("--draft=false", self.y)
        self.assertNotIn("cargo publish\n", self.y)
        self.assertNotRegex(self.y, r"cargo publish(?! --dry-run)")

    def test_guards_are_behaviour_not_strings(self):
        # The guards themselves are tested by .github/scripts/test_release_meta.py (real git, every
        # refusal case). Here: the workflow still calls them in a form that can fail the job.
        gate = load_release_meta()
        self.assertEqual(gate.lint_workflow(self.y), [])
        self.assertNotEqual(gate.lint_workflow(self.y.replace("release_meta.py meta\n", "release_meta.py meta || true\n")), [])

    def test_builds_locked_for_the_four_targets(self):
        self.assertIn("cargo build --release --locked", self.y)
        for t in ("x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-pc-windows-msvc"):
            self.assertIn(f"target: {t}", self.y)
            self.assertIn(t, self.notes)

    def test_binary_name_is_set_in_one_place(self):
        self.assertEqual(len(re.findall(r"^\s+BIN_NAME:", self.y, re.M)), 1)
        self.assertEqual(len(re.findall(r"^\s+CLI_PACKAGE:", self.y, re.M)), 1)
        # the name is not typed anywhere else in the file
        rest = re.sub(r"^\s+(BIN_NAME|CLI_PACKAGE):.*$", "", self.y, flags=re.M)
        rest = "\n".join(l for l in rest.splitlines() if not l.lstrip().startswith("#"))
        self.assertNotIn("disk-cleaner", rest)

    def test_attests_and_checksums(self):
        self.assertIn("SHA256SUMS", self.y)
        self.assertIn("actions/attest@", self.y)
        for perm in ("id-token: write", "attestations: write"):
            self.assertIn(perm, self.y)

    def test_notes_say_unsigned_and_how_to_verify(self):
        flat = re.sub(r"\s+", " ", self.notes)
        for needed in ("not code-signed or notarized", "Gatekeeper", "SmartScreen", "SHA256SUMS",
                       "gh attestation verify", "com.apple.quarantine"):
            self.assertIn(needed, flat)
        self.assertTrue(set(re.findall(r"@([A-Z]+)@", self.notes)) <= {"BIN", "TAG", "VERSION", "REPO", "SHA"})

    def test_notes_use_the_brand_name_and_say_the_command_will_be_renamed(self):
        # BRAND.md section 1: display name PruneDisk; copyable names stay disk-cleaner, with one short
        # line saying the command will be renamed.
        first = self.notes.splitlines()[0]
        self.assertIn("PruneDisk", first)
        flat = re.sub(r"\s+", " ", self.notes)
        self.assertRegex(flat, r"still called `@BIN@`; it will be renamed to `prunedisk`")
        self.assertEqual(load_release_meta().RELEASE_TITLE % "v1.2.3", "PruneDisk v1.2.3")

    def test_notes_say_what_the_attestation_does_not_prove(self):
        flat = re.sub(r"\s+", " ", self.notes)
        self.assertIn("It does not show", flat)
        self.assertIn("Every file listed in `SHA256SUMS` is attested", flat)

    def test_desktop_extension_point_is_marked(self):
        self.assertIn("EXTENSION POINT (desktop bundles)", self.y)
        # The claim that every published file type is attested holds only if the attest step reads
        # SHA256SUMS rather than a list of extensions.
        self.assertIn("subject-checksums: dist/SHA256SUMS", self.y)
        self.assertNotRegex(self.y, r"subject-path:")


class TestReleaseWording(unittest.TestCase):
    """README, site and notes must read true before the first release exists and after one is
    published. A tag push only makes a draft that nobody else sees, so no copy may say a tag puts
    files on the releases page, and nothing that needs a release may be written in the present tense."""

    TAG_MAKES_RELEASE = re.compile(
        r"(?i)(when|once|after) a version is tagged|appear on the releases page once|"
        r"attached to [^.]*releases page[^.]* when a version is tagged|releases page carries")
    PRESENT_TENSE_DOWNLOAD = re.compile(
        r"(?i)the program is a single file you unpack and run|"
        r"(?<!, )\bprebuilt (command-line )?programs are (published|attached|available)")

    @classmethod
    def setUpClass(cls):
        cls.texts = {
            "README.md": re.sub(r"\s+", " ", read(ROOT_README)),
            "site/index.html": re.sub(r"\s+", " ", re.sub(r"<[^>]+>", " ", read(INDEX))),
            ".github/release-notes.md": re.sub(r"\s+", " ", read(ROOT / ".github" / "release-notes.md")),
        }

    def test_a_tag_is_not_called_a_release(self):
        for name, flat in self.texts.items():
            self.assertNotRegex(flat, self.TAG_MAKES_RELEASE, name)
            self.assertNotRegex(flat, self.PRESENT_TENSE_DOWNLOAD, name)

    # Sentences that are false inside a published release (the README is packaged in every archive).
    STALE_IN_A_RELEASE = re.compile(
        r"(?i)(?<!lists )\bno (tagged )?release\b|download page yet|installers and releases|not released")

    def packaged_files(self):
        y = read(RELEASE_YML)
        m = re.search(r'^\s*cp "target/\$TARGET/release/\$\{BIN_NAME\}\$\{EXT\}" (.+) "dist/\$name/"\s*$', y, re.M)
        self.assertIsNotNone(m, "release.yml: the staging cp line changed; update this test")
        return m.group(1).split()

    def test_everything_packaged_reads_true_in_a_release(self):
        files = self.packaged_files()
        self.assertIn("README.md", files)
        for rel in files:
            flat = re.sub(r"\s+", " ", read(ROOT / rel))
            self.assertNotRegex(flat, self.STALE_IN_A_RELEASE, rel + " is packaged in every release archive")

    def test_readme_and_site_say_a_tag_makes_a_draft(self):
        for name in ("README.md", "site/index.html"):
            flat = self.texts[name]
            self.assertIn("Tagging a version only prepares a draft", flat, name)
            self.assertIn("Once the maintainer publishes a release, prebuilt", flat, name)
            self.assertIn("lists no release yet", flat, name)

    def test_verification_is_not_oversold(self):
        for name in ("README.md", "site/index.html"):
            flat = self.texts[name]
            self.assertNotIn("shows which repository and workflow built", flat, name)
            self.assertIn("It does not show the code is free of bugs", flat, name)

    def test_site_lists_the_same_platforms(self):
        flat = re.sub(r"\s+", " ", read(INDEX))
        for p in ("Linux (x86-64)", "macOS (Apple Silicon and Intel)", "Windows (x86-64)"):
            self.assertIn(p, flat)


class TestPagesGate(unittest.TestCase):
    """The deploy job runs only when Pages is enabled with GitHub Actions as its source. Pages being off
    skips the deploy and the run stays green; any other answer from the API fails, so a broken token or
    an outage is never mistaken for "Pages is off"."""

    def test_job_parsers_fire(self):
        yml = ("jobs:\n  a:\n    permissions:\n      pages: read\n    steps: []\n"
               "  b:\n    needs: a\n    if: x\n")
        self.assertEqual(job_permissions(job_block(yml, "a")), {"pages": "read"})
        self.assertIn("needs: a", job_block(yml, "b"))
        self.assertEqual(job_block(yml, "c"), "")

    def test_decision_for_each_answer(self):
        gate = load_gate()
        on = gate.decide(200, b'{"build_type": "workflow", "html_url": "https://x.example/p/"}')
        self.assertTrue(on.enabled)
        off = gate.decide(404, b'{"message": "Not Found"}')
        self.assertFalse(off.enabled)
        self.assertEqual(off.level, "notice")
        legacy = gate.decide(200, b'{"build_type": "legacy"}')
        self.assertFalse(legacy.enabled)
        self.assertEqual(legacy.level, "warning")
        for status, body in ((401, b"{}"), (403, b'{"message": "Resource not accessible"}'),
                             (500, b""), (200, b"not json"), (200, b"[]"), (302, b"")):
            with self.assertRaises(gate.GateError, msg=f"{status} {body!r}"):
                gate.decide(status, body)

    def run_main(self, fetch, env):
        import contextlib
        import io
        import os
        import tempfile
        gate = load_gate()
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "output"
            out.write_text("", encoding="utf-8")
            full = {"GITHUB_REPOSITORY": "o/r", "GITHUB_TOKEN": "tok-DO-NOT-PRINT", "GITHUB_OUTPUT": str(out)}
            full.update(env)
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf), contextlib.redirect_stderr(buf):
                code = gate.main(env={k: v for k, v in full.items() if v is not None}, fetch=fetch)
            return code, out.read_text(encoding="utf-8"), buf.getvalue()

    def test_main_writes_the_output_and_exit_code(self):
        import urllib.error
        code, out, log = self.run_main(lambda url, token: (404, b'{"message": "Not Found"}'), {})
        self.assertEqual((code, out.strip()), (0, "enabled=false"))
        self.assertIn("::notice", log)
        code, out, log = self.run_main(lambda url, token: (200, b'{"build_type": "workflow"}'), {})
        self.assertEqual((code, out.strip()), (0, "enabled=true"))
        code, out, log = self.run_main(lambda url, token: (403, b"{}"), {})
        self.assertEqual((code, out.strip()), (1, ""))
        self.assertIn("::error", log)

        def down(url, token):
            raise urllib.error.URLError("no route")
        code, out, log = self.run_main(down, {})
        self.assertEqual((code, out.strip()), (1, ""))
        code, out, log = self.run_main(lambda url, token: (404, b""), {"GITHUB_TOKEN": None})
        self.assertEqual(code, 1, "no token must fail, not skip")
        code, out, log = self.run_main(lambda url, token: (404, b""), {"GITHUB_REPOSITORY": "../x"})
        self.assertEqual(code, 1, "a malformed repository name must fail")
        code, out, log = self.run_main(lambda url, token: (404, b""), {"GITHUB_API_URL": "http://api.example.test"})
        self.assertEqual(code, 1, "the token is only sent over https")

    def test_main_asks_the_right_url_and_never_prints_the_token(self):
        seen = []

        def fetch(url, token):
            seen.append((url, token))
            return 403, b'{"message": "Bad credentials"}'
        code, out, log = self.run_main(fetch, {"GITHUB_API_URL": "https://api.example.test"})
        self.assertEqual(seen, [("https://api.example.test/repos/o/r/pages", "tok-DO-NOT-PRINT")])
        self.assertNotIn("tok-DO-NOT-PRINT", log + out)

    def test_real_fetch_does_not_follow_redirects_or_leak_the_token(self):
        # The real fetch() against a throwaway server on 127.0.0.1: a redirect comes back as its own
        # status (so decide() fails) and the redirect target never sees the request or the token.
        import http.server
        import threading
        seen = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                seen.append((self.path, self.headers.get("Authorization")))
                if self.path == "/redirect":
                    self.send_response(302)
                    self.send_header("Location", "/target")
                    self.end_headers()
                elif self.path == "/denied":
                    body = b'{"message": "Resource not accessible"}'
                    self.send_response(403)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                else:
                    body = b'{"build_type": "workflow"}'
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)

            def log_message(self, *args):
                pass

        server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            gate = load_gate()
            base = f"http://127.0.0.1:{server.server_address[1]}"
            status, _ = gate.fetch(base + "/redirect", "tok-x")
            self.assertEqual(status, 302)
            self.assertEqual([p for p, _ in seen], ["/redirect"], "the redirect must not be followed")
            with self.assertRaises(gate.GateError):
                gate.decide(status, b"")
            self.assertEqual(gate.fetch(base + "/denied", "tok-x"), (403, b'{"message": "Resource not accessible"}'))
            self.assertEqual(gate.fetch(base + "/ok", "tok-x"), (200, b'{"build_type": "workflow"}'))
            self.assertTrue(all(auth == "Bearer tok-x" for _, auth in seen), seen)
            self.assertGreater(gate.TIMEOUT, 0)
        finally:
            server.shutdown()
            server.server_close()

    def test_pages_deploy_is_gated_at_job_level(self):
        y = read(PAGES_YML)
        gate, deploy = job_block(y, "gate"), job_block(y, "deploy")
        self.assertIn(GATE_CMD, gate)
        self.assertEqual(job_permissions(gate), {"contents": "read", "pages": "read"})
        self.assertRegex(gate, r"outputs:\s*\n\s*enabled: \$\{\{ steps\.gate\.outputs\.enabled \}\}")
        self.assertRegex(gate, r"id: gate")
        self.assertRegex(deploy, re.compile(r"^    needs: gate\s*$", re.M))
        self.assertRegex(deploy, re.compile(r"^    if: needs\.gate\.outputs\.enabled == 'true'\s*$", re.M))
        self.assertIn("deploy-pages", deploy)
        self.assertNotIn("deploy-pages", gate)
        self.assertNotIn("environment", gate, "the gate must not touch the github-pages environment")
        self.assertLess(y.index("  gate:"), y.index("  deploy:"))

    def test_pull_requests_run_the_same_gate_read_only(self):
        y = read(SITE_YML)
        job = job_block(y, "pages-gate")
        self.assertIn(GATE_CMD, job)
        self.assertEqual(job_permissions(job), {"contents": "read", "pages": "read"})
        self.assertNotIn("environment", job)

    def test_gate_script_is_not_published(self):
        import tempfile
        spec_mod = load_gate()
        self.assertTrue(callable(spec_mod.decide))
        import importlib.util
        spec = importlib.util.spec_from_file_location("prepare", SITE / "tools" / "prepare.py")
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        with tempfile.TemporaryDirectory() as tmp:
            out = mod.prepare(Path(tmp) / "out")
            self.assertFalse((out / "tools" / "pages_gate.py").exists())


if __name__ == "__main__":
    unittest.main()
