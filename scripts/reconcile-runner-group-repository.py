#!/usr/bin/env python3
"""Grant one repository access to an existing GitHub Actions runner group."""

import argparse
import json
import subprocess
ORGANIZATION = "wisent-ai"


def request(gh_bin: str, method: str, path: str) -> dict:
    result = subprocess.run(
        [gh_bin, "api", "--method", method, path],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout) if result.stdout else {}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("group")
    parser.add_argument("repository")
    parser.add_argument("--gh-bin", default="gh")
    args = parser.parse_args()

    groups = request(args.gh_bin, "GET", f"/orgs/{ORGANIZATION}/actions/runner-groups")
    group = next(
        (candidate for candidate in groups.get("runner_groups", []) if candidate.get("name") == args.group),
        None,
    )
    if group is None:
        raise SystemExit(f"runner group {args.group!r} does not exist")

    repository = request(args.gh_bin, "GET", f"/repos/{ORGANIZATION}/{args.repository}")
    request(
        args.gh_bin,
        "PUT",
        f"/orgs/{ORGANIZATION}/actions/runner-groups/{group['id']}/repositories/{repository['id']}",
    )
    print(json.dumps({"group": args.group, "repository": args.repository, "status": "granted"}))


if __name__ == "__main__":
    main()
