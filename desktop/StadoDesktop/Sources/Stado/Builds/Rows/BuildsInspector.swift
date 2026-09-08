import SwiftUI
import WisentDesignSystem

/// What an opened recipe discloses, under its own row.
///
/// `inspector` is internal rather than private only because `table` sits in
/// `BuildsTable.swift`: Swift scopes `private` to one file.
extension BuildsView {
    /// The recipe itself, under its own row: what each job runs, what it
    /// uploads, how often the poller looks, and the two verbs that change or
    /// remove the recipe.
    ///
    /// Change and Delete live here rather than in the row because the row's
    /// width is spent on what the fleet is doing right now. They also read in
    /// the right order: an operator opens a recipe to see what it builds, and
    /// the verb that rewrites it is one line under the answer.
    func inspector(_ recipe: BuildRecipe) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            HStack(alignment: .top, spacing: WisentDesign.Space.x5) {
                WisentField(label: "build command", value: recipe.command)
                WisentField(
                    label: "artifacts",
                    value: recipe.artifacts.isEmpty
                        ? "none — a build uploads nothing"
                        : recipe.artifacts.joined(separator: "\n"),
                    tone: recipe.artifacts.isEmpty ? .warning : .neutral
                )
                WisentField(label: "poll", value: "every \(recipe.intervalSeconds.formatted(.number))s")
            }
            HStack(spacing: WisentDesign.Space.x2) {
                WisentActionButton(
                    action: WisentAction(
                        "Change…",
                        symbol: "slider.horizontal.3",
                        isEnabled: !store.mutation.isWorking
                    ) {
                        editor = BuildRecipeEditor(original: recipe)
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Delete…",
                        symbol: "trash",
                        kind: .destructive,
                        isEnabled: !store.mutation.isWorking
                    ) {
                        decision = BuildDecision(kind: .remove, recipe: recipe)
                    }
                )
                Spacer(minLength:
                    0)
                Text("enable, disable and run stay on the row above")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.muted)
            }
        }
        .padding(.vertical, WisentDesign.Space.x3)
        .padding(.trailing, WisentDesign.Space.x4)
        .padding(.leading, Column.recipe + WisentDesign.Space.x4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(WisentDesign.canvasMuted.opacity(0.5))
        .overlay(alignment: .bottom) {
            Rectangle()
                .fill(WisentDesign.border.opacity(0.6))
                .frame(height: WisentDesign.hairline)
        }
    }
}
