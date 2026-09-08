import SwiftUI
import WisentDesignSystem

extension HostsView {
    @ViewBuilder
    func table(_ snapshot: DashboardSnapshot) -> some View {
        let rows = hosts(snapshot)
        if rows.isEmpty {
            VStack {
                if facet == .all {
                    WisentEmptyPanel(
                        title: "No registered hosts",
                        detail: "The Stado registry and capacity store exposed no host in this snapshot. A fleet starts with one machine: name it, put a minted key on it, and enroll it.",
                        symbol: "server.rack",
                        action: addMachineAction(kind: .primary)
                    )
                } else {
                    WisentEmptyPanel(
                        title: "No hosts in this filter",
                        detail: "Hosts exist in this snapshot, but none of them match the selected facet.",
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
            let minority = minorityStatus(in: rows)
            // Claiming, and the reason it is not, come before hardware: the
            // hardware of a host that takes no work is not what the operator
            // needs. Kind, GPU, role and VRAM are one selection away, in the
            // inspector.
            //
            // The beacon age gets no column of its own. Seven columns already
            // ask 706 pt of fixed width plus a flexible reason column, against
            // the 556 pt this middle zone has at the 1280 pt default, so the
            // reason column is squeezed as it stands. A beacon age is also
            // identical and unremarkable on every healthy row — the ink that
            // makes the one exception harder to find, not easier. Silence is
            // named in the alarm above the table, counted in the Link facets,
            // and read in the inspector's Link section.
            ConsoleTable(head: [
                ConsoleHeaderCell("Host", width: 200),
                ConsoleHeaderCell("Claiming", width: 92),
                ConsoleHeaderCell("Why not"),
                ConsoleHeaderCell("Free disk", width: 136, trailing: true),
                ConsoleHeaderCell("CPU free", width: 72, trailing: true),
                ConsoleHeaderCell("Reported", width: 112, trailing: true),
                ConsoleHeaderCell("State", width: 104, trailing: true),
            ]) {
                ForEach(rows) { host in
                    ConsoleTableRow(isSelected: selection == host.id, select: { selection = host.id }) {
                        ConsoleCell(text: host.displayName, width: 200, identifier: true, strong: true)
                        ConsoleCell(
                            text: claimingLabel(host),
                            width: 92,
                            tone: claimingTone(host),
                            strong: claimingTone(host) == .danger
                        )
                        ConsoleCell(text: gateReason(host), tone: claimingTone(host) == .danger ? .danger : .neutral)
                        ConsoleCell(
                            text: freeDisk(host),
                            width: 136,
                            trailing: true,
                            digits: true,
                            tone: diskTone(host)
                        )
                        ConsoleCell(
                            text: cpuCell(host),
                            width: 72,
                            trailing: true,
                            digits: true
                        )
                        ConsoleCell(
                            text: ConsoleFormat.age(reportAge(host)),
                            width: 112,
                            trailing: true,
                            digits: true
                        )
                        stateCell(host, minority: minority)
                    }
                }
            }
        }
    }

    /// The chip marks the minority. On a fleet where every host is live, the
    /// live count lives in the facet rail and no row carries a badge.
    @ViewBuilder
    private func stateCell(_ host: WorkerNode, minority: WorkerAvailability?) -> some View {
        if host.status == minority {
            HStack {
                Spacer(minLength: 0)
                WisentStatusChip(text: label(for: host.status), tone: tone(for: host.status))
            }
            .frame(width: 104)
        } else {
            ConsoleCell(text: "", width: 104)
        }
    }
}
