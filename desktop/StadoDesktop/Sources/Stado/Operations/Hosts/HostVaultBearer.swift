import Foundation

/// Non-secret receipt from `stado credentials token mint --json`.
///
/// The bearer is deliberately not represented. Desktop keeps only the grant
/// metadata the command returns after removing the token, plus the optional
/// owner-vault coordinate used to register an existing bearer.
struct HostVaultBearerReceipt: Decodable, Sendable {
    let target: String
    let status: String
    let skarbiec: HostVaultBearerGrant
    let tokenSource: HostVaultBearerSource?
    let storedItem: HostVaultBearerStoredItem?
    let detail: String?

    var succeeded: Bool {
        status == "token_minted" || status == "token_registered"
    }

    enum CodingKeys: String, CodingKey {
        case target, status, skarbiec, detail
        case tokenSource = "token_source"
        case storedItem = "stored_item"
    }
}

/// The grant as Skarbiec's `grant issue` answers it, which the mint receipt
/// carries whole: `ok`, `consumer`, `capabilities`, `workload_bound` and
/// `audience` are written in every answer, so a receipt that lacks one is a
/// decoding error naming the key, not a grant that failed, served nobody or
/// is unbound.
struct HostVaultBearerGrant: Decodable, Sendable {
    let ok: Bool
    let consumer: String
    let capabilities: [HostVaultBearerCapability]
    let workloadBound: Bool
    let audience: String
    let expiresAt: UInt64?
    let tokenFile: String?

    enum CodingKeys: String, CodingKey {
        case ok, consumer, capabilities, audience
        case workloadBound = "workload_bound"
        case expiresAt = "expires_at"
        case tokenFile = "token_file"
    }
}

struct HostVaultBearerCapability: Decodable, Sendable {
    let action: String
    let item: String
    let field: String?

    var displayValue: String {
        switch field {
        case .some(let field): return "\(action):\(item)#\(field)"
        case .none: return "\(action):\(item)"
        }
    }
}

struct HostVaultBearerSource: Decodable, Sendable {
    let item: String
    let field: String
}

/// The owner-vault item `--store-item` wrote the newly minted bearer into,
/// reported by its encrypted-record revision, never its value.
struct HostVaultBearerStoredItem: Decodable, Sendable {
    let item: String
    let after: HostVaultBearerItemPhase
}

struct HostVaultBearerItemPhase: Decodable, Sendable {
    let state: String
    let revision: String
}
