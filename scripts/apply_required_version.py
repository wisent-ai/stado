#!/usr/bin/env python3
"""Apply the exact version required by a failed release gate."""

from pathlib import Path
import re
import subprocess
import sys


def main() -> None:
    _, run_id = sys.argv
    root = Path(__file__).resolve().parent.parent
    report = subprocess.run(
        [
            "/opt/homebrew/bin/gh",
            "run",
            "view",
            run_id,
            "--repo",
            "wisent-ai/stado",
            "--log-failed",
        ],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    required_versions = set(
        re.findall(
            r"Cargo\.toml declares [^,\n]+, but [^\n]+ requires "
            r"(?P<version>[0-9]+(?:\.[0-9]+)+)",
            report,
        )
    )
    if len(required_versions) != 1:
        raise SystemExit(
            "release gate did not report exactly one candidate-required version"
        )
    required = required_versions.pop()
    subprocess.run(
        ["cargo", "set-version", required],
        cwd=root / "stado-rs",
        check=True,
    )
    print(f"applied AutoVersion-required release version {required}")


if __name__ == "__main__":
    main()
