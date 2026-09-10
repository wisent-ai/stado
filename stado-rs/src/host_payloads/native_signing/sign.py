"""Sign one staged native file with a certificate supplied on standard input."""
import json
import os
import subprocess
import sys


def sign(request):
    for name in ("program", "identifier", "target", "certificate", "private_key"):
        if not isinstance(request.get(name), str) or not request[name]:
            raise ValueError("native signing request is missing {}".format(name))
    argv = [request["program"], "signing", "sign", "--identifier", request["identifier"]]
    previous = request.get("previous")
    if previous:
        argv.extend(["--previous", previous])
    argv.extend([request["target"], "--json"])
    # The certificate and its key travel in this child's environment, never in
    # argv, so a process listing on this host cannot read either of them.
    environment = dict(os.environ,
                       WISENT_CODESIGN_CERTIFICATE_PEM=request["certificate"],
                       WISENT_CODESIGN_PRIVATE_KEY_PEM=request["private_key"])
    result = subprocess.run(argv, env=environment, capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError("{} exited {}: {}".format(
            " ".join(argv), result.returncode, (result.stderr or result.stdout).strip()))
    return json.loads(result.stdout)


try:
    print(json.dumps(sign(json.load(sys.stdin))))
except (OSError, ValueError, RuntimeError) as error:
    print("native signing failed: {}".format(error), file=sys.stderr)
    sys.exit(1)
