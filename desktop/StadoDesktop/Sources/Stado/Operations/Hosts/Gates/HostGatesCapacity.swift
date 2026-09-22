import Foundation

// MARK: - The capacity half of `stado host gates <host> --json`
/// One hold a placed workload keeps on the host, from `capacity.reservations`.
struct HostGatesReservation: Decodable, Sendable, Identifiable {
    let reservationID: String
    let kind: String
    let product: String
    let holder: String
    let cpuCores: Int
    let ramGB: Double
    let vramGB: Int
    let acquiredAt: String

    var id: String { reservationID }

    enum CodingKeys: String, CodingKey {
        case reservationID = "reservation_id"
        case kind, product, holder
        case cpuCores = "cpu_cores"
        case ramGB = "ram_gb"
        case vramGB = "vram_gb"
        case acquiredAt = "acquired_at"
    }

    /// One line, the same words `stado host gates` prints for the hold.
    var summary: String {
        "\(kind) (\(product)) held by \(holder) since \(acquiredAt): \(cpuCores) core(s), \(StadoFormat.decimal(ramGB)) GB"
    }
}

/// The sum of the holds, from `capacity.reserved`.
struct HostGatesReserved: Decodable, Sendable {
    let cpuCores: Int
    let ramGB: Double
    let vramGB: Int

    enum CodingKeys: String, CodingKey {
        case cpuCores = "cpu_cores"
        case ramGB = "ram_gb"
        case vramGB = "vram_gb"
    }
}

struct HostGatesCapacity: Decodable, Sendable {
    let publishedAt: String?
    let ageSeconds: Double?
    let acceptingJobs: Bool?
    /// The agent's own word for why it is not accepting, verbatim from the
    /// report; nil while the host accepts or published no reason.
    let admissionReason: String?
    let runningJobs: Int?
    /// Placed workloads holding the host — Jeden sessions, browser tasks —
    /// and what they hold. The CPU, RAM and VRAM figures are net of them.
    let runningWorkloads: Int?
    let reserved: HostGatesReserved?
    let reservations: [HostGatesReservation]
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
        case admissionReason = "admission_reason"
        case runningJobs = "running_jobs"
        case runningWorkloads = "running_workloads"
        case reserved, reservations
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
        admissionReason = try values.decodeIfPresent(String.self, forKey: .admissionReason)
        runningJobs = try values.decodeIfPresent(Int.self, forKey: .runningJobs)
        runningWorkloads = try values.decodeIfPresent(Int.self, forKey: .runningWorkloads)
        reserved = try values.decodeIfPresent(HostGatesReserved.self, forKey: .reserved)
        reservations = try values.decodeIfPresent([HostGatesReservation].self, forKey: .reservations) ?? []
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

    /// The `reserved:` sentence `stado host gates` prints, or nil when
    /// nothing is held.
    var reservedSummary: String? {
        guard let held = runningWorkloads, held > 0 else { return nil }
        let sum = reserved
        return "\(held) placed workload(s) hold \(sum?.cpuCores ?? 0) core(s), \(StadoFormat.decimal(sum?.ramGB ?? 0)) GB RAM, \(sum?.vramGB ?? 0) GB VRAM; the capacity figures are net of them"
    }

    /// Who holds the accelerator, read from the published diagnostics with
    /// the same rule as the CLI's `accelerator:` line.
    var acceleratorSummary: String? {
        guard let diagnostics else { return nil }
        if let error = diagnostics["accelerator_holders_error"]?.stringValue {
            return "unknown: \(error)"
        }
        guard let model = diagnostics["accelerator_memory_model"]?.stringValue else { return nil }
        if model == "unified" {
            return "shares the host's memory; no per-process VRAM"
        }
        let holders: [String] = (diagnostics["accelerator_holders"].flatMap { value -> [StorageReconciliationJSON]? in
            if case let .array(items) = value { return items }
            return nil
        } ?? []).map { holder in
            let pid = holder["pid"]?.displayValue ?? "?"
            let process = holder["process"]?.stringValue ?? "?"
            let used = holder["used_vram_gb"]?.displayValue ?? "?"
            let owner: String
            if let job = holder["stado_job"]?.stringValue {
                owner = "Stado job \(job)"
            } else if let unit = holder["unit"]?.stringValue {
                owner = "unit \(unit), not a Stado job"
            } else {
                owner = "not a Stado job"
            }
            return "pid \(pid) \(process) \(used) GB (\(owner))"
        }
        let unattributed = diagnostics["vram_unattributed_gb"]?.displayValue
        if holders.isEmpty {
            if let unattributed, unattributed != "0", unattributed != "0.0" {
                return "no process holds the accelerator through the driver, yet \(unattributed) GB is in use"
            }
            return "no process holds the accelerator"
        }
        var parts = holders
        if let unattributed, unattributed != "0", unattributed != "0.0" {
            parts.append("\(unattributed) GB held by no listed process")
        }
        return "held by " + parts.joined(separator: "; ")
    }
}
