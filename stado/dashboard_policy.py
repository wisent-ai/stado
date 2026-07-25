"""Registry policy surface for the dashboard: sanitized reads + whitelisted writes."""
from __future__ import annotations

from typing import Any

from .targets.store import load_registry, update_target


def policy_view() -> dict[str, Any]:
    """Per-target policy state safe to render in the dashboard."""
    data, generation = load_registry()
    targets = []
    for entry in data.get("targets", []):
        weles = entry.get("weles") if isinstance(entry.get("weles"), dict) else None
        targets.append({
            "name": entry.get("name"),
            "kind": entry.get("kind"),
            "pinned_only": bool(entry.get("pinned_only", False)),
            "disk_cleanup": entry.get("disk_cleanup"),
            "weles": {"recordings_dir": weles.get("recordings_dir")} if weles else None,
        })
    return {"generation": generation, "targets": targets}


def apply_policy_update(payload: dict[str, Any]) -> dict[str, Any]:
    """Validate the dashboard payload shape, then apply via the canonical store."""
    if not isinstance(payload, dict):
        raise ValueError("payload must be a JSON object")
    target = payload.get("target")
    if not isinstance(target, str) or not target.strip():
        raise ValueError("payload.target must be a non-empty string")
    updates = {key: payload[key] for key in ("disk_cleanup", "weles", "pinned_only") if key in payload}
    if not updates:
        raise ValueError("no policy sections in payload (disk_cleanup | weles | pinned_only)")
    return update_target(target.strip(), updates)
