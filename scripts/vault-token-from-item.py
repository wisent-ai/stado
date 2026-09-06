#!/usr/bin/env python3
"""Run Skarbiec token-mint with an existing owner-vault field, on its own host."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def invoke(skarbiec, arguments):
    result = subprocess.run(
        [skarbiec, *arguments], capture_output=True, text=True, check=False
    )
    if result.returncode:
        detail = result.stderr.strip() or f"exit status {result.returncode}"
        raise SystemExit(f"skarbiec {arguments[0]} failed: {detail}")
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise SystemExit(f"skarbiec {arguments[0]} returned unreadable JSON: {error}") from error


def main():
    skarbiec, item, field, *arguments = sys.argv[1:]
    if not arguments or arguments[0] != "token-mint":
        raise SystemExit("an existing vault field can only supply token-mint")
    source = invoke(skarbiec, ["get", item])
    fields = source.get("fields") if isinstance(source, dict) else None
    token = fields.get(field) if isinstance(fields, dict) else None
    if not isinstance(token, str) or not token or token.strip() != token:
        raise SystemExit(f"{item}#{field} must contain one nonempty bearer without surrounding whitespace")
    work = Path.home() / ".stado" / "work" / "vault-token-mint"
    work.mkdir(parents=True, exist_ok=True, mode=0o700)
    with tempfile.TemporaryDirectory(prefix="source-", dir=work) as directory:
        token_path = Path(directory) / "bearer"
        descriptor = os.open(token_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            output.write(token)
        report = invoke(skarbiec, [*arguments, "--token-file", str(token_path)])
    if not isinstance(report, dict) or report.get("ok") is not True:
        raise SystemExit("skarbiec token-mint did not report a successful registration")
    report.pop("token", None)
    print(json.dumps(report))


if __name__ == "__main__":
    main()
