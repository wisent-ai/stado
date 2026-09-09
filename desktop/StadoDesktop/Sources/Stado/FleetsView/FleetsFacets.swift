import SwiftUI
import WisentDesignSystem

/// The one filter the screen holds — every fleet, the fleets with machines,
/// the empty ones — and the three zones the facet rail sits in.
///
/// The enum and the members below are internal rather than private only
/// because the stored facet lives on `FleetsView` in `FleetsView.swift` and
/// the table that filters by it sits in `FleetsView/FleetsTable.swift`:
/// Swift scopes `private` to one file.
enum FleetFacet: String, Hashable {
    case all
    case withMembers
    case empty
}

extension FleetsView {
    // MARK: Three zones

    var zones: some View {
        HStack(spacing:
            0) {
            WisentFacetRail(
                groups: [
                    WisentFacetGroup(
                        "Fleets",
                        facets: [
                            facetRow(.all, "All fleets", groupStore.fleets.count, .neutral),
                            facetRow(
                                .withMembers,
                                "With machines",
                                groupStore.fleets.count { !$0.members.isEmpty },
                                .neutral
                            ),
                            facetRow(
                                .empty,
                                "Empty",
                                groupStore.fleets.count { $0.members.isEmpty },
                                .neutral
                            ),
                        ]
                    )
                ]
            )
            table
            inspector
        }
        .frame(maxHeight: .infinity)
    }

    private func facetRow(_ value: FleetFacet, _ label: String, _ count: Int, _ tone: WisentTone) -> WisentFacet {
        WisentFacet(
            id: value.rawValue,
            label: label,
            count: count,
            tone: tone,
            isSelected: facet == value,
            select: {
                facet = value
                selection = nil
            }
        )
    }
}
