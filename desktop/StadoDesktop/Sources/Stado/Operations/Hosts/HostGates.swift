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
    let claiming: Bool
    /// Verbatim, in the agent's words. Never rewritten here: a paraphrase of a
    /// blocker is a second source of truth about why work is not being taken.
    let blockers: [String]
    let disk: HostGatesDisk?
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
        !claiming && !blockers.isEmpty && blockers.allSatisfy { $0 == "pinned_only" }
    }

    /// Claiming nothing in a way that is not declared policy.
    var refusingUnpinned: Bool {
        !claiming && !pinnedByDesign
    }

    enum CodingKeys: String, CodingKey {
        case host, claiming, blockers, disk, capacity
        case waitingJobs = "waiting_jobs"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decodeIfPresent(String.self, forKey: .host) ?? ""
        claiming = try values.decodeIfPresent(Bool.self, forKey: .claiming) ?? false
        blockers = try values.decodeIfPresent([String].self, forKey: .blockers) ?? []
        disk = try values.decodeIfPresent(HostGatesDisk.self, forKey: .disk)
        capacity = try values.decodeIfPresent(HostGatesCapacity.self, forKey: .capacity)
        waitingJobs =
            try values.decodeIfPresent([HostGatesWaitingJob].self, forKey: .waitingJobs) ?? []
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

struct HostGatesDisk: Decodable, Sendable {
    let freeGB: Double?
    let lowWatermarkGB: Double?
    let targetFreeGB: Double?
    let policyMode: String?

    /// The comparison the host itself makes when it decides whether to claim.
    /// `nil` when either number is missing, which is different from "there is
    /// enough room".
    var isBelowWatermark: Bool? {
        guard let freeGB, let lowWatermarkGB else { return nil }
        return freeGB < lowWatermarkGB
    }

    enum CodingKeys: String, CodingKey {
        case freeGB = "free_gb"
        case lowWatermarkGB = "low_watermark_gb"
        case targetFreeGB = "target_free_gb"
        case policyMode = "policy_mode"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        freeGB = try values.decodeIfPresent(Double.self, forKey: .freeGB)
        lowWatermarkGB = try values.decodeIfPresent(Double.self, forKey: .lowWatermarkGB)
        targetFreeGB = try values.decodeIfPresent(Double.self, forKey: .targetFreeGB)
        policyMode = try values.decodeIfPresent(String.self, forKey: .policyMode)
    }
}

struct HostGatesCapacity: Decodable, Sendable {
    let publishedAt: String?
    let ageSeconds: Double?
    let acceptingJobs: Bool?
    let runningJobs: Int?
    let availableCPUCores: Int?
    let totalCPUCores: Int?
    let availableAccelerators: [String: Int]
    let freeRAMGB: Double?
    let totalRAMGB: Double?
    let freeVRAMGB: Double?
    let totalVRAMGB: Double?

    enum CodingKeys: String, CodingKey {
        case publishedAt = "published_at"
        case ageSeconds = "age_seconds"
        case acceptingJobs = "accepting_jobs"
        case runningJobs = "running_jobs"
        case availableCPUCores = "available_cpu_cores"
        case totalCPUCores = "total_cpu_cores"
        case availableAccelerators = "available_accelerators"
        case freeRAMGB = "free_ram_gb"
        case totalRAMGB = "total_ram_gb"
        case freeVRAMGB = "free_vram_gb"
        case totalVRAMGB = "total_vram_gb"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        publishedAt = try values.decodeIfPresent(String.self, forKey: .publishedAt)
        ageSeconds = try values.decodeIfPresent(Double.self, forKey: .ageSeconds)
        acceptingJobs = try values.decodeIfPresent(Bool.self, forKey: .acceptingJobs)
        runningJobs = try values.decodeIfPresent(Int.self, forKey: .runningJobs)
        availableCPUCores = try values.decodeIfPresent(Int.self, forKey: .availableCPUCores)
        totalCPUCores = try values.decodeIfPresent(Int.self, forKey: .totalCPUCores)
        availableAccelerators =
            try values.decodeIfPresent([String: Int].self, forKey: .availableAccelerators) ?? [:]
        freeRAMGB = try values.decodeIfPresent(Double.self, forKey: .freeRAMGB)
        totalRAMGB = try values.decodeIfPresent(Double.self, forKey: .totalRAMGB)
        freeVRAMGB = try values.decodeIfPresent(Double.self, forKey: .freeVRAMGB)
        totalVRAMGB = try values.decodeIfPresent(Double.self, forKey: .totalVRAMGB)
    }
}
