import Foundation

/// Recorded product payloads for the Space screens' cleaner tests.
///
/// Verbatim shapes, not invented ones. The listing is what
/// `stado space cleaners list charless-mac-mini --json` prints, the coverage
/// is the `coverage` object of `stado space report`, and the figures are that
/// host's on 2026-09-09: 56.3 GB under `~/.stado/local-storage`, 11.3 GB under
/// `~/.stado/local-backup`, 24.4 GB under `/private/var`, and a janitor pass
/// that ended `cap_reached` while the host was still short.
enum CleanerFixtures {
    /// One declared cleaner, one implemented cleaner the host has not
    /// declared, and one the installed binary predates.
    static let listing = """
    {
      "target": "charless-mac-mini",
      "installed_stado": "0.16.38",
      "declares_policy": true,
      "cleaners": [
        {
          "cleaner": "backup_twins",
          "declared": false,
          "declaration": null,
          "sweeps": "same-disk replica objects whose primary copy is intact",
          "default_root": ".stado/local-backup",
          "since": "0.13.0",
          "supported_by_installed_binary": true,
          "detail": "this product implements it and this host does not declare it; arm it with `stado space cleaners declare <target> --cleaner backup_twins`"
        },
        {
          "cleaner": "queue_workdirs",
          "declared": false,
          "declaration": null,
          "sweeps": "work trees of jobs the queue reports terminal",
          "default_root": ".stado/work/jobs",
          "since": "0.12.0",
          "supported_by_installed_binary": false,
          "detail": "undeclared, and the binary installed here (0.11.0) predates it: deliver at least 0.12.0 first"
        },
        {
          "cleaner": "release_store",
          "declared": true,
          "declaration": {"min_age_seconds": 0, "keep_newest": 2},
          "sweeps": "published release versions past the rollback ladder this host keeps",
          "default_root": ".stado/local-storage/ecosystem/releases",
          "since": "0.15.26",
          "supported_by_installed_binary": true,
          "detail": "declared: this host sweeps published release versions past the rollback ladder this host keeps"
        }
      ]
    }
    """

    /// The coverage section with all three kinds of row: swept by a declared
    /// cleaner, reachable by one nobody declared, and reached by nothing.
    static let coverage = """
    {
      "need_bytes": 7000000000,
      "deficit_bytes": 5000000000,
      "covered": [],
      "covered_bytes": 0,
      "uncovered": [
        {
          "path": "/private/var",
          "bytes": 24400000000,
          "mechanism": null,
          "mechanism_declared": false
        },
        {
          "path": "/Users/charles/.stado/local-storage",
          "bytes": 56300000000,
          "mechanism": "release_store",
          "mechanism_declared": true
        },
        {
          "path": "/Users/charles/.stado/local-backup",
          "bytes": 11300000000,
          "mechanism": "backup_twins",
          "mechanism_declared": false
        }
      ],
      "unarmed": [
        {
          "cleaner": "backup_twins",
          "root": "/Users/charles/.stado/local-backup",
          "bytes": 11300000000,
          "within_path": null,
          "within_bytes": null,
          "since": "0.13.0",
          "supported_by_installed_binary": true,
          "detail": "backup_twins sweeps same-disk replica objects whose primary copy is intact under /Users/charles/.stado/local-backup and this host does not declare it"
        }
      ],
      "uncovered_bytes": 92000000000,
      "cleaner_bytes": 67600000000,
      "unswept_bytes": 24400000000,
      "verdict": "uncovered",
      "detail": "6.5 GiB short of the declared target",
      "janitor": {"outcome": "cap_reached", "detail": "the last pass ended cap_reached"}
    }
    """

    /// What a `stado` that predates the mechanism fields answers, so a Desktop
    /// meeting an older host still renders the rest of the report.
    static let coverageWithoutMechanisms = """
    {
      "need_bytes": null,
      "deficit_bytes": null,
      "covered": [],
      "covered_bytes": 0,
      "uncovered": [{"path": "/private/var", "bytes": 24400000000}],
      "uncovered_bytes": 24400000000,
      "verdict": "undeclared",
      "detail": "this target declares no free-space watermark",
      "janitor": {"outcome": "never_run", "detail": "the last pass ended never_run"}
    }
    """
}
