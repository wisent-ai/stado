import Foundation

/// A freshly minted invitation, whatever it carries.
///
/// This value exists for exactly as long as the screen that shows it. The
/// online invitation's token and the one line built around it are never
/// written to disk: an invitation code that can be read back a second time is
/// a password on the filesystem, and the whole point of showing it once is that
/// it is not one. The offline fragment is the opposite and says so about
/// itself — it carries the public half of a key and nothing else — so it is
/// kept, because the operator has to be able to send it again.
///
/// `stado fleet invite create --json` writes the common fields in both modes and,
/// by mode, either the token with its one line or the fragment with its
/// closing command. Each is decoded strictly: a document that lacks what its
/// mode promises is reported by key, never shown as an invitation with an
/// empty line or an empty fragment.
struct MachineInvite: Decodable, Equatable, Sendable {
    /// What the operator sends, by mode.
    enum Delivery: Equatable, Sendable {
        /// The secret and the one line built around it by the control plane
        /// against its own configured address. The app does not build the
        /// line: a line assembled here would carry this Mac's idea of the
        /// endpoint.
        case online(token: String, joinCommand: String)
        /// The fragment to paste on the machine being added — it creates
        /// ~/.ssh, appends the fleet's public key idempotently, fixes the
        /// modes, checks that SSH is listening, and prints the address its
        /// owner has to send back — and the command that finishes the
        /// enrollment once that address arrives, spelled by the control plane.
        case offline(snippet: String, nextStep: String)
    }

    let id: String
    let mode: MachineInviteMode
    let targetName: String
    let expiresAt: String
    let usesAllowed: Int
    let delivery: Delivery
    let publicKey: String
    let authorizedKeysLine: String
    /// What the control plane found when it asked whether the one line would
    /// work. Written in both modes: it is the reason this is the mode it is.
    let checkpoint: MachineInviteCheckpoint
    /// Where the one line's address came from: `enrollment.url`, `ingress`,
    /// or `api.url`. The distinction the operator needs is whether the line
    /// outlives the process that published it, and only the control plane
    /// knows which source answered.
    let baseSource: String
    /// True when the address is a published quick-tunnel entrance: it dies
    /// with `stado fleet ingress down` and a restarted ingress answers under
    /// a different name.
    let baseIsTemporary: Bool
    /// The control plane's own sentence about that temporariness, written
    /// only for a temporary address and shown verbatim — a paraphrase here
    /// would drift from what the CLI says.
    let baseWarning: String?

    private enum CodingKeys: String, CodingKey {
        case id, mode, token, snippet, checkpoint
        case targetName = "target_name"
        case expiresAt = "expires_at"
        case usesAllowed = "uses_allowed"
        case joinCommand = "join_command"
        case nextStep = "next_step"
        case publicKey = "public_key"
        case authorizedKeysLine = "authorized_keys_line"
        case baseSource = "base_source"
        case baseIsTemporary = "base_is_temporary"
        case baseWarning = "base_warning"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decode(String.self, forKey: .id)
        mode = try values.decode(MachineInviteMode.self, forKey: .mode)
        targetName = try values.decode(String.self, forKey: .targetName)
        expiresAt = try values.decode(String.self, forKey: .expiresAt)
        usesAllowed = try values.decode(Int.self, forKey: .usesAllowed)
        publicKey = try values.decode(String.self, forKey: .publicKey)
        authorizedKeysLine = try values.decode(String.self, forKey: .authorizedKeysLine)
        checkpoint = try values.decode(MachineInviteCheckpoint.self, forKey: .checkpoint)
        baseSource = try values.decode(String.self, forKey: .baseSource)
        baseIsTemporary = try values.decode(Bool.self, forKey: .baseIsTemporary)
        baseWarning = try values.decodeIfPresent(String.self, forKey: .baseWarning)
        switch mode {
        case .online:
            delivery = .online(
                token: try values.decode(String.self, forKey: .token),
                joinCommand: try values.decode(String.self, forKey: .joinCommand)
            )
        case .offline:
            delivery = .offline(
                snippet: try values.decode(String.self, forKey: .snippet),
                nextStep: try values.decode(String.self, forKey: .nextStep)
            )
        }
    }

    /// Everything about the invitation that outlives the window: the
    /// fragment is kept, the token and its line are not.
    var record: MachineInviteRecord {
        let snippet: String?
        switch delivery {
        case .online: snippet = nil
        case .offline(let fragment, _): snippet = fragment
        }
        return MachineInviteRecord(
            id: id,
            mode: mode,
            targetName: targetName,
            mintedAt: Date(),
            expiresAt: expiresAt,
            usesAllowed: usesAllowed,
            publicKey: publicKey,
            authorizedKeysLine: authorizedKeysLine,
            snippet: snippet,
            checkpoint: checkpoint,
            baseSource: baseSource,
            baseIsTemporary: baseIsTemporary,
            baseWarning: baseWarning
        )
    }
}
