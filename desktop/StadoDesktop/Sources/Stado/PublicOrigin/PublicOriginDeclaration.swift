import Foundation

/// One `public_origins` row as `GET /api/registry.json` projects it.
///
/// A public origin is a registry declaration, not a value a client derives:
/// `/docs/channels` states that release clients do not choose a network
/// provider or derive that origin from a host's control route. This console
/// therefore reads the declaration and never composes one from a hostname it
/// happens to know about.
struct FleetPublicOrigin: Decodable, Identifiable, Sendable {
    let name: String
    let hostname: String
    let target: String
    let publication: String
    let upstream: String
    let paths: [String]

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name, hostname, target, publication, upstream, paths
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        name = try values.decode(String.self, forKey: .name)
        hostname = try values.decodeIfPresent(String.self, forKey: .hostname) ?? ""
        target = try values.decodeIfPresent(String.self, forKey: .target) ?? ""
        publication = try values.decodeIfPresent(String.self, forKey: .publication) ?? ""
        upstream = try values.decodeIfPresent(String.self, forKey: .upstream) ?? ""
        paths = try values.decodeIfPresent([String].self, forKey: .paths) ?? []
    }

    /// The origin URL this declaration implies. Composed here only to say
    /// what the declaration means; the report's own `origin` is what the
    /// screen shows for a measured row.
    var originURL: String { "https://\(hostname)" }
}
