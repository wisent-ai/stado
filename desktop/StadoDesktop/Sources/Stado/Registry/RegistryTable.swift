import SwiftUI
import WisentDesignSystem

/// One row per declared target: its work root and its queue eligibility.
///
/// `table` is internal rather than private only because `zones` sits in
/// `Registry/RegistryFacets.swift`: Swift scopes `private` to one file.
extension RegistryView {
    @ViewBuilder
    var table: some View {
        let rows = targets
        if rows.isEmpty {
            VStack {
                if facet == .all {
                    WisentEmptyPanel(
                        title: "No declared targets",
                        detail: "The canonical registry projection contains no target for this fleet.",
                        symbol: "book.closed"
                    )
                } else {
                    WisentEmptyPanel(
                        title: "No targets in this filter",
                        detail: "Targets exist in this projection, but none of them match the selected facet.",
                        symbol: "line.3.horizontal.decrease.circle",
                        action: WisentAction("Clear filters", kind: .primary) {
                            facet = .all
                            selection = nil
                        }
                    )
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(WisentDesign.surface)
        } else {
            ConsoleTable(head: [
                ConsoleHeaderCell("Target", width:
                    220),
                ConsoleHeaderCell("Work root", width:
                    260),
                ConsoleHeaderCell("Queue", width:
                    128, trailing: true),
            ]) {
                ForEach(rows) { target in
                    ConsoleTableRow(isSelected: selection == target.name, select: { selection = target.name }) {
                        ConsoleCell(text: target.name, width:
                            220, identifier: true, strong: true)
                        ConsoleCell(text: target.workRoot ?? "home", width:
                            260, identifier: true)
                        ConsoleCell(
                            text: target.pinnedOnly == true ? "Routed only" : "Open",
                            width:
                                128,
                            trailing: true
                        )
                    }
                }
            }
        }
    }
}
