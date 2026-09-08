import Foundation

// MARK: - Typed host filesystem inventory

/// `GET /api/host/inventory?target=…`, narrowed to the fixed Cargo filesystem
/// section the Hosts inspector renders.
struct HostInventoryReport: Decodable, Sendable {
    let target: String
    let status: String
    let error: String?
    let cargo: HostCargoInventory?
}

/// The managed account's `$HOME/.cargo` and fixed `bin` child.
struct HostCargoInventory: Decodable, Sendable {
    let home: HostFilesystemMetadata
    let bin: HostFilesystemMetadata
    let entries: [HostFilesystemMetadata]
    let entriesSeen: UInt64
    let entriesComplete: Bool
    let entriesState: String
    let complete: Bool

    enum CodingKeys: String, CodingKey {
        case home, bin, entries, complete
        case entriesSeen = "entries_seen"
        case entriesComplete = "entries_complete"
        case entriesState = "entries_state"
    }
}

/// One lstat row from the fixed Cargo inventory.
struct HostFilesystemMetadata: Decodable, Sendable {
    let name: String
    let nameState: String
    let kind: String
    let metadataState: String
    let bytes: UInt64?
    let mode: String
    let uid: UInt64?
    let gid: UInt64?
    let modifiedEpoch: Int64?
    let symlinkTarget: String
    let symlinkTargetState: String

    enum CodingKeys: String, CodingKey {
        case name, kind, bytes, mode, uid, gid
        case nameState = "name_state"
        case metadataState = "metadata_state"
        case modifiedEpoch = "modified_epoch"
        case symlinkTarget = "symlink_target"
        case symlinkTargetState = "symlink_target_state"
    }
}
