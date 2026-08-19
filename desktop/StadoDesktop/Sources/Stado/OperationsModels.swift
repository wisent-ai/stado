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

// MARK: - Fleet services

/// Which launchd domain a declared unit-file path places the unit in.
///
/// The same classification the CLI's `UnitDomain::from_path` performs, done
/// client-side because `service list --json` carries the path and not the
/// verdict: `/Library/LaunchDaemons` loads as root, `/Library/LaunchAgents`
/// loads for whichever user is logged in, a home `Library/LaunchAgents` loads
/// for that user, and anything else — a systemd path, an empty path — is
/// unknown. The registry's `$HOME/...` idiom arrives unexpanded, so the user
/// domain is matched on the segment, not on a home prefix.
enum ServiceDomain: String, Sendable {
    case system
    case anyUser = "any-user"
    case user
    case unknown

    init(path: String) {
        if path.hasPrefix("/Library/LaunchDaemons/") {
            self = .system
        } else if path.hasPrefix("/Library/LaunchAgents/") {
            self = .anyUser
        } else if path.contains("/Library/LaunchAgents/") {
            self = .user
        } else {
            self = .unknown
        }
    }

    /// True when loading the unit takes root. The approved channel is
    /// unprivileged, so a system LaunchDaemon gets no restart button — the
    /// CLI itself refuses the same request with the same reason.
    var requiresPrivilegedBootstrap: Bool { self == .system }
}

/// Why one `failed` unit died, gathered best-effort from the host itself.
///
/// `stado service status <name> --json` attaches this to failed entries; the
/// fields are optional because the evidence is gathered over a channel that
/// may itself be the thing that is down.
struct ServiceFailure: Decodable, Sendable {
    /// The last launchd exit status, as a string exactly as `launchctl list`
    /// carried it.
    let lastExit: String?
    /// Where the stderr tail came from, or the reason there is none.
    let errorOrigin: String?
    let errorLines: [String]
    let note: String?

    enum CodingKeys: String, CodingKey {
        case note
        case lastExit = "last_exit"
        case errorOrigin = "error_origin"
        case errorLines = "error_lines"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        lastExit = operationalMetadata(try values.decodeIfPresent(String.self, forKey: .lastExit))
        errorOrigin = operationalMetadata(try values.decodeIfPresent(String.self, forKey: .errorOrigin))
        errorLines = try values.decodeIfPresent([String].self, forKey: .errorLines) ?? []
        note = operationalMetadata(try values.decodeIfPresent(String.self, forKey: .note))
    }
}

/// `stado service list --json`: one registry-managed service on one host,
/// with the state the host's latest health beacon reports.
///
/// The read is beacon-only — no ssh, no per-host round trip — so it stays
/// answerable while a host is wedged, and a host that never published a
/// beacon arrives as `unknown` rows whose detail says so, not as an absent
/// answer. Every field decodes leniently: the beacon and the registry
/// document are both operator-facing state, and a half-filled record should
/// read as a listed service with blanks rather than vanish from the list.
struct FleetServiceEntry: Decodable, Identifiable, Sendable {
    let host: String
    /// The name the CLI addresses the service by.
    let name: String
    /// systemd unit name; empty for a launchd service.
    let unit: String
    /// launchd label; empty for a systemd service.
    let label: String
    /// The host's own name for the unit: the label, or the systemd unit.
    let unitID: String
    /// The declared unit-file path, `$HOME`-relative where the declaration is.
    let path: String
    let kind: String
    /// The state word the beacon reported: `active`, `inactive`, `failed`,
    /// `missing`, `unknown` — or whatever other word the host used.
    let state: String
    /// The beacon's `reported_at`, verbatim: a confident-looking `active`
    /// from a five-day-old beacon is visibly five days old.
    let reportedAt: String
    /// Why the state is what it is, when that is not self-evident.
    let detail: String
    /// Failure evidence, merged in by the store from `service status --json`;
    /// `service list --json` itself does not carry it.
    var failure: ServiceFailure?

    var id: String { "\(host)/\(unitID.isEmpty ? name : unitID)" }

    var domain: ServiceDomain { ServiceDomain(path: path) }

    var isFailed: Bool { state == "failed" }

    enum CodingKeys: String, CodingKey {
        case host, name, unit, label, path, kind, state, detail, failure
        case unitID = "unit_id"
        case reportedAt = "reported_at"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decodeIfPresent(String.self, forKey: .host) ?? ""
        name = try values.decodeIfPresent(String.self, forKey: .name) ?? ""
        unit = try values.decodeIfPresent(String.self, forKey: .unit) ?? ""
        label = try values.decodeIfPresent(String.self, forKey: .label) ?? ""
        unitID = try values.decodeIfPresent(String.self, forKey: .unitID) ?? ""
        path = try values.decodeIfPresent(String.self, forKey: .path) ?? ""
        kind = try values.decodeIfPresent(String.self, forKey: .kind) ?? ""
        state = try values.decodeIfPresent(String.self, forKey: .state) ?? ""
        reportedAt = try values.decodeIfPresent(String.self, forKey: .reportedAt) ?? ""
        detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
        failure = try values.decodeIfPresent(ServiceFailure.self, forKey: .failure)
    }
}

/// One element of the `stado service restart <name> --host <host> --json`
/// payload: the remote program's own markers, plus the postcondition probe
/// the host ran before the connection closed.
struct ServiceRestartReport: Decodable, Sendable {
    let host: String
    let unit: String
    /// The outcome word from the `STADO_SERVICE` marker; `restarted` is the
    /// one this command wants.
    let status: String
    let detail: String
    let postcondition: Postcondition?

    struct Postcondition: Decodable, Sendable {
        let intent: String
        /// `met`, `unmet`, or `unobserved`.
        let state: String
        let detail: String

        enum CodingKeys: String, CodingKey {
            case intent, state, detail
        }

        init(from decoder: Decoder) throws {
            let values = try decoder.container(keyedBy: CodingKeys.self)
            intent = try values.decodeIfPresent(String.self, forKey: .intent) ?? ""
            state = try values.decodeIfPresent(String.self, forKey: .state) ?? ""
            detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
        }
    }

    enum CodingKeys: String, CodingKey {
        case host, unit, status, detail, postcondition
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decodeIfPresent(String.self, forKey: .host) ?? ""
        unit = try values.decodeIfPresent(String.self, forKey: .unit) ?? ""
        status = try values.decodeIfPresent(String.self, forKey: .status) ?? ""
        detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
        postcondition = try values.decodeIfPresent(Postcondition.self, forKey: .postcondition)
    }

    /// The CLI's success rule, mirrored: the outcome word AND the host
    /// observed in the state the restart intended. A step that succeeds is
    /// not the same fact as a machine that works.
    var succeeded: Bool {
        status == "restarted" && (postcondition == nil || postcondition?.state == "met")
    }

    /// The one-line failure, in the same shape the CLI prints.
    var failureText: String {
        let reported = detail.isEmpty ? status : "\(status): \(detail)"
        guard let postcondition, postcondition.state != "met" else { return reported }
        return "\(reported); postcondition \(postcondition.state): \(postcondition.intent) (\(postcondition.detail))"
    }
}

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

/// One row of the Releases table: the pair, its diagnosis, and what the host
/// itself reported it runs.
struct ReleaseRow: Identifiable, Sendable {
    let pair: ReleaseInventoryPair
    let diagnosis: ReleaseDiagnosis
    /// Straight off `release status --json`, never recomputed here.
    let software: ReleaseSoftwareReport?

    init(
        pair: ReleaseInventoryPair,
        diagnosis: ReleaseDiagnosis,
        software: ReleaseSoftwareReport? = nil
    ) {
        self.pair = pair
        self.diagnosis = diagnosis
        self.software = software
    }

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
    ///
    /// A host that cannot be shown to run what the fleet declares never sorts
    /// below a moving rollout, whatever the release agent's own state file says
    /// about the rollout. `brama desired=0.2.27 observed=unreported` sat quietly
    /// in a list for a day; a row whose software verdict failed is pulled up
    /// beside the blocked ones so it cannot do that again.
    var attentionRank: Int {
        let rollout: Int = switch diagnosis {
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
        return software?.failed == true ? min(rollout, 1) : rollout
    }
}
