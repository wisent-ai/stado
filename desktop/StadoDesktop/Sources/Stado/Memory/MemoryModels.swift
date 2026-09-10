import Foundation

/// One host's memory as the pass read it, decoded from the `memory_reclaim`
/// block of `GET /api/cleanup.json` — and, unchanged, from the `memory` block
/// of a host beacon, because both carry the report the pass wrote.
///
/// Every numeric field is optional because the host readers are honest: a
/// machine whose kernel would not answer `swap_total_bytes` reports the field
/// as absent, and a console that renders an absent reading as zero has
/// invented a measurement.
struct MemoryReadingSnapshot: Codable, Sendable {
    let availableBytes: Int?
    let availableMB: Int?
    let totalBytes: Int?
    let swapUsedBytes: Int?
    let swapTotalBytes: Int?
    let swapUsedPct: Int?
    /// macOS only: pages held by the compressor.
    let compressorPages: Int?
    /// macOS only: lifetime swapouts.
    let swapouts: Int?

    enum CodingKeys: String, CodingKey {
        case swapouts
        case availableBytes = "available_bytes"
        case availableMB = "available_mb"
        case totalBytes = "total_bytes"
        case swapUsedBytes = "swap_used_bytes"
        case swapTotalBytes = "swap_total_bytes"
        case swapUsedPct = "swap_used_pct"
        case compressorPages = "compressor_pages"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        availableBytes = try values.decodeIfPresent(Int.self, forKey: .availableBytes)
        availableMB = try values.decodeIfPresent(Int.self, forKey: .availableMB)
        totalBytes = try values.decodeIfPresent(Int.self, forKey: .totalBytes)
        swapUsedBytes = try values.decodeIfPresent(Int.self, forKey: .swapUsedBytes)
        swapTotalBytes = try values.decodeIfPresent(Int.self, forKey: .swapTotalBytes)
        swapUsedPct = try values.decodeIfPresent(Int.self, forKey: .swapUsedPct)
        compressorPages = try values.decodeIfPresent(Int.self, forKey: .compressorPages)
        swapouts = try values.decodeIfPresent(Int.self, forKey: .swapouts)
    }

    /// Whether this platform answered the compressor counters at all.
    var reportsCompressor: Bool { compressorPages != nil || swapouts != nil }
}

/// What one declared repair did on one pass.
struct MemoryRepairReport: Codable, Sendable {
    let examined: Int
    let eligible: Int
    let repaired: Int
    /// Why a subject was not acted on, counted by the pass's own reason word.
    let skipped: [String: Int]
    /// The declared subjects this repair named, so a `report` pass still says
    /// what an `enforce` pass would touch.
    let subjects: [String]

    enum CodingKeys: String, CodingKey {
        case examined, eligible, repaired, skipped, subjects
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        examined = try values.decodeIfPresent(Int.self, forKey: .examined) ?? Int.zero
        eligible = try values.decodeIfPresent(Int.self, forKey: .eligible) ?? Int.zero
        repaired = try values.decodeIfPresent(Int.self, forKey: .repaired) ?? Int.zero
        skipped = try values.decodeIfPresent([String: Int].self, forKey: .skipped) ?? [:]
        subjects = try values.decodeIfPresent([String].self, forKey: .subjects) ?? []
    }

    var sortedSkipped: [(String, Int)] {
        skipped.sorted { $0.key < $1.key }.map { ($0.key, $0.value) }
    }
}

/// Which per-pass budget stopped the pass.
struct MemoryCapsReport: Codable, Sendable {
    let repairs: Bool
    let deadline: Bool

    enum CodingKeys: String, CodingKey {
        case repairs, deadline
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        repairs = try values.decodeIfPresent(Bool.self, forKey: .repairs) ?? false
        deadline = try values.decodeIfPresent(Bool.self, forKey: .deadline) ?? false
    }

    var activeLabels: [String] {
        [
            repairs ? "repair budget" : nil,
            deadline ? "pass deadline" : nil,
        ].compactMap { $0 }
    }
}

/// The whole answer one memory pass wrote.
struct MemoryReclaimReport: Codable, Sendable {
    /// The admission reason a host publishes while it is over its memory
    /// watermark and its declaration refuses placement.
    static let admissionReason = "memory_pressure_active"
    /// The outcome word of a host on which no pass has ever completed.
    static let neverRun = "never_run"

    let hostname: String?
    let targetName: String?
    let writer: String?
    let writerVersion: String?
    /// True when the host declares no `memory_reclaim` and is measured
    /// against the reporting default. `mode: report` alone cannot tell a
    /// deliberate choice from an absent declaration.
    let policyDefaulted: Bool
    let mode: String?
    let checkIntervalSeconds: Int?
    let startedAt: String?
    let durationMs: Int?
    let outcome: String
    let before: MemoryReadingSnapshot?
    let after: MemoryReadingSnapshot?
    let lowBytes: Int?
    let targetBytes: Int?
    let highSwapUsedPct: Int?
    let maxRepairsPerPass: Int?
    let pressureActive: Bool?
    let refusePlacement: Bool
    /// Absent — not empty — on a pass that never reached its repairs. A table
    /// of zeros and a pass that stopped at the interval gate are different
    /// facts.
    let repairs: [String: MemoryRepairReport]?
    let caps: MemoryCapsReport?
    let lockBusy: Bool
    let activeJobCount: Int?
    let lastSuccessAt: String?
    let errors: [String]

    enum CodingKeys: String, CodingKey {
        case hostname, writer, mode, outcome, repairs, caps, errors
        case targetName = "target_name"
        case writerVersion = "writer_version"
        case policyDefaulted = "policy_defaulted"
        case checkIntervalSeconds = "check_interval_seconds"
        case startedAt = "started_at"
        case durationMs = "duration_ms"
        case before = "memory_before"
        case after = "memory_after"
        case lowBytes = "low_bytes"
        case targetBytes = "target_bytes"
        case highSwapUsedPct = "high_swap_used_pct"
        case maxRepairsPerPass = "max_repairs_per_pass"
        case pressureActive = "pressure_active"
        case refusePlacement = "refuse_placement"
        case lockBusy = "lock_busy"
        case activeJobCount = "active_job_count"
        case lastSuccessAt = "last_success_at"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        hostname = try values.decodeIfPresent(String.self, forKey: .hostname)
        targetName = try values.decodeIfPresent(String.self, forKey: .targetName)
        writer = try values.decodeIfPresent(String.self, forKey: .writer)
        writerVersion = try values.decodeIfPresent(String.self, forKey: .writerVersion)
        policyDefaulted = try values.decodeIfPresent(Bool.self, forKey: .policyDefaulted) ?? false
        mode = try values.decodeIfPresent(String.self, forKey: .mode)
        checkIntervalSeconds = try values.decodeIfPresent(Int.self, forKey: .checkIntervalSeconds)
        startedAt = try values.decodeIfPresent(String.self, forKey: .startedAt)
        durationMs = try values.decodeIfPresent(Int.self, forKey: .durationMs)
        outcome = try values.decodeIfPresent(String.self, forKey: .outcome) ?? Self.neverRun
        before = try values.decodeIfPresent(MemoryReadingSnapshot.self, forKey: .before)
        after = try values.decodeIfPresent(MemoryReadingSnapshot.self, forKey: .after)
        lowBytes = try values.decodeIfPresent(Int.self, forKey: .lowBytes)
        targetBytes = try values.decodeIfPresent(Int.self, forKey: .targetBytes)
        highSwapUsedPct = try values.decodeIfPresent(Int.self, forKey: .highSwapUsedPct)
        maxRepairsPerPass = try values.decodeIfPresent(Int.self, forKey: .maxRepairsPerPass)
        pressureActive = try values.decodeIfPresent(Bool.self, forKey: .pressureActive)
        refusePlacement = try values.decodeIfPresent(Bool.self, forKey: .refusePlacement) ?? false
        repairs = try values.decodeIfPresent([String: MemoryRepairReport].self, forKey: .repairs)
        caps = try values.decodeIfPresent(MemoryCapsReport.self, forKey: .caps)
        lockBusy = try values.decodeIfPresent(Bool.self, forKey: .lockBusy) ?? false
        activeJobCount = try values.decodeIfPresent(Int.self, forKey: .activeJobCount)
        lastSuccessAt = try values.decodeIfPresent(String.self, forKey: .lastSuccessAt)
        errors = try values.decodeIfPresent([String].self, forKey: .errors) ?? []
    }

    /// The reading the operator is shown: what the pass left behind when it
    /// re-read the host, otherwise what it found.
    var currentReading: MemoryReadingSnapshot? { after ?? before }

    var hasEverRun: Bool { outcome != Self.neverRun }

    /// Whether the pass reached its repairs at all.
    var examinedRepairs: Bool { repairs != nil }

    var namedRepairs: [(String, MemoryRepairReport)] {
        (repairs ?? [:]).sorted { $0.key < $1.key }.map { ($0.key, $0.value) }
    }

    /// This host has stopped accepting new jobs: its declaration refuses
    /// placement and it is over a watermark right now.
    var isRefusingPlacement: Bool { refusePlacement && pressureActive == true }

    var outcomePresentation: OutcomePresentation {
        switch outcome {
        case Self.neverRun:
            OutcomePresentation(title: "No memory pass yet", detail: "No memory pass has completed on this host.", symbol: "clock.badge.questionmark", severity: .neutral)
        case "healthy_noop":
            OutcomePresentation(title: "Healthy", detail: "The host is under both declared watermarks.", symbol: "checkmark.circle.fill", severity: .healthy)
        case "reclaimed_target":
            OutcomePresentation(title: "Target restored", detail: "Repairs returned memory and the host reached its declared target.", symbol: "checkmark.circle.fill", severity: .healthy)
        case "reclaimed_progress":
            OutcomePresentation(title: "Memory returned", detail: "Repairs returned memory; the declared target needs another pass.", symbol: "arrow.up.circle.fill", severity: .warning)
        case "interval_noop":
            OutcomePresentation(title: "Checked recently", detail: "The declared interval between passes has not elapsed.", symbol: "clock.fill", severity: .neutral)
        case "report_only":
            OutcomePresentation(title: "Report only", detail: "The host is over a watermark and its mode repairs nothing.", symbol: "doc.text.magnifyingglass", severity: .warning)
        case "blocked_running_jobs":
            OutcomePresentation(title: "Waiting for active work", detail: "The host is over a watermark and jobs a repair would disturb are running.", symbol: "pause.circle.fill", severity: .warning)
        case "cap_reached":
            OutcomePresentation(title: "Pass budget reached", detail: "The per-pass repair budget stopped this pass.", symbol: "gauge.with.dots.needle.67percent", severity: .warning)
        case "no_eligible_items":
            OutcomePresentation(title: "No eligible subject", detail: "The host is over a watermark and no declared repair matched anything.", symbol: "exclamationmark.triangle.fill", severity: .warning)
        case "lock_busy":
            OutcomePresentation(title: "Another writer has the pass", detail: "The janitor unit or the queue agent holds the pass lock.", symbol: "hourglass", severity: .neutral)
        case "partial_error":
            OutcomePresentation(title: "Pass incomplete", detail: "A repair was attempted and failed; the rest of the pass still ran.", symbol: "exclamationmark.triangle.fill", severity: .critical)
        case "invalid_or_unavailable_policy":
            OutcomePresentation(title: "Declaration unavailable", detail: "The pass refused because the canonical declaration could not be read or executed.", symbol: "xmark.shield.fill", severity: .critical)
        default:
            OutcomePresentation(title: outcome.humanizedIdentifier, detail: "The memory pass returned this outcome.", symbol: "info.circle.fill", severity: .neutral)
        }
    }
}
