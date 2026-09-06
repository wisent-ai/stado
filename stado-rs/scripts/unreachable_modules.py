#!/usr/bin/env python3
"""Every `.rs` file under `src/` must be reachable from a crate root.

A Rust source file that no `mod` declaration names is not a compile error.
It is not compiled at all. `cargo check` passes, `cargo clippy` passes, the
file sits in the tree, `git log` shows it landing, and nothing it contains
runs. The only symptom is silence.

One way this has already happened in this repository:

- On 2026-08-31 at 15:48Z one commit replaced `src/cli/mod.rs` with a
  six-day-old copy, deleting nine `pub mod` declarations - `builds`,
  `database`, `egress`, `fleet`, `product`, `release_evidence`,
  `release_quarantine`, `service_converge`, `stream` - while all nine files
  stayed on disk. `main` stopped compiling, but not because a module was
  undeclared: it failed 27 errors away, in unrelated callers, and no
  diagnostic named a missing declaration. A file listing looked untouched.
So the declaration and the tree disagree and nothing compares them. This
compares them: resolve every `mod` declaration from `src/lib.rs` and each
`[[bin]]` path in `Cargo.toml`, then report any `.rs` file the walk never
reached.

Exit status is the contract: zero means every file is reachable, non-zero
names the ones that are not.

Files already unreachable when this check was written are listed in
`scripts/unreachable_modules.known`, one path per line with the reason beside
it. That file is a ratchet, not an amnesty: anything unreachable and unlisted
fails, and a listed path that no longer exists also fails, because a stale
entry would silently re-admit a real orphan under the same name later.

Usage:
    scripts/unreachable_modules.py [--crate DIR] [--known FILE] [--json]
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

# `mod name;` / `pub mod name;` / `pub(crate) mod name;`, and the raw
# identifier spelling `mod r#box;` that a keyword module needs. A `mod name {`
# block is deliberately NOT matched: it declares its contents inline and
# resolves to no file.
MOD_DECLARATION = re.compile(
    r"^[^\S\n]*(?:pub(?:\([^)]*\))?[^\S\n]+)?mod[^\S\n]+(?:r#)?(\w+)[^\S\n]*;",
    re.MULTILINE,
)

# `path = "src/bin/whatever.rs"` inside Cargo.toml. Every `[[bin]]` is its own
# crate root, so a file reachable only from one of them still counts.
CARGO_BIN_PATH = re.compile(r'^path\s*=\s*"(src/[^"]+\.rs)"', re.MULTILINE)


def read_known(path: Path | None, crate: Path) -> list[str]:
    """Paths recorded as already-unreachable, `# reason` stripped."""
    if path is None:
        return []
    resolved = path if path.is_absolute() else crate / path
    if not resolved.is_file():
        print(f"error: {resolved} is not a file", file=sys.stderr)
        raise SystemExit(2)
    entries = []
    for raw in resolved.read_text(encoding="utf-8").splitlines():
        entry = raw.split("#", 1)[0].strip()
        if entry:
            entries.append(entry)
    return entries


def crate_roots(crate: Path) -> list[Path]:
    """`src/lib.rs` plus every `[[bin]]` path Cargo declares."""
    roots = []
    lib = crate / "src" / "lib.rs"
    if lib.is_file():
        roots.append(lib)
    manifest = crate / "Cargo.toml"
    if manifest.is_file():
        text = manifest.read_text(encoding="utf-8", errors="replace")
        for match in CARGO_BIN_PATH.finditer(text):
            candidate = crate / match.group(1)
            if candidate.is_file():
                roots.append(candidate)
    return roots


def child_directory(path: Path, roots: set[Path]) -> Path:
    """Where a file's `mod` declarations resolve.

    `foo/mod.rs` and a crate root own their own directory; `foo/bar.rs` owns
    `foo/bar/`. Getting this wrong in the other direction would make the walk
    miss real files and invent orphans, so it is the one rule here worth
    stating.
    """
    if path.name == "mod.rs" or path in roots:
        return path.parent
    return path.parent / path.stem


def reachable(crate: Path) -> tuple[set[Path], list[Path]]:
    roots = crate_roots(crate)
    root_set = set(roots)
    seen: set[Path] = set()
    stack = list(roots)
    while stack:
        current = stack.pop()
        if current in seen or not current.is_file():
            continue
        seen.add(current)
        directory = child_directory(current, root_set)
        text = current.read_text(encoding="utf-8", errors="replace")
        for name in MOD_DECLARATION.findall(text):
            for candidate in (directory / f"{name}.rs", directory / name / "mod.rs"):
                if candidate.is_file() and candidate not in seen:
                    stack.append(candidate)
    return seen, roots


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--crate",
        default=Path(__file__).resolve().parent.parent,
        type=Path,
        help="crate directory holding Cargo.toml and src/ (default: this script's crate)",
    )
    parser.add_argument(
        "--known",
        type=Path,
        metavar="FILE",
        help="file listing already-unreachable paths, one per line, `# reason` accepted",
    )
    parser.add_argument("--json", action="store_true", help="emit the report as JSON")
    args = parser.parse_args()

    crate: Path = args.crate.resolve()
    source = crate / "src"
    if not source.is_dir():
        print(f"error: {source} is not a directory", file=sys.stderr)
        return 2

    every = {path.resolve() for path in source.rglob("*.rs")}
    seen, roots = reachable(crate)
    if not roots:
        print(f"error: {crate} declares no crate root", file=sys.stderr)
        return 2

    known = {(crate / entry).resolve() for entry in read_known(args.known, crate)}
    stale = sorted(path for path in known if path not in every)
    orphans = sorted(every - {path.resolve() for path in seen} - known)

    def relative(path: Path) -> str:
        return str(path.relative_to(crate))

    if args.json:
        print(
            json.dumps(
                {
                    "crate": str(crate),
                    "roots": [relative(path) for path in roots],
                    "files": len(every),
                    "reachable": len(every) - len(orphans) - len(known),
                    "known": [relative(path) for path in sorted(known)],
                    "stale": [relative(path) for path in stale],
                    "unreachable": [relative(path) for path in orphans],
                },
                indent=2,
            )
        )
    else:
        print(f"{len(every)} .rs files under {relative(source)}")
        print(f"{len(roots)} crate root(s): {', '.join(relative(p) for p in roots)}")
        for path in stale:
            print(f"stale --known entry (no such file): {relative(path)}")
        if orphans:
            print(f"\n{len(orphans)} file(s) no `mod` declaration reaches:")
            for path in orphans:
                print(f"  {relative(path)}")
            print(
                "\nEach is compiled into nothing. Declare it, delete it, or record it\n"
                "in the --known file with the reason beside it."
            )
        else:
            print("\nevery file is reachable from a crate root")

    # A stale entry is also a failure: it is a claim about a file that no
    # longer exists, and leaving it would silently re-admit a real orphan
    # under the same name later.
    return 1 if orphans or stale else 0


if __name__ == "__main__":
    sys.exit(main())
