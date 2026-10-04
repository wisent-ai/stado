import Foundation

struct CleanupResponse: Codable, Sendable {
    let ok: Bool
    let service: String
    let report: CleanupReport
}

/// The disk-full rule as a report states it: the threshold, how full the
/// volume was when the pass read it, and whether the rule deleted.
struct CleanupRule: Codable, Sendable {
    let fullPercent: Int
    let usedPercent: Double?
    let triggered: Bool

    enum CodingKeys: String, CodingKey {
        case triggered
        case fullPercent = "full_percent"
        case usedPercent = "used_percent"
    }

    /// The one line the Disk and Space screens show.
    var summary: String {
        let state = triggered ? "triggered" : "not triggered"
        guard let usedPercent else {
            return "Deletes everything the fleet put on this host at \(fullPercent)% used — volume not read"
        }
        return "Deletes everything the fleet put on this host at \(fullPercent)% used — volume \(String(format: "%.1f", usedPercent))% used (\(state))"
    }
}

struct CleanupReport: Codable, Sendable {
    let version: Int
    let rule: CleanupRule
    let startedAt: String?
    let durationMs: Int
    let outcome: String
    let totalBytes: Int?
    let freeBytesBefore: Int?
    let freeBytesAfter: Int?
    let pressureActive: Bool?
    let cleaners: CleanupCleaners?
    let lockBusy: Bool
    let activeJobCount: Int
    let lastSuccessAt: String?
    let errors: [String]

    enum CodingKeys: String, CodingKey {
        case version, rule, outcome, cleaners, errors
        case startedAt = "started_at"
        case durationMs = "duration_ms"
        case totalBytes = "total_bytes"
        case freeBytesBefore = "free_bytes_before"
        case freeBytesAfter = "free_bytes_after"
        case pressureActive = "pressure_active"
        case lockBusy = "lock_busy"
        case activeJobCount = "active_job_count"
        case lastSuccessAt = "last_success_at"
    }

    var reclaimedBytes: Int {
        guard let before = freeBytesBefore, let after = freeBytesAfter else { return 0 }
        return max(0, after - before)
    }

    var outcomePresentation: OutcomePresentation {
        switch outcome {
        case "never_run":
            OutcomePresentation(title: "No cleanup pass yet", detail: "The dashboard has no completed cleanup report.", symbol: "clock.badge.questionmark", severity: .neutral)
        case "healthy_noop":
            OutcomePresentation(title: "Under the threshold", detail: "The volume is under 80% used, so the pass deleted nothing.", symbol: "checkmark.circle.fill", severity: .healthy)
        case "reclaimed_below_threshold":
            OutcomePresentation(title: "Back under the threshold", detail: "The pass deleted what the fleet put on the host and the volume is under 80% used again.", symbol: "checkmark.circle.fill", severity: .healthy)
        case "still_full":
            OutcomePresentation(title: "Still full", detail: "The pass deleted what the fleet put on the host; the rest of the volume is the user's data.", symbol: "exclamationmark.triangle.fill", severity: .warning)
        case "report_only":
            OutcomePresentation(title: "Preview", detail: "The pass counted what a pass at the threshold would delete and deleted nothing.", symbol: "doc.text.magnifyingglass", severity: .neutral)
        case "lock_recovery_report_only":
            OutcomePresentation(title: "Lock held", detail: "A previous kernel lock is still held or could not be inspected. This pass records the cause without scanning or deleting.", symbol: "lock.trianglebadge.exclamationmark.fill", severity: .warning)
        case "blocked_running_jobs":
            OutcomePresentation(title: "Waiting for active work", detail: "The volume is full and the Hugging Face cache waits while jobs are running.", symbol: "pause.circle.fill", severity: .warning)
        case "no_eligible_items":
            OutcomePresentation(title: "Nothing of the fleet's left", detail: "The volume is full of data the rule never takes.", symbol: "exclamationmark.triangle.fill", severity: .warning)
        case "lock_busy", "lock_busy_unattributed":
            OutcomePresentation(title: "Cleanup already running", detail: "Another pass holds the cleanup lock.", symbol: "hourglass", severity: .neutral)
        case "lock_busy_workloads":
            OutcomePresentation(title: "Waiting for running jobs", detail: "Running jobs hold the cleanup lock. On a full volume, new jobs wait until a pass has run.", symbol: "hourglass", severity: .warning)
        case "partial_error":
            OutcomePresentation(title: "Cleanup incomplete", detail: "The pass completed with sanitized errors.", symbol: "exclamationmark.triangle.fill", severity: .critical)
        case "volume_unreadable":
            OutcomePresentation(title: "Volume unreadable", detail: "The janitor could not read the volume, so the rule could not be applied.", symbol: "xmark.shield.fill", severity: .critical)
        default:
            OutcomePresentation(title: outcome.humanizedIdentifier, detail: "The cleanup service returned this outcome.", symbol: "info.circle.fill", severity: .neutral)
        }
    }
}

struct CleanupCleaners: Codable, Sendable {
    private let reports: [String: CleanerReport]

    init(from decoder: Decoder) throws {
        reports = try decoder.singleValueContainer().decode([String: CleanerReport].self)
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(reports)
    }

    var namedReports: [(String, CleanerReport)] {
        reports.sorted { $0.key < $1.key }
    }
}

struct CleanerReport: Codable, Sendable {
    let scannedItems: Int
    let eligibleItems: Int
    let deletedItems: Int
    let expectedBytes: Int
    let actualFreeDeltaBytes: Int
    let skipped: [String: Int]

    enum CodingKeys: String, CodingKey {
        case skipped
        case scannedItems = "scanned_items"
        case eligibleItems = "eligible_items"
        case deletedItems = "deleted_items"
        case expectedBytes = "expected_bytes"
        case actualFreeDeltaBytes = "actual_free_delta_bytes"
    }
}

struct OutcomePresentation: Sendable {
    enum Severity: Sendable {
        case healthy
        case neutral
        case warning
        case critical
    }

    let title: String
    let detail: String
    let symbol: String
    let severity: Severity
}

extension String {
    var humanizedIdentifier: String {
        replacingOccurrences(of: "_", with: " ")
            .split(separator: " ")
            .map { $0.lowercased() }
            .joined(separator: " ")
            .capitalized
    }
}
