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
    /// `skarbiec` Linux publication failed unexplained on 2026-09-10.
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

/// The memory half of `stado host gates <host> --json`.
///
/// The host's own declaration decides whether memory pressure withholds it
/// from job selection, so this carries the refusal, the reading it was made
/// on and both watermarks rather than a colour.
struct HostGatesMemory: Decodable, Sendable {
    let pressureActive: Bool?
    let refusePlacement: Bool?
    let availableGB: Double?
    let totalGB: Double?
    let lowWatermarkGB: Double?
    /// Swap over its watermark while memory still has headroom: reported,
    /// never a reason this host takes no work.
    let swapPressureOnly: Bool?
    let swapUsedPct: Int?
    let swapHighWatermarkPct: Int?
    let policyMode: String?
    let passOutcome: String?

    enum CodingKeys: String, CodingKey {
        case pressureActive = "pressure_active"
        case refusePlacement = "refuse_placement"
        case availableGB = "available_gb"
        case totalGB = "total_gb"
        case lowWatermarkGB = "low_watermark_gb"
        case swapUsedPct = "swap_used_pct"
        case swapPressureOnly = "swap_pressure_only"
        case swapHighWatermarkPct = "swap_high_watermark_pct"
        case policyMode = "policy_mode"
        case passOutcome = "pass_outcome"
    }

    /// This host is withholding itself from selection right now.
    var isRefusingPlacement: Bool { pressureActive == true }

    /// The operator's sentence, in the same shape the CLI prints.
    var summary: String {
        var clauses: [String] = []
        if let availableGB {
            clauses.append(
                lowWatermarkGB.map { "\(StadoFormat.decimal(availableGB)) GB available against a \(StadoFormat.decimal($0)) GB watermark" }
                    ?? "\(StadoFormat.decimal(availableGB)) GB available, no watermark published"
            )
        }
        if let swapUsedPct, let swapHighWatermarkPct {
            clauses.append("swap \(swapUsedPct)% against \(swapHighWatermarkPct)%")
        }
        if isRefusingPlacement {
            clauses.append("refusing placement (memory_pressure_active)")
        } else if swapPressureOnly == true {
            clauses.append("taking work; swap over its watermark with memory headroom (memory_swap_over_watermark)")
        } else if refusePlacement == false {
            clauses.append("reporting only; this host does not refuse placement")
        } else if !clauses.isEmpty {
            clauses.append("taking work")
        }
        return clauses.isEmpty ? "Not observed" : clauses.joined(separator: " · ")
    }
}

struct HostDiagnosticRead: Decodable, Sendable, Identifiable {
    let operation: String
    let source: String
    let state: String
    let elapsedMs: Double
    let budgetMs: Double
    let startedAt: String?
    let finishedAt: String
    let detail: String?
    var id: String { operation }
    var complete: Bool { state == "complete" || state == "absent" }

    enum CodingKeys: String, CodingKey {
        case operation, source, state, detail
        case elapsedMs = "elapsed_ms", budgetMs = "budget_ms"
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

struct HostGatesDisk: Decodable, Sendable {
    let freeGB: Double?
    let lowWatermarkGB: Double?
    let targetFreeGB: Double?
    let policyMode: String?
    let freeBytes: UInt64?
    let observedAt: String?
    let readState: String?
    let pressureSource: String?
    let pressureUnresolved: Bool?
    let belowWatermark: Bool?

    /// The comparison the host itself makes when it decides whether to claim.
    /// `nil` when either number is missing, which is different from "there is
    /// enough room".
    var isBelowWatermark: Bool? {
        if let belowWatermark { return belowWatermark }
        guard let freeGB, let lowWatermarkGB else { return nil }
        return freeGB < lowWatermarkGB
    }

    enum CodingKeys: String, CodingKey {
        case freeGB = "free_gb"
        case lowWatermarkGB = "low_watermark_gb"
        case targetFreeGB = "target_free_gb"
        case policyMode = "policy_mode"
        case freeBytes = "free_bytes", observedAt = "observed_at", readState = "read_state"
        case pressureSource = "pressure_source", pressureUnresolved = "pressure_unresolved"
        case belowWatermark = "below_watermark"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        freeGB = try values.decodeIfPresent(Double.self, forKey: .freeGB)
        lowWatermarkGB = try values.decodeIfPresent(Double.self, forKey: .lowWatermarkGB)
        targetFreeGB = try values.decodeIfPresent(Double.self, forKey: .targetFreeGB)
        policyMode = try values.decodeIfPresent(String.self, forKey: .policyMode)
        freeBytes = try values.decodeIfPresent(UInt64.self, forKey: .freeBytes)
        observedAt = try values.decodeIfPresent(String.self, forKey: .observedAt)
        readState = try values.decodeIfPresent(String.self, forKey: .readState)
        pressureSource = try values.decodeIfPresent(String.self, forKey: .pressureSource)
        pressureUnresolved = try values.decodeIfPresent(Bool.self, forKey: .pressureUnresolved)
        belowWatermark = try values.decodeIfPresent(Bool.self, forKey: .belowWatermark)
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
    let diagnostics: StorageReconciliationJSON?

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
        case diagnostics
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
        diagnostics = try values.decodeIfPresent(StorageReconciliationJSON.self, forKey: .diagnostics)
    }
}
