import Foundation

struct DashboardSnapshot: Decodable, Sendable {
    let ready: Bool
    let now: String?
    let bucket: String?
    let counts: JobCounts
    let byModelState: [String: JobCounts]
    let liveAgents: [WorkerNode]
    let staleAgents: [WorkerNode]
    let workers: [WorkerNode]
    let recentFailed: [FailedJob]
    let completedRecent: [CompletedJob]
    let throughput: Throughput
    let lastRefreshSeconds: Double?

    enum CodingKeys: String, CodingKey {
        case ready, now, bucket, counts, throughput
        case byModelState
        case liveAgents
        case staleAgents
        case workers
        case recentFailed
        case completedRecent
        case lastRefreshSeconds
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        ready = try values.decodeIfPresent(Bool.self, forKey: .ready) ?? false
        now = try values.decodeIfPresent(String.self, forKey: .now)
        bucket = try values.decodeIfPresent(String.self, forKey: .bucket)
        counts = try values.decodeIfPresent(JobCounts.self, forKey: .counts) ?? .zero
        byModelState = try values.decodeIfPresent([String: JobCounts].self, forKey: .byModelState) ?? [:]
        liveAgents = try values.decodeIfPresent([WorkerNode].self, forKey: .liveAgents) ?? []
        staleAgents = try values.decodeIfPresent([WorkerNode].self, forKey: .staleAgents) ?? []
        workers = try values.decodeIfPresent([WorkerNode].self, forKey: .workers) ?? []
        recentFailed = try values.decodeIfPresent([FailedJob].self, forKey: .recentFailed) ?? []
        completedRecent = try values.decodeIfPresent([CompletedJob].self, forKey: .completedRecent) ?? []
        throughput = try values.decodeIfPresent(Throughput.self, forKey: .throughput) ?? .unavailable
        lastRefreshSeconds = try values.decodeIfPresent(Double.self, forKey: .lastRefreshSeconds)
    }
}

struct JobCounts: Decodable, Sendable {
    let queue: Int
    let running: Int
    let completed: Int
    let failed: Int

    init(queue: Int, running: Int, completed: Int, failed: Int) {
        self.queue = queue
        self.running = running
        self.completed = completed
        self.failed = failed
    }

    enum CodingKeys: String, CodingKey {
        case queue, running, completed, failed
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        queue = try values.decodeIfPresent(Int.self, forKey: .queue) ?? 0
        running = try values.decodeIfPresent(Int.self, forKey: .running) ?? 0
        completed = try values.decodeIfPresent(Int.self, forKey: .completed) ?? 0
        failed = try values.decodeIfPresent(Int.self, forKey: .failed) ?? 0
    }
}

enum WorkerAvailability: String, Decodable, Sendable {
    case live
    case stale
    case unavailable
}

struct WorkerNode: Decodable, Identifiable, Sendable {
    let targetName: String?
    let consumerID: String?
    let declared: Bool
    let status: WorkerAvailability
    let availabilityReason: String
    let kind: String?
    let hostnames: [String]
    let gpuType: String?
    let role: String?
    let acceptingJobs: Bool?
    let runningJobs: Int?
    let availableCPUCores: Int?
    let totalCPUCores: Int?
    let availableAccelerators: [String: Int]
    let freeRAMGB: Double?
    let totalRAMGB: Double?
    let freeVRAMGB: Double?
    let totalVRAMGB: Double?
    let publishedAt: String?
    let ageSeconds: Double?

    var id: String {
        targetName ?? consumerID ?? "unknown-\(publishedAt ?? "worker")"
    }

    var displayName: String {
        if let targetName, !targetName.isEmpty {
            return targetName
        }
        if let consumerID, !consumerID.isEmpty {
            return consumerID
        }
        return "Unnamed worker"
    }


    enum CodingKeys: String, CodingKey {
        case targetName, consumerID = "consumerId", declared, status, availabilityReason
        case kind, hostnames, gpuType, role, acceptingJobs, runningJobs
        case availableCPUCores = "availableCpuCores"
        case totalCPUCores = "totalCpuCores"
        case availableAccelerators, publishedAt, ageSeconds
        case freeRAMGB = "freeRamGb"
        case totalRAMGB = "totalRamGb"
        case freeVRAMGB = "freeVramGb"
        case totalVRAMGB = "totalVramGb"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        targetName = try values.decodeIfPresent(String.self, forKey: .targetName)
        consumerID = try values.decodeIfPresent(String.self, forKey: .consumerID)
        declared = try values.decodeIfPresent(Bool.self, forKey: .declared) ?? false
        status = try values.decodeIfPresent(WorkerAvailability.self, forKey: .status) ?? .unavailable
        availabilityReason = try values.decodeIfPresent(String.self, forKey: .availabilityReason)
            ?? "Worker availability was not reported."
        kind = try values.decodeIfPresent(String.self, forKey: .kind)
        hostnames = try values.decodeIfPresent([String].self, forKey: .hostnames) ?? []
        gpuType = try values.decodeIfPresent(String.self, forKey: .gpuType)
        role = try values.decodeIfPresent(String.self, forKey: .role)
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
        publishedAt = try values.decodeIfPresent(String.self, forKey: .publishedAt)
        ageSeconds = try values.decodeIfPresent(Double.self, forKey: .ageSeconds)
    }
}

struct CompletedJob: Decodable, Identifiable, Sendable {
    let jobID: String
    let model: String?
    let task: String?
    let wallSeconds: Double?
    let completedAt: String?

    var id: String { jobID }

    enum CodingKeys: String, CodingKey {
        case jobID = "jobId"
        case model, task, wallSeconds, completedAt
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        jobID = try values.decodeIfPresent(String.self, forKey: .jobID) ?? "Unavailable"
        model = operationalMetadata(try values.decodeIfPresent(String.self, forKey: .model))
        task = operationalMetadata(try values.decodeIfPresent(String.self, forKey: .task))
        wallSeconds = try values.decodeIfPresent(Double.self, forKey: .wallSeconds)
        completedAt = try values.decodeIfPresent(String.self, forKey: .completedAt)
    }
}

struct FailedJob: Decodable, Identifiable, Sendable {
    let jobID: String
    let model: String?
    let task: String?
    let error: String?

    var id: String { jobID }

    enum CodingKeys: String, CodingKey {
        case jobID = "jobId"
        case model, task, error
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        jobID = try values.decodeIfPresent(String.self, forKey: .jobID) ?? "Unavailable"
        model = operationalMetadata(try values.decodeIfPresent(String.self, forKey: .model))
        task = operationalMetadata(try values.decodeIfPresent(String.self, forKey: .task))
        error = try values.decodeIfPresent(String.self, forKey: .error)
    }
}

struct Throughput: Decodable, Sendable {
    let averageWallSecondsPerCompletedJob: Double?
    let samples: Int
    let projectedRemainingSeconds: Double?

    enum CodingKeys: String, CodingKey {
        case averageWallSecondsPerCompletedJob = "avgWallSecondsPerCompletedJob"
        case samples, projectedRemainingSeconds
    }

    init(
        averageWallSecondsPerCompletedJob: Double?,
        samples: Int,
        projectedRemainingSeconds: Double?
    ) {
        self.averageWallSecondsPerCompletedJob = averageWallSecondsPerCompletedJob
        self.samples = samples
        self.projectedRemainingSeconds = projectedRemainingSeconds
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        averageWallSecondsPerCompletedJob = try values.decodeIfPresent(Double.self, forKey: .averageWallSecondsPerCompletedJob)
        samples = try values.decodeIfPresent(Int.self, forKey: .samples) ?? 0
        projectedRemainingSeconds = try values.decodeIfPresent(Double.self, forKey: .projectedRemainingSeconds)
    }
}

func operationalMetadata(_ value: String?) -> String? {
    guard let value = value?.trimmingCharacters(in: .whitespacesAndNewlines),
          !value.isEmpty,
          value != "(unknown)"
    else {
        return nil
    }
    return value
}

enum StadoFormat {
    private static let fractionalDateStrategy = Date.ISO8601FormatStyle(includingFractionalSeconds: true)
    private static let dateStrategy = Date.ISO8601FormatStyle()

    static func date(_ value: String?) -> Date? {
        guard let value else { return nil }
        return (try? fractionalDateStrategy.parse(value)) ?? (try? dateStrategy.parse(value))
    }

    /// A `ps` start stamp as a host prints it — `Mon Aug 11 09:12:33 2026`.
    ///
    /// `stado service list --unowned` carries that field verbatim rather than
    /// normalising it, so the only way an age appears beside a four-day-old
    /// process is for this app to read the host's own spelling. Fixed locale
    /// and the host's own zone: `ps` prints in the machine's local time, and a
    /// French-locale laptop must not fail to read an English month name.
    static func processStart(_ value: String?) -> Date? {
        guard let value = value?.trimmingCharacters(in: .whitespacesAndNewlines), !value.isEmpty else {
            return nil
        }
        return processStartFormatter.date(from: value) ?? date(value)
    }

    private static let processStartFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "EEE MMM d HH:mm:ss yyyy"
        return formatter
    }()

    static func duration(_ seconds: Double?) -> String {
        guard let seconds, seconds.isFinite, seconds >= 0 else { return "Unavailable" }
        if seconds < 60 {
            return "\(Int(seconds.rounded())) sec"
        }
        if seconds < 3_600 {
            return "\(Int((seconds / 60).rounded())) min"
        }
        return "\((seconds / 3_600).formatted(.number.precision(.fractionLength(0...1)))) hr"
    }

    static func decimal(_ value: Double?) -> String {
        guard let value, value.isFinite else { return "Unavailable" }
        return value.formatted(.number.precision(.fractionLength(0...1)))
    }
}
