#!/usr/bin/env python3
"""Normalize the managed resolver SSH identity without exposing its bytes."""

import base64
import binascii
import json
import os
from pathlib import Path
import subprocess
import tempfile

key_path = Path(
    os.environ.get(
        "STADO_RESOLVER_SSH_KEY_FILE",
        Path.home() / ".stado" / "resolver-ssh-key",
    )
)
raw = key_path.read_bytes()
if not raw:
    raise SystemExit(f"resolver SSH identity is empty: {key_path}")

candidates = [raw]
try:
    text = raw.decode("utf-8").strip()
except UnicodeDecodeError:
    text = ""
if text:
    candidates.append((text + "\n").encode())
    if "\\n" in text:
        candidates.append(
            (text.replace("\\r", "\r").replace("\\n", "\n") + "\n").encode()
        )
    try:
        decoded_json = json.loads(text)
    except json.JSONDecodeError:
        decoded_json = None
    if isinstance(decoded_json, str):
        candidates.append((decoded_json.rstrip("\n") + "\n").encode())
    try:
        decoded_base64 = base64.b64decode(text, validate=True)
    except (ValueError, binascii.Error):
        decoded_base64 = b""
    if decoded_base64:
        candidates.append(decoded_base64.rstrip(b"\n") + b"\n")

key_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
selected = None
for candidate in candidates:
    candidate = candidate.replace(b"\r\n", b"\n").rstrip(b"\n") + b"\n"
    handle, temporary_name = tempfile.mkstemp(prefix=".resolver-ssh-key.", dir=key_path.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(handle, "wb") as output:
            output.write(candidate)
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o600)
        checked = subprocess.run(
            ["/usr/bin/ssh-keygen", "-y", "-f", temporary],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        if checked.returncode == 0:
            selected = temporary
            break
    finally:
        if selected != temporary:
            temporary.unlink(missing_ok=True)

if selected is None:
    raise SystemExit("resolver SSH identity is not a supported private-key encoding")
os.replace(selected, key_path)
os.chmod(key_path, 0o600)
subprocess.run(["/usr/bin/ssh-keygen", "-lf", key_path], check=True)
