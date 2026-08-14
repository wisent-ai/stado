#!/usr/bin/env python3
"""Measure this host's capabilities and publish them where the fleet reads them.

A measurement nobody can read changes nothing, which is the whole failure this
replaces: the mini could not own a window for weeks and every layer above had to
rediscover that by watching a browser crash. One published object ends that --
placement reads it before it schedules, the runtime guard reads it before it
starts a headed browser, and `registry doctor` reads it to catch a declaration
the world contradicts.

Transport, copied rather than invented. The queue's Stado object store already
speaks this API and this script speaks it the same way:

  * URL shape and bearer: `stado-rs/src/queue/stado_object.rs:162-186`
    (`url()` builds `<base>/api/object`, `object_url()` appends `?uri=<stado://
    namespace/key>`, `request()` sets `Authorization: Bearer <token>`).
  * PUT headers: `stado-rs/src/queue/stado_object.rs:194-214` -- an explicit
    `Content-Length` next to the body, because the object endpoint requires the
    header and reqwest omits it for an empty payload.
  * Where the base URL, namespace, token file and CA come from:
    `storage.stado` in the host's own `~/.config/stado/config.json`, the block
    `scripts/point-stado-at-object-api.py` writes and
    `stado-rs/src/queue/stado_object.rs:132-156` reads for `ca_file`.

The host-health beacon's own bearer is deliberately NOT used here. That grant is
route-scoped to `PUT /api/host-health`
(`stado-rs/src/dashboard/mod.rs:1696-1699` checks `host-health:publish` and
nothing else), so reusing it for an object write would only ever earn a 401. The
object route authorizes per namespace and prefix instead
(`stado-rs/src/dashboard/mod.rs:2607-2641`), which is why
`scripts/authorize-host-capabilities-objects.py` has to grant
`host_capabilities/` before this script can succeed.

Idempotent: the same host publishes to the same key every time, and the object
is overwritten rather than versioned. Prints the URI and the byte count written.
"""

import datetime
import json
import os
import pathlib
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
CONFIG = HOME / ".config" / "stado" / "config.json"
PREFIX = "host_capabilities"
# The measurement launches a browser and waits for it, so the publisher's own
# patience has to exceed the browser's; anything tighter reports a publish
# failure for a host that was merely slow.
MEASURE_TIMEOUT = len("a" * 180)
PUT_TIMEOUT = len("a" * 30)


def measure():
    """Run the measurement helper installed beside this one.

    Two files rather than one function, because a host operator has to be able
    to run the measurement alone and see the evidence without publishing it, and
    because the periodic unit must publish exactly what a human would see.
    """
    here = pathlib.Path(__file__).resolve().parent
    for name in ("measure-host-capabilities", "measure-host-capabilities.py"):
        candidate = here / name
        if candidate.is_file():
            break
    else:
        raise SystemExit(
            f"measure-host-capabilities is not installed next to {__file__}; "
            "install it with `stado host install-helper <host> "
            "scripts/measure-host-capabilities.py measure-host-capabilities`"
        )
    started = time.time()
    try:
        proc = subprocess.run(
            [sys.executable, str(candidate)],
            capture_output=True,
            text=True,
            timeout=MEASURE_TIMEOUT,
            check=False,
        )
    except subprocess.TimeoutExpired:
        # A unit that hangs publishes nothing and says nothing, which is the
        # failure mode this whole model exists to end. The bound turns it into a
        # message with a duration in it.
        raise SystemExit(
            f"{candidate} did not finish within {MEASURE_TIMEOUT}s; nothing was published"
        )
    if proc.returncode != ZERO:
        raise SystemExit(
            f"{candidate} failed: {proc.stderr.strip() or proc.stdout.strip()}"
        )
    print(f"measured    in {time.time() - started:.1f}s")
    return json.loads(proc.stdout)


def storage_settings():
    """The object API this host is pointed at, from its own Stado config."""
    if not CONFIG.is_file():
        raise SystemExit(f"{CONFIG} does not exist; this host is pointed at no store")
    storage = json.loads(CONFIG.read_text(encoding="utf-8")).get("storage", {})
    settings = storage.get("stado", {})
    for required in ("url", "namespace", "token_file"):
        if not str(settings.get(required, "")).strip():
            raise SystemExit(f"storage.stado.{required} is not set in {CONFIG}")
    return settings


def bearer(token_file):
    path = pathlib.Path(os.path.expanduser(token_file))
    if not path.is_file():
        raise SystemExit(f"storage.stado.token_file {path} does not exist")
    token = path.read_text(encoding="utf-8").strip()
    if not token:
        raise SystemExit(f"storage.stado.token_file {path} is empty")
    return token


def context_for(url, ca_file):
    """A TLS context that trusts the fleet's own authority as well as the system's.

    The tailnet endpoint is signed by an authority no system trust store carries,
    and the failure looks exactly like an unreachable host rather than like an
    untrusted one. `storage.stado.ca_file` is the certificate every other Stado
    client already adds for that reason; it is added, never substituted.
    """
    if not url.startswith("https://"):
        return NONE
    context = ssl.create_default_context()
    if str(ca_file or "").strip():
        context.load_verify_locations(os.path.expanduser(ca_file))
    return context


def put(settings, key, body):
    uri = f"stado://{settings['namespace']}/{key}"
    endpoint = (
        f"{settings['url'].rstrip('/')}/api/object?"
        + urllib.parse.urlencode({"uri": uri})
    )
    request = urllib.request.Request(endpoint, data=body, method="PUT")
    request.add_header("Authorization", f"Bearer {bearer(settings['token_file'])}")
    request.add_header("Content-Type", "application/json")
    request.add_header("Content-Length", str(len(body)))
    request.add_header("Accept", "application/json")
    try:
        with urllib.request.urlopen(
            request, timeout=PUT_TIMEOUT, context=context_for(settings["url"], settings.get("ca_file"))
        ) as answer:
            return uri, answer.status, answer.read().decode("utf-8", "replace").strip()
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", "replace").strip()
        raise SystemExit(f"PUT {uri} returned HTTP {error.code}: {detail}")
    except (urllib.error.URLError, OSError) as error:
        raise SystemExit(f"PUT {uri} could not reach {settings['url']}: {error}")


def main():
    # launchd and systemd both keep this process's stdout in a file, and a block
    # buffer there means a run that is merely slow looks identical to a run that
    # wedged: the log stays empty either way. Line buffering makes the log a
    # progress report, which is the difference between diagnosing a stuck unit
    # in one look and inferring it from a process listing.
    sys.stdout.reconfigure(line_buffering=True)
    print(f"started     {datetime.datetime.now(datetime.timezone.utc).isoformat()}")
    document = measure()
    host = document["host"]
    body = (json.dumps(document, indent=len("ba"), sort_keys=False) + "\n").encode("utf-8")
    settings = storage_settings()
    print(f"putting     {len(body)} bytes to {settings['url']}")
    uri, status, answer = put(settings, f"{PREFIX}/{host}.json", body)
    for name, capability in document["capabilities"].items():
        print(f"measured    {name:<15} {str(capability['value']).lower():<5} {capability['detail']}")
    print(f"endpoint    {settings['url']}")
    print(f"uri         {uri}")
    print(f"published   {len(body)} bytes  HTTP {status}  {answer[: len('a' * 120)]}")
    return NONE


sys.exit(main())
