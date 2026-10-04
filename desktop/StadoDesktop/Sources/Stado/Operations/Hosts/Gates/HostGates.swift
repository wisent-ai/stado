import Foundation

// MARK: - Host gates, reclamation, and managed services

/// `stado host gates <host> --json`.
///
/// One question first — is this host claiming work — and then the agent's own
/// sentences for why it is not. A host that quietly claimed nothing for hours
/// while its disk sat at 2 GB under a 55 GB policy is the reason `claiming` is
/// a field of its own rather than something an operator infers from the disk
/// numbers below it.
struct HostGates: Decodable, Identifiable, Sendable {
    let host: String
    let claiming: Bool?
    let complete: Bool?
    let observations: [HostDiagnosticRead]
    /// Verbatim, in the agent's words. Never rewritten here: a paraphrase of a
    /// blocker is a second source of truth about why work is not being taken.
    let blockers: [String]
    let disk: HostGatesDisk?
    /// What this host published about its own memory. The disk half of this
    /// panel has always been complete; a host refusing every job for memory
    /// pressure showed two RAM totals and no reason at all, which is how a
    /// publication fails unexplained.
    let memory: HostGatesMemory?
    let capacity: HostGatesCapacity?
    /// Queued jobs pinned to this host, oldest first — the refusal's own
    /// consequence, so "not claiming" arrives with a size and an age.
    let waitingJobs: [HostGatesWaitingJob]

    var id: String { host }

    /// The registry pinned this host on purpose: it claims only work
    /// addressed to it. Absence by choice is not an incident, so a
    /// `pinned_only` refusal alone is policy — red when it is costing work,
    /// which is exactly when `waitingJobs` is non-empty.
    var pinnedByDesign: Bool {
        complete == true && claiming == false && !blockers.isEmpty && blockers.allSatisfy { $0 == "pinned_only" }
    }

    /// Claiming nothing in a way that is not declared policy.
    var refusingUnpinned: Bool {
        complete == true && claiming == false && !pinnedByDesign
    }

    enum CodingKeys: String, CodingKey {
        case host, claiming, blockers, disk, memory, capacity
        case complete, observations
        case waitingJobs = "waiting_jobs"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decode(String.self, forKey: .host)
        claiming = try values.decodeIfPresent(Bool.self, forKey: .claiming)
        complete = try values.decodeIfPresent(Bool.self, forKey: .complete)
        observations = try values.decodeIfPresent([HostDiagnosticRead].self, forKey: .observations) ?? []
        blockers = try values.decodeIfPresent([String].self, forKey: .blockers) ?? []
        disk = try values.decodeIfPresent(HostGatesDisk.self, forKey: .disk)
        capacity = try values.decodeIfPresent(HostGatesCapacity.self, forKey: .capacity)
        memory = try values.decodeIfPresent(HostGatesMemory.self, forKey: .memory)
        waitingJobs =
            try values.decodeIfPresent([HostGatesWaitingJob].self, forKey: .waitingJobs) ?? []
    }
}

/// The memory half of `stado host gates <host> --json`: readings only.
/// Memory never withholds a host from work; the numbers explain a host that
/// has gone quiet.
struct HostGatesMemory: Decodable, Sendable {
    let availableGB: Double?
    let totalGB: Double?
    let swapUsedPct: Int?
    let source: String?

    enum CodingKeys: String, CodingKey {
        case availableGB = "available_gb"
        case totalGB = "total_gb"
        case swapUsedPct = "swap_used_pct"
        case source
    }

    /// The operator's sentence, in the same shape the CLI prints.
    var summary: String {
        var clauses: [String] = []
        switch (availableGB, totalGB) {
        case let (available?, total?):
            clauses.append("\(StadoFormat.decimal(available)) of \(StadoFormat.decimal(total)) GB available")
        case let (available?, nil):
            clauses.append("\(StadoFormat.decimal(available)) GB available")
        default:
            break
        }
        if let swapUsedPct {
            clauses.append("swap \(swapUsedPct)% used")
        }
        return clauses.isEmpty ? "Not observed" : clauses.joined(separator: " · ")
    }
}

struct HostDiagnosticRead: Decodable, Sendable, Identifiable {
    let operation: String
    let source: String
    let state: String
    let elapsedMs: Double
    let startedAt: String?
    let finishedAt: String
    let detail: String?
    var id: String { operation }
    var complete: Bool { state == "complete" || state == "absent" }

    enum CodingKeys: String, CodingKey {
        case operation, source, state, detail
        case elapsedMs = "elapsed_ms"
        case startedAt = "started_at", finishedAt = "finished_at"
    }
}

/// One queued job a non-claiming host is starving.
struct HostGatesWaitingJob: Decodable, Sendable, Identifiable {
    let jobID: String
    let ageSeconds: Int?

    var id: String { jobID }

    enum CodingKeys: String, CodingKey {
        case jobID = "job_id"
        case ageSeconds = "age_seconds"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        jobID = try values.decodeIfPresent(String.self, forKey: .jobID) ?? ""
        ageSeconds = try values.decodeIfPresent(Int.self, forKey: .ageSeconds)
    }
}

/// The disk half of `stado host gates <host> --json`, measured against the
/// one disk-full rule: how full the volume holding the agent's home is, how
/// far that is from the threshold, and whether the threshold has been reached.
struct HostGatesDisk: Decodable, Sendable {
    let freeGB: Double?
    let usedPercent: Double?
    let headroomGB: Double?
    let fullPercent: Int?
    let full: Bool?
    let freeBytes: UInt64?
    let observedAt: String?
    let readState: String?
    let pressureSource: String?
    let pressureUnresolved: Bool?

    enum CodingKeys: String, CodingKey {
        case freeGB = "free_gb"
        case usedPercent = "used_percent"
        case headroomGB = "headroom_gb"
        case fullPercent = "full_percent"
        case full
        case freeBytes = "free_bytes", observedAt = "observed_at", readState = "read_state"
        case pressureSource = "pressure_source", pressureUnresolved = "pressure_unresolved"
    }

    /// The operator's sentence, in the shape the CLI prints.
    var ruleSummary: String {
        let threshold = fullPercent.map { "\($0)%" } ?? "the threshold"
        guard let usedPercent else { return "Volume not read" }
        var text = "\(String(format: "%.1f", usedPercent))% used against \(threshold)"
        if let headroomGB {
            text += headroomGB >= 0
                ? " · \(StadoFormat.decimal(headroomGB)) GB before the rule deletes"
                : " · past the threshold by \(StadoFormat.decimal(-headroomGB)) GB"
        }
        return text
    }
}

