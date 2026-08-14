#!/usr/bin/env python3
"""Build the `stado` binary on the host that owns the checkout, and measure it.

`check-stado-build.py` proves a change type-checks. That is not the same as being
able to RUN the changed command, and a new `stado registry doctor` finding can only
be shown to produce the row it claims by producing it. This builds the binary the
same way and leaves it where the caller can execute it, so the rows are evidence
rather than a description of evidence.

Same three constraints as the type-check, for the same reasons: the agent sandbox
cannot create a build directory at all, macOS refuses a launchd-run helper's writes
under ~/Documents, and the host agent's secret-safe umask strips the execute bit
from every directory cargo creates, so cargo cannot enter the target tree it just
made. Hence: run through the Stado host agent, keep CARGO_TARGET_DIR in ~/.cache,
and relax the umask for this process only.

The debug profile on purpose: it shares the warm target directory the type-check
already populated, so this costs a link instead of a from-scratch release build,
and an unoptimized binary prints exactly the same rows.

Prints the compiler's verdict and the artifact it produced. Installs nothing, and
never touches the binary the fleet runs.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
HOME = pathlib.Path(os.path.expanduser("~"))
TREE = HOME / "Documents" / "CodingProjects" / "Wisent" / "wisent-compute" / "stado-rs"
CARGO = HOME / ".cargo" / "bin" / "cargo"
SCRATCH = HOME / ".cache" / "stado-build"
PRODUCT = SCRATCH / "debug" / "stado"
TIMEOUT = 3600
KEEP = ("error", "warning: unused", "Finished", "Compiling stado", "Checking stado")


def main():
    if not TREE.is_dir():
        raise SystemExit(f"no stado-rs checkout at {TREE}")
    if not CARGO.is_file():
        raise SystemExit(f"no cargo at {CARGO}")
    # Prove the TCC read side before spending time in the compiler: if launchd's
    # session cannot see the checkout, every later line would be noise.
    if not os.access(TREE / "Cargo.toml", os.R_OK):
        raise SystemExit(f"source_read denied at {TREE}")
    print(f"source        {TREE}")
    print(f"target_dir    {SCRATCH}")
    SCRATCH.mkdir(parents=True, exist_ok=True)
    os.umask(0o022)
    os.chmod(SCRATCH, 0o755)
    proc = subprocess.run(
        [str(CARGO), "build", "--bin", "stado"],
        capture_output=True,
        text=True,
        check=False,
        cwd=str(TREE),
        timeout=TIMEOUT,
        env={
            **os.environ,
            "CARGO_TARGET_DIR": str(SCRATCH),
            "PATH": f"{CARGO.parent}:/opt/homebrew/bin:/usr/bin:/bin",
        },
    )
    lines = [line.rstrip() for line in (proc.stdout + proc.stderr).splitlines() if line.strip()]
    picked = [line for line in lines if any(marker in line for marker in KEEP)]
    for line in (picked or lines)[-len("a" * 40) :]:
        print(line[: len("a" * 165)])
    print(f"exit {proc.returncode}")
    if PRODUCT.is_file():
        # The caller executes this path directly, so the size and the mode are the
        # two facts that decide whether it can.
        os.chmod(PRODUCT, 0o755)
        print(f"product       {PRODUCT}")
        print(f"product_size  {os.path.getsize(PRODUCT)}")
        print(f"product_mode  {oct(PRODUCT.stat().st_mode & 0o777)}")
    else:
        print(f"product       {PRODUCT} (absent)")
    return NONE


sys.exit(main())
