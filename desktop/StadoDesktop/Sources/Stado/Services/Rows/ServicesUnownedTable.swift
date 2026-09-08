import SwiftUI
import WisentDesignSystem

/// The processes no declared unit owns.
///
/// `unownedTable` is internal rather than private only because the facet
/// dispatch sits in `ServicesFleetTable.swift`: Swift scopes `private` to one
/// file.
extension ServicesView {
    @ViewBuilder
    var unownedTable: some View {
        if store.unownedProcesses.isEmpty {
            VStack {
                WisentEmptyPanel(
                    title: store.unownedProblem == nil
                        ? "Every product process belongs to a unit"
                        : "Unowned processes are unknown",
                    detail: store.unownedProblem
                        ?? "stado service list --unowned found no product process running outside a declared unit, so everything running is something the fleet can restart.",
                    symbol: "questionmark.circle",
                    action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary, isEnabled: !store.isRefreshing) {
                        Task { await store.refresh(hosts: hosts) }
                    }
                )
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(WisentDesign.surface)
        } else {
            VStack(spacing:
                0
            ) {
                Text("No declared unit owns these processes. Nothing updates them, nothing restarts them after they die, and nothing stops them on a release: they run until somebody signs in and ends them by hand.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(WisentDesign.Space.x4)
                    .background(WisentDesign.canvasMuted)
                ConsoleTable(head: [
                    ConsoleHeaderCell("Host", width:
                        150),
                    ConsoleHeaderCell("PID", width:
                        78, trailing: true),
                    ConsoleHeaderCell("Running for", width:
                        108, trailing: true),
                    ConsoleHeaderCell("Started", width:
                        190),
                    ConsoleHeaderCell("Product guess", width:
                        150),
                    ConsoleHeaderCell("Command"),
                ]) {
                    ForEach(store.unownedProcesses) { process in
                        ConsoleTableRow(
                            isSelected: selection == process.id,
                            select: { selection = process.id }
                        ) {
                            ConsoleCell(text: process.host, width:
                                150, identifier: true, strong: true)
                            ConsoleCell(
                                text: value(process.pid),
                                width:
                                    78,
                                trailing: true,
                                identifier: true,
                                digits: true
                            )
                            // The age when the host's stamp parsed, and the
                            // stamp itself beside it either way: four days is
                            // the fact that mattered, and an unparsed stamp
                            // must not read as a process with no age.
                            ConsoleCell(
                                text: process.age == nil ? "—" : StadoFormat.duration(process.age),
                                width:
                                    108,
                                trailing: true,
                                digits: true,
                                tone: isLongLived(process) ? .warning : .neutral
                            )
                            ConsoleCell(
                                text: process.startedAt ?? "Not reported",
                                width:
                                    190,
                                identifier: true,
                                tone: isLongLived(process) ? .warning : .neutral
                            )
                            ConsoleCell(text: process.productGuess ?? "No guess", width:
                                150)
                            ConsoleCell(text: value(process.command), identifier: true)
                        }
                    }
                }
            }
        }
    }
}
