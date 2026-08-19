import Foundation
import WisentDesignSystem

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

/// What `stado builds run <name> --json` answers with: the job it enqueued for
/// each platform, and the recipe as the registry now records it.
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

/// The build recipes in the canonical registry, read and written through the
/// product CLI.
///
/// Reads run `stado builds list --json`; the two writes this screen allows —
/// flipping a recipe's enablement and enqueuing a build of every platform now
/// — run the same `stado builds` commands an operator would type, so the
/// confirmation dialog can quote the exact invocation. A refresh that fails
/// keeps the rows from the last successful read on screen: a registry that
/// stopped answering does not erase what it last said.
@MainActor
final class BuildsStore: ObservableObject {
    @Published private(set) var recipes: [BuildRecipe] = []
    /// The list command's own sentence when the last read produced no answer.
    @Published private(set) var problem: String?
    @Published private(set) var isRefreshing = false
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var mutation: WisentMutationOutcome = .idle

    private let cli: StadoCLI
    private var refreshGeneration = 0

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func listArguments() -> [String] {
        ["builds", "list", "--json"]
    }

    nonisolated static func enablementArguments(name: String, enabled: Bool) -> [String] {
        ["builds", enabled ? "enable" : "disable", name, "--json"]
    }

    nonisolated static func runArguments(name: String) -> [String] {
        ["builds", "run", name, "--json"]
    }

    /// The recipe's platforms in one clause, for a sentence an operator reads
    /// once. A recipe with no platform declares nothing to build, and saying so
    /// is better than an empty gap in the sentence.
    nonisolated static func platformList(_ recipe: BuildRecipe) -> String {
        recipe.platforms.isEmpty ? "no platform" : recipe.platforms.joined(separator: " and ")
    }

    func refresh() async {
        guard !isRefreshing else { return }
        refreshGeneration += 1
        let generation = refreshGeneration
        isRefreshing = true
        defer {
            if generation == refreshGeneration {
                isRefreshing = false
            }
        }

        do {
            let listed = try await cli.json([BuildRecipe].self, arguments: Self.listArguments())
            guard generation == refreshGeneration else { return }
            recipes = listed.sorted { $0.name < $1.name }
            problem = nil
        } catch {
            guard generation == refreshGeneration else { return }
            problem = Self.message(for: error)
        }
        lastUpdated = Date()
    }

    /// `stado builds enable|disable <name> --json`. The CLI answers with the
    /// recipe as the registry now records it, which replaces the row before
    /// the follow-up read confirms it.
    func setEnabled(_ recipe: BuildRecipe, to enabled: Bool) async {
        mutation = .working(enabled ? "Enabling \(recipe.name)" : "Disabling \(recipe.name)")
        do {
            let updated = try await cli.json(
                BuildRecipe.self,
                arguments: Self.enablementArguments(name: recipe.name, enabled: enabled)
            )
            replace(updated)
            mutation = .succeeded(
                enabled
                    ? "\(updated.name) is enabled. Every new commit on \(updated.repo) at \(updated.ref) is built once per platform: \(Self.platformList(updated))."
                    : "\(updated.name) is disabled. The control plane stops polling it; nothing new is built until it is enabled again."
            )
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        await refresh()
    }

    /// `stado builds run <name> --json`: one build job per platform the recipe
    /// declares, enqueued now, without waiting for the poller to notice a
    /// commit.
    func run(_ recipe: BuildRecipe) async {
        mutation = .working("Enqueuing a build of \(recipe.name) on \(Self.platformList(recipe))")
        do {
            let receipt = try await cli.json(
                BuildRunReceipt.self,
                arguments: Self.runArguments(name: recipe.name)
            )
            replace(receipt.recipe)
            let enqueued = receipt.enqueued
            mutation = .succeeded(
                enqueued.isEmpty
                    ? "Enqueued the build of \(receipt.name). The Queue screen tracks its jobs from here."
                    : "Enqueued \(receipt.jobs.count == 1 ? "job" : "jobs") for \(receipt.name): \(enqueued). The Queue screen tracks them from here."
            )
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        await refresh()
    }

    func clearMutation() {
        mutation = .idle
    }

    private func replace(_ updated: BuildRecipe) {
        if let index = recipes.firstIndex(where: { $0.name == updated.name }) {
            recipes[index] = updated
        } else {
            recipes = (recipes + [updated]).sorted { $0.name < $1.name }
        }
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}
