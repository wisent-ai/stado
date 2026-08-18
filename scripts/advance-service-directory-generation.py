#!/usr/bin/env python3
"""Advance `service_directory.generation` after the directory itself was edited.

The resolver holds the invariant: a directory that changed while its generation
stayed put is refused, because two different answers to "where is X" under one
generation is how a fleet ends up with half its hosts dialling a dead endpoint.
`cli/resolver.rs::refresh` rejects it, the cache goes stale inside
`max_stale_seconds`, every adapter starts answering "service directory cache is
stale", and the process exits EX_CONFIG on the next loop.

That happened on 2026-08-18: `stado-object-api` for `operator-host` was
corrected from `http://127.0.0.1:18776` -- its own resolver adapter, the
self-referencing endpoint `audit-registry-declarations.py` reports -- to
`http://127.0.0.1:18765`, the object API that host actually runs. The new value is
right. Only the generation was missing, and with the adapter down every `stado`
command on that host failed with `registry store unreachable`, because the
configured store URL is the adapter. A restart clears it for one process, since a
fresh resolver adopts whatever it first reads; it does not settle the invariant
for the next process to refresh.

So this bumps the generation, and nothing else.

Idempotent by content: the digest of the directory this script last published a
generation for is recorded in `~/.stado/service-directory-digest.json`, and a run
whose digest matches it changes nothing. The first run bumps, because a directory
whose generation this script never published cannot be shown to match it -- which
is exactly the state that broke the resolver.

The push is compare-and-swap, so a concurrent writer makes it fail rather than
silently win. `STADO_STORE_URL` overrides the store the CLI talks to, which is
required while the stale directory has the configured adapter down.
"""

import hashlib
import json
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
ONE = len("a")
HOME = pathlib.Path(os.path.expanduser("~"))
STADO = HOME / ".stado" / "bin" / "stado"
STORE_URL = os.environ.get("STADO_STORE_URL", "")
MARKER = HOME / ".stado" / "service-directory-digest.json"
STAGED = HOME / ".stado" / "registry-advance-directory-generation.json"


def environment():
    merged = {**os.environ}
    if STORE_URL:
        merged["WC_STADO_STORAGE_URL"] = STORE_URL
    return merged


def run(*args):
    return subprocess.run(args, capture_output=True, text=True, check=False, env=environment())


def digest_of(directory):
    """Digest of the directory's content, with `generation` excluded.

    The generation is the announcement, not the content: including it would make
    every bump look like a fresh change and the marker would never settle.
    """
    content = {key: value for key, value in directory.items() if key != "generation"}
    return hashlib.sha256(
        json.dumps(content, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def recorded():
    if not MARKER.is_file():
        return {}
    try:
        return json.loads(MARKER.read_text(encoding="utf-8"))
    except (ValueError, OSError):
        return {}


def main():
    pulled = run(str(STADO), "registry", "pull")
    if pulled.returncode != ZERO:
        raise SystemExit(f"registry pull failed: {(pulled.stderr or pulled.stdout).strip()[:200]}")
    document = json.loads(pulled.stdout)
    directory = document.get("service_directory")
    if not isinstance(directory, dict):
        raise SystemExit("the registry carries no service_directory object")
    current = directory.get("generation")
    if not isinstance(current, int):
        raise SystemExit(f"service_directory.generation is {current!r}, not an integer")
    fingerprint = digest_of(directory)
    previous = recorded()
    print(f"generation {current}")
    print(f"digest     {fingerprint[:16]}")
    if previous.get("digest") == fingerprint:
        print(
            f"settled    generation {previous.get('generation')} was published for this exact "
            "directory; nothing written"
        )
        return NONE

    directory["generation"] = current + ONE
    print(f"advancing  {current} -> {directory['generation']}")
    print(f"services   {len(directory.get('services') or {})} declared")
    STAGED.parent.mkdir(parents=True, exist_ok=True)
    STAGED.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    validated = run(str(STADO), "registry", "validate", str(STAGED))
    print(f"validate   {(validated.stdout or validated.stderr).strip().splitlines()[-1:]}")
    if validated.returncode != ZERO:
        raise SystemExit("the edited registry does not validate; nothing was pushed")
    pushed = run(str(STADO), "registry", "push", str(STAGED))
    print(f"push       {(pushed.stdout or pushed.stderr).strip().splitlines()[-1:]}")
    if pushed.returncode != ZERO:
        raise SystemExit("push refused; the canonical document is unchanged")
    STAGED.unlink()
    MARKER.write_text(
        json.dumps({"generation": directory["generation"], "digest": fingerprint}, indent=2) + "\n",
        encoding="utf-8",
    )
    print(f"recorded   {MARKER}")
    print("advanced   every resolver accepts the corrected directory on its next refresh")
    return NONE


sys.exit(main())
