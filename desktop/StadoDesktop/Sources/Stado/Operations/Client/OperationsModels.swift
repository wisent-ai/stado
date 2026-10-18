import Foundation

/// What the Hosts, Queue and Posture screens show, built from one
/// `stado overview --json` document ([`FleetOverview`]) so the screens and
/// the CLI read the same answer.
struct DashboardSnapshot: Sendable {
    /// When the CLI assembled the document.
    let generatedAt: String
    let counts: JobCounts
    let byModelState: [String: JobCounts]
    /// The hosts publishing capacity now.
    let liveAgents: [WorkerNode]
    /// Every live publisher, then every declared local worker that is not
    /// publishing.
    let workers: [WorkerNode]
    let recentFailed: [FailedJob]
    let completedRecent: [CompletedJob]
    let throughput: Throughput
}

struct JobCounts: Sendable {
    let queue: Int
    let running: Int
    let completed: Int
    let failed: Int
}

enum WorkerAvailability: String, Sendable {
    case live
    case stale
    case unavailable
}

struct WorkerNode: Identifiable, Sendable {
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

    /// The target name, else the consumer identity: the key the Hosts screen
    /// selects by, and one of the two is always set by [`FleetOverview`].
    var id: String { displayName }

    var displayName: String {
        if let targetName, !targetName.isEmpty {
            return targetName
        }
        if let consumerID, !consumerID.isEmpty {
            return consumerID
        }
        return "Unnamed worker"
    }
}

struct CompletedJob: Identifiable, Sendable {
    let jobID: String
    let model: String?
    let task: String?
    let wallSeconds: Double?
    let completedAt: String?

    var id: String { jobID }
}

struct FailedJob: Identifiable, Sendable {
    let jobID: String
    let model: String?
    let task: String?
    let error: String?

    var id: String { jobID }
}

struct Throughput: Sendable {
    /// Nil when no completed job carries both of its stamps.
    let averageWallSecondsPerCompletedJob: Double?
    /// How many completed jobs the average is over.
    let samples: Int
    /// Job stamps that did not read, so their jobs are not in the average.
    let unreadableStamps: [UnreadableStamp]
}

/// One job stamp `stado overview` could not read: the job, the field, the
/// value it holds and the parser's error.
struct UnreadableStamp: Decodable, Sendable {
    let jobID: String
    let field: String
    let value: String
    let error: String

    enum CodingKeys: String, CodingKey {
        case jobID = "job_id"
        case field, value, error
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

    /// The ISO 8601 time `value` spells, with or without fractional seconds;
    /// nil when it is absent or spells neither form.
    static func date(_ value: String?) -> Date? {
        guard let value else { return nil }
        do {
            return try fractionalDateStrategy.parse(value)
        } catch {
            do {
                return try dateStrategy.parse(value)
            } catch {
                return nil
            }
        }
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

    /// A wall time in the units it spans, as the system's duration style
    /// spells them.
    static func duration(_ seconds: Double?) -> String {
        guard let seconds, seconds.isFinite, seconds >= .zero else { return "Unavailable" }
        return Duration.seconds(seconds).formatted(
            .units(allowed: [.hours, .minutes, .seconds], width: .abbreviated)
        )
    }

    static func decimal(_ value: Double?) -> String {
        guard let value, value.isFinite else { return "Unavailable" }
        return value.formatted(.number)
    }

    /// A value a row may leave out, shown as the absence the caller names
    /// ("No region", "No report") instead of a dash that says nothing.
    static func named(_ value: String?, absent: String) -> String {
        guard let value, !value.isEmpty else { return absent }
        return value
    }
}
