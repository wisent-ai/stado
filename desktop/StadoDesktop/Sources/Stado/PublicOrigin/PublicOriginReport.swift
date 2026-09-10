import Foundation

/// What the resolver was asked and what it answered, as
/// `stado.public-origin-report.v1` carries it.
///
/// The answers list is the resolver's own: an empty list beside
/// `dns_unresolved` is the measurement, and an empty list beside
/// `dns_unavailable` is the absence of one. The state word tells them apart,
/// so this type never collapses them into "no addresses".
struct PublicOriginResolution: Decodable, Sendable {
    let state: PublicOriginResolutionState
    /// The resolver actually used, so a report is attributable.
    let resolver: String?
    let answers: [String]
    let detail: String?

    enum CodingKeys: String, CodingKey {
        case state, resolver, answers, detail
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        state = PublicOriginResolutionState(
            try values.decodeIfPresent(String.self, forKey: .state) ?? ""
        )
        resolver = try values.decodeIfPresent(String.self, forKey: .resolver)
        answers = try values.decodeIfPresent([String].self, forKey: .answers) ?? []
        detail = try values.decodeIfPresent(String.self, forKey: .detail)
    }
}

/// Whether the declared paths are published, and which of them are missing.
struct PublicOriginPublicationReport: Decodable, Sendable {
    let state: PublicOriginPublicationState
    let publishedPaths: [String]
    let missingPaths: [String]
    /// Whether the node's publication mechanism is enabled at all. Absent on
    /// a publication whose reader could not tell.
    let funnelEnabled: Bool?
    let detail: String?

    enum CodingKeys: String, CodingKey {
        case state, detail
        case publishedPaths = "published_paths"
        case missingPaths = "missing_paths"
        case funnelEnabled = "funnel_enabled"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        state = PublicOriginPublicationState(
            try values.decodeIfPresent(String.self, forKey: .state) ?? ""
        )
        publishedPaths = try values.decodeIfPresent([String].self, forKey: .publishedPaths) ?? []
        missingPaths = try values.decodeIfPresent([String].self, forKey: .missingPaths) ?? []
        funnelEnabled = try values.decodeIfPresent(Bool.self, forKey: .funnelEnabled)
        detail = try values.decodeIfPresent(String.self, forKey: .detail)
    }
}

/// What the public edge says it selects, and the endpoint that was read to
/// learn it.
struct PublicOriginEdgeSelection: Decodable, Sendable {
    let state: PublicOriginEdgeSelectionState
    /// The origin the edge selects. Absent when the edge declared none or
    /// could not be read.
    let origin: String?
    let endpoint: String?
    let detail: String?
    let diagnosis: StorageReconciliationJSON?
    let observation: StorageReconciliationJSON?
    let readback: StorageReconciliationJSON?
    let readbackObservation: StorageReconciliationJSON?

    enum CodingKeys: String, CodingKey {
        case state, origin, endpoint, detail
        case diagnosis, observation
        case readback
        case readbackObservation = "readback_observation"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        state = PublicOriginEdgeSelectionState(
            try values.decodeIfPresent(String.self, forKey: .state) ?? ""
        )
        origin = try values.decodeIfPresent(String.self, forKey: .origin)
        endpoint = try values.decodeIfPresent(String.self, forKey: .endpoint)
        detail = try values.decodeIfPresent(String.self, forKey: .detail)
        diagnosis = try values.decodeIfPresent(StorageReconciliationJSON.self, forKey: .diagnosis)
        observation = try values.decodeIfPresent(StorageReconciliationJSON.self, forKey: .observation)
        readback = try values.decodeIfPresent(StorageReconciliationJSON.self, forKey: .readback)
        readbackObservation = try values.decodeIfPresent(StorageReconciliationJSON.self, forKey: .readbackObservation)
    }
}

/// One row of `stado web origin status --json`.
///
/// The declaration travels with the measurement on purpose: a public release
/// origin is a registry declaration, and a report that named only a verdict
/// would leave an operator guessing which hostname, which host and which
/// paths it was about.
struct PublicOriginReport: Decodable, Identifiable, Sendable {
    static let schemaName = "stado.public-origin-report.v1"

    let schema: String
    let name: String
    let hostname: String
    /// The origin URL a public edge would fetch, as the command composed it.
    let origin: String
    let target: String
    let publication: String
    let upstream: String
    let paths: [String]
    let verdict: PublicOriginVerdict
    /// Stado's own sentence about this origin. Shown verbatim, never
    /// paraphrased: the operator and the release gate read the same words.
    let originError: String?
    let resolution: PublicOriginResolution?
    let publicationState: PublicOriginPublicationReport?
    let edgeSelection: PublicOriginEdgeSelection?
    let observations: StorageReconciliationJSON?
    let registryObservation: StorageReconciliationJSON?

    var id: String { name.isEmpty ? origin : name }

    enum CodingKeys: String, CodingKey {
        case schema, name, hostname, origin, target, publication, upstream, paths, verdict, resolution
        case originError = "origin_error"
        case publicationState = "publication_state"
        case edgeSelection = "edge_selection"
        case observations
        case registryObservation = "registry_observation"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        schema = try values.decodeIfPresent(String.self, forKey: .schema) ?? ""
        name = try values.decodeIfPresent(String.self, forKey: .name) ?? ""
        hostname = try values.decodeIfPresent(String.self, forKey: .hostname) ?? ""
        origin = try values.decodeIfPresent(String.self, forKey: .origin) ?? ""
        target = try values.decodeIfPresent(String.self, forKey: .target) ?? ""
        publication = try values.decodeIfPresent(String.self, forKey: .publication) ?? ""
        upstream = try values.decodeIfPresent(String.self, forKey: .upstream) ?? ""
        paths = try values.decodeIfPresent([String].self, forKey: .paths) ?? []
        verdict = PublicOriginVerdict(
            try values.decodeIfPresent(String.self, forKey: .verdict) ?? ""
        )
        originError = try values.decodeIfPresent(String.self, forKey: .originError)
        resolution = try values.decodeIfPresent(PublicOriginResolution.self, forKey: .resolution)
        publicationState = try values.decodeIfPresent(
            PublicOriginPublicationReport.self,
            forKey: .publicationState
        )
        edgeSelection = try values.decodeIfPresent(
            PublicOriginEdgeSelection.self,
            forKey: .edgeSelection
        )
        observations = try values.decodeIfPresent(StorageReconciliationJSON.self, forKey: .observations)
        registryObservation = try values.decodeIfPresent(StorageReconciliationJSON.self, forKey: .registryObservation)
    }

    /// Whether a convergence of this declaration is the operation on offer.
    /// A verdict that names no declaration has nothing for the converge to
    /// read, and a serving origin has nothing to change.
    var isConvergeable: Bool {
        guard !name.isEmpty else { return false }
        return switch verdict {
        case .serving, .originUndeclared, .diagnosticIncomplete: false
        default: true
        }
    }

    var declaredPathsLabel: String {
        paths.isEmpty ? "No path declared" : paths.joined(separator: ", ")
    }
}
