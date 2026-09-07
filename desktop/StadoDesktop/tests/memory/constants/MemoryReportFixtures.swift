import Foundation

/// Recorded dashboard payloads for the Memory screen's tests.
///
/// Verbatim shapes, not invented ones: the cleanup envelope is what
/// `GET /api/cleanup.json` answers with a memory pass installed, the readings
/// are charless-mac-mini's on 2026-09-06 (1.3 GB available with 86% of swap
/// in use, 797k compressor pages, 12.2M lifetime swapouts), and the registry
/// projection is what `GET /api/registry.json` returns for a target that
/// declares `memory_reclaim`.
enum MemoryReportFixtures {
    /// The dashboard's cleanup envelope with one memory block inside it, so
    /// the report is decoded through the same envelope the screen reads.
    static func envelope(memory: String) -> String {
        """
        {"ok": true, "service": "cleanup", "report": {
            "version": 3,
            "outcome": "healthy_noop",
            "duration_ms": 18,
            "cleaners": {},
            "caps": {"bytes": false, "items": false, "scan": false, "deadline": false},
            "lock_busy": false,
            "active_job_count": 0,
            "errors": [],
            "memory_reclaim": \(memory)
        }}
        """
    }

    /// A host that declares no `memory_reclaim`: `policy_defaulted` is true,
    /// the repair table is null because the pass repaired nothing, and the
    /// watermarks are the ones the writer resolved from the default.
    static let defaulted = envelope(
        memory: """
        {"hostname": "charless-mac-mini", "target_name": "mac-mini",
         "writer": "janitor", "writer_version": "0.9.3",
         "policy_defaulted": true, "mode": "report", "check_interval_seconds": 300,
         "started_at": "2026-09-07T04:15:00Z", "duration_ms": 41, "outcome": "report_only",
         "memory_before": {"available_bytes": 1363148800, "available_mb": 1300,
             "total_bytes": 34359738368, "swap_used_bytes": 4617089024,
             "swap_total_bytes": 5368709120, "swap_used_pct": 86,
             "compressor_pages": 797000, "swapouts": 12200000},
         "memory_after": null, "low_bytes": 943718400, "target_bytes": 1572864000,
         "high_swap_used_pct": 80, "pressure_active": true, "refuse_placement": false,
         "repairs": null, "caps": {"repairs": false, "deadline": false},
         "lock_busy": false, "active_job_count": 0, "last_success_at": null, "errors": []}
        """
    )

    /// A host on which no pass has ever completed: every numeric field is
    /// absent and the outcome says so.
    static let neverRun = envelope(
        memory: """
        {"hostname": "charless-mac-mini", "target_name": "mac-mini",
         "policy_defaulted": true, "outcome": "never_run",
         "memory_before": {}, "memory_after": null, "repairs": null, "errors": []}
        """
    )

    /// A host over its watermark that declares `refuse_placement`, with one
    /// declared repair reported.
    static let refusing = envelope(
        memory: """
        {"hostname": "charless-mac-mini", "target_name": "mac-mini",
         "writer": "queue-agent", "writer_version": "0.9.3",
         "policy_defaulted": false, "mode": "enforce", "started_at": "2026-09-07T04:20:00Z",
         "duration_ms": 260, "outcome": "reclaimed_progress",
         "memory_before": {"available_bytes": 943718400, "available_mb": 900,
             "swap_used_bytes": 4617089024, "swap_total_bytes": 5368709120, "swap_used_pct": 86},
         "memory_after": {"available_bytes": 1258291200, "available_mb": 1200,
             "swap_used_bytes": 4400000000, "swap_total_bytes": 5368709120, "swap_used_pct": 81},
         "low_bytes": 1048576000, "target_bytes": 1572864000, "high_swap_used_pct": 80,
         "pressure_active": true, "refuse_placement": true,
         "repairs": {"restart_unit": {"examined": 3, "eligible": 1, "repaired": 1,
             "skipped": {"not_declared": 1, "younger_than_min_age": 1},
             "subjects": ["ai.wisent.precheck-runner"]}},
         "caps": {"repairs": true, "deadline": false},
         "lock_busy": false, "active_job_count": 0,
         "last_success_at": "2026-09-07T04:20:00Z", "errors": []}
        """
    )

    /// The same declared refusal on a host that is comfortably under both
    /// watermarks, which is not a refusal.
    static let declaredRefusalClear = envelope(
        memory: """
        {"hostname": "charless-mac-mini", "target_name": "mac-mini", "mode": "enforce",
         "outcome": "healthy_noop", "policy_defaulted": false,
         "memory_before": {"available_bytes": 8589934592, "available_mb": 8192,
             "swap_used_bytes": 1073741824, "swap_total_bytes": 5368709120, "swap_used_pct": 20},
         "low_bytes": 1048576000, "target_bytes": 1572864000, "high_swap_used_pct": 80,
         "pressure_active": false, "refuse_placement": true, "repairs": {}, "errors": []}
        """
    )

    /// The registry projection: one target that declares `memory_reclaim`
    /// with a single armed repair, and one that declares nothing.
    static let registryProjection = """
    {"generation": 41, "targets": [
        {"name": "mac-mini", "pinned_only": false,
         "memory_reclaim": {"mode": "report", "check_interval_seconds": 300,
             "low_free_mb": 900, "target_free_mb": 1500, "high_swap_used_pct": 80,
             "max_repairs_per_pass": 2, "refuse_placement": false,
             "repairs": {"restart_unit": {"units": ["ai.wisent.precheck-runner"],
                 "min_age_seconds": 120}}}},
        {"name": "control-host"}
    ]}
    """

    /// The exact body the editor must post for the edit the write test makes:
    /// enforce mode, a raised low watermark, a lowered swap watermark and a
    /// placement refusal — and nothing else.
    static let expectedPatchBody = """
    {"target": "mac-mini", "memory_reclaim": {"mode": "enforce", "low_free_mb": 1200, "high_swap_used_pct": 70, "refuse_placement": true}}
    """
}
