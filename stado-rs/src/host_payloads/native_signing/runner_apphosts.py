"""Sign GitHub's .NET apphosts with the minimum runtime entitlement."""
import json
from pathlib import Path
import plistlib
import sys
import tempfile

from wisent_products.signing import credentials
from wisent_products.signing.core import command, identifier, inspect, sign


def sign_apphosts(paths):
    with credentials.scope(paths[0].parent):
        for path in paths:
            report = sign(path, identifier("stado", path.name))
            observed = command(["/usr/bin/codesign", "--display", "--entitlements", "-", "--xml", str(path)])
            entitlements = plistlib.loads(observed.stdout.encode()) if observed.stdout.strip() else {}
            # .NET 8's distributed host needs executable memory and loads its
            # separately signed runtime libraries. Do not grant debugger access.
            for name in ("allow-jit", "allow-unsigned-executable-memory",
                         "allow-dyld-environment-variables", "disable-library-validation"):
                entitlements["com.apple.security.cs." + name] = True
            with tempfile.TemporaryDirectory(prefix=".runner-entitlements-", dir=path.parent) as directory:
                entitlement_file = Path(directory) / "runtime.plist"
                entitlement_file.write_bytes(plistlib.dumps(entitlements))
                command(["/usr/bin/codesign", "--force", "--sign", credentials.supplied_identity(),
                         "--keychain", str(credentials.keychain(command)),
                         "--identifier", report["identifier"], "--timestamp=none",
                         "--preserve-metadata=requirements", "--options", "runtime",
                         "--entitlements", str(entitlement_file), str(path)])
            command(["/usr/bin/codesign", "--verify", "--strict", str(path)])
            actual = command(["/usr/bin/codesign", "--display", "--entitlements", "-", "--xml", str(path)])
            if plistlib.loads(actual.stdout.encode()).get("com.apple.security.cs.allow-jit") is not True:
                raise RuntimeError("runner apphost does not carry its required JIT entitlement")
            print(json.dumps(inspect(path)))


if __name__ == "__main__":
    sign_apphosts([Path(path) for path in sys.argv[1:]])
