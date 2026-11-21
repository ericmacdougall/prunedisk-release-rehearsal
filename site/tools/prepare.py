"""Prepare the publishable copy of site/. Used by both pages.yml (deploy) and site.yml (rehearsal),
so what is rehearsed is exactly what is deployed.

    python3 site/tools/prepare.py <output-dir>

The output directory is created fresh. Contributor files are left out of it.
"""
import shutil
import sys
from pathlib import Path

SITE = Path(__file__).resolve().parents[1]
EXCLUDED = ("README.md", "brand/BRAND.md", "tests", "tools")


def prepare(out):
    out = Path(out)
    if out.exists():
        shutil.rmtree(out)
    shutil.copytree(SITE, out, ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
    for rel in EXCLUDED:
        target = out / rel
        if target.is_dir():
            shutil.rmtree(target)
        elif target.exists():
            target.unlink()
    left = [rel for rel in EXCLUDED if (out / rel).exists()]
    if left or not (out / "index.html").is_file():
        raise SystemExit(f"prepare failed: left over {left}, index.html present={(out / 'index.html').is_file()}")
    return out


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: prepare.py <output-dir>")
    dest = prepare(sys.argv[1])
    print(f"prepared {dest} without {', '.join(EXCLUDED)}")
