import Foundation

/// What the resolver was asked and what it answered, as
/// `stado.public-origin-report.v1` carries it.
///
/// The answers list is the resolver's own: an empty list beside
/// `dns_unresolved` is the measurement, and an empty list beside
/// `dns_unavailable` is the absence of one. The state word tells them apart,
/// so this type never collapses them into "no addresses". Every field is
/// written by the CLI for every resolution, so each is decoded as required.
struct PublicOriginResolution: Decodable, Sendable {
    let state: PublicOriginResolutionState
    /// The resolver actually used, so a report is attributable.
    let resolver: String
    let hostname: String
    let answers: [String]
    let detail: String

    enum CodingKeys: String, CodingKey {
        case state, resolver, hostname, answers, detail
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        state = PublicOriginResolutionState(try values.decode(String.self, forKey: .state))
        resolver = try values.decode(String.self, forKey: .resolver)
        hostname = try values.decode(String.self, forKey: .hostname)
        answers = try values.decode([String].self, forKey: .answers)
        detail = try values.decode(String.self, forKey: .detail)
    }
}

/// Whether the declared paths are published, and which of them are missing.
///
/// A publication read from the node carries its paths and whether its
/// mechanism is enabled; a publication the web edge serves carries neither,
/// so those fields are absent rather than empty.
struct PublicOriginPublicationReport: Decodable, Sendable {
    let state: PublicOriginPublicationState
    let publishedPaths: [String]?
    let missingPaths: [String]?
    /// Whether the node's publication mechanism is enabled at all. `null`
    /// when the reader could not tell, absent for a web-edge publication.
    let funnelEnabled: Bool?
    let detail: String

    enum CodingKeys: String, CodingKey {
        case state, detail
        case publishedPaths = "published_paths"
        case missingPaths = "missing_paths"
        case funnelEnabled = "funnel_enabled"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        state = PublicOriginPublicationState(try values.decode(String.self, forKey: .state))
        publishedPaths = try values.decodeIfPresent([String].self, forKey: .publishedPaths)
        missingPaths = try values.decodeIfPresent([String].self, forKey: .missingPaths)
        funnelEnabled = try values.decodeIfPresent(Bool.self, forKey: .funnelEnabled)
        detail = try values.decode(String.self, forKey: .detail)
    }
}

/// What the public edge says it selects, and the endpoint that was read to
/// learn it.
struct PublicOriginEdgeSelection: Decodable, Sendable {
    let state: PublicOriginEdgeSelectionState
    /// The origin the edge selects. `null` when the edge declared none or
    /// could not be read.
    let origin: String?
    let endpoint: String
    let detail: String
    let diagnosis: StorageReconciliationJSON
    let observation: StorageReconciliationJSON
    let readback: StorageReconciliationJSON
    let readbackObservation: StorageReconciliationJSON

    enum CodingKeys: String, CodingKey {
        case state, origin, endpoint, detail
        case diagnosis, observation
        case readback
        case readbackObservation = "readback_observation"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        state = PublicOriginEdgeSelectionState(try values.decode(String.self, forKey: .state))
        origin = try values.decodeIfPresent(String.self, forKey: .origin)
        endpoint = try values.decode(String.self, forKey: .endpoint)
        detail = try values.decode(String.self, forKey: .detail)
        diagnosis = try values.decode(StorageReconciliationJSON.self, forKey: .diagnosis)
        observation = try values.decode(StorageReconciliationJSON.self, forKey: .observation)
        readback = try values.decode(StorageReconciliationJSON.self, forKey: .readback)
        readbackObservation = try values.decode(StorageReconciliationJSON.self, forKey: .readbackObservation)
    }
}

/// One row of `stado web origin status --json`.
///
/// The declaration travels with the measurement on purpose: a public release
/// origin is a registry declaration, and a report that named only a verdict
/// would leave an operator guessing which hostname, which host and which
/// paths it was about. Two rows carry no declaration, and say so with
/// `null` rather than with empty strings: the row for an origin the edge
/// selects that nothing declares (`origin-undeclared`), and the row written
/// when the registry itself could not be read (`diagnostic-incomplete`).
struct PublicOriginReport: Decodable, Identifiable, Sendable {
    static let schemaName = "stado.public-origin-report.v1"

    let schema: String
    /// The declaration's name; `null` for a row no declaration names.
    let name: String?
    let hostname: String?
    /// The origin URL a public edge would fetch, as the command composed it.
    let origin: String?
    let target: String?
    let publication: String?
    let upstream: String?
    let paths: [String]?
    let verdict: PublicOriginVerdict
    /// Whether every reading the verdict needs was obtained.
    let complete: Bool
    /// Stado's own sentence about this origin. Shown verbatim, never
    /// paraphrased: the operator and the release gate read the same words.
    let originError: String?
    let resolution: PublicOriginResolution?
    let publicationState: PublicOriginPublicationReport?
    let edgeSelection: PublicOriginEdgeSelection
    let observations: StorageReconciliationJSON
    let registryObservation: StorageReconciliationJSON?

    /// The declaration's name, the selected origin, or the CLI's own word
    /// for a boundary that is neither declared nor reported.
    var id: String { subject }

    enum CodingKeys: String, CodingKey {
        case schema, name, hostname, origin, target, publication, upstream, paths, verdict, complete, resolution
        case originError = "origin_error"
        case publicationState = "publication_state"
        case edgeSelection = "edge_selection"
        case observations
        case registryObservation = "registry_observation"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        schema = try values.decode(String.self, forKey: .schema)
        name = try values.decodeIfPresent(String.self, forKey: .name)
        hostname = try values.decodeIfPresent(String.self, forKey: .hostname)
        origin = try values.decodeIfPresent(String.self, forKey: .origin)
        target = try values.decodeIfPresent(String.self, forKey: .target)
        publication = try values.decodeIfPresent(String.self, forKey: .publication)
        upstream = try values.decodeIfPresent(String.self, forKey: .upstream)
        paths = try values.decodeIfPresent([String].self, forKey: .paths)
        verdict = PublicOriginVerdict(try values.decode(String.self, forKey: .verdict))
        complete = try values.decode(Bool.self, forKey: .complete)
        originError = try values.decodeIfPresent(String.self, forKey: .originError)
        resolution = try values.decodeIfPresent(PublicOriginResolution.self, forKey: .resolution)
        publicationState = try values.decodeIfPresent(PublicOriginPublicationReport.self, forKey: .publicationState)
        edgeSelection = try values.decode(PublicOriginEdgeSelection.self, forKey: .edgeSelection)
        observations = try values.decode(StorageReconciliationJSON.self, forKey: .observations)
        registryObservation = try values.decodeIfPresent(StorageReconciliationJSON.self, forKey: .registryObservation)
    }

    /// What the row is about, as the CLI names it on stderr: the declared
    /// name, else the origin the edge selected, else the boundary itself.
    var subject: String {
        if let name { return name }
        if let origin, !origin.isEmpty { return origin }
        return "the public origin boundary"
    }

    /// The heading of the row: the declaration's name, or the public edge
    /// for a row nothing declares.
    var title: String {
        if let name { return name }
        return "Public edge"
    }

    /// Whether a convergence of this declaration is the operation on offer.
    /// A verdict that names no declaration has nothing for the converge to
    /// read, and a serving origin has nothing to change.
    var convergeableName: String? {
        guard let name else { return nil }
        switch verdict {
        case .serving, .originUndeclared, .diagnosticIncomplete: return nil
        default: return name
        }
    }

    var isConvergeable: Bool { convergeableName != nil }

    /// The declared paths, or that the row carries no declaration, or that
    /// the declaration names none.
    var declaredPathsLabel: String {
        guard let paths else { return "No declaration in this row" }
        return paths.isEmpty ? "No path declared" : paths.joined(separator: ", ")
    }
}
