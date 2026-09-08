import Foundation

/// A machine that has put its hand up and is waiting for an operator.
struct FleetPendingRequest: Codable, Identifiable, Equatable, Sendable {
    let hostname: String
    let os: String
    let arch: String
    let kind: String
    let status: String
    let requestedAt: String?
    /// The SSH address the machine reported for itself. Present when the
    /// request came from an invitation, absent for a machine that ran `join`
    /// with credentials of its own.
    let destination: String?
    let inviteID: String?
    let installedKeyFingerprint: String?
    /// The name the registry row will take. It comes from the invitation, not
    /// from the machine, which is why an invited machine can report itself as
    /// `studio-air` and still be enrolled as `studio`. Null for a plain join,
    /// where the hostname is the name.
    let targetName: String?

    var id: String { hostname }

    var requestedDate: Date? { EnrollmentTime.date(from: requestedAt) }

    /// Whether approval can do the probing enrollment by itself. Without an
    /// address there is nothing for the fleet to connect back to, and the
    /// operator has to supply one.
    var isReachable: Bool {
        guard let destination else { return false }
        return !destination.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var platform: String {
        let parts = [os, arch].filter { !$0.isEmpty }
        return parts.isEmpty ? "not reported" : parts.joined(separator: " ")
    }

    /// What the entry will be called, which is what the operator has to
    /// recognise afterwards in the Hosts table and in every stado command.
    var registryName: String {
        guard let targetName, !targetName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { return hostname }
        return targetName
    }

    private enum CodingKeys: String, CodingKey {
        case hostname, os, arch, kind, status
        case requestedAt = "requested_at"
        case destination
        case inviteID = "invite_id"
        case installedKeyFingerprint = "installed_key_fingerprint"
        case targetName = "target_name"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        hostname = try values.decode(String.self, forKey: .hostname)
        os = try values.decodeIfPresent(String.self, forKey: .os) ?? ""
        arch = try values.decodeIfPresent(String.self, forKey: .arch) ?? ""
        kind = try values.decodeIfPresent(String.self, forKey: .kind) ?? "join"
        status = try values.decodeIfPresent(String.self, forKey: .status) ?? "pending"
        requestedAt = try values.decodeIfPresent(String.self, forKey: .requestedAt)
        destination = try values.decodeIfPresent(String.self, forKey: .destination)
        inviteID = try values.decodeIfPresent(String.self, forKey: .inviteID)
        installedKeyFingerprint = try values.decodeIfPresent(String.self, forKey: .installedKeyFingerprint)
        targetName = try values.decodeIfPresent(String.self, forKey: .targetName)
    }
}

/// `{"pending": [...]}` — the envelope `fleet pending --json` prints.
struct FleetPendingList: Decodable, Sendable {
    let pending: [FleetPendingRequest]
}
