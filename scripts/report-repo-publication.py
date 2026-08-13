#!/usr/bin/env python3
"""Report whether the repositories named on the command line are published.

"Committed" and "pushed" are different facts, and a working tree can be clean
while the branch is ahead of its remote -- the state that reads as finished and
is not. One row per repository: the branch, the tip, how many commits are ahead
of the tracked remote, and how many paths are dirty or untracked.

Read-only: runs no fetch, mutates nothing, and prints no file contents. Exits
non-zero when any named repository is ahead of its upstream or is not a git
worktree, so a caller can use it as a publication check rather than a report to
read by eye.

Usage: report-repo-publication.py <repo-path> [repo-path...]
"""
import pathlib
import subprocess
import sys

FIRST = len(["argv0"])
EMPTY = len([])
FIELD = len("repository-column-padding-")


def git(repo, *arguments):
    result = subprocess.run(
        ["git", "-C", str(repo), *arguments],
        capture_output=True,
        text=True,
        check=False,
    )
    return result.stdout.strip() if result.returncode == EMPTY else ""


def row(*cells):
    print("".join(cell.ljust(FIELD) for cell in cells).rstrip())


def main():
    repositories = sys.argv[FIRST:]
    if not repositories:
        print(f"usage: {pathlib.Path(sys.argv[EMPTY]).name} <repo-path> [repo-path...]", file=sys.stderr)
        return FIRST

    row("REPOSITORY", "BRANCH", "TIP", "AHEAD", "DIRTY")
    failures = EMPTY
    for raw in repositories:
        repo = pathlib.Path(raw).expanduser()
        name = repo.name
        if not (repo / ".git").exists():
            row(name, "-", "-", "-", "not a git worktree")
            failures += FIRST
            continue

        branch = git(repo, "branch", "--show-current") or "(detached)"
        tip = git(repo, "rev-parse", "--short", "HEAD") or "-"
        upstream = git(repo, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}")
        if not upstream:
            # A branch with no configured upstream is not necessarily unpublished;
            # it is usually one that was pushed before tracking was set. The
            # remote-tracking ref answers that without touching the network, and
            # reporting "no-upstream" for a branch that is level with its remote
            # is a false alarm an operator then has to disprove by hand.
            candidate = f"origin/{branch}"
            if git(repo, "rev-parse", "--verify", "--quiet", candidate):
                upstream = candidate
        if upstream:
            ahead = git(repo, "rev-list", "--count", f"{upstream}..HEAD") or "?"
        else:
            ahead = "no-upstream"
        dirty = str(len(git(repo, "status", "--porcelain").splitlines()))

        row(name, branch, tip, ahead, dirty)
        if ahead not in {str(EMPTY), "no-upstream"}:
            failures += FIRST

    return FIRST if failures else EMPTY


sys.exit(main())
