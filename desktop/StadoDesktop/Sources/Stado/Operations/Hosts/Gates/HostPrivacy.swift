import Foundation

/// What macOS lets a host's Stado process read, as `stado host privacy HOST
/// --json` reports it from the host's latest beacon, beside the grants the
/// registry declares for the host (`targets[].privacy_grants`). The host
/// process measures this itself, because macOS decides per program and a
/// denial once clicked stays until the operator changes it in System Settings.
struct HostPrivacy: Decodable, Sendable {
    struct Folder: Decodable, Sendable {
        let state: String?
        let path: String?
        let detail: String?
    }

    struct Measurement: Decodable, Sendable {
        let program: String?
        let folders: [String: Folder]
    }

    /// One declared grant and the state measured for it; `not measured` for
    /// a program other than the Stado process that published the beacon.
    struct Grant: Decodable, Sendable, Identifiable {
        let program: String
        let folder: String
        let reason: String
        let path: String
        let state: String
        var id: String { "\(program)|\(folder)" }
        var refused: Bool { state == "denied" || state == "unreadable" }
    }

    let host: String
    let reportedAt: String?
    let privacy: Measurement
    let grants: [Grant]
    let settingsURL: String

    enum CodingKeys: String, CodingKey {
        case host, privacy, grants
        case reportedAt = "reported_at"
        case settingsURL = "settings_url"
    }

    struct FolderName: Identifiable, Sendable {
        let key: String
        let name: String
        var id: String { key }
    }

    /// The folders in the order the command prints them, with their names.
    static let folders: [FolderName] = [
        FolderName(key: "documents", name: "Documents"),
        FolderName(key: "desktop", name: "Desktop"),
        FolderName(key: "downloads", name: "Downloads"),
    ]

    /// The declared grants macOS does not honour.
    var refusedGrants: [Grant] {
        grants.filter(\.refused)
    }

    static func name(of folder: String) -> String {
        folders.first { $0.key == folder }?.name ?? folder
    }
}
