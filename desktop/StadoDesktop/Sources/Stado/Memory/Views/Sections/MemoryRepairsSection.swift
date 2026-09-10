import SwiftUI
import WisentDesignSystem

/// Every declared repair, by name, with the subjects it named and what the
/// last pass did with them.
///
/// Reports the last observed state. The declaration editor below this section
/// can change the same repairs and subjects as the CLI, through a reviewed write.
struct MemoryRepairsSection: View {
    let state: MemoryPolicyState

    var body: some View {
        WisentSectionBox(
            title: "Repairs",
            detail: "Declared repairs and their last observed result. Use Change the declaration below to add, remove or edit repairs and their subjects.",
            trailing: trailingLabel
        ) {
            if rows.isEmpty {
                WisentPanel {
                    Text(emptyDetail)
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            } else {
                WisentTableFrame {
                    VStack(spacing: .zero) {
                        ConsoleTableHead(cells: [
                            ConsoleHeaderCell("Repair", width: MemoryColumnWidth.repair),
                            ConsoleHeaderCell("Examined", width: MemoryColumnWidth.tally, trailing: true),
                            ConsoleHeaderCell("Eligible", width: MemoryColumnWidth.tally, trailing: true),
                            ConsoleHeaderCell("Repaired", width: MemoryColumnWidth.tally, trailing: true),
                            ConsoleHeaderCell("Subjects"),
                        ])
                        ForEach(rows, id: \.name) { row in
                            ConsoleTableRow {
                                ConsoleCell(text: row.name, width: MemoryColumnWidth.repair, strong: true)
                                ConsoleCell(
                                    text: tally(row.report?.examined),
                                    width: MemoryColumnWidth.tally,
                                    trailing: true,
                                    digits: true
                                )
                                ConsoleCell(
                                    text: tally(row.report?.eligible),
                                    width: MemoryColumnWidth.tally,
                                    trailing: true,
                                    digits: true
                                )
                                ConsoleCell(
                                    text: tally(row.report?.repaired),
                                    width: MemoryColumnWidth.tally,
                                    trailing: true,
                                    digits: true
                                )
                                ConsoleCell(text: subjectLabel(row), identifier: true)
                            }
                        }
                    }
                }
            }
        }

        ForEach(rows, id: \.name) { row in
            if let report = row.report, !report.skipped.isEmpty {
                WisentSectionBox(
                    title: "Skipped: \(row.name)",
                    detail: "Exact reasons and counts returned by this repair."
                ) {
                    WisentPanel {
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                            ForEach(report.sortedSkipped, id: \.0) { reason in
                                HStack(alignment: .firstTextBaseline) {
                                    Text(reason.0)
                                        .font(WisentTypeScale.identifier())
                                        .textSelection(.enabled)
                                        .fixedSize(horizontal: false, vertical: true)
                                    Spacer()
                                    Text(reason.1.formatted(.number))
                                        .monospacedDigit()
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// One row per repair this host has anything to say about: what the
    /// registry declares, and what the pass reported on it.
    struct Row: Identifiable {
        let name: String
        let report: MemoryRepairReport?
        let declared: FleetMemoryRepairPolicy?

        var id: String { name }
    }

    var rows: [Row] {
        let reported = state.report?.repairs ?? [:]
        let declared = state.declared?.repairs ?? [:]
        let names = Set(reported.keys).union(declared.keys).sorted()
        return names.map { name in
            Row(name: name, report: reported[name], declared: declared[name])
        }
    }

    private var trailingLabel: String? {
        guard let report = state.report else { return nil }
        if !report.examinedRepairs {
            return "the last pass never reached its repairs"
        }
        return state.repairsArmed ? "armed by enforce mode" : "declared, not armed"
    }

    private var emptyDetail: String {
        if state.isDefaulted {
            return "This target declares no repair. Add repair declarations in the editor below; the reporting default performs none."
        }
        return "This target's memory_reclaim declares an empty repair map, so its passes read the host's memory and change nothing."
    }

    private func tally(_ value: Int?) -> String {
        value.map { $0.formatted(.number) } ?? "—"
    }

    private func subjectLabel(_ row: Row) -> String {
        let reported = row.report?.subjects ?? []
        let declared = row.declared?.declaredSubjects ?? []
        let subjects = reported.isEmpty ? declared : reported
        return subjects.isEmpty ? "No subject declared" : subjects.joined(separator: ", ")
    }
}
