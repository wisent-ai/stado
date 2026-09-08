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
struct MachineInvite: Decodable, Equatable, Sendable {
    let id: String
    let mode: MachineInviteMode
    /// The online invitation's secret. Empty for the offline one, which has
    /// none: there is no route for anything to present it to.
    let token: String
    let targetName: String
    let expiresAt: String
    let usesAllowed: Int
    /// The one line to send to the owner of the machine, assembled by the
    /// control plane against its own configured address. The app does not
    /// build it: a line assembled here would carry this Mac's idea of the
    /// endpoint. Empty in the offline mode, where no such line exists.
    let joinCommand: String
    /// The offline fragment to paste on the machine being added. It creates
    /// ~/.ssh, appends the fleet's public key idempotently, fixes the modes,
    /// checks that SSH is listening, and prints the address its owner has to
    /// send back. Empty in the online mode.
    let snippet: String
    /// The command that finishes an offline enrollment once that address
    /// arrives, spelled by the control plane.
    let nextStep: String
    let publicKey: String
    let authorizedKeysLine: String
    /// What the control plane found when it asked whether the one line would
    /// work. Present in both modes: it is the reason this is the mode it is.
    let checkpoint: MachineInviteCheckpoint?
    /// Where the one line's address came from: `enrollment.url`, `ingress`,
    /// or `api.url`. The distinction the operator needs is whether the line
    /// outlives the process that published it, and only the control plane
    /// knows which source answered.
    let baseSource: String
    /// True when the address is a published quick-tunnel entrance: it dies
    /// with `stado fleet ingress down` and a restarted ingress answers under
    /// a different name.
    let baseIsTemporary: Bool
    /// The control plane's own sentence about that temporariness, shown
    /// verbatim — a paraphrase here would drift from what the CLI says.
    let baseWarning: String

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
        token = try values.decodeIfPresent(String.self, forKey: .token) ?? ""
        targetName = try values.decodeIfPresent(String.self, forKey: .targetName) ?? ""
        expiresAt = try values.decodeIfPresent(String.self, forKey: .expiresAt) ?? ""
        usesAllowed = try values.decodeIfPresent(Int.self, forKey: .usesAllowed) ?? 1
        joinCommand = try values.decodeIfPresent(String.self, forKey: .joinCommand) ?? ""
        snippet = try values.decodeIfPresent(String.self, forKey: .snippet) ?? ""
        nextStep = try values.decodeIfPresent(String.self, forKey: .nextStep) ?? ""
        publicKey = try values.decodeIfPresent(String.self, forKey: .publicKey) ?? ""
        authorizedKeysLine = try values.decodeIfPresent(String.self, forKey: .authorizedKeysLine) ?? ""
        checkpoint = try values.decodeIfPresent(MachineInviteCheckpoint.self, forKey: .checkpoint)
        baseSource = try values.decodeIfPresent(String.self, forKey: .baseSource) ?? ""
        baseIsTemporary = try values.decodeIfPresent(Bool.self, forKey: .baseIsTemporary) ?? false
        baseWarning = try values.decodeIfPresent(String.self, forKey: .baseWarning) ?? ""
        // A release older than the offline mode names no mode at all. What it
        // sent decides which one it meant: a fragment is an offline
        // invitation whatever the release calls itself.
        let declared = try values.decodeIfPresent(String.self, forKey: .mode)
        mode = declared.flatMap(MachineInviteMode.init(rawValue:)) ?? (snippet.isEmpty ? .online : .offline)
    }

    /// Everything about the invitation that outlives the window.
    var record: MachineInviteRecord {
        MachineInviteRecord(
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
