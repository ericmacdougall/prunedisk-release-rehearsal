"""Check that every relative href/src of a served page resolves under the same path prefix.

    python3 site/tools/smoke.py http://127.0.0.1:8080/disk-cleaner/

Project Pages sites are served from a subpath, so root-relative references would break; they fail here.
Prints one status line per reference and exits 1 on any failure.
"""
import re
import sys
import urllib.error
import urllib.request
from html.parser import HTMLParser
from urllib.parse import urljoin, urlparse


class Refs(HTMLParser):
    def __init__(self):
        super().__init__()
        self.refs = []

    def handle_starttag(self, tag, attrs):
        for k, v in attrs:
            if k in ("href", "src") and v:
                self.refs.append(v)


def relative_refs(html):
    p = Refs()
    p.feed(html)
    return sorted({r for r in p.refs if not (r.startswith("#") or urlparse(r).scheme or r.startswith("//"))})


def status(url):
    try:
        with urllib.request.urlopen(url, timeout=10) as r:
            return r.status
    except urllib.error.HTTPError as e:
        return e.code


def main(base):
    if not base.endswith("/"):
        base += "/"
    with urllib.request.urlopen(base, timeout=10) as r:
        html = r.read().decode("utf-8")
    bad = []
    if re.search(r'(?:href|src)="/(?!/)', html):
        bad.append("root-relative reference")
    refs = relative_refs(html)
    if not refs:
        bad.append("no relative references found (parser broken?)")
    for ref in refs:
        code = status(urljoin(base, ref))
        print(f"{code} {urlparse(base).path}{ref}")
        if code != 200:
            bad.append(ref)
    if bad:
        print("FAILED:", bad)
        sys.exit(1)
    print(f"all {len(refs)} relative references resolve with 200")


if __name__ == "__main__":
    main(sys.argv[1])
