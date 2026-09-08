import SwiftUI
import WisentDesignSystem

/// What the rows read out of a recipe: the enablement binding, the expansion
/// flip, the tones, the three texts, and the panel that stands in when the
/// registry declares nothing.
///
/// All of these are internal rather than private only because the rows that
/// call them sit in `Rows/`: Swift scopes `private` to one file.
extension BuildsView {
    /// The switch shows what the registry records; flipping it asks first.
    /// The value on screen only changes when the CLI's answer comes back.
    func enablementBinding(_ recipe: BuildRecipe) -> Binding<Bool> {
        Binding(
            get: { recipe.enabled },
            set: { enabled in
                decision = BuildDecision(kind: enabled ? .enable : .disable, recipe: recipe)
            }
        )
    }

    func toggleExpansion(_ recipe: BuildRecipe) {
        if expanded.contains(recipe.id) {
            expanded.remove(recipe.id)
        } else {
            expanded.insert(recipe.id)
        }
    }

    func tone(for run: BuildRun?) -> WisentTone {
        switch run?.status {
        case "failed": .danger
        case "succeeded": .success
        case "running": .warning
        default: .neutral
        }
    }

    /// A platform with no run yet is muted, not toned: never having built is
    /// not a state worth a colour.
    func color(for run: BuildRun?) -> Color {
        run == nil ? WisentDesign.muted : tone(for: run).color
    }

    /// The age of the newest run across every platform, so the collapsed row
    /// still answers "did anything happen lately". "failed" with no age would
    /// leave the operator asking "failed when?", which is the whole question on
    /// this screen, and the per-platform rows below carry the rest.
    func latestText(_ recipe: BuildRecipe) -> String {
        guard let run = recipe.newestRun else { return "Never ran" }
        return atText(run)
    }

    /// The stamp the registry wrote, as an age. An unparseable stamp is shown
    /// verbatim: a console that silently drops a value it cannot read is worse
    /// than one that shows the registry's own string.
    func atText(_ run: BuildRun?) -> String {
        guard let run else { return "—" }
        guard let date = DisplayFormat.date(run.at) else {
            return run.at.isEmpty ? "unrecorded" : run.at
        }
        return ConsoleFormat.relative(date)
    }

    /// The tag the built commit carried, which is the only thing a build can
    /// declare. "untagged" is a conclusion; a run still in flight has not
    /// reached one.
    func versionText(_ run: BuildRun?) -> String {
        guard let run else { return "—" }
        if let version = run.version { return version }
        return run.status == "running" ? "—" : "untagged"
    }

    /// Empty because the registry declares nothing, or empty because it could
    /// not be read. The first is answered by authoring a recipe, the second by
    /// reading again — two states, two remedies, so the one button is the one
    /// that applies.
    var empty: some View {
        VStack {
            WisentEmptyPanel(
                title: store.problem == nil ? "No build recipes" : "Build recipes are unknown",
                detail: store.problem
                    ?? "The registry declares nothing to build. A recipe names a repository and branch to watch, the command each job runs in the checkout, the paths it uploads, and at least one platform — a build job can only be claimed by a worker that is that platform. A new recipe starts disabled; enabling it here is what makes the control plane poll it.",
                symbol: "hammer",
                action: store.problem == nil
                    ? WisentAction("New recipe…", symbol: "plus", kind: .primary) {
                        editor = BuildRecipeEditor(original: nil)
                    }
                    : WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary, isEnabled: !store.isRefreshing) {
                        Task { await store.refresh() }
                    }
            )
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(WisentDesign.surface)
    }
}
