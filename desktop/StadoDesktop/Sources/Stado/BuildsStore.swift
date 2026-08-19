import Foundation
import WisentDesignSystem

/// One build recipe, exactly as `stado builds list --json` prints it.
///
/// Field names are the registry's own (snake_case, the branch serialized as
/// `ref`); nothing here is renamed or derived, so a row on the Builds screen
/// can be checked against the CLI's output word for word.
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
    let lastRun: BuildRun?

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
        case lastRun = "last_run"
    }
}

/// What the last enqueued build of a recipe did, as the registry recorded it.
struct BuildRun: Decodable, Hashable, Sendable {
    /// "succeeded" | "failed" | "running", in the scheduler's own words.
    let status: String
    /// RFC3339 stamp of when the job was enqueued or concluded.
    let at: String
    let jobID: String
    let artifactURIs: [String]

    enum CodingKeys: String, CodingKey {
        case status
        case at
        case jobID = "job_id"
        case artifactURIs = "artifact_uris"
    }
}

/// What `stado builds run <name> --json` answers with: the job it enqueued and
/// the recipe as the registry now records it.
struct BuildRunReceipt: Decodable, Sendable {
    let name: String
    let jobID: String
    let recipe: BuildRecipe

    enum CodingKeys: String, CodingKey {
        case name
        case jobID = "job_id"
        case recipe
    }
}

/// The build recipes in the canonical registry, read and written through the
/// product CLI.
///
/// Reads run `stado builds list --json`; the two writes this screen allows —
/// flipping a recipe's enablement and enqueuing one build now — run the same
/// `stado builds` commands an operator would type, so the confirmation dialog
/// can quote the exact invocation. A refresh that fails keeps the rows from
/// the last successful read on screen: a registry that stopped answering does
/// not erase what it last said.
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
                    ? "\(updated.name) is enabled. New commits on \(updated.repo) at \(updated.ref) will be built."
                    : "\(updated.name) is disabled. The control plane stops polling it; nothing new is built until it is enabled again."
            )
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        await refresh()
    }

    /// `stado builds run <name> --json`: one build job, enqueued now, without
    /// waiting for the poller to notice a commit.
    func run(_ recipe: BuildRecipe) async {
        mutation = .working("Enqueuing a build of \(recipe.name)")
        do {
            let receipt = try await cli.json(
                BuildRunReceipt.self,
                arguments: Self.runArguments(name: recipe.name)
            )
            replace(receipt.recipe)
            mutation = .succeeded("Enqueued job \(receipt.jobID) for \(receipt.name). The Queue screen tracks it from here.")
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
