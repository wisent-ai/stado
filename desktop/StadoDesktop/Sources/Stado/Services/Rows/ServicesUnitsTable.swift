import SwiftUI
import WisentDesignSystem

/// The declared units, and the flag on the ones serving replaced code.
///
/// `unitsTable` is internal rather than private only because the facet
/// dispatch sits in `ServicesFleetTable.swift`: Swift scopes `private` to one
/// file.
extension ServicesView {
    @ViewBuilder
    var unitsTable: some View {
        let rows = facet == .replaced ? store.mismatched : store.units
        if rows.isEmpty {
            emptyUnits
        } else {
            ConsoleTable(head: [
                ConsoleHeaderCell("Host", width:
                    150),
                ConsoleHeaderCell("Unit", width:
                    190),
                ConsoleHeaderCell("Binary", width:
                    130),
                ConsoleHeaderCell("State", width:
                    92),
                ConsoleHeaderCell("Declared program"),
                ConsoleHeaderCell("Running binary"),
                ConsoleHeaderCell("Match", width:
                    96, trailing: true),
            ]) {
                ForEach(rows) { row in
                    ConsoleTableRow(isSelected: selection == row.id, select: { selection = row.id }) {
                        ConsoleCell(text: row.host, width:
                            150, identifier: true, strong: true)
                        ConsoleCell(text: value(row.unit.unit), width:
                            190, identifier: true)
                        ConsoleCell(text: value(row.unit.binary), width:
                            130, identifier: true)
                        ConsoleCell(text: value(row.unit.state), width:
                            92)
                        ConsoleCell(text: value(row.unit.root), identifier: true)
                        ConsoleCell(
                            text: row.unit.runningBinary ?? "Not reported",
                            identifier: true,
                            tone: row.unit.servesReplacedCode ? .danger : .neutral
                        )
                        matchCell(row.unit)
                    }
                }
            }
        }
    }

    /// The flag, and only where it means something: a unit whose process
    /// matches its program gets no pill, so the ones that do not stand out.
    @ViewBuilder
    private func matchCell(_ unit: ServiceUnit) -> some View {
        HStack {
            Spacer(minLength:
                0
            )
            if unit.servesReplacedCode {
                WisentStatusChip(text: "Replaced", tone: .danger)
            } else if unit.binaryMatchesProcess == nil {
                WisentStatusChip(text: "Unknown", tone: .warning)
            } else {
                ConsoleCell(text: "", width:
                    0)
            }
        }
        .frame(width:
            96
        )
    }

    @ViewBuilder
    private var emptyUnits: some View {
        VStack {
            if hosts.isEmpty {
                WisentEmptyPanel(
                    title: "No registry hosts to ask",
                    detail: "The canonical registry projection lists no target, so no host was asked what it runs. Nothing here is inferred from local configuration.",
                    symbol: "gearshape.2"
                )
            } else if facet == .replaced {
                WisentEmptyPanel(
                    title: "Every process is running the code on disk",
                    detail: "For each declared unit the host reported, the path the process is executing is the program under the directory the unit declares.",
                    symbol: "checkmark.seal",
                    action: WisentAction("All units", kind: .primary) {
                        facet = .units
                        selection = nil
                    }
                )
            } else if store.failures.count == hosts.count {
                WisentEmptyPanel(
                    title: "No host reported its units",
                    detail: "Every stado service converge invocation failed; the reasons are quoted above, in the words the command used.",
                    symbol: "gearshape.2",
                    action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary) {
                        Task { await store.refresh(hosts: hosts) }
                    }
                )
            } else {
                WisentEmptyPanel(
                    title: "No declared units",
                    detail: "The hosts answered, and the registry declares no managed unit on any of them. A product nobody declared is a product nothing supervises.",
                    symbol: "gearshape.2"
                )
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(WisentDesign.surface)
    }
}
