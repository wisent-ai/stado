import Foundation

/// What is kept about an invitation once its code has been shown.
///
/// The waiting half of an invitation lasts as long as it takes the other
/// person to read a message, so this outlives the window. The online
/// invitation's secret does not: a public key is not one, an identifier is not
/// one, and those are what the operator needs in order to recognise the reply
/// when it lands.
struct MachineInviteRecord: Codable, Equatable, Sendable {
    let id: String
    /// Which invitation this was. The offline one has no code and is never
    /// answered by the machine, so the whole screen reads differently.
    let mode: MachineInviteMode
    let targetName: String
    let mintedAt: Date
    let expiresAt: String
    let usesAllowed: Int
    let publicKey: String
    let authorizedKeysLine: String
    /// The offline fragment, kept so it can be sent again. Nothing in it is a
    /// secret — it is the public half of a key and four lines of shell — and
    /// the alternative to keeping it is reminting, which mints a second key
    /// pair for a machine that already has one.
    let snippet: String
    /// Why this invitation is the mode it is. Kept because it is the operator's
    /// next question after a restart, and because a mode the control plane
    /// chose has to stay distinguishable from one the operator chose.
    let checkpoint: MachineInviteCheckpoint?
    /// Where the one line's address came from, and the control plane's own
    /// sentence about it when that address dies with the ingress. Kept because
    /// the line was sent in a message that outlives this window, and the
    /// operator returning here after a restart still has to know that tearing
    /// the ingress down kills the line they already sent.
    let baseSource: String
    let baseIsTemporary: Bool
    let baseWarning: String

    var isOffline: Bool { mode == .offline }

    var expiryDate: Date? { EnrollmentTime.date(from: expiresAt) }

    var isExpired: Bool {
        guard let expiryDate else { return false }
        return expiryDate <= Date()
    }

    private enum CodingKeys: String, CodingKey {
        case id, mode, targetName, mintedAt, expiresAt, usesAllowed
        case publicKey, authorizedKeysLine, snippet, checkpoint
        case baseSource, baseIsTemporary, baseWarning
    }
}

/// Read leniently, because the reader is this app's own earlier state.
///
/// A record written before the offline mode existed names no mode, carries no
/// fragment and no checkpoint. Refusing it would take an invitation that is
/// still waiting to be answered off the screen at the moment the app is
/// updated, which is exactly when the operator is least likely to believe the
/// fleet rather than the window.
extension MachineInviteRecord {
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            id: try values.decode(String.self, forKey: .id),
            mode: try values.decodeIfPresent(MachineInviteMode.self, forKey: .mode) ?? .online,
            targetName: try values.decodeIfPresent(String.self, forKey: .targetName) ?? "",
            mintedAt: try values.decodeIfPresent(Date.self, forKey: .mintedAt) ?? Date(),
            expiresAt: try values.decodeIfPresent(String.self, forKey: .expiresAt) ?? "",
            usesAllowed: try values.decodeIfPresent(Int.self, forKey: .usesAllowed) ?? 1,
            publicKey: try values.decodeIfPresent(String.self, forKey: .publicKey) ?? "",
            authorizedKeysLine: try values.decodeIfPresent(String.self, forKey: .authorizedKeysLine) ?? "",
            snippet: try values.decodeIfPresent(String.self, forKey: .snippet) ?? "",
            checkpoint: try values.decodeIfPresent(MachineInviteCheckpoint.self, forKey: .checkpoint),
            baseSource: try values.decodeIfPresent(String.self, forKey: .baseSource) ?? "",
            baseIsTemporary: try values.decodeIfPresent(Bool.self, forKey: .baseIsTemporary) ?? false,
            baseWarning: try values.decodeIfPresent(String.self, forKey: .baseWarning) ?? ""
        )
    }
}
