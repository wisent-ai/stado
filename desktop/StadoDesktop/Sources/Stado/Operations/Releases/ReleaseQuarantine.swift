import Foundation

/// `stado release quarantine list <product> --target <host> --json`.
struct ReleaseQuarantineReport: Decodable, Sendable {
    let product: String
    let target: String
    let entries: [ReleaseQuarantineEntry]

    enum CodingKeys: String, CodingKey {
        case product, target, entries
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        product = try values.decodeIfPresent(String.self, forKey: .product) ?? ""
        target = try values.decodeIfPresent(String.self, forKey: .target) ?? ""
        entries = try values.decodeIfPresent([ReleaseQuarantineEntry].self, forKey: .entries) ?? []
    }
}

/// One digest the host refuses to roll out again.
///
/// `isDesiredDigest` is the field the section exists for: a quarantined digest
/// nobody desires is history, and the one that matches desired state is the
/// rollout being skipped on every pass until a human clears it.
/// `release doctor` omits the flag; `quarantine list` sets it.
struct ReleaseQuarantineEntry: Decodable, Identifiable, Sendable {
    let digest: String
    let reason: String
    let quarantinedAt: String?
    let isDesiredDigest: Bool

    var id: String { digest }

    var quarantinedAge: Double? {
        guard let quarantined = StadoFormat.date(quarantinedAt) else { return nil }
        return Date().timeIntervalSince(quarantined)
    }

    /// Twelve characters is what an operator compares against a build log; the
    /// full digest stays available in the row's own field.
    var shortDigest: String {
        let bare = digest.hasPrefix("sha256:") ? String(digest.dropFirst(7)) : digest
        return bare.count > 12 ? String(bare.prefix(12)) : bare
    }

    enum CodingKeys: String, CodingKey {
        case digest, reason
        case quarantinedAt = "quarantined_at"
        case isDesiredDigest = "is_desired_digest"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        digest = try values.decodeIfPresent(String.self, forKey: .digest) ?? ""
        reason = try values.decodeIfPresent(String.self, forKey: .reason) ?? ""
        quarantinedAt = try values.decodeIfPresent(String.self, forKey: .quarantinedAt)
        isDesiredDigest = try values.decodeIfPresent(Bool.self, forKey: .isDesiredDigest) ?? false
    }
}

/// The order a held-digest list is read in.
extension Array where Element == ReleaseQuarantineEntry {
    /// The digest the registry desires first, then the rest newest first.
    ///
    /// The host answers in digest order, which is the one order that says
    /// nothing: on control-host it put the digest actually blocking the
    /// brama rollout seventh of seven, below six refusals that are history.
    /// Recency orders the rest, because a refusal recorded an hour ago is
    /// about the release being attempted now.
    var desiredFirst: [ReleaseQuarantineEntry] {
        map { (entry: $0, quarantined: StadoFormat.date($0.quarantinedAt)) }
            .sorted { lhs, rhs in
                if lhs.entry.isDesiredDigest != rhs.entry.isDesiredDigest {
                    return lhs.entry.isDesiredDigest
                }
                switch (lhs.quarantined, rhs.quarantined) {
                case let (left?, right?) where left != right:
                    return left > right
                case (nil, .some):
                    return false
                case (.some, nil):
                    return true
                default:
                    return lhs.entry.digest < rhs.entry.digest
                }
            }
            .map(\.entry)
    }
}

/// `stado release quarantine clear … --reason <text> --json`. The audit record
/// the CLI wrote, read back so the screen states what was recorded rather than
/// what was requested.
struct ReleaseQuarantineClearance: Decodable, Sendable {
    let product: String
    let target: String
    let digest: String
    let cleared: Bool
    let reason: String
    let auditedAt: String?
    let stateBackup: String?

    enum CodingKeys: String, CodingKey {
        case product, target, digest, cleared, reason
        case auditedAt = "audited_at"
        case stateBackup = "state_backup"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        product = try values.decodeIfPresent(String.self, forKey: .product) ?? ""
        target = try values.decodeIfPresent(String.self, forKey: .target) ?? ""
        digest = try values.decodeIfPresent(String.self, forKey: .digest) ?? ""
        cleared = try values.decodeIfPresent(Bool.self, forKey: .cleared) ?? false
        reason = try values.decodeIfPresent(String.self, forKey: .reason) ?? ""
        auditedAt = try values.decodeIfPresent(String.self, forKey: .auditedAt)
        stateBackup = try values.decodeIfPresent(String.self, forKey: .stateBackup)
    }
}
