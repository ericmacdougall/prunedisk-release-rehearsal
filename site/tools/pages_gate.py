"""Decide whether the Pages deploy may run. Used by the gate job in pages.yml, and by the pages-gate job
in site.yml so pull requests show the same decision.

    python3 -I site/tools/pages_gate.py

Asks the GitHub API whether Pages is enabled for $GITHUB_REPOSITORY (GET /repos/<owner>/<repo>/pages,
with $GITHUB_TOKEN, which needs only `pages: read`) and writes `enabled=true` or `enabled=false` to
$GITHUB_OUTPUT (or prints it when that is unset).

- 200 with build_type "workflow": enabled=true, the deploy runs.
- 404: Pages is not enabled. enabled=false, the deploy is skipped and the run stays green.
- 200 with another build_type (Pages publishes from a branch): enabled=false with a warning, because
  this workflow is not how that site is published.
- Anything else (401, 403, 5xx, a redirect, a body that is not a JSON object, a network error, a missing
  token or repository name): exit 1. A broken token or an outage is never read as "Pages is off".

The token is sent only in the Authorization header and is never printed.
"""
import json
import os
import re
import sys
import urllib.error
import urllib.request
from collections import namedtuple

Decision = namedtuple("Decision", "enabled level message")
REPO = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
TIMEOUT = 20


class GateError(Exception):
    pass


def decide(status, body):
    if status == 404:
        return Decision(False, "notice",
                        "GitHub Pages is not enabled for this repository, so the deploy is skipped. "
                        "A maintainer enables it in Settings > Pages with Source: GitHub Actions.")
    if status != 200:
        raise GateError(f"the Pages API answered HTTP {status}; refusing to guess whether Pages is enabled")
    try:
        info = json.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, ValueError):
        raise GateError("the Pages API answered 200 with a body that is not JSON") from None
    if not isinstance(info, dict):
        raise GateError("the Pages API answered 200 with JSON that is not an object")
    build_type = info.get("build_type")
    if build_type == "workflow":
        return Decision(True, "notice", "GitHub Pages is enabled with GitHub Actions as its source; deploying.")
    return Decision(False, "warning",
                    f"GitHub Pages is enabled but publishes with build_type {build_type!r}, not from this "
                    "workflow, so the deploy is skipped. Set Settings > Pages > Source to GitHub Actions "
                    "to publish site/ with pages.yml.")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect is answered as its own status (and so fails the gate) instead of being followed with
    the token in its Authorization header."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def fetch(url, token):
    """(status, body) of a GET. HTTP errors are returned as their status; network errors raise."""
    req = urllib.request.Request(url, headers={
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {token}",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "pages-gate",
    })
    opener = urllib.request.build_opener(NoRedirect)
    try:
        with opener.open(req, timeout=TIMEOUT) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def main(env=None, fetch=fetch):
    env = os.environ if env is None else env
    repo, token = env.get("GITHUB_REPOSITORY", ""), env.get("GITHUB_TOKEN", "")
    api = env.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")
    if not REPO.match(repo) or ".." in repo:
        print(f"::error::pages gate: GITHUB_REPOSITORY is missing or malformed ({repo!r})")
        return 1
    if not token:
        print("::error::pages gate: GITHUB_TOKEN is not set")
        return 1
    if not api.startswith("https://"):
        print(f"::error::pages gate: GITHUB_API_URL must be https ({api!r})")
        return 1
    url = f"{api}/repos/{repo}/pages"
    try:
        status, body = fetch(url, token)
        d = decide(status, body)
    except GateError as e:
        print(f"::error::pages gate: {e} ({url})")
        return 1
    except (urllib.error.URLError, OSError) as e:
        print(f"::error::pages gate: cannot reach {url}: {e}")
        return 1
    print(f"::{d.level}::{d.message}")
    line = f"enabled={'true' if d.enabled else 'false'}"
    out = env.get("GITHUB_OUTPUT")
    if out:
        with open(out, "a", encoding="utf-8") as f:
            f.write(line + "\n")
    print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
