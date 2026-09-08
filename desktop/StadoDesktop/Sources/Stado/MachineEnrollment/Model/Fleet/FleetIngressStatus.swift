import Foundation

/// `stado fleet ingress status --json` — whether a public entrance for the
/// one-line invitation is standing, and whether it still answers.
///
/// Decoded leniently because the shape belongs to the control plane: a field
/// this release does not know is not a reason to show the operator nothing.
struct FleetIngressStatus: Decodable, Equatable, Sendable {
    let published: Bool
    let baseURL: String
    let mode: String
    let standingSeconds: Int?
    let secondsSinceVerified: Int?
    let listenerPort: Int?
    let reachable: Bool
    let reason: String
    let detail: String
    /// A quick-tunnel entrance: dies with `ingress down`, returns under a
    /// different address after `ingress up`.
    let temporary: Bool
    let listenerAlive: Bool
    let tunnelAlive: Bool

    private enum CodingKeys: String, CodingKey {
        case published, mode, reachable, reason, detail, temporary
        case baseURL = "base_url"
        case standingSeconds = "standing_seconds"
        case secondsSinceVerified = "seconds_since_verified"
        case listenerPort = "listener_port"
        case listenerAlive = "listener_alive"
        case tunnelAlive = "tunnel_alive"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        published = try values.decodeIfPresent(Bool.self, forKey: .published) ?? false
        baseURL = try values.decodeIfPresent(String.self, forKey: .baseURL) ?? ""
        mode = try values.decodeIfPresent(String.self, forKey: .mode) ?? ""
        standingSeconds = try values.decodeIfPresent(Int.self, forKey: .standingSeconds)
        secondsSinceVerified = try values.decodeIfPresent(Int.self, forKey: .secondsSinceVerified)
        listenerPort = try values.decodeIfPresent(Int.self, forKey: .listenerPort)
        reachable = try values.decodeIfPresent(Bool.self, forKey: .reachable) ?? false
        reason = try values.decodeIfPresent(String.self, forKey: .reason) ?? ""
        detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
        temporary = try values.decodeIfPresent(Bool.self, forKey: .temporary) ?? false
        listenerAlive = try values.decodeIfPresent(Bool.self, forKey: .listenerAlive) ?? false
        tunnelAlive = try values.decodeIfPresent(Bool.self, forKey: .tunnelAlive) ?? false
    }
}
