"""Run Stado's runner reconciliation with ephemeral fleet signing credentials."""
import json
import os
import subprocess
import sys


def reconcile(request):
    for key in ("script", "certificate", "private_key", "apphost_signer", "signer"):
        if not isinstance(request.get(key), str) or not request[key]:
            raise ValueError("runner signing request is missing " + key)
    environment = dict(os.environ,
                       WISENT_PRODUCTS_BIN=request["signer"],
                       WISENT_CODESIGN_CERTIFICATE_PEM=request["certificate"],
                       WISENT_CODESIGN_PRIVATE_KEY_PEM=request["private_key"],
                       STADO_RUNNER_APPHOST_SIGNER=request["apphost_signer"])
    result = subprocess.run(["/bin/bash", "-s"], input=request["script"],
                            text=True, env=environment)
    return result.returncode


if __name__ == "__main__":
    try:
        sys.exit(reconcile(json.load(sys.stdin)))
    except (OSError, ValueError) as error:
        print("runner signing failed: {}".format(error), file=sys.stderr)
        sys.exit(1)
