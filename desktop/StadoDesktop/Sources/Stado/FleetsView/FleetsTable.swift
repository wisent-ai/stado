import SwiftUI
import WisentDesignSystem

/// The centre zone: the fleets the selected facet keeps, and the empty state
/// for a facet that keeps none.
///
/// Internal rather than private only because `zones` sits in a sibling file:
/// Swift scopes `private` to one file.
extension FleetsView {
    private var filteredFleets: [FleetGroup] {
        switch facet {
        case .all: groupStore.fleets
        case .withMembers: groupStore.fleets.filter { !$0.members.isEmpty }
        case .empty: groupStore.fleets.filter { $0.members.isEmpty }
        }
    }

    @ViewBuilder
    var table: some View {
        let rows = filteredFleets
        if rows.isEmpty {
            WisentEmptyPanel(
                title: "No fleets in this filter",
                detail: "Fleets exist, but none of them match the selected facet.",
                symbol: "line.3.horizontal.decrease.circle",
                action: WisentAction("Clear filters", kind: .primary) {
                    facet = .all
                    selection = nil
                }
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(WisentDesign.surface)
        } else {
            ConsoleTable(head: [
                ConsoleHeaderCell("Fleet", width:
                    180),
                ConsoleHeaderCell("Machines", width:
                    90, trailing: true),
                ConsoleHeaderCell("Notes"),
            ]) {
                ForEach(rows) { fleet in
                    ConsoleTableRow(isSelected: selection == fleet.id, select: { selection = fleet.id }) {
                        ConsoleCell(text: fleet.name, width:
                            180, identifier: true, strong: true)
                        ConsoleCell(
                            text: fleet.members.count.formatted(.number),
                            width:
                                90,
                            trailing: true,
                            digits: true
                        )
                        ConsoleCell(text: fleet.notes.isEmpty ? "—" : fleet.notes)
                    }
                }
            }
        }
    }
}
