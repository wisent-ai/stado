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

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        entries = try values.decodeIfPresent([ReleaseInventoryEntry].self, forKey: .entries) ?? []
        runs = try values.decodeIfPresent([ReleasePipelineRunRecord].self, forKey: .runs) ?? []
    }
}

/// One release-pipeline run: what was submitted, where it stands, and — when a
/// platform or the whole run died — the recorded failure with the job's own
/// last output lines. This mirrors the run object the pipeline persists, so
/// the screen shows the store's truth, not a paraphrase.
struct ReleasePipelineRunRecord: Decodable, Sendable, Identifiable {
    let runID: String
    let product: String
    let version: String
    let channel: String
    let state: String
    let updatedAt: String
    let failure: String?
    let platforms: [String: PlatformLeg]

    var id: String { runID }

    enum CodingKeys: String, CodingKey {
        case runID = "run_id"
        case product
        case version
        case channel
        case state
        case updatedAt = "updated_at"
        case failure
        case platforms
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        runID = try values.decodeIfPresent(String.self, forKey: .runID) ?? ""
        product = try values.decodeIfPresent(String.self, forKey: .product) ?? ""
        version = try values.decodeIfPresent(String.self, forKey: .version) ?? ""
        channel = try values.decodeIfPresent(String.self, forKey: .channel) ?? ""
        state = try values.decodeIfPresent(String.self, forKey: .state) ?? ""
        updatedAt = try values.decodeIfPresent(String.self, forKey: .updatedAt) ?? ""
        failure = try values.decodeIfPresent(String.self, forKey: .failure)
        platforms =
            try values.decodeIfPresent([String: PlatformLeg].self, forKey: .platforms) ?? [:]
    }

    struct PlatformLeg: Decodable, Sendable {
        let state: String
        let jobID: String
        /// The queue's live word on the platform's build job, attached by the
        /// CLI only while the run is in flight.
        let jobState: String?
        let failure: String?
        /// Crates compiled so far, from the job's streamed log.
        let compiledCrates: Int?
        /// An estimate against this platform's previous run — cargo publishes
        /// no total of its own, so the previous run is the denominator.
        let compilePercent: Int?

        enum CodingKeys: String, CodingKey {
            case state
            case jobID = "job_id"
            case jobState = "job_state"
            case failure
            case compileProgress = "compile_progress"
        }

        enum ProgressKeys: String, CodingKey {
            case compiled
            case percent
        }

        init(from decoder: Decoder) throws {
            let values = try decoder.container(keyedBy: CodingKeys.self)
            state = try values.decodeIfPresent(String.self, forKey: .state) ?? ""
            jobID = try values.decodeIfPresent(String.self, forKey: .jobID) ?? ""
            jobState = try values.decodeIfPresent(String.self, forKey: .jobState)
            failure = try values.decodeIfPresent(String.self, forKey: .failure)
            if let progress = try? values.nestedContainer(
                keyedBy: ProgressKeys.self, forKey: .compileProgress
            ) {
                compiledCrates = try progress.decodeIfPresent(Int.self, forKey: .compiled)
                compilePercent = try progress.decodeIfPresent(Int.self, forKey: .percent)
            } else {
                compiledCrates = nil
                compilePercent = nil
            }
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

/// One product target as the inventory lists it: its identity, and the software
/// report the CLI attached to it.
struct ReleaseInventoryEntry: Decodable, Sendable {
    let pair: ReleaseInventoryPair
    /// Absent only when the CLI predates the software half of the report. It is
    /// carried as `nil` rather than as a passing verdict, because "this console
    /// is newer than the CLI" and "the host is accounted for" are different
    /// facts and only one of them is good news.
    let software: ReleaseSoftwareReport?

    enum CodingKeys: String, CodingKey {
        case software
    }

    init(from decoder: Decoder) throws {
        pair = try ReleaseInventoryPair(from: decoder)
        let values = try decoder.container(keyedBy: CodingKeys.self)
        software = try values.decodeIfPresent(ReleaseSoftwareReport.self, forKey: .software)
    }
}

/// What a host said it runs, and whether the CLI could account for it.
///
/// `state` and `verdict` are two different questions and stay two fields.
/// `state` is whether anybody looked — `observed`, `unverified`, `never`.
/// `verdict` is what the look implies — `ok` or `failed`. Folding them would let
/// this screen paint "nobody has ever asked this host" in the same colour as
/// "the host answered and it is fine", which is the exact reading that let a
/// stale skarbiec strip a live subscription's tags for a day.
struct ReleaseSoftwareReport: Decodable, Sendable {
    let state: String
    let verdict: String
    let failed: Bool
    /// `just now`, `14m ago`, `stale (3h)`, `never` — the CLI's own phrase.
    let observed: String
    let reported: Int
    let release: Int
    let unmanaged: Int
    let scripts: Int
    /// Verbatim, in the CLI's words, in the CLI's order. Never re-worded here.
    let findings: [String]

    var hasReport: Bool { state != "never" }

    enum CodingKeys: String, CodingKey {
        case state, verdict, failed, observed, reported, release, unmanaged, scripts, findings
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        state = try values.decodeIfPresent(String.self, forKey: .state) ?? "never"
        verdict = try values.decodeIfPresent(String.self, forKey: .verdict) ?? "failed"
        // Defaulted to a failure, deliberately. A payload this console cannot
        // read is not evidence that the fleet is healthy.
        failed = try values.decodeIfPresent(Bool.self, forKey: .failed) ?? true
        observed = try values.decodeIfPresent(String.self, forKey: .observed) ?? "never"
        reported = try values.decodeIfPresent(Int.self, forKey: .reported) ?? 0
        release = try values.decodeIfPresent(Int.self, forKey: .release) ?? 0
        unmanaged = try values.decodeIfPresent(Int.self, forKey: .unmanaged) ?? 0
        scripts = try values.decodeIfPresent(Int.self, forKey: .scripts) ?? 0
        findings = try values.decodeIfPresent([String].self, forKey: .findings) ?? []
    }
}

struct ReleaseInventoryPair: Decodable, Identifiable, Hashable, Sendable {
    let product: String
    let target: String

    var id: String { "\(product)/\(target)" }

    enum CodingKeys: String, CodingKey {
        case product, target
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        product = try values.decodeIfPresent(String.self, forKey: .product) ?? ""
        target = try values.decodeIfPresent(String.self, forKey: .target) ?? ""
    }

    init(product: String, target: String) {
        self.product = product
        self.target = target
    }
}
