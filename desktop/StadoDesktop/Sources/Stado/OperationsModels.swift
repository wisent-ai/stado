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

    static let zero = JobCounts(queue: 0, running: 0, completed: 0, failed: 0)

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
    let freeSlots: [String: Int]
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

    var availableSlots: Int {
        freeSlots.values.reduce(0, +)
    }

    enum CodingKeys: String, CodingKey {
        case targetName, consumerID = "consumerId", declared, status, availabilityReason
        case kind, hostnames, gpuType, role, freeSlots, publishedAt, ageSeconds
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
        freeSlots = try values.decodeIfPresent([String: Int].self, forKey: .freeSlots) ?? [:]
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
    let liveTotalFreeSlots: Int
    let projectedRemainingSeconds: Double?

    static let unavailable = Throughput(
        averageWallSecondsPerCompletedJob: nil,
        samples: 0,
        liveTotalFreeSlots: 0,
        projectedRemainingSeconds: nil
    )

    enum CodingKeys: String, CodingKey {
        case averageWallSecondsPerCompletedJob = "avgWallSecondsPerCompletedJob"
        case samples, liveTotalFreeSlots, projectedRemainingSeconds
    }

    init(
        averageWallSecondsPerCompletedJob: Double?,
        samples: Int,
        liveTotalFreeSlots: Int,
        projectedRemainingSeconds: Double?
    ) {
        self.averageWallSecondsPerCompletedJob = averageWallSecondsPerCompletedJob
        self.samples = samples
        self.liveTotalFreeSlots = liveTotalFreeSlots
        self.projectedRemainingSeconds = projectedRemainingSeconds
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        averageWallSecondsPerCompletedJob = try values.decodeIfPresent(Double.self, forKey: .averageWallSecondsPerCompletedJob)
        samples = try values.decodeIfPresent(Int.self, forKey: .samples) ?? 0
        liveTotalFreeSlots = try values.decodeIfPresent(Int.self, forKey: .liveTotalFreeSlots) ?? 0
        projectedRemainingSeconds = try values.decodeIfPresent(Double.self, forKey: .projectedRemainingSeconds)
    }
}

private func operationalMetadata(_ value: String?) -> String? {
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

    var id: String { host }

    enum CodingKeys: String, CodingKey {
        case host, claiming, blockers, disk, capacity
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decodeIfPresent(String.self, forKey: .host) ?? ""
        claiming = try values.decodeIfPresent(Bool.self, forKey: .claiming) ?? false
        blockers = try values.decodeIfPresent([String].self, forKey: .blockers) ?? []
        disk = try values.decodeIfPresent(HostGatesDisk.self, forKey: .disk)
        capacity = try values.decodeIfPresent(HostGatesCapacity.self, forKey: .capacity)
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
    let freeSlots: Int?
    let slotsDeclared: Int?

    enum CodingKeys: String, CodingKey {
        case publishedAt = "published_at"
        case ageSeconds = "age_seconds"
        case freeSlots = "free_slots"
        case slotsDeclared = "slots_declared"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        publishedAt = try values.decodeIfPresent(String.self, forKey: .publishedAt)
        ageSeconds = try values.decodeIfPresent(Double.self, forKey: .ageSeconds)
        freeSlots = try values.decodeIfPresent(Int.self, forKey: .freeSlots)
        slotsDeclared = try values.decodeIfPresent(Int.self, forKey: .slotsDeclared)
    }
}

/// One `stado host reclaim` pass, in either mode. `mode` is the command's own
/// word for what it did, so a preview and an applied pass cannot be confused
/// for one another after the fact.
struct HostReclaimPass: Decodable, Sendable {
    let host: String
    let mode: String
    let stages: [HostReclaimStage]
    let freeGBBefore: Double?
    let freeGBAfter: Double?

    var isDryRun: Bool { mode != "apply" }

    var reclaimedGB: Double? {
        guard let freeGBBefore, let freeGBAfter else { return nil }
        return freeGBAfter - freeGBBefore
    }

    var itemCount: Int {
        stages.reduce(0) { $0 + $1.items }
    }

    enum CodingKeys: String, CodingKey {
        case host, mode, stages
        case freeGBBefore = "free_gb_before"
        case freeGBAfter = "free_gb_after"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decodeIfPresent(String.self, forKey: .host) ?? ""
        // `dry_run`, the spelling `stado host reclaim` prints, so a report that
        // arrived without the field cannot read as a different mode than the
        // command's own.
        mode = try values.decodeIfPresent(String.self, forKey: .mode) ?? "dry_run"
        stages = try values.decodeIfPresent([HostReclaimStage].self, forKey: .stages) ?? []
        freeGBBefore = try values.decodeIfPresent(Double.self, forKey: .freeGBBefore)
        freeGBAfter = try values.decodeIfPresent(Double.self, forKey: .freeGBAfter)
    }
}

struct HostReclaimStage: Decodable, Identifiable, Sendable {
    let stage: String
    let freeGBBefore: Double?
    let freeGBAfter: Double?
    let items: Int

    var id: String { stage }

    enum CodingKeys: String, CodingKey {
        case stage, items
        case freeGBBefore = "free_gb_before"
        case freeGBAfter = "free_gb_after"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        stage = try values.decodeIfPresent(String.self, forKey: .stage) ?? "unnamed stage"
        freeGBBefore = try values.decodeIfPresent(Double.self, forKey: .freeGBBefore)
        freeGBAfter = try values.decodeIfPresent(Double.self, forKey: .freeGBAfter)
        items = try values.decodeIfPresent(Int.self, forKey: .items) ?? 0
    }
}

/// `stado service converge <host> --json`, read-only: what each declared unit
/// runs, and what the process on the host is actually executing.
struct ServiceConvergeReport: Decodable, Sendable {
    let target: String
    let applied: Bool
    let units: [ServiceUnit]

    enum CodingKeys: String, CodingKey {
        case target, applied
        case units = "binaries"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        target = try values.decodeIfPresent(String.self, forKey: .target) ?? ""
        applied = try values.decodeIfPresent(Bool.self, forKey: .applied) ?? false
        units = try values.decodeIfPresent([ServiceUnit].self, forKey: .units) ?? []
    }
}

struct ServiceUnit: Decodable, Sendable {
    let binary: String
    let declaredVersion: String?
    let installedVersion: String?
    /// The directory the declared program lives in, as the host reported it.
    let root: String
    let unit: String
    let state: String
    let verdict: String
    let detail: String
    /// The path the running process is executing, read from the process rather
    /// than from the unit file.
    let runningBinary: String?
    /// `false` means the process is serving code that is no longer the code on
    /// disk under `root`. Optional because "the host did not say" is not the
    /// same answer as "they differ", and reading the first as the second would
    /// put a red flag on every unit an older agent reports.
    let binaryMatchesProcess: Bool?

    /// The finding that cost two separate debugging sessions: a worker served
    /// code from a directory replaced 26 seconds after the process started.
    var servesReplacedCode: Bool { binaryMatchesProcess == false }

    enum CodingKeys: String, CodingKey {
        case binary, root, unit, state, verdict, detail
        case declaredVersion = "declared_version"
        case installedVersion = "installed_version"
        case runningBinary = "running_binary"
        case binaryMatchesProcess = "binary_matches_process"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        binary = try values.decodeIfPresent(String.self, forKey: .binary) ?? ""
        declaredVersion = try values.decodeIfPresent(String.self, forKey: .declaredVersion)
        installedVersion = try values.decodeIfPresent(String.self, forKey: .installedVersion)
        root = try values.decodeIfPresent(String.self, forKey: .root) ?? ""
        unit = try values.decodeIfPresent(String.self, forKey: .unit) ?? ""
        state = try values.decodeIfPresent(String.self, forKey: .state) ?? ""
        verdict = try values.decodeIfPresent(String.self, forKey: .verdict) ?? ""
        detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
        runningBinary = try values.decodeIfPresent(String.self, forKey: .runningBinary)
        binaryMatchesProcess = try values.decodeIfPresent(Bool.self, forKey: .binaryMatchesProcess)
    }
}

/// One declared unit on one host. `service converge` reports per host, and the
/// screen lists every host at once, so the host travels with the row.
struct ServiceUnitRow: Identifiable, Sendable {
    let host: String
    let unit: ServiceUnit

    var id: String { "\(host)/\(unit.unit)/\(unit.binary)" }
}

/// `stado service list --unowned --json`: product processes running under no
/// declared unit. Nothing updates, restarts, or supervises these.
struct UnownedProcessReport: Decodable, Sendable {
    let processes: [UnownedProcess]

    enum CodingKeys: String, CodingKey {
        case processes = "unowned"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        processes = try values.decodeIfPresent([UnownedProcess].self, forKey: .processes) ?? []
    }
}

struct UnownedProcess: Decodable, Identifiable, Sendable {
    let host: String
    /// A pid as the fleet reports it: `stado service list --unowned` carries it
    /// as a string, because it comes off the host's own `ps` output and is an
    /// identifier to quote back, never a number to do arithmetic on.
    let pid: String
    let command: String
    /// The host's `ps` start stamp, verbatim — `Mon Aug 11 09:12:33 2026`. Kept
    /// as text because it is the fact that mattered: reformatting it and
    /// failing would report a four-day-old process with no age at all.
    let startedAt: String?
    /// What the fleet guesses this process belongs to. A guess is labelled as
    /// one on screen: nothing declared this process, so nothing knows.
    let productGuess: String?

    var id: String { "\(host)#\(pid)" }

    /// Only when the stamp parses. `nil` means "the host said when, and this
    /// app could not read it", which is why the stamp itself is what the screen
    /// shows and the age is the extra.
    var age: Double? {
        guard let started = StadoFormat.processStart(startedAt) else { return nil }
        return Date().timeIntervalSince(started)
    }

    enum CodingKeys: String, CodingKey {
        case host, pid, command
        case startedAt = "started_at"
        case productGuess = "product_guess"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decodeIfPresent(String.self, forKey: .host) ?? ""
        pid = Self.identifier(in: values, forKey: .pid)
        command = try values.decodeIfPresent(String.self, forKey: .command) ?? ""
        startedAt = operationalMetadata(try values.decodeIfPresent(String.self, forKey: .startedAt))
        productGuess = operationalMetadata(try values.decodeIfPresent(String.self, forKey: .productGuess))
    }

    /// A pid that arrives as a JSON number is still a pid. Accepting both
    /// spellings costs four lines and saves the whole list from failing to
    /// decode over one of them.
    private static func identifier(
        in values: KeyedDecodingContainer<CodingKeys>,
        forKey key: CodingKeys
    ) -> String {
        if let text = try? values.decode(String.self, forKey: key) {
            return text
        }
        guard let number = try? values.decode(Int.self, forKey: key) else { return "" }
        return String(number)
    }
}

// MARK: - Release evidence

/// `stado release status --json`, read for one thing only: which products roll
/// out to which targets. The rollout's actual condition comes from
/// `release doctor`, one call per pair, because only that command reaches the
/// host and reads the state file, the candidate and the claiming gates.
struct ReleaseInventory: Decodable, Sendable {
    let pairs: [ReleaseInventoryPair]

    enum CodingKeys: String, CodingKey {
        case pairs = "targets"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        pairs = try values.decodeIfPresent([ReleaseInventoryPair].self, forKey: .pairs) ?? []
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

/// The one word the screen sorts and colours on.
///
/// An unrecognised verdict is carried through rather than folded into a known
/// one: a rollout the console cannot classify must read as unclassified, never
/// as settled.
enum ReleaseVerdict: Hashable, Sendable {
    case settled
    case rolling
    case blocked
    case unrecognised(String)

    init(_ raw: String) {
        switch raw {
        case "settled": self = .settled
        case "rolling": self = .rolling
        case "blocked": self = .blocked
        default: self = .unrecognised(raw)
        }
    }

    /// The CLI's own word, never a translation of it.
    var word: String {
        switch self {
        case .settled: "settled"
        case .rolling: "rolling"
        case .blocked: "blocked"
        case let .unrecognised(raw): raw.isEmpty ? "unreported" : raw
        }
    }

    var needsAttention: Bool {
        if case .settled = self { return false }
        return true
    }
}

/// `stado release doctor <product> --target <host> --json`.
struct ReleaseDoctorReport: Decodable, Sendable {
    let product: String
    let target: String
    /// Absent when the registry declares no desired release for the product.
    let desiredVersion: String?
    /// Absent when the host has never recorded an active release, which is a
    /// different finding from "it runs an old one".
    let observedVersion: String?
    let phase: String
    /// The agent's own sentence about the phase. `pid 46748 is gone` was this
    /// field, and it was the only thing anybody saw for a candidate that died
    /// in ninety seconds.
    let detail: String
    let candidate: ReleaseCandidate
    let quarantined: [ReleaseQuarantineEntry]
    let gates: ReleaseGates
    let verdict: ReleaseVerdict
    /// Verbatim, in the CLI's words, in the CLI's order.
    let blockers: [String]

    var pair: ReleaseInventoryPair {
        ReleaseInventoryPair(product: product, target: target)
    }

    var isConverged: Bool {
        guard let desiredVersion, let observedVersion else { return false }
        return desiredVersion == observedVersion
    }

    enum CodingKeys: String, CodingKey {
        case product, target, phase, detail, candidate, quarantined, gates, verdict, blockers
        case desiredVersion = "desired_version"
        case observedVersion = "observed_version"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        product = try values.decodeIfPresent(String.self, forKey: .product) ?? ""
        target = try values.decodeIfPresent(String.self, forKey: .target) ?? ""
        desiredVersion = try values.decodeIfPresent(String.self, forKey: .desiredVersion)
        observedVersion = try values.decodeIfPresent(String.self, forKey: .observedVersion)
        phase = try values.decodeIfPresent(String.self, forKey: .phase) ?? ""
        detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
        candidate = try values.decodeIfPresent(ReleaseCandidate.self, forKey: .candidate)
            ?? ReleaseCandidate()
        quarantined = try values.decodeIfPresent([ReleaseQuarantineEntry].self, forKey: .quarantined) ?? []
        gates = try values.decodeIfPresent(ReleaseGates.self, forKey: .gates) ?? ReleaseGates()
        verdict = ReleaseVerdict(try values.decodeIfPresent(String.self, forKey: .verdict) ?? "")
        blockers = try values.decodeIfPresent([String].self, forKey: .blockers) ?? []
    }
}

/// The candidate the release agent staged, as the host answers for it now.
///
/// Every field stays optional on purpose: `pid_alive` is null when there is
/// nothing to probe, and rendering that as "not alive" would report a rollout
/// that has not started as one that died.
struct ReleaseCandidate: Decodable, Sendable {
    let port: Int?
    let healthStatus: String
    let pidAlive: Bool?

    /// A candidate exists on the host: `no_candidate` is the CLI's word for
    /// "the agent has staged nothing here".
    var exists: Bool {
        healthStatus != "no_candidate"
    }

    init(port: Int? = nil, healthStatus: String = "no_candidate", pidAlive: Bool? = nil) {
        self.port = port
        self.healthStatus = healthStatus
        self.pidAlive = pidAlive
    }

    enum CodingKeys: String, CodingKey {
        case port
        case healthStatus = "health_status"
        case pidAlive = "pid_alive"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        port = try values.decodeIfPresent(Int.self, forKey: .port)
        healthStatus = try values.decodeIfPresent(String.self, forKey: .healthStatus) ?? "no_candidate"
        pidAlive = try values.decodeIfPresent(Bool.self, forKey: .pidAlive)
    }
}

/// The host's claiming gates as `release doctor` reports them. A rollout on a
/// host that stopped claiming for disk is blocked by the disk, and this is the
/// section that says so where the rollout is being read.
struct ReleaseGates: Decodable, Sendable {
    let diskPressureUnresolved: Bool
    let freeGB: Double?
    let lowWatermarkGB: Double?

    init(diskPressureUnresolved: Bool = false, freeGB: Double? = nil, lowWatermarkGB: Double? = nil) {
        self.diskPressureUnresolved = diskPressureUnresolved
        self.freeGB = freeGB
        self.lowWatermarkGB = lowWatermarkGB
    }

    enum CodingKeys: String, CodingKey {
        case diskPressureUnresolved = "disk_pressure_unresolved"
        case freeGB = "free_gb"
        case lowWatermarkGB = "low_watermark_gb"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        diskPressureUnresolved = try values.decodeIfPresent(Bool.self, forKey: .diskPressureUnresolved) ?? false
        freeGB = try values.decodeIfPresent(Double.self, forKey: .freeGB)
        lowWatermarkGB = try values.decodeIfPresent(Double.self, forKey: .lowWatermarkGB)
    }
}

/// `stado release quarantine list <product> --target <host> --json`.
struct ReleaseQuarantineReport: Decodable, Sendable {
    let product: String
    let target: String
    let entries: [ReleaseQuarantineEntry]

    enum CodingKeys: String, CodingKey {
        case product, target, entries
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        product = try values.decodeIfPresent(String.self, forKey: .product) ?? ""
        target = try values.decodeIfPresent(String.self, forKey: .target) ?? ""
        entries = try values.decodeIfPresent([ReleaseQuarantineEntry].self, forKey: .entries) ?? []
    }
}

/// One digest the host refuses to roll out again.
///
/// `isDesiredDigest` is the field the section exists for: a quarantined digest
/// nobody desires is history, and the one that matches desired state is the
/// rollout being skipped on every pass until a human clears it.
/// `release doctor` omits the flag; `quarantine list` sets it.
struct ReleaseQuarantineEntry: Decodable, Identifiable, Sendable {
    let digest: String
    let reason: String
    let quarantinedAt: String?
    let isDesiredDigest: Bool

    var id: String { digest }

    var quarantinedAge: Double? {
        guard let quarantined = StadoFormat.date(quarantinedAt) else { return nil }
        return Date().timeIntervalSince(quarantined)
    }

    /// Twelve characters is what an operator compares against a build log; the
    /// full digest stays available in the row's own field.
    var shortDigest: String {
        let bare = digest.hasPrefix("sha256:") ? String(digest.dropFirst(7)) : digest
        return bare.count > 12 ? String(bare.prefix(12)) : bare
    }

    enum CodingKeys: String, CodingKey {
        case digest, reason
        case quarantinedAt = "quarantined_at"
        case isDesiredDigest = "is_desired_digest"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        digest = try values.decodeIfPresent(String.self, forKey: .digest) ?? ""
        reason = try values.decodeIfPresent(String.self, forKey: .reason) ?? ""
        quarantinedAt = try values.decodeIfPresent(String.self, forKey: .quarantinedAt)
        isDesiredDigest = try values.decodeIfPresent(Bool.self, forKey: .isDesiredDigest) ?? false
    }
}

/// `stado release quarantine clear … --reason <text> --json`. The audit record
/// the CLI wrote, read back so the screen states what was recorded rather than
/// what was requested.
struct ReleaseQuarantineClearance: Decodable, Sendable {
    let product: String
    let target: String
    let digest: String
    let cleared: Bool
    let reason: String
    let auditedAt: String?
    let stateBackup: String?

    enum CodingKeys: String, CodingKey {
        case product, target, digest, cleared, reason
        case auditedAt = "audited_at"
        case stateBackup = "state_backup"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        product = try values.decodeIfPresent(String.self, forKey: .product) ?? ""
        target = try values.decodeIfPresent(String.self, forKey: .target) ?? ""
        digest = try values.decodeIfPresent(String.self, forKey: .digest) ?? ""
        cleared = try values.decodeIfPresent(Bool.self, forKey: .cleared) ?? false
        reason = try values.decodeIfPresent(String.self, forKey: .reason) ?? ""
        auditedAt = try values.decodeIfPresent(String.self, forKey: .auditedAt)
        stateBackup = try values.decodeIfPresent(String.self, forKey: .stateBackup)
    }
}

/// Which of the candidate's own streams to read off the host.
///
/// stderr first, and stderr alone by default: in the incident the answer was
/// in `.err` while `.out` was empty, and a reader that opens stdout first
/// buries it.
enum ReleaseLogStreamSelection: String, CaseIterable, Identifiable, Sendable {
    case err
    case out
    case both

    var id: String { rawValue }

    var title: String {
        switch self {
        case .err: "stderr"
        case .out: "stdout"
        case .both: "Both"
        }
    }
}

/// `stado release logs <product> --target <host> --stream … --lines … --json`.
struct ReleaseLogsReport: Decodable, Sendable {
    let product: String
    let target: String
    let version: String
    let streams: [ReleaseLogStream]

    enum CodingKeys: String, CodingKey {
        case product, target, version, streams
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        product = try values.decodeIfPresent(String.self, forKey: .product) ?? ""
        target = try values.decodeIfPresent(String.self, forKey: .target) ?? ""
        version = try values.decodeIfPresent(String.self, forKey: .version) ?? ""
        streams = try values.decodeIfPresent([ReleaseLogStream].self, forKey: .streams) ?? []
    }
}

/// The order a held-digest list is read in.
extension Array where Element == ReleaseQuarantineEntry {
    /// The digest the registry desires first, then the rest newest first.
    ///
    /// The host answers in digest order, which is the one order that says
    /// nothing: on control-host it put the digest actually blocking the
    /// brama rollout seventh of seven, below six refusals that are history.
    /// Recency orders the rest, because a refusal recorded an hour ago is
    /// about the release being attempted now.
    var desiredFirst: [ReleaseQuarantineEntry] {
        map { (entry: $0, quarantined: StadoFormat.date($0.quarantinedAt)) }
            .sorted { lhs, rhs in
                if lhs.entry.isDesiredDigest != rhs.entry.isDesiredDigest {
                    return lhs.entry.isDesiredDigest
                }
                switch (lhs.quarantined, rhs.quarantined) {
                case let (left?, right?) where left != right:
                    return left > right
                case (nil, .some):
                    return false
                case (.some, nil):
                    return true
                default:
                    return lhs.entry.digest < rhs.entry.digest
                }
            }
            .map(\.entry)
    }
}

/// One log file on the host: where it is, how big it is, and its tail.
///
/// A stream with no lines is not a blank pane. The file was either never
/// created or opened and never written to, and those are different findings
/// about a candidate that died — so `state` is carried and, when the CLI
/// predates it, derived from the bytes.
struct ReleaseLogStream: Decodable, Identifiable, Sendable {
    let stream: String
    let path: String
    let bytes: Int?
    let lines: [String]
    let state: String

    var id: String { stream }

    var isMissing: Bool { state == "missing" }
    var isEmpty: Bool { state == "empty" }

    enum CodingKeys: String, CodingKey {
        case stream, path, bytes, lines, state
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        stream = try values.decodeIfPresent(String.self, forKey: .stream) ?? ""
        path = try values.decodeIfPresent(String.self, forKey: .path) ?? ""
        bytes = try values.decodeIfPresent(Int.self, forKey: .bytes)
        lines = try values.decodeIfPresent([String].self, forKey: .lines) ?? []
        if let reported = try values.decodeIfPresent(String.self, forKey: .state) {
            state = reported
        } else if bytes == nil {
            state = "missing"
        } else if lines.isEmpty {
            state = "empty"
        } else {
            state = "read"
        }
    }
}

/// What `release doctor` said about one product/target pair, or why it could
/// not say anything.
///
/// A diagnosis that failed is a state of its own. Folding it into "no data"
/// would render an unreachable host exactly like a settled rollout, which is
/// the reading this screen exists to prevent.
enum ReleaseDiagnosis: Sendable {
    case pending
    case diagnosed(ReleaseDoctorReport)
    case failed(String)
}

/// One row of the Releases table: the pair, and its diagnosis.
struct ReleaseRow: Identifiable, Sendable {
    let pair: ReleaseInventoryPair
    let diagnosis: ReleaseDiagnosis

    var id: String { pair.id }
    var product: String { pair.product }
    var target: String { pair.target }

    var report: ReleaseDoctorReport? {
        guard case let .diagnosed(report) = diagnosis else { return nil }
        return report
    }

    var problem: String? {
        guard case let .failed(problem) = diagnosis else { return nil }
        return problem
    }

    var isPending: Bool {
        guard case .pending = diagnosis else { return false }
        return true
    }

    /// Sort key. Blocked first, then a rollout nobody could diagnose, then the
    /// ones still moving, and settled last.
    var attentionRank: Int {
        switch diagnosis {
        case .pending:
            4
        case .failed:
            1
        case let .diagnosed(report):
            switch report.verdict {
            case .blocked: 0
            case .rolling: 2
            case .unrecognised: 3
            case .settled: 5
            }
        }
    }
}
