import Foundation

struct CredentialInspection: Decodable, Sendable {
    struct Management: Decodable, Sendable {
        let mode: String?
        let controller: String?
        let writer: String?
        let operation_id: String?
    }

    struct Item: Decodable, Identifiable, Sendable {
        let id: String
        let kind: String?
        let state: String?
        let management: Management?
        let revision: UInt64?
        let updated_at: String?
        let tags: [String]?
        let recipients: [String]?
        let item_uid: String?
        let versions: Int?
        let deleted: Bool?

        private enum CodingKeys: String, CodingKey {
            case id, name, kind, state, management, revision, updated_at
            case tags, recipients, item_uid, versions, deleted
        }

        init(from decoder: Decoder) throws {
            let fields = try decoder.container(keyedBy: CodingKeys.self)
            id = try fields.decodeIfPresent(String.self, forKey: .id)
                ?? fields.decode(String.self, forKey: .name)
            kind = try fields.decodeIfPresent(String.self, forKey: .kind)
            state = try fields.decodeIfPresent(String.self, forKey: .state)
            management = try fields.decodeIfPresent(Management.self, forKey: .management)
            revision = try fields.decodeIfPresent(UInt64.self, forKey: .revision)
            updated_at = try fields.decodeIfPresent(String.self, forKey: .updated_at)
            tags = try fields.decodeIfPresent([String].self, forKey: .tags)
            recipients = try fields.decodeIfPresent([String].self, forKey: .recipients)
            item_uid = try fields.decodeIfPresent(String.self, forKey: .item_uid)
            versions = try fields.decodeIfPresent(Int.self, forKey: .versions)
            deleted = try fields.decodeIfPresent(Bool.self, forKey: .deleted)
        }
    }

    struct Grant: Decodable, Sendable {
        struct Capability: Decodable, Sendable {
            let action: String
            let item: String
            let field: String?
        }
        let consumer: String
        let audience: String?
        let expires_at: UInt64?
        let workload_bound: Bool?
        let capabilities: [Capability]
    }

    let host: String?
    let vault: String
    let items_total: Int?
    let items: [Item]
    let grants: [Grant]?
}

