import Foundation

// MARK: - Release evidence

/// `stado release status --json`, read for two things: which products roll out
/// to which targets, and what each target's own software report says.
///
/// The rollout's *progress* still comes from `release doctor`, one call per pair,
/// because only that command reaches the host and reads the state file, the
/// candidate and the claiming gates. The software verdict comes from here and is
/// not recomputed: the CLI already decided it, in the same words it prints, so
/// this console reads a verdict rather than growing a second opinion about what
/// `unmanaged` means.
struct ReleaseInventory: Decodable, Sendable {
    let entries: [ReleaseInventoryEntry]
    /// The newest pipeline runs, exactly as `release status --json` reports
    /// them: identity, state, per-platform job states, and the persisted
    /// failure of anything that died. Absent in older CLI payloads.
    let runs: [ReleasePipelineRunRecord]

    var pairs: [ReleaseInventoryPair] { entries.map(\.pair) }

    enum CodingKeys: String, CodingKey {
        case entries = "targets"
        case runs
    }

    /// `release status --json` writes `targets` and `runs` together, and its
    /// run-only answer (`--run`, `--version`) writes `runs` alone: that answer
    /// is about no target, so its target list is the empty one.
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        switch try values.decodeIfPresent([ReleaseInventoryEntry].self, forKey: .entries) {
        case .some(let listed): entries = listed
        case .none: entries = []
        }
        runs = try values.decode([ReleasePipelineRunRecord].self, forKey: .runs)
    }
}

/// One release-pipeline run: what was submitted, where it stands, and — when a
/// platform or the whole run died — the recorded failure with the job's own
/// last output lines. This mirrors the run object the pipeline persists, so
/// the screen shows the store's truth, not a paraphrase.
struct ReleasePipelineRunRecord: Decodable, Sendable, Identifiable {
    let runID: String
    let sourceCommit: String
    let product: String
    let version: String
    let channel: String
    let state: String
    /// Where the run stands, as `release status --json` decides it:
    /// `failed`, `published` or `in_flight`. The screen used to re-decide
    /// this by matching state words, which is Stado's list in a second
    /// language; an older Stado that does not report it leaves this empty.
    let phase: String
    let updatedAt: String
    let failure: String?
    let platforms: [String: PlatformLeg]
    let deliveries: [String: DeliveryLeg]
    let diagnosticDetails: String?
    let hasFailure: Bool

    var id: String { runID }

    enum CodingKeys: String, CodingKey {
        case runID = "run_id"
        case sourceCommit = "source_commit"
        case product
        case version
        case channel
        case state
        case phase
        case updatedAt = "updated_at"
        case failure
        case platforms
        case deliveries
    }

    /// Every field is decoded as the run object records it and the listing
    /// settles it: a run that lacks one is a decoding error naming the key.
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        runID = try values.decode(String.self, forKey: .runID)
        sourceCommit = try values.decode(String.self, forKey: .sourceCommit)
        product = try values.decode(String.self, forKey: .product)
        version = try values.decode(String.self, forKey: .version)
        channel = try values.decode(String.self, forKey: .channel)
        state = try values.decode(String.self, forKey: .state)
        phase = try values.decode(String.self, forKey: .phase)
        updatedAt = try values.decode(String.self, forKey: .updatedAt)
        failure = try values.decodeIfPresent(String.self, forKey: .failure)
        platforms = try values.decode([String: PlatformLeg].self, forKey: .platforms)
        deliveries = try values.decode([String: DeliveryLeg].self, forKey: .deliveries)
        // A run has failed when any of its three records carries a failure.
        let failures = [failure].compactMap { $0 }
            + platforms.values.compactMap(\.failure)
            + deliveries.values.compactMap(\.failure)
        hasFailure = !failures.isEmpty
        diagnosticDetails = Self.diagnostics(
            runID: runID, failure: failure, platforms: platforms, deliveries: deliveries
        )
    }

    struct DeliveryLeg: Decodable, Sendable {
        let state: String
        let platform: String
        let jobID: String
        let required: Bool
        let failure: String?
        let outputPrefix: String
        let receiptSHA256: String?

        enum CodingKeys: String, CodingKey {
            case state, platform, required, failure
            case jobID = "job_id"
            case outputPrefix = "output_prefix"
            case receiptSHA256 = "receipt_sha256"
        }
    }

    private static func diagnostics(
        runID: String,
        failure: String?,
        platforms: [String: PlatformLeg],
        deliveries: [String: DeliveryLeg]
    ) -> String? {
        var sections: [String] = []
        if let failure {
            sections.append("Run \(runID):\n\(failure)")
        }
        for (name, leg) in platforms.sorted(by: { $0.key < $1.key }) {
            if let failure = leg.failure {
                sections.append("Platform \(name) (\(leg.state), job_id=\(leg.jobID.debugDescription)):\n\(failure)")
            }
        }
        for (name, leg) in deliveries.sorted(by: { $0.key < $1.key }) {
            var detail = "Delivery \(name) (\(leg.state), \(leg.platform), required=\(leg.required), job_id=\(leg.jobID.debugDescription))"
            detail += "\nOutput prefix: \(leg.outputPrefix.debugDescription)"
            if let digest = leg.receiptSHA256 {
                detail += "\nAccepted receipt SHA-256: \(digest)"
            }
            if let failure = leg.failure {
                detail += "\n\(failure)"
            }
            sections.append(detail)
        }
        return sections.isEmpty ? nil : sections.joined(separator: "\n\n")
    }

    /// `compile_progress`, attached by the CLI only to a running leg:
    /// `compiled` with a `percent` once a previous run gives the denominator,
    /// or `unreadable` with why the job's log could not be counted.
    struct CompileProgress: Decodable, Sendable {
        let compiled: Int?
        let percent: Int?
        let unreadable: String?
    }

    struct PlatformLeg: Decodable, Sendable {
        let state: String
        let jobID: String
        /// The queue's live word on the platform's build job, attached by the
        /// CLI only while the run is in flight.
        let jobState: String?
        let failure: String?
        /// What this platform's build cost, in seconds, from the job's own
        /// `started_at` and `completed_at`. A run that is still building
        /// reports the time it has spent so far. The run object stores no
        /// duration: the job record owns it and the CLI joins the two.
        let buildSeconds: Int?
        /// The crates compiled so far, while the leg runs.
        let compileProgress: CompileProgress?

        /// Crates compiled so far, from the job's streamed log.
        var compiledCrates: Int? { compileProgress?.compiled }
        /// An estimate against this platform's previous run — cargo publishes
        /// no total of its own, so the previous run is the denominator.
        var compilePercent: Int? { compileProgress?.percent }

        enum CodingKeys: String, CodingKey {
            case state
            case jobID = "job_id"
            case jobState = "job_state"
            case failure
            case compileProgress = "compile_progress"
            case buildSeconds = "build_seconds"
        }
    }

    var updated: Date? {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: updatedAt) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: updatedAt)
    }
}
