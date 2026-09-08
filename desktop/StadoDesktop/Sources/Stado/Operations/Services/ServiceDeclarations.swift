import Foundation

/// `stado service deploy NAME --host HOST --json` — the declaration-driven
/// write. The action word is the verdict; the registry generation proves the
/// declaration that was installed, and the service object is the row the
/// screen re-reads afterwards.
struct ServiceDeployReport: Decodable, Sendable {
    let action: String
    let service: FleetServiceEntry
    let registryGeneration: String

    enum CodingKeys: String, CodingKey {
        case action, service
        case registryGeneration = "registry_generation"
    }

    var succeeded: Bool { action == "deployed" }
}

/// One preconfigured Wisent service, exactly as `stado service catalog
/// --json` reports it. The app keeps no copy of the list: a build that gains
/// a service has to reach this screen without shipping a new app.
struct WisentCatalogEntry: Decodable, Identifiable, Sendable {
    let name: String
    let summary: String
    let program: String
    let args: [String]

    var id: String { name }
}

/// `stado service remove <name> --host <host> --json`, one document: the
/// whole "remove this service" outcome — registry entry dropped at
/// `generation`, and the unit file's fate under `file`. The file half has
/// its own status because a stopped-and-forgotten service whose file the
/// channel may not delete is a real, nameable end state, not an error to
/// hide: `removed`, `absent`, `refused` or `failed`, with the refusal's
/// reason verbatim.
struct ServiceRemoveReport: Decodable, Sendable {
    let target: String
    let unit: String
    let generation: String
    let file: FileOutcome

    struct FileOutcome: Decodable, Sendable {
        let path: String
        let status: String
        let detail: String?
    }

    var succeeded: Bool { file.status == "removed" || file.status == "absent" }

    var fileSentence: String {
        file.detail.map { "\(file.status) — \($0)" } ?? file.status
    }
}

extension FleetServiceEntry {
    /// True only where `stado space file remove`'s host-side guards could
    /// pass: a path inside a user's own `Library/LaunchAgents` (never the
    /// system `/Library/LaunchDaemons`) or under `.stado`. Offering the verb
    /// anywhere else would be a button that can only be refused.
    var removableByRemoveFile: Bool {
        guard !path.isEmpty, !path.contains("..") else { return false }
        if path.hasPrefix("/Library/") { return false }
        return path.contains("/Library/LaunchAgents/") || path.contains("/.stado/")
    }
}
