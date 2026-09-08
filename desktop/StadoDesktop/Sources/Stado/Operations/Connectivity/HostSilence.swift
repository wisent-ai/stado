import Foundation
import WisentDesignSystem

/// One recorded gap in a host's beacon stream — `host_silence/<host>/<at>.json`
/// read back.
///
/// `endedAt` nil is the live case and the one the Posture screen exists to
/// raise: the host is quiet right now.
struct HostSilenceRecord: Decodable, Identifiable, Sendable {
    let host: String
    let startedAt: String
    let endedAt: String?
    let durationSeconds: Int?
    /// The first refusal a reader hit while the host was quiet, in that
    /// component's own sentence.
    let firstReaderError: String?
    /// Which components noticed — resolver, cli, dashboard.
    let observedBy: [String]

    var id: String { "\(host)|\(startedAt)" }

    var isOpen: Bool { endedAt == nil }

    /// How long the gap lasted, or has lasted so far. The record's own figure
    /// when it carries one; otherwise measured from `startedAt`, because an
    /// open silence has no recorded duration until it closes.
    var elapsedSeconds: Double? {
        if let durationSeconds { return Double(durationSeconds) }
        guard let started = StadoFormat.date(startedAt) else { return nil }
        let ended = endedAt.flatMap(StadoFormat.date) ?? Date()
        let seconds = ended.timeIntervalSince(started)
        return seconds >= 0 ? seconds : nil
    }

    enum CodingKeys: String, CodingKey {
        case host
        case startedAt = "started_at"
        case endedAt = "ended_at"
        case durationSeconds = "duration_seconds"
        case firstReaderError = "first_reader_error"
        case observedBy = "observed_by"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decodeIfPresent(String.self, forKey: .host) ?? ""
        startedAt = try values.decodeIfPresent(String.self, forKey: .startedAt) ?? ""
        endedAt = try values.decodeIfPresent(String.self, forKey: .endedAt)
        durationSeconds = try values.decodeIfPresent(Int.self, forKey: .durationSeconds)
        firstReaderError = try values.decodeIfPresent(String.self, forKey: .firstReaderError)
        observedBy = try values.decodeIfPresent([String].self, forKey: .observedBy) ?? []
    }
}

/// How often a reader refused to answer about this host inside the window, and
/// under which stable reason tokens.
///
/// The tokens are the product's, not this console's: `directory_cache_stale`,
/// `authority_unreachable`, `beacon_stale`. Their verbatim sentences live in
/// the refusal blobs; the count and the tokens are what a screen can aggregate.
struct HostReaderRefusals: Decodable, Sendable {
    let windowSeconds: Int
    let count: Int
    let reasons: [String: Int]

    /// Descending by count, then by token, so the same reading orders the same
    /// way twice.
    var rankedReasons: [(reason: String, count: Int)] {
        reasons
            .map { (reason: $0.key, count: $0.value) }
            .sorted { $0.count == $1.count ? $0.reason < $1.reason : $0.count > $1.count }
    }

    enum CodingKeys: String, CodingKey {
        case count, reasons
        case windowSeconds = "window_seconds"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        windowSeconds = try values.decodeIfPresent(Int.self, forKey: .windowSeconds) ?? 0
        count = try values.decodeIfPresent(Int.self, forKey: .count) ?? 0
        reasons = try values.decodeIfPresent([String: Int].self, forKey: .reasons) ?? [:]
    }
}

/// The one word the Link section colours on.
///
/// `silent` and `degraded` are both failures the command exits 1 for, so both
/// get a panel. An unrecognised verdict is carried through rather than folded
/// into `healthy`: a link this console cannot classify must never read as fine.
enum HostLinkVerdict: Hashable, Sendable {
    case healthy
    case silent
    case degraded
    case unrecognised(String)

    init(_ raw: String) {
        switch raw {
        case "healthy": self = .healthy
        case "silent": self = .silent
        case "degraded": self = .degraded
        default: self = .unrecognised(raw)
        }
    }

    /// The CLI's own word, never a translation of it.
    var word: String {
        switch self {
        case .healthy: "healthy"
        case .silent: "silent"
        case .degraded: "degraded"
        case let .unrecognised(raw): raw.isEmpty ? "unreported" : raw
        }
    }

    /// Severity is the layout: a healthy link is one line, and everything else
    /// is a panel carrying the command's own blockers.
    var tone: WisentTone {
        switch self {
        case .healthy: .neutral
        case .silent, .degraded: .danger
        case .unrecognised: .warning
        }
    }

    var needsAttention: Bool {
        if case .healthy = self { return false }
        return true
    }

    /// The sentence the inspector and the facet rail both label this with.
    var label: String {
        switch self {
        case .healthy: "Healthy"
        case .silent: "Silent"
        case .degraded: "Degraded"
        case let .unrecognised(raw): raw.isEmpty ? "Not reported" : raw.humanizedIdentifier
        }
    }
}
