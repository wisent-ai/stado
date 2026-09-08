import SwiftUI
import WisentDesignSystem

/// One row per declared target: the three cleanup numbers the janitor reads,
/// the queue eligibility, and the mode.
///
/// `table` is internal rather than private only because `zones` sits in
/// `Registry/RegistryFacets.swift`: Swift scopes `private` to one file. The
/// mode cell below is read only from this file and stays private.
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
            let minority = minorityMode(in: rows)
            ConsoleTable(head: [
                ConsoleHeaderCell("Target", width:
                    220),
                ConsoleHeaderCell("Low free", width:
                    88, trailing: true),
                ConsoleHeaderCell("Target free", width:
                    96, trailing: true),
                ConsoleHeaderCell("Items / pass", width:
                    96, trailing: true),
                ConsoleHeaderCell("Queue", width:
                    128, trailing: true),
                ConsoleHeaderCell("Mode", width:
                    96, trailing: true),
            ]) {
                ForEach(rows) { target in
                    ConsoleTableRow(isSelected: selection == target.name, select: { selection = target.name }) {
                        ConsoleCell(text: target.name, width:
                            220, identifier: true, strong: true)
                        ConsoleCell(text: gigabytes(target.cleanup?.lowFreeGB), width:
                            88, trailing: true, digits: true)
                        ConsoleCell(text: gigabytes(target.cleanup?.targetFreeGB), width:
                            96, trailing: true, digits: true)
                        ConsoleCell(
                            text: target.cleanup?.maxItemsPerPass?.formatted(.number) ?? "—",
                            width:
                                96,
                            trailing: true,
                            digits: true
                        )
                        ConsoleCell(
                            text: target.pinnedOnly == true ? "Routed only" : "Open",
                            width:
                                128,
                            trailing: true
                        )
                        modeCell(target, minority: minority)
                    }
                }
            }
        }
    }

    /// The mode pill appears only where the mode is the minority; a fleet that
    /// is uniformly in report mode says so once, in the facet rail.
    @ViewBuilder
    private func modeCell(_ target: FleetPolicyTarget, minority: String?) -> some View {
        if let mode = target.cleanup?.mode, mode == minority {
            HStack {
                Spacer(minLength:
                    0)
                WisentStatusChip(text: mode.capitalized, tone: mode == FleetCleanupMode.enforce.rawValue ? .warning : .neutral)
            }
            .frame(width:
                96)
        } else {
            ConsoleCell(
                text: target.cleanup?.mode?.capitalized ?? "Not declared",
                width:
                    96,
                trailing: true
            )
        }
    }
}
