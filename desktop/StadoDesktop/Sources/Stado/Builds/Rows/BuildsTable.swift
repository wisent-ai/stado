import SwiftUI
import WisentDesignSystem

/// The recipe table and the recipe row it repeats.
///
/// `Column` and `table` are internal rather than private only because the
/// screen that draws the table sits in `BuildsView.swift` and the parts the
/// columns measure sit beside this file: Swift scopes `private` to one file.
/// `row` keeps its `private`, because `table` is its only caller and they
/// share this one.
extension BuildsView {
    // MARK: Rows

    /// The recipe row's columns. `recipe` is also the width of the gutter the
    /// platform rows indent past, which is what makes an expanded recipe read
    /// as one block instead of two tables.
    enum Column {
        static let recipe: CGFloat = 148
        static let enabled: CGFloat = 56
        static let seen: CGFloat = 76
        static let platforms: CGFloat = 200
        static let latest: CGFloat = 92
        static let declare: CGFloat = 68
        static let action: CGFloat = 88
    }

    @ViewBuilder
    var table: some View {
        if store.recipes.isEmpty {
            empty
        } else {
            ConsoleTable(head: [
                ConsoleHeaderCell("Recipe", width: Column.recipe),
                ConsoleHeaderCell("Repository"),
                ConsoleHeaderCell("On", width: Column.enabled),
                ConsoleHeaderCell("Last seen", width: Column.seen),
                ConsoleHeaderCell("Platforms", width: Column.platforms),
                ConsoleHeaderCell("Latest", width: Column.latest),
                ConsoleHeaderCell("Declare", width: Column.declare),
                ConsoleHeaderCell("", width: Column.action, trailing: true),
            ]) {
                ForEach(store.recipes) { recipe in
                    row(recipe)
                    if expanded.contains(recipe.id) {
                        inspector(recipe)
                        ForEach(recipe.platformRuns) { entry in
                            platformRow(entry, of: recipe)
                        }
                    }
                }
            }
        }
    }

    /// One recipe. The disclosure is its own button rather than the whole row:
    /// a row-wide button folds the enablement switch and the Run button into a
    /// single accessibility element, and both of those are writes an operator
    /// must be able to reach without a mouse. Every recipe discloses something
    /// — what it builds and how, plus the two verbs that change or remove it —
    /// so the arrow is always there and always opens onto an answer.
    private func row(_ recipe: BuildRecipe) -> some View {
        let isOpen = expanded.contains(recipe.id)
        return ConsoleTableRow(isSelected: isOpen) {
            HStack(spacing: WisentDesign.Space.x1) {
                Button { toggleExpansion(recipe) } label: {
                    Image(systemName: isOpen ? "chevron.down" : "chevron.right")
                        .font(.system(size:
                            8, weight: .semibold))
                        .foregroundStyle(WisentDesign.muted)
                        .frame(width:
                            14, height: WisentAppLayout.tableRowHeight)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(
                    isOpen
                        ? "Hide what \(recipe.name) builds"
                        : "Show what \(recipe.name) builds"
                )
                ConsoleCell(text: recipe.name, identifier: true, strong: true)
            }
            .frame(width: Column.recipe, alignment: .leading)
            ConsoleCell(text: "\(recipe.repo)@\(recipe.ref)", identifier: true)
            Toggle(recipe.enabled ? "Disable \(recipe.name)" : "Enable \(recipe.name)", isOn: enablementBinding(recipe))
                .labelsHidden()
                .toggleStyle(.switch)
                .controlSize(.mini)
                .disabled(store.mutation.isWorking)
                .frame(width: Column.enabled, alignment: .leading)
            ConsoleCell(
                text: recipe.lastSeenRef.map { String($0.prefix(8)) } ?? "Never",
                width: Column.seen,
                identifier: true
            )
            platformStrip(recipe)
            ConsoleCell(
                text: latestText(recipe),
                width: Column.latest,
                tone: recipe.hasFailedRun ? .danger : .neutral
            )
            ConsoleCell(
                text: recipe.autoDeclare ? "auto" : "manual",
                width: Column.declare,
                identifier: true,
                tone: recipe.autoDeclare ? .warning : .neutral
            )
            HStack {
                Spacer(minLength:
                    0)
                WisentActionButton(
                    action: WisentAction("Run now…", kind: .plain, isEnabled: !store.mutation.isWorking) {
                        decision = BuildDecision(
                            kind: .run,
                            recipe: recipe,
                            runID: store.retainedRunID(for: recipe.name)
                        )
                    }
                )
            }
            .frame(width: Column.action)
        }
    }
}
