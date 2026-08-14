#!/usr/bin/env python3
"""Say why this host does not trust the tailnet object endpoint.

`CERTIFICATE_VERIFY_FAILED` is three different faults wearing one message: the
trust anchor this host was told to use may be absent, it may be the wrong
anchor, or the certificate the server presents may not cover the address the
caller dialled. Each has a different repair, so print all three facts instead of
guessing: what the config names, what is on disk, and what the endpoint actually
serves.

Read-only. Prints subjects, issuers, SANs, validity and fingerprints -- a
certificate is public by construction, and the private key is never touched.
"""

import datetime
import json
import os
import pathlib
import socket
import ssl
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
CONFIG = pathlib.Path(os.environ.get("STADO_CONFIG", HOME / ".config" / "stado" / "config.json"))
TIMEOUT = 15


def endpoint():
    document = json.loads(CONFIG.read_text(encoding="utf-8"))
    stado = document.get("storage", {}).get("stado", {})
    return stado.get("url", ""), stado.get("ca_file", ""), document


def presented(host, port):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    with socket.create_connection((host, port), timeout=TIMEOUT) as raw:
        with context.wrap_socket(raw, server_hostname=host) as tls:
            binary = tls.getpeercert(binary_form=True)
            return ssl.DER_cert_to_PEM_cert(binary), tls.getpeercert()


def verify(host, port, ca_file):
    context = ssl.create_default_context(cafile=ca_file) if ca_file else ssl.create_default_context()
    try:
        with socket.create_connection((host, port), timeout=TIMEOUT) as raw:
            with context.wrap_socket(raw, server_hostname=host) as tls:
                return f"trusted, protocol {tls.version()}"
    except ssl.SSLCertVerificationError as error:
        return f"refused: {error.verify_message or error.reason} (code {error.verify_code})"
    except OSError as error:
        return f"unreachable: {error}"


def main():
    url, ca_file, _ = endpoint()
    print(f"config url      {url or '(unset)'}")
    print(f"config ca_file  {ca_file or '(unset)'}")
    anchor = pathlib.Path(os.path.expanduser(ca_file)) if ca_file else NONE
    if anchor:
        print(f"anchor on disk  {'present' if anchor.is_file() else 'ABSENT'}"
              + (f", {anchor.stat().st_size} bytes" if anchor.is_file() else ""))
    if not url.startswith("https://"):
        print("verdict         this host does not use TLS for the store")
        return NONE
    rest = url[len("https://"):]
    host, _, port = rest.partition(":")
    port = int(port or "443")
    try:
        pem, parsed = presented(host, port)
    except OSError as error:
        print(f"endpoint        unreachable before TLS: {error}")
        return NONE
    print(f"served subject  {parsed.get('subject')}")
    print(f"served issuer   {parsed.get('issuer')}")
    print(f"served SANs     {parsed.get('subjectAltName')}")
    print(f"served validity {parsed.get('notBefore')} -> {parsed.get('notAfter')}")
    print(f"served bytes    {len(pem)}")
    print(f"verification    {verify(host, port, os.path.expanduser(ca_file) if ca_file else NONE)}")
    print(f"now             {datetime.datetime.now(datetime.timezone.utc).isoformat(timespec='seconds')}")
    return NONE


sys.exit(main())
