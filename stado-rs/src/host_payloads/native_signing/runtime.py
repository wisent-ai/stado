"""Install the pinned private signing dependency into its Stado-owned cache."""
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


def execute(argv, environment=None):
    result = subprocess.run(argv, env=environment, capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError("{} exited {}: {}".format(
            " ".join(map(str, argv)), result.returncode,
            (result.stderr or result.stdout).strip()))
    return result.stdout


def prepare(request):
    archive = base64.b64decode(request["archive"], validate=True)
    digest = hashlib.sha256(archive).hexdigest()
    if digest != request["sha256"]:
        raise RuntimeError("native signing runtime source digest mismatch")
    root = Path.home() / ".stado" / "cache" / "native-signing" / digest
    for directory in [root.parent.parent.parent, root.parent.parent, root.parent, root]:
        if directory.is_symlink():
            raise RuntimeError("native signing cache is a symlink: {}".format(directory))
        directory.mkdir(mode=0o700, exist_ok=True)
    program = root / "bin" / "wisent-products"
    if program.is_file() and os.access(program, os.X_OK):
        execute([str(program), "signing", "sign", "--help"])
        return {"program": str(program), "source_sha256": digest, "state": "reused"}
    candidates = [shutil.which("uv"), "/opt/homebrew/bin/uv", "/usr/local/bin/uv",
                  str(Path.home() / ".local/bin/uv")]
    uv = next((path for path in candidates if path and os.access(path, os.X_OK)), None)
    if uv is None:
        raise RuntimeError("uv is required to prepare the native signing runtime; checked PATH and managed installer locations")
    source = root / "source.tar.gz"
    with tempfile.NamedTemporaryFile(dir=root, prefix=".source-", delete=False) as stream:
        staged = Path(stream.name)
        try:
            stream.write(archive)
            stream.flush()
            os.fsync(stream.fileno())
            staged.replace(source)
        finally:
            staged.unlink(missing_ok=True)
    environment = dict(os.environ, UV_TOOL_DIR=str(root / "tools"),
                       UV_TOOL_BIN_DIR=str(root / "bin"), TMPDIR=str(root))
    execute([uv, "tool", "install", "--python", ">=3.11", "--from", str(source),
             "--force", "wisent-products"], environment)
    execute([str(program), "signing", "sign", "--help"])
    return {"program": str(program), "source_sha256": digest, "state": "installed"}


try:
    print(json.dumps(prepare(json.load(sys.stdin))))
except (OSError, ValueError, KeyError, RuntimeError) as error:
    print("native signing runtime preparation failed: {}".format(error), file=sys.stderr)
    sys.exit(1)
