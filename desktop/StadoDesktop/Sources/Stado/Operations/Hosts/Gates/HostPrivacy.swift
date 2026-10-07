import Foundation

/// What macOS lets a host's Stado process read, as `stado host privacy HOST
/// --json` reports it from the host's latest beacon. The host process
/// measures this itself, because macOS decides per program and a denial once
/// clicked stays until the operator changes it in System Settings.
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

    let host: String
    let reportedAt: String?
    let privacy: Measurement
    let settingsURL: String

    enum CodingKeys: String, CodingKey {
        case host, privacy
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

    var denied: [String] {
        Self.folders.filter { privacy.folders[$0.key]?.state == "denied" }.map(\.name)
    }
}
