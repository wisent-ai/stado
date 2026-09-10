import Foundation

/// One entry of the fleet's declared memory policy catalog, as
/// `GET /api/memory-policies.json` publishes it.
///
/// The catalog is `stado-rs/data/memory/policies.json`, compiled into the
/// binary that serves this route, and it is the same list `stado space
/// policies` prints. This console does not invent a policy: it offers the
/// declared ones and posts the document the declaration carries, so a host
/// armed from Desktop and a host armed from the command line carry the same
/// bytes and read back as the same reviewed policy.
struct DeclaredMemoryPolicy: Decodable, Identifiable, Sendable {
    let name: String
    let summary: String
    let platforms: [String]
    let roles: [String]
    /// Whether this policy ends processes of a logged-in graphical session.
    /// Applying one that does needs the operator's explicit authorization,
    /// exactly as `--authorize-graphical-session` does on the CLI.
    let endsGraphicalSession: Bool
    let sessionProcesses: [String]
    /// The declaration itself, in the same shape the registry projection uses
    /// for what a host already carries.
    let policy: FleetMemoryPolicy

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name
        case summary
        case platforms
        case roles
        case endsGraphicalSession = "ends_graphical_session"
        case sessionProcesses = "session_processes"
        case policy
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        name = try values.decode(String.self, forKey: .name)
        summary = try values.decodeIfPresent(String.self, forKey: .summary) ?? ""
        platforms = try values.decodeIfPresent([String].self, forKey: .platforms) ?? []
        roles = try values.decodeIfPresent([String].self, forKey: .roles) ?? []
        endsGraphicalSession =
            try values.decodeIfPresent(Bool.self, forKey: .endsGraphicalSession) ?? false
        sessionProcesses = try values.decodeIfPresent([String].self, forKey: .sessionProcesses) ?? []
        policy = try values.decode(FleetMemoryPolicy.self, forKey: .policy)
    }

    /// The repair names this policy permits.
    var repairNames: [String] {
        policy.repairs.keys.sorted()
    }

    /// One line an operator can pick a policy by: what it does, not its
    /// fields, which the review dialog shows in full before anything is
    /// written.
    var headline: String {
        let repairs = repairNames.isEmpty ? "no repair" : repairNames.joined(separator: ", ")
        return "\(policy.mode ?? "unknown") · \(repairs)"
    }
}

/// The catalog document.
struct DeclaredMemoryPolicyCatalog: Decodable, Sendable {
    let declaration: String
    let policies: [DeclaredMemoryPolicy]
}
