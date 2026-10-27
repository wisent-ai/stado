import Foundation

/// `stado fleet ingress status --json` — whether a public entrance for the
/// one-line invitation is standing, and whether it still answers.
///
/// The CLI writes one of two shapes: `{"published": false, "detail": …}` when
/// no ingress is published, and the full entrance otherwise. Each shape is
/// decoded strictly: a field the CLI did not write is a decoding error that
/// names the key, shown on screen as the read's failure, never a `false` or
/// an empty string the screen would then present as the entrance's state.
enum FleetIngressStatus: Decodable, Equatable, Sendable {
    /// No ingress is published; `detail` is the CLI's sentence saying so and
    /// how to stand one up.
    case absent(detail: String)
    case standing(FleetIngressEntrance)

    var published: Bool {
        switch self {
        case .absent: return false
        case .standing: return true
        }
    }

    /// The entrance, when one is standing.
    var entrance: FleetIngressEntrance? {
        switch self {
        case .absent: return nil
        case .standing(let entrance): return entrance
        }
    }

    private enum CodingKeys: String, CodingKey {
        case published, detail
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        if try values.decode(Bool.self, forKey: .published) {
            self = .standing(try FleetIngressEntrance(from: decoder))
            return
        }
        self = .absent(detail: try values.decode(String.self, forKey: .detail))
    }
}

/// A published entrance, every field as `stado fleet ingress status --json`
/// writes it. `standing_seconds` and `seconds_since_verified` are `null`
/// when the stored timestamps do not parse, and `stamp_errors` says why.
/// `listener_alive` and `tunnel_alive` are `null` when the processes belong
/// to another machine, whose state this one cannot read.
struct FleetIngressEntrance: Decodable, Equatable, Sendable {
    let baseURL: String
    let mode: String
    let standingSeconds: Int?
    let secondsSinceVerified: Int?
    let stampErrors: [String]
    let listenerPort: Int
    let reachable: Bool
    let reason: String
    let detail: String
    /// A quick-tunnel entrance: dies with `ingress down`, returns under a
    /// different address after `ingress up`.
    let temporary: Bool
    let listenerAlive: Bool?
    let tunnelAlive: Bool?

    private enum CodingKeys: String, CodingKey {
        case mode, reachable, reason, detail, temporary
        case baseURL = "base_url"
        case standingSeconds = "standing_seconds"
        case secondsSinceVerified = "seconds_since_verified"
        case stampErrors = "stamp_errors"
        case listenerPort = "listener_port"
        case listenerAlive = "listener_alive"
        case tunnelAlive = "tunnel_alive"
    }
}
