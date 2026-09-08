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
    let detail: String?

    var succeeded: Bool {
        status == "token_minted" || status == "token_registered"
    }

    enum CodingKeys: String, CodingKey {
        case target, status, skarbiec, detail
        case tokenSource = "token_source"
    }
}

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

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        ok = try values.decodeIfPresent(Bool.self, forKey: .ok) ?? false
        consumer = try values.decodeIfPresent(String.self, forKey: .consumer) ?? ""
        capabilities =
            try values.decodeIfPresent([HostVaultBearerCapability].self, forKey: .capabilities) ?? []
        workloadBound = try values.decodeIfPresent(Bool.self, forKey: .workloadBound) ?? false
        audience = try values.decodeIfPresent(String.self, forKey: .audience) ?? ""
        expiresAt = try values.decodeIfPresent(UInt64.self, forKey: .expiresAt)
        tokenFile = try values.decodeIfPresent(String.self, forKey: .tokenFile)
    }
}

struct HostVaultBearerCapability: Decodable, Sendable {
    let action: String
    let item: String
    let field: String?

    var displayValue: String {
        "\(action):\(item)\(field.map { "#\($0)" } ?? "")"
    }
}

struct HostVaultBearerSource: Decodable, Sendable {
    let item: String
    let field: String
}
