#!/usr/bin/env python3
"""Say why node-pty cannot start the provider CLI.

`posix_spawnp failed` is all node-pty reports, and it covers a missing
interpreter, a missing working directory, a wrong architecture and a
non-executable file alike. This prints what the binary is, then tries the same
spawn three ways -- inherited environment, the login helper's own environment,
and a plain child process -- so the difference names the cause.

Read-only: it starts `--version`, nothing else.
"""

import json
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
CLAUDE = HOME / ".local" / "bin" / "claude"
NODE = "/opt/homebrew/bin/node"
TREE = HOME / "weles"
PROBE = """
const { spawn } = require('node-pty');
const bin = process.argv[2];
const mode = process.argv[3];
const env = mode === 'minimal' ? { PATH: '/usr/bin:/bin', HOME: process.env.HOME } : process.env;
try {
  const p = spawn(bin, ['--version'], { name: 'xterm-256color', cols: 100, rows: 20, env });
  let out = '';
  p.onData((d) => { out += d; });
  setTimeout(() => { console.log(`${mode}: ok ${JSON.stringify(out.trim().slice(0, 60))}`); p.kill(); process.exit(0); }, 3000);
} catch (error) {
  console.log(`${mode}: ${error.message}`);
  process.exit(0);
}
"""


def run(*args, **kwargs):
    proc = subprocess.run(args, capture_output=True, text=True, check=False, **kwargs)
    return (proc.stdout + proc.stderr).strip()


def main():
    print(f"binary     {CLAUDE} {'present' if CLAUDE.is_file() else 'absent'}")
    if CLAUDE.is_file():
        print(f"  mode     {oct(CLAUDE.stat().st_mode)[-len('755'):]}  size {CLAUDE.stat().st_size}")
        if CLAUDE.is_symlink():
            print(f"  symlink  -> {os.readlink(CLAUDE)}")
        print(f"  file     {run('/usr/bin/file', '-b', str(CLAUDE))[: len('a' * 120)]}")
        with CLAUDE.open('rb') as handle:
            head = handle.readline(len('a' * 200)).decode('utf-8', 'replace').strip()
        print(f"  head     {head[: len('a' * 120)]}")
    print(f"cwd        {os.getcwd()}")
    probe = TREE / "var" / "pty-probe.cjs"
    try:
        probe.parent.mkdir(parents=True, exist_ok=True)
        probe.write_text(PROBE, encoding="utf-8")
    except OSError as error:
        print(f"probe      cannot write {probe}: {error}")
        probe = pathlib.Path("/tmp/pty-probe.cjs")
        probe.write_text(PROBE, encoding="utf-8")
    # node-pty starts a PTY through its own `spawn-helper`; when that file is
    # missing, not executable, or quarantined, every spawn fails with the same
    # opaque `posix_spawnp failed` no matter what the target binary is.
    helpers = sorted(TREE.glob("node_modules/node-pty/build/*/spawn-helper"))
    for helper in helpers:
        stat = helper.stat()
        print(f"spawn-helper {helper} mode {oct(stat.st_mode)[-len('755'):]} size {stat.st_size}")
        print(f"  file     {run('/usr/bin/file', '-b', str(helper))[: len('a' * 100)]}")
        print(f"  xattr    {run('/usr/bin/xattr', str(helper)) or '(none)'}")
    if not helpers:
        module = TREE / "node_modules" / "node-pty"
        print(f"node-pty   {module} {'present' if module.is_dir() else 'absent'}")
        build = module / "build"
        print(f"build dir  {build} {'present' if build.is_dir() else 'absent'}")
        for path in sorted(build.rglob("*")) if build.is_dir() else []:
            if path.is_file():
                print(f"  {path.relative_to(build)}  {path.stat().st_size}  mode {oct(path.stat().st_mode)[-len('755'):]}")
    for mode in ("inherited", "minimal"):
        print(f"pty {mode:<10} {run(NODE, str(probe), str(CLAUDE), mode, cwd=str(TREE))[: len('a' * 200)]}")
    print(f"plain spawn {run(str(CLAUDE), '--version')[: len('a' * 80)]}")
    return NONE


sys.exit(main())
