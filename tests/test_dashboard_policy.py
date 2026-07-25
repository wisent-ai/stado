"""Dashboard registry policy surface: whitelisted writes through the canonical store."""
from __future__ import annotations

import json
import unittest
from unittest.mock import patch

from stado.dashboard_policy import apply_policy_update, policy_view
from stado.targets import store
from stado.targets.validation import RegistryValidationError

REGISTRY = {
    "schema_version": 2,
    "targets": [
        {
            "name": "charless-mac-mini",
            "kind": "local",
            "hostnames": ["charless-mac-mini.local"],
            "weles": {"enabled": True, "actions": ["*"]},
            "disk_cleanup": {
                "mode": "enforce",
                "check_interval_seconds": 300,
                "low_free_gb": 55,
                "target_free_gb": 60,
                "max_bytes_per_pass": 21474836480,
                "max_items_per_pass": 100,
                "max_scan_items": 10000,
                "cleaners": {
                    "huggingface_cache": {"min_age_seconds": 2592000},
                    "weles_recordings": {
                        "min_age_seconds": 604800,
                        "allow_missing_upload_proof": True,
                    },
                },
            },
        }
    ],
}


class FakeBlob:
    """In-memory stand-in for the canonical GCS blob with generation checks."""

    def __init__(self):
        self.generation = 7
        self.uploaded = None

    def reload(self):
        if self.uploaded is not None:
            self.generation = 8

    def download_as_text(self, if_generation_match=None):
        assert if_generation_match == 7, "stale read generation"
        return json.dumps(REGISTRY)

    def upload_from_string(self, payload, content_type=None, if_generation_match=None):
        assert if_generation_match == 7, "write precondition mismatch"
        self.uploaded = json.loads(payload)


def _patched(blob):
    return patch.object(store, "_blob", return_value=blob)


class PolicyViewTests(unittest.TestCase):
    def test_view_exposes_policy_without_secrets(self):
        with _patched(FakeBlob()):
            view = policy_view()
        target = view["targets"][0]
        self.assertEqual(target["name"], "charless-mac-mini")
        self.assertEqual(target["disk_cleanup"]["low_free_gb"], 55)
        self.assertIn("weles", target)
        self.assertNotIn("ssh", target)


class ApplyPolicyUpdateTests(unittest.TestCase):
    def test_rejects_missing_target(self):
        with self.assertRaises(ValueError):
            apply_policy_update({"disk_cleanup": {"low_free_gb": 5}})

    def test_rejects_empty_sections(self):
        with self.assertRaises(ValueError):
            apply_policy_update({"target": "charless-mac-mini"})

    def test_rejects_unknown_section(self):
        with _patched(FakeBlob()), self.assertRaises(ValueError):
            apply_policy_update({"target": "charless-mac-mini", "ssh": "root@box"})

    def test_rejects_unknown_disk_field(self):
        with _patched(FakeBlob()), self.assertRaises(ValueError):
            apply_policy_update({"target": "charless-mac-mini",
                                 "disk_cleanup": {"slots": 4}})

    def test_rejects_invalid_value_type(self):
        with _patched(FakeBlob()), self.assertRaises(RegistryValidationError):
            apply_policy_update({"target": "charless-mac-mini",
                                 "disk_cleanup": {"low_free_gb": "lots"}})

    def test_updates_whitelisted_fields_and_preserves_the_rest(self):
        blob = FakeBlob()
        with _patched(blob):
            result = apply_policy_update({
                "target": "charless-mac-mini",
                "disk_cleanup": {"low_free_gb": 40, "max_items_per_pass": 50},
                "pinned_only": True,
            })
        entry = blob.uploaded["targets"][0]
        self.assertEqual(entry["disk_cleanup"]["low_free_gb"], 40)
        self.assertEqual(entry["disk_cleanup"]["max_items_per_pass"], 50)
        self.assertEqual(entry["disk_cleanup"]["target_free_gb"], 60)
        self.assertTrue(entry["pinned_only"])
        self.assertEqual(result["generation"], 8)

    def test_recordings_dir_moves_the_cleaner_root(self):
        blob = FakeBlob()
        with _patched(blob):
            apply_policy_update({"target": "charless-mac-mini",
                                 "weles": {"recordings_dir": "/data/recordings"}})
        entry = blob.uploaded["targets"][0]
        self.assertEqual(entry["weles"]["recordings_dir"], "/data/recordings")
        cleaner = entry["disk_cleanup"]["cleaners"]["weles_recordings"]
        self.assertEqual(cleaner["root"], "/data/recordings")

    def test_cleaner_updates_stay_whitelisted(self):
        blob = FakeBlob()
        with _patched(blob):
            apply_policy_update({
                "target": "charless-mac-mini",
                "disk_cleanup": {"cleaners": {"weles_recordings": {
                    "min_age_seconds": 1209600,
                    "allow_missing_upload_proof": False,
                }}},
            })
        cleaner = blob.uploaded["targets"][0]["disk_cleanup"]["cleaners"]["weles_recordings"]
        self.assertEqual(cleaner["min_age_seconds"], 1209600)
        self.assertFalse(cleaner["allow_missing_upload_proof"])


if __name__ == "__main__":
    unittest.main()
