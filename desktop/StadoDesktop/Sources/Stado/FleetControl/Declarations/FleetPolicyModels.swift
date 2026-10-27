import Foundation

/// Canonical fleet policy as the dashboard is willing to project it.
///
/// `GET /api/registry.json` deliberately returns a few whitelisted fields per
/// target; routing and SSH material stay inside the registry document and are
/// never sent to an operator client. This type therefore has no room to grow
/// into a registry editor. Nothing about disk cleanup is declared per target:
/// every host's janitor follows the one disk-full rule.
struct FleetPolicy: Decodable, Sendable {
    let generation: String
    let targets: [FleetPolicyTarget]
    /// The document's own `public_origins`, which belong to the deployment
    /// rather than to one target: a public origin names the target that
    /// serves it, and a target may serve none.
    let publicOrigins: [FleetPublicOrigin]

    enum CodingKeys: String, CodingKey {
        case generation, targets
        case publicOrigins = "public_origins"
    }

    /// `GET /api/registry.json` writes its generation as the store's version
    /// text and both lists in every answer (`public_origins` empty when the
    /// fleet publishes nothing); a projection that lacks one is a decoding
    /// error naming the key, not a policy with no generation or no targets.
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        generation = try values.decode(String.self, forKey: .generation)
        targets = try values.decode([FleetPolicyTarget].self, forKey: .targets)
        publicOrigins = try values.decode([FleetPublicOrigin].self, forKey: .publicOrigins)
    }
}

struct FleetPolicyTarget: Decodable, Identifiable, Sendable {
    let name: String
    let pinnedOnly: Bool?
    /// The directory the host's agent keeps the fleet's work in, when the
    /// registry declares one: the volume the disk-full rule is measured on.
    let workRoot: String?
    let welesRecordingsDirectory: String?

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name
        case pinnedOnly = "pinned_only"
        case workRoot = "work_root"
        case weles
    }

    /// `weles` is projected only when the registry declares a recordings
    /// directory for the target, and then carries it; a target without the
    /// block declares none.
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        name = try values.decode(String.self, forKey: .name)
        pinnedOnly = try values.decodeIfPresent(Bool.self, forKey: .pinnedOnly)
        workRoot = try values.decodeIfPresent(String.self, forKey: .workRoot)
        welesRecordingsDirectory =
            try values.decodeIfPresent(WelesBlock.self, forKey: .weles)?.recordingsDirectory
    }

    private struct WelesBlock: Decodable {
        let recordingsDirectory: String

        enum CodingKeys: String, CodingKey {
            case recordingsDirectory = "recordings_dir"
        }
    }
}

/// The one policy patch the dashboard accepts from an operator client.
enum FleetPolicyPatch: Sendable {
    case pinnedOnly(Bool)

    var body: [String: Any] {
        switch self {
        case let .pinnedOnly(value):
            ["pinned_only": value]
        }
    }

    /// The exact JSON object posted to `POST /api/registry/policy`: the named
    /// target plus this patch. One place, so the body an operator reviewed
    /// and the body the client sends cannot drift apart.
    func requestBody(target: String) -> [String: Any] {
        var payload: [String: Any] = ["target": target]
        payload.merge(body) { _, new in new }
        return payload
    }
}

struct RegistryImportConflict: Decodable, Identifiable, Sendable {
    let path: String
    let reason: String

    var id: String { "\(path)\u{0}\(reason)" }
}
