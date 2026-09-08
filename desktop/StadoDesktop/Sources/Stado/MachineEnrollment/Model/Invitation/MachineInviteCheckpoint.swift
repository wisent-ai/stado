import Foundation

/// Whether the control point the one-line invitation depends on actually
/// answered, in the words of the command that asked it.
///
/// The one line is only worth sending if `/join.sh` is really served, so the
/// control plane probes it before assembling one. The three failures are not
/// interchangeable — a name that does not resolve, a refused connection, and a
/// route the release on that host does not serve send the operator to three
/// different places — so the reason travels as its own fact and the sentence
/// behind it is shown exactly as it was written.
struct MachineInviteCheckpoint: Codable, Equatable, Sendable {
    /// The address that was probed, taken from the control plane's own
    /// configuration rather than from any name compiled into this app.
    let url: String
    /// False when nothing was asked: the operator chose the offline
    /// invitation, or no control point address is configured at all.
    let probed: Bool
    let reachable: Bool
    /// The machine-readable reason, exactly as the control plane named it.
    let reason: String
    /// The control plane's own sentence about it, quoted rather than rewritten.
    let detail: String

    static let ok = "ok"
    static let unresolved = "name_does_not_resolve"
    static let refused = "connection_refused"
    static let routeUnknown = "route_unknown"
    static let unconfigured = "not_configured"
    static let chosen = "forced_offline"

    /// Whether the control point failed, as opposed to never having been
    /// asked. A mode the operator chose is not a fault and must not be dressed
    /// as one.
    var isRefusal: Bool { probed && !reachable }

    /// What the reason means, in words. An unrecognised reason is shown as the
    /// control plane spelled it rather than as a guess: a newer release may
    /// name a failure this app has never heard of, and inventing a sentence for
    /// it would be the app lying about the fleet.
    var headline: String {
        switch reason {
        case Self.ok:
            "The control point answered on /join.sh."
        case Self.unresolved:
            "The control point's name does not resolve, so nothing was contacted. That is a fault in the name, not in the machine you are adding."
        case Self.refused:
            "The connection to the control point was refused. The address resolved, so this is about what is listening there and what it is bound to."
        case Self.routeUnknown:
            "The control point answered but does not serve /join.sh. That is a fact about the release running on that host, not about its address."
        case Self.unconfigured:
            "No control point address is configured, so there was nothing to probe."
        case Self.chosen:
            "You asked for the offline invitation, so the control point was not probed."
        default:
            reason
        }
    }

    private enum CodingKeys: String, CodingKey {
        case url, probed, reachable, reason, detail
    }

    /// Read leniently for the same reason the method list is: a control plane
    /// that trimmed one of these fields still answered, and refusing the whole
    /// invitation over a missing sentence would leave the operator with a
    /// minted key and no screen.
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        url = try values.decodeIfPresent(String.self, forKey: .url) ?? ""
        probed = try values.decodeIfPresent(Bool.self, forKey: .probed) ?? false
        reachable = try values.decodeIfPresent(Bool.self, forKey: .reachable) ?? false
        reason = try values.decodeIfPresent(String.self, forKey: .reason) ?? ""
        detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
    }
}
