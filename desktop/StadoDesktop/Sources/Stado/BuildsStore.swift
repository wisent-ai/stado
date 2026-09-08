import Foundation
import WisentDesignSystem

/// The build recipes in the canonical registry, read and written through the
/// product CLI.
///
/// Reads run `stado builds list --json`. Every write — authoring a recipe,
/// changing one, removing one, flipping its enablement, enqueuing a build of
/// every platform now — runs the same `stado builds` command an operator would
/// type, so the confirmation dialog can quote the exact invocation. A refresh
/// that fails keeps the rows from the last successful read on screen: a
/// registry that stopped answering does not erase what it last said.
///
/// The recipe types this store reads and writes, and the argument lists its
/// commands are quoted from, sit beside it in `BuildsStore/`:
/// `BuildRecipe.swift`, `BuildRecipeReceipts.swift`, `BuildRecipeDraft.swift`,
/// `BuildRecipeEdit.swift` and `BuildsStoreCommands.swift`.
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
    /// Caller-retained `stado builds run` run ids, keyed by recipe name.
    private var runIDs: [String: String] = [:]

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    /// The caller-retained `--run-id` for one operator enqueue of `recipe`.
    /// It is generated once and kept until that enqueue succeeds, so a retry
    /// after a failure recovers the same per-platform durable runs instead of
    /// enqueueing a second set of build jobs.
    func retainedRunID(for recipe: String) -> String {
        if let existing = runIDs[recipe] {
            return existing
        }
        let generated = "desktop-\(UUID().uuidString.lowercased())"
        runIDs[recipe] = generated
        return generated
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

    /// `stado builds run <name> --run-id <id> --json`: one build job per
    /// platform the recipe declares, enqueued now, without waiting for the
    /// poller to notice a commit.
    func run(_ recipe: BuildRecipe, runID: String) async {
        mutation = .working("Enqueuing a build of \(recipe.name) on \(Self.platformList(recipe))")
        do {
            let receipt = try await cli.json(
                BuildRunReceipt.self,
                arguments: Self.runArguments(name: recipe.name, runID: runID)
            )
            runIDs.removeValue(forKey: recipe.name)
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

    /// `stado builds add … --json`. The CLI answers with the recipe it wrote,
    /// which lands in the table before the follow-up read confirms it.
    ///
    /// A new recipe is disabled: authoring one polls nothing and builds
    /// nothing until an operator says so, which is why this write asks for no
    /// confirmation of its own.
    func add(_ draft: BuildRecipeDraft) async {
        mutation = .working("Adding \(draft.recipeName)")
        do {
            let created = try await cli.json(
                BuildRecipe.self,
                arguments: Self.addArguments(draft)
            )
            replace(created)
            var sentence =
                "\(created.name) is recorded for \(Self.platformList(created)) and starts disabled: nothing is polled and nothing is built until it is enabled here, and Run now… builds it once without enabling it."
            if created.autoDeclare {
                sentence +=
                    " Auto-declare is on: a succeeded build whose commit carried a semver tag writes that version into the managed versions of every registry host on the run's platform."
            }
            mutation = .succeeded(sentence)
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        await refresh()
    }

    /// `stado builds edit <name> [changed flags] --json`: the fields the
    /// operator changed, and no others.
    ///
    /// Whether the recipe re-fires is decided by which fields moved, so the
    /// outcome says which state the write cleared. Moving the source clears the
    /// last seen commit and the recorded runs — they describe a repository and
    /// branch that are no longer the recipe's; changing how it builds keeps
    /// both, and a platform named for the first time simply has no run yet.
    func edit(_ change: BuildRecipeEdit) async {
        mutation = .working("Changing \(change.name)")
        do {
            let updated = try await cli.json(
                BuildRecipe.self,
                arguments: Self.editArguments(change)
            )
            replace(updated)
            let fields = change.changedFields.joined(separator: ", ")
            mutation = .succeeded(
                change.movesSource
                    ? "\(updated.name) now builds \(updated.repo) at \(updated.ref) (changed: \(fields)). The last seen commit and every recorded run were cleared: they described the source it no longer builds, so the next poll builds the current head of this one."
                    : "\(updated.name) is changed (changed: \(fields)). The last seen commit and the recorded runs are untouched — how it builds moved, not what it builds from, and a platform named for the first time simply has no run yet."
            )
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        await refresh()
    }

    /// `stado builds remove <name> --json`. The registry stops declaring the
    /// recipe; what the recipe already did to the fleet stays done.
    func remove(_ recipe: BuildRecipe) async {
        mutation = .working("Removing \(recipe.name)")
        do {
            let removal = try await cli.json(
                BuildRecipeRemoval.self,
                arguments: Self.removeArguments(name: recipe.name)
            )
            if removal.removed {
                recipes.removeAll { $0.name == removal.name }
                mutation = .succeeded(
                    "\(removal.name) is removed from the registry: nothing polls it and no new job is enqueued for it. A job it already enqueued keeps running and keeps its results, and a version it declared stays declared on the hosts that took it."
                )
            } else {
                mutation = .failed(
                    "The registry still declares \(removal.name). Nothing was removed."
                )
            }
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
