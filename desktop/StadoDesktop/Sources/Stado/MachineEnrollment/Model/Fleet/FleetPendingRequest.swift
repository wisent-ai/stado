import Foundation

/// A machine that has put its hand up and is waiting for an operator.
struct FleetPendingRequest: Codable, Identifiable, Equatable, Sendable {
    let hostname: String
    /// The machine's own facts, `null` when its request did not record them.
    let os: String?
    let arch: String?
    let kind: String?
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

    /// The address `stado fleet approve` probes: the one the machine
    /// reported, or its hostname when it reported none.
    var probedAddress: String {
        if isReachable, let destination {
            return destination
        }
        return hostname
    }

    /// The platform the machine reported, or which of os and arch it did not.
    var platform: String {
        switch (os, arch) {
        case (let os?, let arch?): return "\(os) \(arch)"
        case (let os?, nil): return "\(os) (arch not reported)"
        case (nil, let arch?): return "\(arch) (os not reported)"
        case (nil, nil): return "not reported"
        }
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

    /// Every field is decoded as `stado fleet pending --json` writes it: the
    /// machine's facts are `null` when its request did not record them, the
    /// status and hostname are always written. A document that lacks a field
    /// is reported by key through the store's document reader.
}

/// `{"pending": [...]}` — the envelope `fleet pending --json` prints.
struct FleetPendingList: Decodable, Sendable {
    let pending: [FleetPendingRequest]
}
