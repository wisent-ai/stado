import SwiftUI
import WisentDesignSystem

/// The centre zone: the model-group table, the outcome table, the state pill
/// and the two empty states.
///
/// Internal rather than private only because `zones(_:)` sits in a sibling
/// file: Swift scopes `private` to one file.
extension QueueView {
    @ViewBuilder
    func table(_ snapshot: DashboardSnapshot) -> some View {
        if facet == .models {
            let rows = modelRecords(snapshot)
            if rows.isEmpty {
                emptyTable(
                    title: "No queued or running work",
                    detail: "The latest snapshot reports no model group with queued or running jobs."
                )
            } else {
                ConsoleTable(head: [
                    ConsoleHeaderCell("Model"),
                    ConsoleHeaderCell("Queued", width:
                        74, trailing: true),
                    ConsoleHeaderCell("Running", width:
                        74, trailing: true),
                    ConsoleHeaderCell("Completed", width:
                        84, trailing: true),
                    ConsoleHeaderCell("Failed", width:
                        68, trailing: true),
                ]) {
                    ForEach(rows) { row in
                        ConsoleTableRow(isSelected: selection == row.id, select: { selection = row.id }) {
                            ConsoleCell(text: row.model, identifier: true, strong: true)
                            ConsoleCell(text: row.counts.queue.formatted(.number), width:
                                74, trailing: true, digits: true, tone: row.counts.queue > 0 ? .warning : .neutral)
                            ConsoleCell(text: row.counts.running.formatted(.number), width:
                                74, trailing: true, digits: true, tone: row.counts.running > 0 ? .success : .neutral)
                            ConsoleCell(text: row.counts.completed.formatted(.number), width:
                                84, trailing: true, digits: true)
                            ConsoleCell(text: row.counts.failed.formatted(.number), width:
                                68, trailing: true, digits: true, tone: row.counts.failed > 0 ? .danger : .neutral)
                        }
                    }
                }
            }
        } else {
            let rows = records(snapshot)
            if rows.isEmpty {
                if facet == .allOutcomes {
                    emptyTable(
                        title: "No recent outcomes",
                        detail: "The dashboard has not published a completed or failed job in this snapshot."
                    )
                } else {
                    filteredEmptyTable
                }
            } else {
                let minority = minorityKind(in: rows)
                ConsoleTable(head: [
                    ConsoleHeaderCell("Job", width:
                        220),
                    ConsoleHeaderCell("Model"),
                    ConsoleHeaderCell("Task"),
                    ConsoleHeaderCell("Wall", width:
                        72, trailing: true),
                    ConsoleHeaderCell("State", width:
                        92, trailing: true),
                ]) {
                    ForEach(rows) { row in
                        ConsoleTableRow(isSelected: selection == row.id, select: { selection = row.id }) {
                            ConsoleCell(text: row.jobID, width:
                                220, identifier: true, strong: true)
                            ConsoleCell(text: row.model ?? "—")
                            ConsoleCell(text: row.task ?? "—")
                            ConsoleCell(text: StadoFormat.duration(row.wallSeconds), width:
                                72, trailing: true, digits: true)
                            stateCell(row, minority: minority)
                        }
                    }
                }
            }
        }
    }

    /// A pill only for the minority state. When every visible row completed,
    /// the count belongs in the facet rail and no row wears a badge.
    @ViewBuilder
    func stateCell(_ record: QueueRecord, minority: QueueRecord.Kind?) -> some View {
        if record.kind == minority {
            HStack {
                Spacer(minLength:
                    0)
                WisentStatusChip(text: record.kind.label, tone: record.kind.tone)
            }
            .frame(width:
                92)
        } else {
            ConsoleCell(text: "", width:
                92)
        }
    }

    func emptyTable(title: String, detail: String) -> some View {
        VStack {
            WisentEmptyPanel(title: title, detail: detail, symbol: "tray")
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(WisentDesign.surface)
    }

    /// Empty because a filter says so is a different state, with a different
    /// remedy, from empty because there is nothing.
    var filteredEmptyTable: some View {
        VStack {
            WisentEmptyPanel(
                title: "No rows in this filter",
                detail: "The snapshot has outcomes, but none of them match the selected facet.",
                symbol: "line.3.horizontal.decrease.circle",
                action: WisentAction("Clear filters", kind: .primary) { select(.allOutcomes) }
            )
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(WisentDesign.surface)
    }
}
