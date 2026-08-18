#!/usr/bin/env python3
"""Remove declarations a host contradicts, so placement needs no exception list.

Which machine may run a Weles trajectory should follow from one fact the fleet
already maintains: which hosts run the Weles worker. It stopped following from
that for two reasons, both data rather than code:

  - `lukasz-macbook` still declares `com.wisent.always-on.weles` although the
    unit is not installed there, which `stado registry doctor` reports as
    `missing-plist`. A declaration nothing backs makes the operator's laptop a
    candidate for product browser journeys.
  - that candidacy was then blocked with a hand-written `placement.excludes` on
    the same target -- a second rule to maintain, and one that disappears the
    moment somebody edits the registry.

Retiring the untrue declaration removes both: the laptop is not a candidate
because it does not run the worker, and nothing has to remember why.

Compare-and-swap safe: the document is read, changed, and pushed; a concurrent
writer makes the push fail rather than silently win.
"""

import json
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
STADO = HOME / ".stado" / "bin" / "stado"
TARGET = os.environ.get("RETIRE_TARGET", "lukasz-macbook")
SERVICE = os.environ.get("RETIRE_SERVICE", "com.wisent.always-on.weles")
STAGED = HOME / ".stado" / "registry-retire-untrue.json"


def run(*args, stdin=NONE):
    return subprocess.run(args, capture_output=True, text=True, input=stdin, check=False)


def main():
    pulled = run(str(STADO), "registry", "pull")
    if pulled.returncode != ZERO:
        raise SystemExit(f"registry pull failed: {(pulled.stderr or pulled.stdout).strip()[:200]}")
    document = json.loads(pulled.stdout)
    changed = []
    for entry in document.get("targets", []):
        if entry.get("name") != TARGET:
            continue
        services = entry.get("services") or []
        kept = [item for item in services if SERVICE not in (item.get("name"), item.get("label"))]
        if len(kept) != len(services):
            entry["services"] = kept
            changed.append(f"service {SERVICE} (declared, not installed on this host)")
        if entry.pop("placement", NONE) is not NONE:
            changed.append("placement.excludes (replaced by the service derivation)")
    # The service directory names the declaration it manages, and the validator
    # refuses a document where that name has no declaration on the active host.
    # Removing only the host row therefore never validated: both halves of one
    # fact go together, or neither does.
    directory = (document.get("service_directory") or {}).get("services") or {}
    retired_names = [key for key, entry in directory.items()
                     if entry.get("managed_service") == SERVICE
                     and entry.get("active_host") == TARGET]
    for name in retired_names:
        consumers = sorted((directory[name].get("consumers") or {}))
        directory.pop(name)
        changed.append(
            f"service_directory.services.{name}"
            + (f" (its consumers were {', '.join(consumers)})" if consumers else "")
        )
    # One logical service is named in three places: the host declaration, the
    # directory, and every resolver adapter that forwards to it. A retirement
    # that clears two of the three leaves a document the validator rejects for
    # an "unknown logical service", so the third goes with them.
    for entry in document.get("targets", []):
        resolver = entry.get("service_resolver") or {}
        adapters = resolver.get("adapters") or []
        kept = [item for item in adapters if item.get("service") not in retired_names]
        if len(kept) != len(adapters):
            resolver["adapters"] = kept
            changed.append(f"{entry['name']}: resolver adapters for {', '.join(retired_names)}")
    if not changed:
        print(f"settled    {TARGET} declares neither the service nor the exclusion")
        return NONE
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
    for item in changed:
        print(f"retired    {TARGET}: {item}")
    return NONE


sys.exit(main())
