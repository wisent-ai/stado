import Foundation

/// What the host says about a throwaway account, its home directory, or its
/// lease record. `stado scratch list` reads the account from the machine
/// rather than from the record, so a lease whose account is gone answers
/// `absent` — the state that makes a half-destroyed lease visible instead of
/// merely recorded.
enum ScratchPresence: Decodable, Sendable, Hashable {
    case present
    case absent
    /// What `scratch create` answers for the account it has just made.
    case created
    /// A word this console does not model, kept exactly as it was printed.
    case reported(String)

    init(from decoder: Decoder) throws {
        switch try decoder.singleValueContainer().decode(String.self) {
        case "present": self = .present
        case "absent": self = .absent
        case "created": self = .created
        case let other: self = .reported(other)
        }
    }

    /// Whether the thing is still on the host. `nil` where the report used a
    /// word this console does not model: no claim beats a wrong one.
    var exists: Bool? {
        switch self {
        case .present, .created: true
        case .absent: false
        case .reported: nil
        }
    }

    var word: String {
        switch self {
        case .present: "present"
        case .absent: "absent"
        case .created: "created"
        case let .reported(other): other
        }
    }
}

/// One declared scratch profile. Durations are the declaration's own strings
/// (`1h`, `8h`, `90m`); Desktop never converts one into a number of minutes
/// and never invents a default the declaration did not name.
struct ScratchProfile: Decodable, Identifiable, Sendable {
    let name, summary, mechanism, shell: String
    let platforms: [String]
    let defaultTTL, maxTTL: String

    var id: String { name }

    private enum CodingKeys: String, CodingKey {
        case name, summary, mechanism, shell, platforms
        case defaultTTL = "default_ttl", maxTTL = "max_ttl"
    }
}

/// `stado scratch profiles --json`.
struct ScratchProfileCatalog: Decodable, Sendable {
    let declaration, schema: String
    let profiles: [ScratchProfile]
}

/// What `stado scratch create` will be asked for.
///
/// Choosing a profile seeds the TTL with that profile's declared
/// `default_ttl`, and whatever the field holds is sent verbatim: Desktop
/// neither parses a duration nor converts one, so `90m` refused above a
/// declared `8h` is refused by the command that owns the declaration.
struct ScratchCreateForm: Equatable, Sendable {
    var profile = ""
    var ttl = ""
    var name = ""
    var root = ""

    mutating func select(_ profile: ScratchProfile) {
        self.profile = profile.name
        ttl = profile.defaultTTL
    }

    func arguments(host: String) -> [String] {
        ScratchStore.createArguments(
            host: host,
            profile: profile,
            name: name,
            ttl: ttl,
            root: root
        )
    }
}

/// `stado scratch create --json`: the leased disposable target, and the
/// storage root a run against it is pointed at with `WC_LOCAL_STORAGE_PATH`.
struct ScratchLeaseReceipt: Decodable, Sendable {
    let name, target, profile, mechanism, username, ssh: String
    let createdAt, expiresAt, ttl, storageRoot, registryPath: String
    let account: ScratchPresence
    let verifiedLogin: String
    /// The account's home directory as the host's directory service reports
    /// it, so a confirmation can name the directory instead of guessing
    /// `/Users` or `/home` from a platform.
    let homePath: String
    /// Expired leases this create destroyed on the same host before leasing.
    let reaped: [String]
    let exitCode: Int
    let status: String

    private enum CodingKeys: String, CodingKey {
        case name, target, profile, mechanism, username, ssh, ttl, account, reaped, status
        case createdAt = "created_at", expiresAt = "expires_at"
        case storageRoot = "storage_root", registryPath = "registry_path"
        case verifiedLogin = "verified_login", exitCode = "exit_code"
        case homePath = "home_path"
    }
}

/// One row of `stado scratch list --json`.
struct ScratchLease: Decodable, Identifiable, Sendable {
    let name, username, profile, createdAt, expiresAt: String
    let expired: Bool
    /// Absent where the record carries no readable stamp. A leaked record is
    /// exactly the row an operator needs to see, so it must decode: an
    /// `Int` here made one unreadable record blind the whole section.
    let secondsRemaining: Int?
    let account: ScratchPresence
    let homePath: String?
    let requestedBy: String
    /// Why the record could not be read, when it could not be.
    let unreadable: String?

    var id: String { name }

    private enum CodingKeys: String, CodingKey {
        case name, username, profile, expired, account, unreadable
        case createdAt = "created_at", expiresAt = "expires_at"
        case secondsRemaining = "seconds_remaining", requestedBy = "requested_by"
        case homePath = "home_path"
    }
}

struct ScratchLeaseListing: Decodable, Sendable {
    let target, ssh: String
    let leases: [ScratchLease]
    let exitCode: Int
    let status: String

    private enum CodingKeys: String, CodingKey {
        case target, ssh, leases, status
        case exitCode = "exit_code"
    }
}

/// `stado scratch destroy <NAME> --json`: the account, its home, its record,
/// and when the account went.
struct ScratchDestroyReceipt: Decodable, Sendable {
    let name, target, username: String
    let account, home, record: ScratchPresence
    let destroyedAt, storageRoot: String
    /// The home directory that went with the account, when the read that
    /// preceded the destroy knew it.
    let homePath: String?
    let exitCode: Int
    let status: String

    private enum CodingKeys: String, CodingKey {
        case name, target, username, account, home, record, status
        case destroyedAt = "destroyed_at", storageRoot = "storage_root"
        case exitCode = "exit_code", homePath = "home_path"
    }
}

/// One row of a sweep. A previewed row was not destroyed and carries no
/// stamp; a destroyed row carries the moment its account went.
struct ScratchReapLease: Decodable, Identifiable, Sendable {
    let name, expiresAt: String
    let expired: Bool
    let action: String
    let destroyedAt: String?

    var id: String { name }

    private enum CodingKeys: String, CodingKey {
        case name, expired, action
        case expiresAt = "expires_at", destroyedAt = "destroyed_at"
    }
}

/// `stado scratch reap --json`, with and without `--apply`.
struct ScratchReapReport: Decodable, Sendable {
    let target: String
    let apply: Bool
    let leases: [ScratchReapLease]
    let destroyed, kept, exitCode: Int
    let status: String

    private enum CodingKeys: String, CodingKey {
        case target, apply, leases, destroyed, kept, status
        case exitCode = "exit_code"
    }
}
