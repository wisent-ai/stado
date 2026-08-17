#!/usr/bin/env python3
"""Copy manifest-bound Cloud Storage objects into an Azure Blob container.

The manifest is the one produced by
`stado-rs/scripts/export-wisent-media-locators-host.sh`: it carries every
object key, its live database references, and the source size, MD5, CRC32C and
content type read from Cloud Storage.

Design constraints this script exists to satisfy:

* The paid window is the scarce resource. Every check that does not need the
  source body -- manifest parsing, Azure authentication, destination
  inventory, resume decisions -- happens before a single byte is read, so a
  billing-attached window contains transfer and nothing else.
* A copy is not done until the destination content is proven equal to the
  manifest. Size agreement is not proof; the MD5 recorded at export time is
  compared against the MD5 Azure computed on write.
* Re-running is safe and cheap. An object whose destination already matches
  the manifest is skipped without reading the source again, so an interrupted
  run resumes instead of restarting.
* Keys are preserved exactly. The destination key is the source key, so the
  delivery contract can be rebuilt without a second database rewrite.

Credentials are never accepted on the command line. Cloud Storage uses the
active gcloud account; Azure uses the signed-in Entra identity through
`az account get-access-token`, so shared-key access can stay disabled on the
storage account.
"""

from __future__ import annotations

import argparse
import base64
import concurrent.futures
import hashlib
import json
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

GCS_READ = "https://storage.googleapis.com/storage/v1/b/{bucket}/o/{key}?alt=media"
AZURE_BLOB = "https://{account}.blob.core.windows.net/{container}/{key}"
AZURE_LIST = (
    "https://{account}.blob.core.windows.net/{container}"
    "?restype=container&comp=list&maxresults=5000"
)
AZURE_VERSION = "2021-12-02"
RETRY_STATUS = frozenset({429, 500, 502, 503, 504})


class MigrationError(RuntimeError):
    """A failure that must stop the object it belongs to, never the run."""


def run_capture(command: list[str]) -> str:
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode != 0:
        raise MigrationError(
            f"{command[0]} failed: {result.stderr.strip().splitlines()[-1:] or ['unknown']}"
        )
    return result.stdout.strip()


def gcloud_token(gcloud: str, account: str) -> str:
    command = [gcloud, "auth", "print-access-token"]
    if account:
        command.append(f"--account={account}")
    return run_capture(command)


def azure_token(az: str) -> str:
    document = json.loads(
        run_capture(
            [
                az,
                "account",
                "get-access-token",
                "--resource",
                "https://storage.azure.com/",
                "--output",
                "json",
            ]
        )
    )
    token = document.get("accessToken")
    if not token:
        raise MigrationError("az returned no Azure Storage access token")
    return token


def request_json(url: str, headers: dict[str, str], timeout: int) -> bytes:
    last: Exception | None = None
    for attempt in range(5):
        try:
            with urllib.request.urlopen(
                urllib.request.Request(url, headers=headers), timeout=timeout
            ) as response:
                return response.read()
        except urllib.error.HTTPError as error:
            if error.code not in RETRY_STATUS:
                detail = error.read().decode(errors="replace")[:300]
                raise MigrationError(f"HTTP {error.code}: {detail}") from None
            last = error
        except (urllib.error.URLError, TimeoutError) as error:
            last = error
        time.sleep(min(2**attempt, 8))
    raise MigrationError(f"exhausted retries: {last}")


def destination_inventory(
    account: str, container: str, prefix: str, token: str, timeout: int
) -> dict[str, dict[str, Any]]:
    """Every destination blob under `prefix`, by key, with size and MD5.

    Read once up front: this is what lets an interrupted run resume without
    re-reading a single source object.
    """
    import xml.etree.ElementTree as ElementTree

    found: dict[str, dict[str, Any]] = {}
    marker = ""
    base = AZURE_LIST.format(account=account, container=container)
    if prefix:
        base += "&prefix=" + urllib.parse.quote(prefix)
    while True:
        url = base + (f"&marker={urllib.parse.quote(marker)}" if marker else "")
        body = request_json(
            url,
            {"Authorization": f"Bearer {token}", "x-ms-version": AZURE_VERSION},
            timeout,
        )
        root = ElementTree.fromstring(body)
        for blob in root.iter("Blob"):
            name = blob.findtext("Name") or ""
            properties = blob.find("Properties")
            if not name or properties is None:
                continue
            found[name] = {
                "size": int(properties.findtext("Content-Length") or 0),
                "md5": properties.findtext("Content-MD5") or "",
            }
        marker = root.findtext("NextMarker") or ""
        if not marker:
            return found


def already_correct(entry: dict[str, Any], present: dict[str, Any] | None) -> bool:
    if present is None:
        return False
    source = entry.get("source_metadata") or {}
    if int(source.get("size", -1)) != present["size"]:
        return False
    expected = source.get("md5_base64") or ""
    return bool(expected) and expected == present["md5"]


def copy_object(
    entry: dict[str, Any],
    *,
    bucket: str,
    account: str,
    container: str,
    gcs_token: str,
    blob_token: str,
    timeout: int,
) -> dict[str, Any]:
    key = entry["object_key"]
    source = entry.get("source_metadata") or {}
    body = request_json(
        GCS_READ.format(bucket=bucket, key=urllib.parse.quote(key, safe="")),
        {"Authorization": f"Bearer {gcs_token}"},
        timeout,
    )

    expected_size = int(source.get("size", -1))
    if expected_size >= 0 and len(body) != expected_size:
        raise MigrationError(
            f"source body is {len(body)} bytes, manifest says {expected_size}"
        )
    digest = base64.b64encode(hashlib.md5(body).digest()).decode()
    expected_md5 = source.get("md5_base64") or ""
    if expected_md5 and digest != expected_md5:
        raise MigrationError("source body MD5 disagrees with the manifest")

    content_type = source.get("content_type") or "application/octet-stream"
    headers = {
        "Authorization": f"Bearer {blob_token}",
        "x-ms-version": AZURE_VERSION,
        "x-ms-blob-type": "BlockBlob",
        "Content-Type": content_type,
        "Content-Length": str(len(body)),
        "x-ms-blob-content-md5": digest,
        "x-ms-meta-sourceuri": entry.get("source_uri", ""),
        "x-ms-meta-sourcegeneration": str(source.get("generation") or ""),
    }
    if source.get("cache_control"):
        headers["x-ms-blob-cache-control"] = source["cache_control"]

    url = AZURE_BLOB.format(
        account=account, container=container, key=urllib.parse.quote(key)
    )
    last: Exception | None = None
    for attempt in range(5):
        try:
            request = urllib.request.Request(
                url, data=body, headers=headers, method="PUT"
            )
            with urllib.request.urlopen(request, timeout=timeout) as response:
                if response.status not in (201, 202):
                    raise MigrationError(f"unexpected Azure status {response.status}")
                stored = response.headers.get("Content-MD5", "")
                if stored and stored != digest:
                    raise MigrationError("Azure stored a different MD5 than was sent")
                return {
                    "object_key": key,
                    "state": "copied",
                    "bytes": len(body),
                    "md5_base64": digest,
                }
        except urllib.error.HTTPError as error:
            if error.code not in RETRY_STATUS:
                detail = error.read().decode(errors="replace")[:300]
                raise MigrationError(f"Azure HTTP {error.code}: {detail}") from None
            last = error
        except (urllib.error.URLError, TimeoutError) as error:
            last = error
        time.sleep(min(2**attempt, 8))
    raise MigrationError(f"Azure write exhausted retries: {last}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--source-bucket", default="wisent-images-bucket")
    parser.add_argument("--account", required=True)
    parser.add_argument("--container", required=True)
    parser.add_argument("--key-prefix", default="")
    parser.add_argument("--gcloud", default="/opt/homebrew/share/google-cloud-sdk/bin/gcloud")
    parser.add_argument("--az", default="/opt/homebrew/bin/az")
    parser.add_argument("--gcloud-account", default="")
    parser.add_argument("--concurrency", type=int, default=8)
    parser.add_argument("--timeout", type=int, default=120)
    parser.add_argument("--limit", type=int, default=0, help="copy at most N objects")
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="inventory the destination and print the plan; read no source body",
    )
    parser.add_argument("--report", type=Path, default=None)
    args = parser.parse_args()

    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    entries = manifest.get("entries") or []
    if not entries:
        print("manifest carries no entries", file=sys.stderr)
        return 2

    blob_token = azure_token(args.az)
    present = destination_inventory(
        args.account, args.container, args.key_prefix, blob_token, args.timeout
    )

    pending = [e for e in entries if not already_correct(e, present.get(args.key_prefix + e["object_key"]))]
    settled = len(entries) - len(pending)
    pending_bytes = sum(int((e.get("source_metadata") or {}).get("size", 0)) for e in pending)
    print(
        f"manifest={len(entries)} verified_present={settled} to_copy={len(pending)} "
        f"to_copy_bytes={pending_bytes} destination={args.account}/{args.container}"
    )
    if args.dry_run:
        for entry in pending[:10]:
            print(f"  would copy {entry['object_key']}")
        if len(pending) > 10:
            print(f"  ... and {len(pending) - 10} more")
        return 0

    if args.limit:
        pending = pending[: args.limit]
    gcs = gcloud_token(args.gcloud, args.gcloud_account)

    results: list[dict[str, Any]] = []
    failures: list[dict[str, Any]] = []

    def work(entry: dict[str, Any]) -> dict[str, Any]:
        try:
            return copy_object(
                entry,
                bucket=args.source_bucket,
                account=args.account,
                container=args.container,
                gcs_token=gcs,
                blob_token=blob_token,
                timeout=args.timeout,
            )
        except MigrationError as error:
            return {"object_key": entry["object_key"], "state": "failed", "error": str(error)}

    with concurrent.futures.ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        for outcome in pool.map(work, pending):
            (failures if outcome["state"] == "failed" else results).append(outcome)

    copied_bytes = sum(r["bytes"] for r in results)
    print(f"copied={len(results)} bytes={copied_bytes} failed={len(failures)}")
    for failure in failures[:10]:
        print(f"  FAILED {failure['object_key']}: {failure['error']}", file=sys.stderr)

    if args.report:
        args.report.write_text(
            json.dumps(
                {
                    "source_bucket": args.source_bucket,
                    "destination": f"{args.account}/{args.container}",
                    "key_prefix": args.key_prefix,
                    "manifest_objects": len(entries),
                    "verified_present_before": settled,
                    "copied": results,
                    "failed": failures,
                },
                indent=2,
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )

    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
