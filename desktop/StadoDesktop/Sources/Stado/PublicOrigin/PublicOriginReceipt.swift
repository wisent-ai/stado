import Foundation

/// One handler the convergence would install or found installed.
///
/// `change` stays the command's own word. The contract fixes the field, not a
/// closed set of values, and a console that mapped an unfamiliar word onto
/// "added" would tell an operator a host was written to when it was not.
/// Every field is written for every handler, so each is decoded as required.
struct PublicOriginHandler: Decodable, Identifiable, Equatable, Sendable {
    let path: String
    let upstream: String
    let change: String

    var id: String { "\(path)\u{0}\(upstream)\u{0}\(change)" }

    /// One reviewable line: the public path, the loopback it would be served
    /// from, and what the convergence would do about it.
    var reviewLine: String {
        "\(path) -> \(upstream) (\(change))"
    }
}

/// The node's publication after the convergence, as the receipt writes it:
/// whether the mechanism is enabled, the port it serves, and the declared
/// paths it publishes, misses and serves without a declaration.
struct PublicOriginFunnelState: Decodable, Sendable {
    let enabled: Bool
    let port: Int
    let publishedPaths: [String]
    let missingPaths: [String]
    let undeclaredPaths: [String]

    enum CodingKeys: String, CodingKey {
        case enabled, port
        case publishedPaths = "published_paths"
        case missingPaths = "missing_paths"
        case undeclaredPaths = "undeclared_paths"
    }

    var summary: String {
        let state = enabled ? "enabled" : "not enabled"
        let published = publishedPaths.isEmpty
            ? "no declared path published"
            : "publishing \(publishedPaths.joined(separator: ", "))"
        let missing = missingPaths.isEmpty
            ? ""
            : "; missing \(missingPaths.joined(separator: ", "))"
        return "\(state) on port \(port), \(published)\(missing)"
    }
}

/// The web product whose edge serves the publication, when the publication
/// is a web declaration rather than the node's own.
struct PublicOriginWebDeclaration: Decodable, Sendable {
    let product: String
    let edge: String
    let upstreamService: String

    enum CodingKeys: String, CodingKey {
        case product, edge
        case upstreamService = "upstream_service"
    }

    var summary: String {
        "served by the web product \(product) through its \(edge) edge, upstream \(upstreamService)"
    }
}

/// What happened when the convergence read the origin back over the public
/// internet. A convergence that wrote handlers and could not read them back
/// has not published anything an edge can fetch. `state` and `detail` are
/// always written; `status` is `null` when nothing answered.
struct PublicOriginReadback: Decodable, Sendable {
    let state: String
    /// The HTTP status of the readback, `null` when nothing answered.
    let status: Int?
    let detail: String

    var summary: String {
        guard let status else { return state }
        return "\(state) · HTTP \(status)"
    }
}

/// The receipt `stado web origin converge` prints, both as a plan and as an
/// applied change. A node publication carries `handlers` and `funnel`; a
/// publication a web product's edge serves carries `web_declaration` and no
/// handlers, since the convergence writes none. A receipt that lacks a field
/// the command always writes is a decoding error naming the key.
struct PublicOriginConvergeReceipt: Decodable, Sendable {
    static let schemaName = "stado.public-origin-converge-receipt.v1"

    let schema: String
    let name: String
    let target: String
    let publication: String
    let origin: String
    let applied: Bool
    let status: PublicOriginConvergeStatus
    let handlers: [PublicOriginHandler]?
    let funnel: PublicOriginFunnelState?
    let webDeclaration: PublicOriginWebDeclaration?
    let resolution: PublicOriginResolution
    let readback: PublicOriginReadback
    /// The command's refusal sentence, verbatim, or `null` when it refused
    /// nothing. A convergence may write every handler and still refuse the
    /// publication it cannot make public, so this is not the same fact as
    /// `status`.
    let refusal: String?

    enum CodingKeys: String, CodingKey {
        case schema, name, target, publication, origin, applied, status, handlers, funnel, resolution, readback, refusal
        case webDeclaration = "web_declaration"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        schema = try values.decode(String.self, forKey: .schema)
        name = try values.decode(String.self, forKey: .name)
        target = try values.decode(String.self, forKey: .target)
        publication = try values.decode(String.self, forKey: .publication)
        origin = try values.decode(String.self, forKey: .origin)
        applied = try values.decode(Bool.self, forKey: .applied)
        status = PublicOriginConvergeStatus(try values.decode(String.self, forKey: .status))
        handlers = try values.decodeIfPresent([PublicOriginHandler].self, forKey: .handlers)
        funnel = try values.decodeIfPresent(PublicOriginFunnelState.self, forKey: .funnel)
        webDeclaration = try values.decodeIfPresent(PublicOriginWebDeclaration.self, forKey: .webDeclaration)
        resolution = try values.decode(PublicOriginResolution.self, forKey: .resolution)
        readback = try values.decode(PublicOriginReadback.self, forKey: .readback)
        refusal = try values.decodeIfPresent(String.self, forKey: .refusal)
    }

    /// The handler rows an operator reviews: the receipt's own, or the one
    /// sentence saying why there are none.
    var handlerLines: [String] {
        if let webDeclaration {
            return ["No handler is written: the publication is \(webDeclaration.summary)."]
        }
        guard let handlers else {
            return ["The receipt carries neither handlers nor a web declaration."]
        }
        return handlers.isEmpty ? ["The plan names no handler change."] : handlers.map(\.reviewLine)
    }
}
