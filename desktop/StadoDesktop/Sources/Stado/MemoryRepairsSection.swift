import SwiftUI
import WisentDesignSystem

/// Every declared repair, by name, with the subjects it named and what the
/// last pass did with them.
///
/// Read-only, and it says so on the screen. A repair authorizes a restart of
/// a named unit or the termination of a person's session process; arming one
/// belongs in the registry declaration, where the subject list is declared
/// beside it, not behind a switch in a console that cannot show what that
/// process is doing right now.
struct MemoryRepairsSection: View {
    let state: MemoryPolicyState

    var body: some View {
        WisentSectionBox(
            title: "Repairs",
            detail: "Declared in the canonical registry and read-only here: this console can change the mode, the watermarks, the per-pass budget and the placement refusal, and it never arms or disarms an individual repair.",
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
            return "This target declares no repair, so no repair exists to control. The reporting default arms none on purpose: no memory repair is reversible, and a host that declares an interest in its memory has not authorized a restart of anything. Declare a repair in the canonical registry to change that."
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
