import Foundation

/// What is kept about an invitation once its code has been shown.
///
/// The waiting half of an invitation lasts as long as it takes the other
/// person to read a message, so this outlives the window. The online
/// invitation's secret does not: a public key is not one, an identifier is not
/// one, and those are what the operator needs in order to recognise the reply
/// when it lands.
///
/// The record is this app's own saved state and is read back as it was
/// written. One that cannot be read is reported through the store's saved
/// state failure, with the decoder's reason, rather than quietly replaced by
/// an invitation that was never minted.
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
    /// pair for a machine that already has one. `nil` for the online
    /// invitation, which has none.
    let snippet: String?
    /// Why this invitation is the mode it is. Kept because it is the operator's
    /// next question after a restart, and because a mode the control plane
    /// chose has to stay distinguishable from one the operator chose.
    let checkpoint: MachineInviteCheckpoint
    /// Where the one line's address came from, and the control plane's own
    /// sentence about it when that address dies with the ingress. Kept because
    /// the line was sent in a message that outlives this window, and the
    /// operator returning here after a restart still has to know that tearing
    /// the ingress down kills the line they already sent.
    let baseSource: String
    let baseIsTemporary: Bool
    /// The control plane's sentence about a temporary address; `nil` when the
    /// address is not temporary.
    let baseWarning: String?

    var isOffline: Bool { mode == .offline }

    var expiryDate: Date? { EnrollmentTime.date(from: expiresAt) }

    var isExpired: Bool {
        guard let expiryDate else { return false }
        return expiryDate <= Date()
    }

    /// What to say about a line standing on a temporary ingress: the control
    /// plane's own sentence, or, for a temporary address it wrote none for,
    /// this app's plain statement that says the sentence is missing.
    var temporaryBaseSentence: String {
        if let baseWarning {
            return baseWarning
        }
        return "The line you sent stands on a temporary ingress. Tearing that ingress down, or this machine restarting it, kills the line before its expiry does. (The control plane wrote no sentence of its own about this address.)"
    }
}
