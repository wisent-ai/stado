import SwiftUI
import WisentDesignSystem

/// The pending flip or enqueue, held until the operator confirms it. A build
/// command is arbitrary code on a fleet host, so neither the toggle nor the
/// run button writes anything by itself.
private struct BuildDecision: Identifiable {
    enum Kind: String {
        case enable
        case disable
        case run
    }

    let kind: Kind
    let recipe: BuildRecipe

    var id: String { "\(kind.rawValue)/\(recipe.name)" }
}

/// Which repositories the control plane watches, and what the last build of
/// each produced.
///
/// v1 boundary, stated where the operator acts on it: a build ends at
/// artifacts under the job's results. Declaring the version and delivering it
/// to the fleet stay manual — `stado host declare-version`, then
/// `converge --apply`.
struct BuildsView: View {
    @ObservedObject var store: BuildsStore
    let scope: String

    @State private var decision: BuildDecision?

    var body: some View {
        WisentScreen(
            title: "Builds",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(store.lastUpdated))",
            actions: [
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task { await store.refresh() }
                },
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: 0) {
                if store.lastUpdated == nil, store.isRefreshing {
                    WisentLoadingPanel(
                        title: "Reading build recipes",
                        detail: "stado builds list --json against the canonical registry. Nothing is written."
                    )
                    .padding(WisentDesign.Space.x6)
                    Spacer(minLength: 0)
                } else {
                    notices
                    table
                }
            }
        }
        .task { await store.refresh() }
        .sheet(item: $decision) { pending in
            dialog(pending)
        }
    }

    // MARK: What went wrong, at the top

    @ViewBuilder
    private var notices: some View {
        VStack(spacing: WisentDesign.Space.x3) {
            WisentMutationBar(outcome: store.mutation) { store.clearMutation() }
            if let problem = store.problem {
                WisentAlertPanel(
                    tone: .warning,
                    title: "Build recipes could not be read",
                    detail: problem,
                    command: "stado builds list --json",
                    actions: [
                        WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                            Task { await store.refresh() }
                        },
                    ]
                )
            }
        }
        .padding(.horizontal, WisentDesign.Space.x4)
        .padding(.top, store.problem == nil && store.mutation == .idle ? 0 : WisentDesign.Space.x4)
    }

    // MARK: Rows

    @ViewBuilder
    private var table: some View {
        if store.recipes.isEmpty {
            empty
        } else {
            ConsoleTable(head: [
                ConsoleHeaderCell("Recipe", width: 160),
                ConsoleHeaderCell("Repository"),
                ConsoleHeaderCell("Enabled", width: 64),
                ConsoleHeaderCell("Last seen", width: 96),
                ConsoleHeaderCell("Last run", width: 200),
                ConsoleHeaderCell("", width: 96, trailing: true),
            ]) {
                ForEach(store.recipes) { recipe in
                    row(recipe)
                }
            }
        }
    }

    private func row(_ recipe: BuildRecipe) -> some View {
        ConsoleTableRow {
            ConsoleCell(text: recipe.name, width: 160, identifier: true, strong: true)
            ConsoleCell(text: "\(recipe.repo)@\(recipe.ref)", identifier: true)
            Toggle(recipe.enabled ? "Disable \(recipe.name)" : "Enable \(recipe.name)", isOn: enablementBinding(recipe))
                .labelsHidden()
                .toggleStyle(.switch)
                .controlSize(.mini)
                .disabled(store.mutation.isWorking)
                .frame(width: 64, alignment: .leading)
            ConsoleCell(
                text: recipe.lastSeenRef.map { String($0.prefix(8)) } ?? "Never",
                width: 96,
                identifier: true
            )
            ConsoleCell(
                text: lastRunText(recipe),
                width: 200,
                tone: recipe.lastRun?.status == "failed" ? .danger : .neutral
            )
            HStack {
                Spacer(minLength: 0)
                WisentActionButton(
                    action: WisentAction("Run now…", kind: .plain, isEnabled: !store.mutation.isWorking) {
                        decision = BuildDecision(kind: .run, recipe: recipe)
                    }
                )
            }
            .frame(width: 96)
        }
    }

    /// The switch shows what the registry records; flipping it asks first.
    /// The value on screen only changes when the CLI's answer comes back.
    private func enablementBinding(_ recipe: BuildRecipe) -> Binding<Bool> {
        Binding(
            get: { recipe.enabled },
            set: { enabled in
                decision = BuildDecision(kind: enabled ? .enable : .disable, recipe: recipe)
            }
        )
    }

    /// The status word the scheduler recorded, then the age. "failed" with no
    /// age would leave the operator asking "failed when?", which is the whole
    /// question on this screen.
    private func lastRunText(_ recipe: BuildRecipe) -> String {
        guard let run = recipe.lastRun else { return "Never ran" }
        guard let date = DisplayFormat.date(run.at) else {
            return run.at.isEmpty ? run.status : "\(run.status) \(run.at)"
        }
        return "\(run.status) \(ConsoleFormat.relative(date))"
    }

    private var empty: some View {
        VStack {
            WisentEmptyPanel(
                title: store.problem == nil ? "No build recipes" : "Build recipes are unknown",
                detail: store.problem
                    ?? "The registry declares nothing to build. Add a recipe with stado builds add --name NAME --repo URL --branch BRANCH --command CMD --artifact PATH; it starts disabled, and enabling it here is what makes the control plane poll it.",
                symbol: "hammer",
                action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary, isEnabled: !store.isRefreshing) {
                    Task { await store.refresh() }
                }
            )
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(WisentDesign.surface)
    }

    // MARK: The decision, before the write

    private func dialog(_ pending: BuildDecision) -> WisentDecisionDialog {
        let recipe = pending.recipe
        switch pending.kind {
        case .enable:
            return WisentDecisionDialog(
                tone: .warning,
                title: "Enable builds for \(recipe.name)?",
                lines: [
                    "The control plane starts polling \(recipe.repo) at \(recipe.ref) every \(recipe.intervalSeconds.formatted(.number)) seconds and enqueues one build job for every new commit it sees.",
                    "Each job clones the repository on a fleet host, runs the recipe's build command there, and uploads the declared artifacts under the job's results. Delivering a build to the fleet stays manual: stado host declare-version, then converge --apply.",
                ],
                listing: ["command: \(recipe.command)"] + recipe.artifacts.map { "artifact: \($0)" },
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
                title: "Enqueue a build of \(recipe.name) now?",
                lines: [
                    "One job is enqueued immediately, without waiting for the poller to see a new commit. It claims a worker slot, clones \(recipe.repo) at \(recipe.ref), runs the build command, and uploads the declared artifacts under the job's results.",
                    "The build produces artifacts only. Nothing is declared to the fleet and no host converges onto it.",
                ],
                listing: ["command: \(recipe.command)"] + recipe.artifacts.map { "artifact: \($0)" },
                footnote: "Runs \(StadoCLI.commandLine(BuildsStore.runArguments(name: recipe.name))).",
                actions: [
                    WisentAction("Not now", kind: .secondary) { decision = nil },
                    WisentAction("Enqueue the build", symbol: "hammer", kind: .primary) {
                        decision = nil
                        Task { await store.run(recipe) }
                    },
                ]
            )
        }
    }
}
