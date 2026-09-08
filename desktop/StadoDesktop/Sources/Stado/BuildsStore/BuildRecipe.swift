import Foundation

/// One build recipe, exactly as `stado builds list --json` prints it.
///
/// Field names are the registry's own (snake_case, the branch serialized as
/// `ref`); nothing here is renamed or derived, so a row on the Builds screen
/// can be checked against the CLI's output word for word.
///
/// The four fields the registry writes with `#[serde(default)]` are decoded
/// with `decodeIfPresent`: a registry written before builds became
/// per-platform declares no platforms and records no runs, and that recipe
/// still has to list.
struct BuildRecipe: Decodable, Identifiable, Hashable, Sendable {
    let name: String
    let repo: String
    let ref: String
    let command: String
    let artifacts: [String]
    let enabled: Bool
    let intervalSeconds: UInt64
    /// The commit the poller last saw on `repo@ref`; `nil` until the first
    /// poll, which is a different fact from "the last build failed".
    let lastSeenRef: String?
    /// The platforms this recipe builds for, from the registry's own platform
    /// keys (`darwin-arm64`, `linux-amd64`). One build job is enqueued per
    /// platform, and only a worker on that platform can claim it.
    let platforms: [String]
    /// Whether a succeeded run whose commit carried a semver tag declares that
    /// version on every registry host of the run's platform. This is the one
    /// field on the row that writes to the fleet.
    let autoDeclare: Bool
    /// The run recorded for each platform, keyed by the platform string.
    let runs: [String: BuildRun]

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name
        case repo
        case ref
        case command
        case artifacts
        case enabled
        case intervalSeconds = "interval_seconds"
        case lastSeenRef = "last_seen_ref"
        case platforms
        case autoDeclare = "auto_declare"
        case runs
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        name = try values.decode(String.self, forKey: .name)
        repo = try values.decode(String.self, forKey: .repo)
        ref = try values.decode(String.self, forKey: .ref)
        command = try values.decode(String.self, forKey: .command)
        artifacts = try values.decodeIfPresent([String].self, forKey: .artifacts) ?? []
        enabled = try values.decodeIfPresent(Bool.self, forKey: .enabled) ?? false
        intervalSeconds = try values.decode(UInt64.self, forKey: .intervalSeconds)
        lastSeenRef = try values.decodeIfPresent(String.self, forKey: .lastSeenRef)
        platforms = try values.decodeIfPresent([String].self, forKey: .platforms) ?? []
        autoDeclare = try values.decodeIfPresent(Bool.self, forKey: .autoDeclare) ?? false
        runs = try values.decodeIfPresent([String: BuildRun].self, forKey: .runs) ?? [:]
    }

    /// Every platform this recipe has something to say about, each paired with
    /// the run the registry recorded for it.
    ///
    /// Declared platforms are first class: one that has never built is a row
    /// that says so, not a row that is missing. A run recorded for a platform
    /// the recipe no longer declares is kept too — dropping it would hide the
    /// last thing that actually happened.
    var platformRuns: [BuildPlatformRun] {
        var keys = Set(platforms)
        keys.formUnion(runs.keys)
        return keys.sorted().map { BuildPlatformRun(platform: $0, run: runs[$0]) }
    }

    /// The newest recorded run across every platform, by the RFC3339 stamp the
    /// registry wrote. Nil while no platform has run.
    var newestRun: BuildRun? {
        runs.values.max { $0.at < $1.at }
    }

    var hasFailedRun: Bool {
        runs.values.contains { $0.status == "failed" }
    }
}

/// One platform of a recipe: what it is, and what the registry recorded for
/// it. `run` is nil for a declared platform that has not built yet.
struct BuildPlatformRun: Identifiable, Hashable, Sendable {
    let platform: String
    let run: BuildRun?

    var id: String { platform }
}

/// What the last enqueued build of a recipe on one platform did, as the
/// registry recorded it.
struct BuildRun: Decodable, Hashable, Sendable {
    /// "succeeded" | "failed" | "running", in the scheduler's own words.
    let status: String
    /// RFC3339 stamp of when the job was enqueued or concluded.
    let at: String
    let jobID: String
    let artifactURIs: [String]
    /// The semver tag on the built commit, without its `v`. Nil when the
    /// commit carried no exact-semver tag, which is what keeps an untagged
    /// build from being declared anywhere.
    let version: String?
    /// Whether this run's version was declared on the hosts of its platform.
    let declared: Bool

    enum CodingKeys: String, CodingKey {
        case status
        case at
        case jobID = "job_id"
        case artifactURIs = "artifact_uris"
        case version
        case declared
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        status = try values.decode(String.self, forKey: .status)
        at = try values.decode(String.self, forKey: .at)
        jobID = try values.decode(String.self, forKey: .jobID)
        artifactURIs = try values.decodeIfPresent([String].self, forKey: .artifactURIs) ?? []
        version = try values.decodeIfPresent(String.self, forKey: .version)
        declared = try values.decodeIfPresent(Bool.self, forKey: .declared) ?? false
    }
}
