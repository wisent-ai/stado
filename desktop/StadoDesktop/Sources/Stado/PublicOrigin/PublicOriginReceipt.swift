import Foundation

/// One handler the convergence would install or found installed.
///
/// `change` stays the command's own word. The contract fixes the field, not a
/// closed set of values, and a console that mapped an unfamiliar word onto
/// "added" would tell an operator a host was written to when it was not.
struct PublicOriginHandler: Decodable, Identifiable, Equatable, Sendable {
    let path: String
    let upstream: String
    let change: String

    var id: String { "\(path)\u{0}\(upstream)\u{0}\(change)" }

    enum CodingKeys: String, CodingKey {
        case path, upstream, change
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        path = try values.decodeIfPresent(String.self, forKey: .path) ?? ""
        upstream = try values.decodeIfPresent(String.self, forKey: .upstream) ?? ""
        change = try values.decodeIfPresent(String.self, forKey: .change) ?? ""
    }

    /// One reviewable line: the public path, the loopback it would be served
    /// from, and what the convergence would do about it.
    var reviewLine: String {
        "\(path) -> \(upstream) (\(change.isEmpty ? "not reported" : change))"
    }
}

/// The node's publication after the convergence.
struct PublicOriginFunnelState: Decodable, Sendable {
    let enabled: Bool?
    let ports: [Int]

    enum CodingKeys: String, CodingKey {
        case enabled, ports
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        enabled = try values.decodeIfPresent(Bool.self, forKey: .enabled)
        ports = try values.decodeIfPresent([Int].self, forKey: .ports) ?? []
    }

    var summary: String {
        let state: String
        switch enabled {
        case true: state = "enabled"
        case false: state = "not enabled"
        default: state = "not reported"
        }
        guard !ports.isEmpty else { return state }
        return "\(state) on \(ports.map(String.init).joined(separator: ", "))"
    }
}

/// What happened when the convergence read the origin back over the public
/// internet. A convergence that wrote handlers and could not read them back
/// has not published anything an edge can fetch.
struct PublicOriginReadback: Decodable, Sendable {
    let state: String
    /// The HTTP status of the readback, absent when nothing answered.
    let status: Int?
    let detail: String?

    enum CodingKeys: String, CodingKey {
        case state, status, detail
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        state = try values.decodeIfPresent(String.self, forKey: .state) ?? ""
        status = try values.decodeIfPresent(Int.self, forKey: .status)
        detail = try values.decodeIfPresent(String.self, forKey: .detail)
    }

    var summary: String {
        let word = state.isEmpty ? "not reported" : state
        guard let status else { return word }
        return "\(word) · HTTP \(status)"
    }
}

/// The receipt `stado web origin converge` prints, both as a plan and as an
/// applied change.
struct PublicOriginConvergeReceipt: Decodable, Sendable {
    static let schemaName = "stado.public-origin-converge-receipt.v1"

    let schema: String
    let name: String
    let target: String
    let publication: String
    let status: PublicOriginConvergeStatus
    let handlers: [PublicOriginHandler]
    let funnel: PublicOriginFunnelState?
    let resolution: PublicOriginResolution?
    let readback: PublicOriginReadback?
    /// The command's refusal sentence, verbatim. A convergence may write
    /// every handler and still refuse the publication it cannot make public,
    /// so this is not the same fact as `status`.
    let refusal: String?

    enum CodingKeys: String, CodingKey {
        case schema, name, target, publication, status, handlers, funnel, resolution, readback, refusal
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        schema = try values.decodeIfPresent(String.self, forKey: .schema) ?? ""
        name = try values.decode(String.self, forKey: .name)
        target = try values.decodeIfPresent(String.self, forKey: .target) ?? ""
        publication = try values.decodeIfPresent(String.self, forKey: .publication) ?? ""
        status = PublicOriginConvergeStatus(
            try values.decodeIfPresent(String.self, forKey: .status) ?? ""
        )
        handlers = try values.decodeIfPresent([PublicOriginHandler].self, forKey: .handlers) ?? []
        funnel = try values.decodeIfPresent(PublicOriginFunnelState.self, forKey: .funnel)
        resolution = try values.decodeIfPresent(PublicOriginResolution.self, forKey: .resolution)
        readback = try values.decodeIfPresent(PublicOriginReadback.self, forKey: .readback)
        refusal = try values.decodeIfPresent(String.self, forKey: .refusal)
    }
}
