"""Canonical write path for the compute-target registry.

Reads and writes are generation-checked against the single GCS document so
concurrent operators (CLI, dashboard, agents) never silently clobber each
other. Only whitelisted policy fields are mutable through this path —
everything else stays edit-by-registry-push so reviews stay meaningful.
"""
from __future__ import annotations

import json

from google.cloud import storage

from . import GCS_REGISTRY_URI
from .validation import validate_registry

# Per-target fields operator surfaces (dashboard, CLI) may mutate.
EDITABLE_DISK_FIELDS = {
    "mode", "check_interval_seconds", "low_free_gb", "target_free_gb",
    "max_bytes_per_pass", "max_items_per_pass", "max_scan_items",
}
EDITABLE_CLEANER_FIELDS = {"min_age_seconds", "allow_missing_upload_proof", "root"}
EDITABLE_SECTIONS = {"disk_cleanup", "weles", "pinned_only"}
KNOWN_CLEANERS = {"weles_recordings", "huggingface_cache"}


def _blob():
    _, remainder = GCS_REGISTRY_URI.split("//", 1)
    bucket_name, blob_name = remainder.split("/", 1)
    return storage.Client().bucket(bucket_name).blob(blob_name)


def load_registry() -> tuple[dict, int]:
    """Fetch the canonical registry and its generation, atomically."""
    blob = _blob()
    blob.reload()
    if blob.generation is None:
        raise OSError("canonical registry generation unavailable")
    generation = int(blob.generation)
    return json.loads(blob.download_as_text(if_generation_match=generation)), generation


def push_registry(data: dict, expect_generation: int) -> int:
    """Validate then upload with a generation precondition; returns the new generation."""
    validate_registry(data)
    blob = _blob()
    payload = json.dumps(data, indent=2).encode() + b"\n"
    blob.upload_from_string(payload, content_type="application/json",
                            if_generation_match=int(expect_generation))
    blob.reload()
    return int(blob.generation)


def update_target(name: str, updates: dict) -> dict:
    """Apply whitelisted partial updates to one registry target.

    Only EDITABLE_* fields are honored; anything else raises so a fat-fingered
    form or client bug can never silently rewrite a host's identity, ssh, or
    dispatch settings. weles.recordings_dir also moves the cleaner root so
    writer and cleaner never drift apart.
    """
    unknown_sections = set(updates) - EDITABLE_SECTIONS
    if unknown_sections:
        raise ValueError(f"unknown policy sections {sorted(unknown_sections)!r}")
    data, generation = load_registry()
    entries = [t for t in data.get("targets", []) if t.get("name") == name]
    if not entries:
        raise KeyError(f"target not in registry: {name}")
    entry = entries[0]

    if "pinned_only" in updates:
        pinned_only = updates["pinned_only"]
        if not isinstance(pinned_only, bool):
            raise ValueError("pinned_only must be a boolean")
        entry["pinned_only"] = pinned_only

    if "weles" in updates:
        weles_updates = updates["weles"]
        if not isinstance(weles_updates, dict) or set(weles_updates) - {"recordings_dir"}:
            raise ValueError("weles updates support only recordings_dir")
        recordings_dir = weles_updates.get("recordings_dir")
        if recordings_dir is not None and (not isinstance(recordings_dir, str) or not recordings_dir.startswith("/")):
            raise ValueError("weles.recordings_dir must be an absolute path string or null")
        weles = entry.setdefault("weles", {"enabled": True, "actions": ["*"]})
        weles["recordings_dir"] = recordings_dir
        cleanup = entry.get("disk_cleanup")
        if isinstance(cleanup, dict):
            cleaner = cleanup.setdefault("cleaners", {}).setdefault(
                "weles_recordings", {"min_age_seconds": 604800})
            cleaner["root"] = recordings_dir

    if "disk_cleanup" in updates:
        disk_updates = updates["disk_cleanup"]
        if not isinstance(disk_updates, dict):
            raise ValueError("disk_cleanup updates must be an object")
        unknown_fields = set(disk_updates) - EDITABLE_DISK_FIELDS - {"cleaners"}
        if unknown_fields:
            raise ValueError(f"unknown disk_cleanup fields {sorted(unknown_fields)!r}")
        cleanup = entry.setdefault("disk_cleanup", {})
        for key, value in disk_updates.items():
            if key != "cleaners":
                cleanup[key] = value
                continue
            if not isinstance(value, dict):
                raise ValueError("disk_cleanup.cleaners must be an object")
            for cleaner_name, cleaner_updates in value.items():
                if cleaner_name not in KNOWN_CLEANERS:
                    raise ValueError(f"unknown cleaner: {cleaner_name}")
                if not isinstance(cleaner_updates, dict) or set(cleaner_updates) - EDITABLE_CLEANER_FIELDS:
                    raise ValueError(f"unknown fields for cleaner {cleaner_name}")
                cleaner = cleanup.setdefault("cleaners", {}).setdefault(cleaner_name, {})
                cleaner.update(cleaner_updates)

    new_generation = push_registry(data, generation)
    return {"target": entry, "generation": new_generation}
