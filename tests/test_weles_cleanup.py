"""Weles recordings cleaner: proof-bypass opt-in, active-run guard, root override."""
import os
import time
from pathlib import Path

from stado.providers.local.disk.cleanup import _base_report, _scan_weles
from stado.targets import DiskCleanerPolicy, DiskCleanupPolicy, _from_dict
from stado.targets.validation import validate_registry

DAY = 86400
NOW = time.time()


def _policy(mode="enforce", allow=False, root=None, target_free_gb=10 ** 6):
    return DiskCleanupPolicy(
        mode=mode,
        check_interval_seconds=300,
        low_free_gb=0,
        target_free_gb=target_free_gb,
        max_bytes_per_pass=20 * 1024 ** 3,
        max_items_per_pass=5,
        max_scan_items=10000,
        cleaners={"weles_recordings": DiskCleanerPolicy(
            min_age_seconds=7 * DAY,
            allow_missing_upload_proof=allow,
            root=root,
        )},
    )


def _make_run(home: Path, name: str, age_days: int, fresh_child=False, payload=1024):
    run = Path(home) / "weles" / "recordings" / name
    run.mkdir(parents=True)
    blob = run / "netlog.json"
    blob.write_bytes(b"x" * payload)
    child_time = NOW - (1 * DAY if fresh_child else age_days * DAY)
    os.utime(blob, (child_time, child_time))
    old = NOW - age_days * DAY
    os.utime(run, (old, old))
    return run


def _report():
    return _base_report(0)


def test_default_keeps_unproven_recordings(tmp_path):
    _make_run(tmp_path, "old-run", 30)
    report = _report()
    _scan_weles(tmp_path, _policy(allow=False), NOW, 100, report)
    assert (tmp_path / "weles" / "recordings" / "old-run").is_dir()
    assert report["cleaners"]["weles_recordings"]["skipped"]["upload_proof_unavailable_v1"] == 1


def test_opt_in_deletes_old_recordings(tmp_path):
    _make_run(tmp_path, "old-run", 30)
    report = _report()
    _scan_weles(tmp_path, _policy(allow=True), NOW, 100, report)
    assert not (tmp_path / "weles" / "recordings" / "old-run").exists()
    assert report["cleaners"]["weles_recordings"]["deleted_items"] == 1


def test_opt_in_keeps_young_recordings(tmp_path):
    _make_run(tmp_path, "young-run", 2)
    report = _report()
    _scan_weles(tmp_path, _policy(allow=True), NOW, 100, report)
    assert (tmp_path / "weles" / "recordings" / "young-run").is_dir()
    assert report["cleaners"]["weles_recordings"]["skipped"]["too_young"] == 1


def test_active_run_survives_old_dir_mtime(tmp_path):
    _make_run(tmp_path, "live-run", 30, fresh_child=True)
    report = _report()
    _scan_weles(tmp_path, _policy(allow=True), NOW, 100, report)
    assert (tmp_path / "weles" / "recordings" / "live-run").is_dir()
    assert report["cleaners"]["weles_recordings"]["skipped"]["active_run"] == 1


def test_reserved_local_dir_never_scanned_for_delete(tmp_path):
    _make_run(tmp_path, "local", 30)
    report = _report()
    _scan_weles(tmp_path, _policy(allow=True), NOW, 100, report)
    assert (tmp_path / "weles" / "recordings" / "local").is_dir()
    assert report["cleaners"]["weles_recordings"]["skipped"]["reserved_or_hidden"] == 1


def test_root_override_is_honored(tmp_path):
    custom = tmp_path / "elsewhere"
    run = custom / "old-run"
    run.mkdir(parents=True)
    child = run / "f"
    child.write_bytes(b"x" * 64)
    old = NOW - 30 * DAY
    os.utime(child, (old, old))
    os.utime(run, (old, old))
    report = _report()
    _scan_weles(tmp_path, _policy(allow=True, root=str(custom)), NOW, 100, report)
    assert not run.exists()
    assert report["cleaners"]["weles_recordings"]["deleted_items"] == 1


def test_report_only_mode_never_deletes(tmp_path):
    _make_run(tmp_path, "old-run", 30)
    report = _report()
    _scan_weles(tmp_path, _policy(mode="report", allow=True), NOW, 100, report)
    assert (tmp_path / "weles" / "recordings" / "old-run").is_dir()
    assert report["cleaners"]["weles_recordings"]["eligible_items"] == 1


def test_target_parsing_round_trips_new_fields():
    t = _from_dict({
        "name": "h", "kind": "local",
        "weles": {"enabled": True, "actions": ["*"], "recordings_dir": "/data/rec"},
        "disk_cleanup": {
            "mode": "enforce", "check_interval_seconds": 300,
            "low_free_gb": 2, "target_free_gb": 60,
            "max_bytes_per_pass": 1, "max_items_per_pass": 5, "max_scan_items": 10,
            "cleaners": {"weles_recordings": {
                "min_age_seconds": 604800,
                "allow_missing_upload_proof": True,
                "root": "/data/rec",
            }},
        },
    })
    assert t.weles.recordings_dir == "/data/rec"
    cleaner = t.disk_cleanup.cleaners["weles_recordings"]
    assert cleaner.allow_missing_upload_proof is True
    assert cleaner.root == "/data/rec"


def test_validation_accepts_new_keys():
    validate_registry({"schema_version": 2, "targets": [{
        "name": "h", "kind": "local",
        "weles": {"enabled": True, "actions": ["*"], "recordings_dir": "/data/rec"},
        "disk_cleanup": {
            "mode": "enforce", "check_interval_seconds": 300,
            "low_free_gb": 2, "target_free_gb": 60,
            "max_bytes_per_pass": 1024 ** 2, "max_items_per_pass": 5, "max_scan_items": 10,
            "cleaners": {"weles_recordings": {
                "min_age_seconds": 604800,
                "allow_missing_upload_proof": True,
                "root": "/data/rec",
            }},
        },
    }]})
