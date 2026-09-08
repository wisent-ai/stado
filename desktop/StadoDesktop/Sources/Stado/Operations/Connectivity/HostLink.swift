import Foundation

// MARK: - Connectivity, sleep and silence

/// `stado host link <host> --json`.
///
/// The reading that did not exist on 2026-08-19, when `control-host` went
/// unreachable for six minutes and the only evidence anywhere was an operator's
/// two ping packets: the product recorded nothing, and the reader-side refusals
/// went to a log file nobody was watching. Every field here is the CLI's own
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
    /// Newest first, as the command orders them.
    let silences: [HostSilenceRecord]
    let readerRefusals: HostReaderRefusals?
    /// Whether anybody is logged in on the screen of that machine. `nil` when
    /// the command carried no `session` object at all, which is the same
    /// answer to an operator as a reported `unknown`: nobody said.
    let session: HostLinkSession?
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
        session?.headline ?? "Not reported"
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
        case host, endpoint, silences, verdict, blockers, session
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
        case readerRefusals = "reader_refusals"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decodeIfPresent(String.self, forKey: .host) ?? ""
        beaconAgeSeconds = try values.decodeIfPresent(Int.self, forKey: .beaconAgeSeconds)
        sshReachable = try values.decodeIfPresent(Bool.self, forKey: .sshReachable) ?? false
        pathKind = (try values.decodeIfPresent(String.self, forKey: .pathKind))
            .map(HostLinkPathKind.init)
        endpoint = try values.decodeIfPresent(String.self, forKey: .endpoint)
        selectedConnection = try values.decodeIfPresent(String.self, forKey: .selectedConnection)
        connectionPaths =
            try values.decodeIfPresent([HostConnectionPathProbe].self, forKey: .connectionPaths) ?? []
        connectionProbeError = try values.decodeIfPresent(String.self, forKey: .connectionProbeError)
        beaconPublisher =
            try values.decodeIfPresent(HostBeaconPublisherDiagnosis.self, forKey: .beaconPublisher)
        lastSleepAt = try values.decodeIfPresent(String.self, forKey: .lastSleepAt)
        lastWakeAt = try values.decodeIfPresent(String.self, forKey: .lastWakeAt)
        interfaceChanges =
            try values.decodeIfPresent([HostLinkInterfaceChange].self, forKey: .interfaceChanges) ?? []
        silences = try values.decodeIfPresent([HostSilenceRecord].self, forKey: .silences) ?? []
        readerRefusals = try values.decodeIfPresent(HostReaderRefusals.self, forKey: .readerRefusals)
        session = try values.decodeIfPresent(HostLinkSession.self, forKey: .session)
        verdict = HostLinkVerdict(try values.decodeIfPresent(String.self, forKey: .verdict) ?? "")
        blockers = try values.decodeIfPresent([String].self, forKey: .blockers) ?? []
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
/// on that host actually resolves a service through. On 2026-09-05
/// `lukasz-macbook` carried `weles-admission` at `18794` while its own
/// resolver adapter for that service binds `17614`; the file was the only
/// statement of the address and nothing displayed it.
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
