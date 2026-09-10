import SwiftUI
import WisentDesignSystem

/// The pending flip, enqueue or removal, held until the operator confirms it.
/// A build command is arbitrary code on a fleet host and a removal is a
/// registry write, so neither the toggle, the run button nor the delete button
/// writes anything by itself.
struct BuildDecision: Identifiable {
    enum Kind: String {
        case enable
        case disable
        case run
        case remove
    }

    let kind: Kind
    let recipe: BuildRecipe
    /// The caller-retained `--run-id` an enqueue confirmation carries, so the
    /// command the operator is shown is the command that runs.
    let runID: String

    init(kind: Kind, recipe: BuildRecipe, runID: String = "") {
        self.kind = kind
        self.recipe = recipe
        self.runID = runID
    }

    var id: String { "\(kind.rawValue)/\(recipe.name)" }
}

/// What the recipe form is authoring: a recipe that does not exist yet, or the
/// one it was opened from.
///
/// The identity of the add form is a string no recipe can be named — the CLI
/// takes kebab-case only — so opening the form on a recipe and opening it on
/// nothing are never the same sheet.
struct BuildRecipeEditor: Identifiable {
    let original: BuildRecipe?

    var id: String { original?.name ?? "+new" }
}

/// Which repositories the control plane watches, what each platform's last
/// build produced, and which version came out of it.
///
/// The boundary, stated where the operator acts on it: a build ends at
/// artifacts under the job's results plus the version the built commit's tag
/// names. A recipe with auto-declare on writes that version into the
/// managed versions of the hosts on the run's platform — nothing more.
/// Promoting a signed release stays a separate, deliberate step
/// (`stado release promote`, which verifies the manifest and its signature),
/// and delivering it stays `converge --apply`.
///
/// The screen's own parts live in `Builds/`: the table, the recipe row, the
/// inspector and the platform rows in `Builds/Rows/`, the values those rows
/// read in `Builds/BuildsRowValues.swift`, the four confirmations in
/// `Builds/BuildsDialogs.swift`, and the recipe form in `Builds/Form/`.
/// `BuildDecision`, `BuildRecipeEditor` and the three `@State` properties
/// below are internal rather than private only because those files hold the
/// buttons that set them: Swift scopes `private` to one file.
struct BuildsView: View {
    @ObservedObject var store: BuildsStore
    let scope: String

    @State var decision: BuildDecision?
    /// The recipe form, open on a new recipe or on an existing one. It is its
    /// own sheet rather than a case of `decision`, because the form confirms
    /// its own change: swapping one sheet's subject mid-flight gives AppKit two
    /// presentations to arbitrate over one binding.
    @State var editor: BuildRecipeEditor?
    /// Which recipes have their per-platform runs open. Expansion is additive
    /// and remembered across refreshes: a refresh must not close the rows the
    /// operator opened to watch a build.
    @State var expanded: Set<String> = []

    var body: some View {
        WisentScreen(
            title: "Builds",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(store.lastUpdated))",
            actions: [
                WisentAction("New recipe…", symbol: "plus", kind: .primary) {
                    editor = BuildRecipeEditor(original: nil)
                },
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task { await store.refresh() }
                },
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing:
                0) {
                if store.lastUpdated == nil, store.isRefreshing {
                    WisentLoadingPanel(
                        title: "Reading build recipes",
                        detail: "stado builds list --json against the canonical registry. Nothing is written."
                    )
                    .padding(WisentDesign.Space.x6)
                    Spacer(minLength:
                        0)
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
        .sheet(item: $editor) { pending in
            BuildRecipeFormView(
                original: pending.original,
                taken: Set(store.recipes.map(\.name)),
                submit: { outcome in
                    editor = nil
                    switch outcome {
                    case let .add(draft):
                        Task { await store.add(draft) }
                    case let .change(change):
                        Task { await store.edit(change) }
                    }
                },
                cancel: { editor = nil }
            )
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
}
