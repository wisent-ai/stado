import Foundation

/// What `stado builds run <name> --run-id <id> --json` answers with: the job it
/// enqueued for each platform, and the recipe as the registry now records it.
/// `--run-id` is required by the command, and reusing the same token recovers
/// the same durable run instead of enqueuing a second one.
struct BuildRunReceipt: Decodable, Sendable {
    let name: String
    /// Platform to queue job id. One entry per platform the recipe declares.
    let jobs: [String: String]
    let recipe: BuildRecipe

    enum CodingKeys: String, CodingKey {
        case name
        case jobs
        case recipe
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        name = try values.decode(String.self, forKey: .name)
        jobs = try values.decodeIfPresent([String: String].self, forKey: .jobs) ?? [:]
        recipe = try values.decode(BuildRecipe.self, forKey: .recipe)
    }

    /// "darwin-arm64 6f2c1ab0, linux-amd64 9d40e21c" — every job the one
    /// command enqueued, so the operator can find each on the Queue screen.
    var enqueued: String {
        jobs
            .sorted { $0.key < $1.key }
            .map { "\($0.key) \($0.value)" }
            .joined(separator: ", ")
    }
}

/// What `stado builds remove <name> --json` answers with: the recipe it was
/// asked about, and whether the registry stopped carrying it.
struct BuildRecipeRemoval: Decodable, Sendable {
    let name: String
    let removed: Bool
}

/// The release platforms a recipe may name, in the order the published
/// platform table declares them.
///
/// The form offers exactly these words. A word the table does not carry is a
/// usage error from the CLI, and a console that lets an operator type one is a
/// console that hands back a refusal it could have prevented.
enum BuildPlatforms {
    static let all: [String] = ["darwin-arm64", "linux-amd64"]
}
