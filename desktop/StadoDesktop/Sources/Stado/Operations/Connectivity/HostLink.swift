import Foundation
import WisentDesignSystem

// MARK: - Connectivity, sleep and silence

/// `stado host link <host> --json`.
///
/// The reading that is otherwise missing when a host goes unreachable for
/// minutes and the only evidence anywhere is an operator's ping packets: the
/// product records nothing, and the reader-side refusals go to a log file
/// nobody is watching. Every field here is the CLI's own
/// answer. Nothing is derived from a second source, and nothing absent is
/// rendered as a zero — a host whose beacon carries no `link` block has not
/// reported its path, which is a different fact from reporting `unknown`.
struct HostLink: Decodable, Identifiable, Sendable {
    let host: String
    /// How old the newest beacon for this host is. `nil` when no beacon has
    /// ever been published, which is not the same as "0 s ago".
    let beaconAgeSeconds: Int?
    let sshReachable: Bool
    /// `direct`, `relay` or `unknown` as the beacon's link block spelled it;
    /// `nil` when the beacon carried no link block at all.
    let pathKind: HostLinkPathKind?
    let endpoint: String?
    /// The route the selector would use for one real host operation after all
    /// declared routes were probed.
    let selectedConnection: String?
    /// Preferred route followed by ordered alternates, with the answer from the
    /// side-effect-free SSH probe for each one.
    let connectionPaths: [HostConnectionPathProbe]
    /// A resolver failure that prevented the route set itself from being read.
    let connectionProbeError: String?
    /// The managed beacon publisher's newest recorded outcome. Present only
    /// when the beacon is stale while the host itself still answers.
    let beaconPublisher: HostBeaconPublisherDiagnosis?
    let lastSleepAt: String?
    let lastWakeAt: String?
    let interfaceChanges: [HostLinkInterfaceChange]
    /// Every datum of the link block the host could not read, each with its
    /// cause, verbatim. `nil` when the beacon's Stado did not report it.
    let linkUnread: [String]?
    /// Newest first, as the command orders them.
    let silences: [HostSilenceRecord]
    let readerRefusals: HostReaderRefusals?
    /// Whether anybody is logged in on the screen of that machine. `nil` when
    /// the command carried no `session` object at all, which is the same
    /// answer to an operator as a reported `unknown`: nobody said.
    let session: HostLinkSession?
    /// What the host's own disk scans last decided: the load reading the
    /// latest admission was decided on and every scan standing skipped above
    /// the host's ceiling. `nil` when the beacon carried no `scans` block,
    /// which a publisher from before the block leaves out.
    let scans: HostScans?
    /// Which processes the host's last beacon named as running it out of
    /// memory before its next beacon. `nil` when the beacon carried no
    /// verdict, which a publisher from before it, or a reading the kernel
    /// refused, leaves out.
    let memory: HostMemory?
    let verdict: HostLinkVerdict
    /// Verbatim. A blocker paraphrased here is a second opinion about why a
    /// host went quiet.
    let blockers: [String]

    /// The one line the Link section renders for the session fact.
    ///
    /// An absent object and a reported `unknown` both read "Not reported". A
    /// console that guessed "nobody is logged in" from silence would be
    /// asserting the fact this reading exists to establish.
    var sessionLine: String {
        guard let session else { return "Not reported" }
        return session.headline
    }

    /// The one line the Link section renders for what the host could not
    /// read of its own link: each cause on its own line, "None" when every
    /// probe answered, and "Not reported" when the beacon's Stado did not say.
    var unreadLine: String {
        guard let linkUnread else { return "Not reported" }
        guard !linkUnread.isEmpty else { return "None" }
        return linkUnread.joined(separator: "\n")
    }

    var id: String { host }

    /// Whether the beacon carried a link block at all.
    ///
    /// `stado host link` prints `path_kind: "unknown"` with every other link
    /// field empty when there was no block to read — checked against the live
    /// answer for `control-host` — so a bare `unknown` is the absence of a
    /// report, not a report of an unknown path. The distinction decides whether
    /// an operator chases the network or the collector, and the command states
    /// which one it is in its own blocker sentence.
    var linkReported: Bool {
        if endpoint != nil || lastSleepAt != nil || lastWakeAt != nil || !interfaceChanges.isEmpty {
            return true
        }
        guard let pathKind else { return false }
        return pathKind != .unknown
    }

    /// The silence that has not ended. At most one: a silence closes on the
    /// first fresher beacon, so an open one is the current gap.
    var openSilence: HostSilenceRecord? {
        silences.first { $0.isOpen }
    }

    enum CodingKeys: String, CodingKey {
        case host, endpoint, silences, verdict, blockers, session, scans, memory
        case beaconPublisher = "beacon_publisher"
        case beaconAgeSeconds = "beacon_age_seconds"
        case sshReachable = "ssh_reachable"
        case selectedConnection = "selected_connection"
        case connectionPaths = "connection_paths"
        case connectionProbeError = "connection_probe_error"
        case pathKind = "path_kind"
        case lastSleepAt = "last_sleep_at"
        case lastWakeAt = "last_wake_at"
        case interfaceChanges = "interface_changes"
        case linkUnread = "link_unread"
        case readerRefusals = "reader_refusals"
    }

    /// Every field `stado host link --json` always writes is required: a
    /// report without one is not this command's answer, and decoding fails
    /// naming the missing key rather than standing an empty value in for it.
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decode(String.self, forKey: .host)
        beaconAgeSeconds = try values.decodeIfPresent(Int.self, forKey: .beaconAgeSeconds)
        sshReachable = try values.decode(Bool.self, forKey: .sshReachable)
        pathKind = (try values.decodeIfPresent(String.self, forKey: .pathKind))
            .map(HostLinkPathKind.init)
        endpoint = try values.decodeIfPresent(String.self, forKey: .endpoint)
        selectedConnection = try values.decodeIfPresent(String.self, forKey: .selectedConnection)
        connectionPaths =
            try values.decode([HostConnectionPathProbe].self, forKey: .connectionPaths)
        connectionProbeError = try values.decodeIfPresent(String.self, forKey: .connectionProbeError)
        beaconPublisher =
            try values.decodeIfPresent(HostBeaconPublisherDiagnosis.self, forKey: .beaconPublisher)
        lastSleepAt = try values.decodeIfPresent(String.self, forKey: .lastSleepAt)
        lastWakeAt = try values.decodeIfPresent(String.self, forKey: .lastWakeAt)
        interfaceChanges =
            try values.decode([HostLinkInterfaceChange].self, forKey: .interfaceChanges)
        linkUnread = try values.decodeIfPresent([String].self, forKey: .linkUnread)
        silences = try values.decode([HostSilenceRecord].self, forKey: .silences)
        readerRefusals = try values.decodeIfPresent(HostReaderRefusals.self, forKey: .readerRefusals)
        session = try values.decodeIfPresent(HostLinkSession.self, forKey: .session)
        scans = try values.decodeIfPresent(HostScans.self, forKey: .scans)
        memory = try values.decodeIfPresent(HostMemory.self, forKey: .memory)
        verdict = HostLinkVerdict(try values.decode(String.self, forKey: .verdict))
        blockers = try values.decode([String].self, forKey: .blockers)
    }
}

/// One declared SSH route in `stado host link <host> --json`.
///
/// These are host-control routes, not the beacon's direct/relay network path:
/// the distinction is visible in the Hosts inspector because one describes
/// how Stado reaches the machine and the other describes how its beacon left.
struct HostConnectionPathProbe: Decodable, Identifiable, Sendable {
    let name: String
    let destination: String
    let reachable: Bool
    let error: String?

    var id: String { name }
}

/// One `~/.stado/forwards/<service>.url` marker as `stado host inventory`
/// reports it: the address consumers on that host dial, whether anything
/// answers there, and whether it is the address the fleet declares for them.
///
/// The console had no surface for these at all, and they are what a product
/// on that host actually resolves a service through. A marker can name one
/// port while the host's own resolver adapter for that service binds
/// another; the file is the only statement of the address and nothing else
/// displays it.
struct HostForwardMarker: Decodable, Identifiable, Sendable {
    let name: String
    let url: String
    /// `matches`, `stale`, `unreadable` or `unknown`: whether anything answers
    /// where the marker points.
    let reconciliation: String
    /// The address the fleet declares for this host, when it declares one.
    let declaredUrl: String?
    /// `directory-endpoint` when this host serves the service,
    /// `resolver-adapter` when it dials its own adapter, `undeclared` when the
    /// fleet claims neither.
    let declaredSource: String
    /// `matches`, `disagrees` or `undeclared`.
    let declarationVerdict: String

    var id: String { name }

    private enum CodingKeys: String, CodingKey {
        case name
        case url
        case reconciliation
        case declaredUrl = "declared_url"
        case declaredSource = "declared_source"
        case declarationVerdict = "declaration_verdict"
    }
}

// MARK: - Which processes run a host out of memory

/// The memory verdict of a host's beacon, as `stado host link <host> --json`
/// carries it under `memory`: at the rate each process grew over the beacon's
/// last interval, against the memory the kernel says a new allocation can
/// still obtain, the fewest processes, largest growth first, whose growth
/// together leaves the host no memory before its next beacon. The line is
/// the one `stado host link` prints as `memory:`.
struct HostMemory: Decodable, Sendable {
    let horizonSeconds: Double
    /// `measured_interval` or `within`.
    let horizonSource: String
    /// `nil` with `availableUnread` naming why, and then nothing was judged.
    let availableBytes: Int64?
    let availableUnread: String?
    let grownBytes: UInt64
    /// `nil` when nothing grew or nothing was judged.
    let exhaustsAvailableInSeconds: Double?
    let processes: [HostMemoryGrowth]

    enum CodingKeys: String, CodingKey {
        case processes
        case horizonSeconds = "horizon_seconds"
        case horizonSource = "horizon_source"
        case availableBytes = "available_bytes"
        case availableUnread = "available_unread"
        case grownBytes = "grown_bytes"
        case exhaustsAvailableInSeconds = "exhausts_available_in_seconds"
    }

    var line: String {
        let horizon = "the next reading in \(horizonSeconds.formatted()) s (\(horizonSource))"
        if let availableUnread {
            return "not judged, since the available memory was not read: \(availableUnread)"
        }
        let available = availableBytes.map { "\($0)" } ?? "not reported"
        if !processes.isEmpty {
            let named = processes.map { "pid \($0.pid) (\($0.program))" }.joined(separator: ", ")
            let seconds = exhaustsAvailableInSeconds.map { $0.formatted() } ?? "not reported"
            return "RUNAWAY: \(named) use up the \(available) available byte(s) in \(seconds) s at this interval's growth, before \(horizon)"
        }
        if let exhaustsAvailableInSeconds {
            return "\(available) byte(s) available; all growth of the interval together (\(grownBytes) byte(s)) would use it up in \(exhaustsAvailableInSeconds.formatted()) s, past \(horizon)"
        }
        return "\(available) byte(s) available; no process grew in the interval"
    }

    /// The Link section's field: the verdict's line, or that the beacon has
    /// not reported one.
    static func description(of memory: HostMemory?) -> String {
        memory?.line ?? "Not reported"
    }

    /// A named process is the host running out of memory before anybody
    /// looks again; a verdict left unjudged is worth the eye too.
    static func tone(of memory: HostMemory?) -> WisentTone {
        guard let memory else { return .neutral }
        if !memory.processes.isEmpty { return .danger }
        return memory.availableUnread == nil ? .neutral : .warning
    }
}

/// One process the verdict names: what it grew, its resident size, and how
/// soon it alone uses up the available memory.
struct HostMemoryGrowth: Decodable, Sendable {
    let pid: Int32
    let program: String
    let residentBytes: UInt64
    let grownBytes: UInt64
    let exhaustsAvailableInSeconds: Double

    enum CodingKeys: String, CodingKey {
        case pid, program
        case residentBytes = "resident_bytes"
        case grownBytes = "grown_bytes"
        case exhaustsAvailableInSeconds = "exhausts_available_in_seconds"
    }
}
