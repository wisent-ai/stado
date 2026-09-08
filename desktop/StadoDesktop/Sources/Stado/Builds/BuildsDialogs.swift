import SwiftUI
import WisentDesignSystem

/// The four confirmations a build recipe can ask for, and the one sentence
/// about the fleet write they share.
///
/// `dialog` is internal rather than private only because the sheet that
/// presents it sits in `BuildsView.swift`: Swift scopes `private` to one
/// file. `declarationLine` keeps its `private`, because `dialog` is its only
/// caller and they share this one.
extension BuildsView {
    // MARK: The decision, before the write

    func dialog(_ pending: BuildDecision) -> WisentDecisionDialog {
        let recipe = pending.recipe
        switch pending.kind {
        case .enable:
            return WisentDecisionDialog(
                tone: .warning,
                title: "Enable builds for \(recipe.name)?",
                lines: [
                    "The control plane starts polling \(recipe.repo) at \(recipe.ref) every \(recipe.intervalSeconds.formatted(.number)) seconds. Every new commit it sees is built once per platform — \(BuildsStore.platformList(recipe)) — and each job can only be claimed by a worker of that platform.",
                    "Each job clones the repository on a fleet host, runs the recipe's build command there, and uploads the declared artifacts under the job's results. It also records the version: the semver tag on the built commit, or none when the commit carries no tag.",
                    declarationLine(recipe),
                ],
                listing: ["command: \(recipe.command)"]
                    + recipe.platforms.map { "platform: \($0)" }
                    + recipe.artifacts.map { "artifact: \($0)" },
                footnote: "Runs \(StadoCLI.commandLine(BuildsStore.enablementArguments(name: recipe.name, enabled: true))).",
                actions: [
                    WisentAction("Keep it disabled", kind: .secondary) { decision = nil },
                    WisentAction("Enable", symbol: "play.circle", kind: .primary) {
                        decision = nil
                        Task { await store.setEnabled(recipe, to: true) }
                    },
                ]
            )
        case .disable:
            return WisentDecisionDialog(
                tone: .warning,
                title: "Disable builds for \(recipe.name)?",
                lines: [
                    "The control plane stops polling \(recipe.repo) at \(recipe.ref). Commits made while it is disabled are not built, and nothing catches up on them when it is enabled again — only the next new commit is.",
                    "A job already enqueued keeps running; this stops new ones.",
                ],
                footnote: "Runs \(StadoCLI.commandLine(BuildsStore.enablementArguments(name: recipe.name, enabled: false))).",
                actions: [
                    WisentAction("Keep it enabled", kind: .secondary) { decision = nil },
                    WisentAction("Disable", symbol: "pause.circle", kind: .primary) {
                        decision = nil
                        Task { await store.setEnabled(recipe, to: false) }
                    },
                ]
            )
        case .run:
            return WisentDecisionDialog(
                tone: .warning,
                title: recipe.platforms.count == 1
                    ? "Enqueue a build of \(recipe.name) now?"
                    : "Enqueue \(recipe.platforms.count) builds of \(recipe.name) now?",
                lines: [
                    "One job per platform is enqueued immediately — \(BuildsStore.platformList(recipe)) — without waiting for the poller to see a new commit. Each is admitted from the worker's live CPU, memory, disk, and accelerator capacity on its own platform, clones \(recipe.repo) at \(recipe.ref), runs the build command, and uploads the declared artifacts under the job's results.",
                    "Each job also records the version: the semver tag on the built commit, or none when the commit carries no tag.",
                    declarationLine(recipe),
                ],
                listing: ["command: \(recipe.command)"]
                    + recipe.platforms.map { "platform: \($0)" }
                    + recipe.artifacts.map { "artifact: \($0)" },
                footnote: "Runs \(StadoCLI.commandLine(BuildsStore.runArguments(name: recipe.name, runID: pending.runID))).",
                actions: [
                    WisentAction("Not now", kind: .secondary) { decision = nil },
                    WisentAction(
                        recipe.platforms.count == 1 ? "Enqueue the build" : "Enqueue the builds",
                        symbol: "hammer",
                        kind: .primary
                    ) {
                        decision = nil
                        Task { await store.run(recipe, runID: pending.runID) }
                    },
                ]
            )
        case .remove:
            return WisentDecisionDialog(
                tone: .danger,
                title: "Delete the build recipe \(recipe.name)?",
                lines: [
                    "The registry stops declaring \(recipe.name). What it built \(recipe.repo) at \(recipe.ref) with — the command, the artifact paths, its platforms, the commit it last saw and every run it recorded — is gone from the registry with it, and nothing here brings it back.",
                    "What already happened stays: a job it enqueued keeps running and keeps its results under the queue, and a version it declared stays the managed version of the hosts that took it. Deleting a recipe never un-declares anything.",
                    "To stop building it without losing it, disable it instead — the switch on its row.",
                ],
                listing: ["source: \(recipe.repo)@\(recipe.ref)", "command: \(recipe.command)"]
                    + recipe.platforms.map { "platform: \($0)" }
                    + recipe.artifacts.map { "artifact: \($0)" }
                    + ["last seen: \(recipe.lastSeenRef ?? "never polled")"]
                    + ["recorded runs: \(recipe.runs.count.formatted(.number))"],
                footnote: "Runs \(StadoCLI.commandLine(BuildsStore.removeArguments(name: recipe.name))).",
                actions: [
                    WisentAction("Keep the recipe", kind: .primary) { decision = nil },
                    WisentAction("Delete it", symbol: "trash", kind: .destructive) {
                        decision = nil
                        Task { await store.remove(recipe) }
                    },
                ]
            )
        }
    }

    /// What a succeeded build does to the fleet, which is the one sentence in
    /// these dialogs that is about a write and not about a build. Auto-declare
    /// is the only path from this screen to a host's managed versions, and even
    /// then a signed release is a separate, deliberate step.
    private func declarationLine(_ recipe: BuildRecipe) -> String {
        if recipe.autoDeclare {
            return "This recipe declares automatically: a succeeded build whose commit carried a semver tag writes that version into the managed versions of every registry host on the run's platform. An untagged commit declares nothing. Promoting a signed release is still separate — stado release promote verifies the manifest and its signature — and delivery is still converge --apply."
        }
        return "Nothing is declared to the fleet and no host converges onto it. The version a build records is a fact on the recipe until stado host declare-version writes it, and a signed release is a separate step: stado release promote verifies the manifest and its signature, then converge --apply delivers it."
    }
}
