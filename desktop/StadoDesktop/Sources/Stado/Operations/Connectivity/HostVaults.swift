import Foundation

/// One vault a host holds: an owner, two counts and a path, which is all
/// `stado credentials vault list --host` transports. Item names never cross the wire.
struct HostVault: Decodable, Identifiable, Sendable {
    let path: String
    let owner: String?
    let items: Int?
    let recipients: Int?

    var id: String { path }
}

/// Which of those vaults the host's own credential operations resolve to.
///
/// `declared` and `discovered` are answers; `reads_owner` is a client host
/// that holds no vault by design and reads the owner through
/// `secrets.skarbiec.url`, with any vault files left on it named in `detail`
/// as copies; `ambiguous`, `declared-absent` and `none` mean every owner
/// write and authoritative read on that host is refused, and `unreadable`
/// means the host's installed release has no `secrets.skarbiec.vault_file`
/// field to report.
struct HostVaultAuthority: Decodable, Sendable {
    let state: String
    let path: String?
    let detail: String?
}

/// Why a reachable host stopped publishing beacons, read from the managed
/// publisher's own declared log by `stado host link`.
struct HostBeaconPublisherDiagnosis: Decodable, Sendable {
    let unit: String
    let code: String
    let detail: String
    let repairable: Bool
    let repairCommand: String?

    enum CodingKeys: String, CodingKey {
        case unit, code, detail, repairable
        case repairCommand = "repair_command"
    }
}
