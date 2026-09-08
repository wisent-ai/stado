import SwiftUI
import WisentDesignSystem

/// One row per product target: the verdict, the two versions side by side, the
/// host's own software word, and the row's own sentence.
///
/// `table` is internal rather than private only because `body` sits in
/// `ReleasesView.swift`: Swift scopes `private` to one file. Every cell and
/// every tone below is read only from this file and stays private.
extension ReleasesView {
    // MARK: Table

    var table: some View {
        ConsoleTable(head: [
            ConsoleHeaderCell("Verdict", width:
                82),
            ConsoleHeaderCell("Product", width:
                130),
            ConsoleHeaderCell("Target", width:
                140),
            ConsoleHeaderCell("Desired", width:
                100),
            ConsoleHeaderCell("Observed", width:
                100),
            ConsoleHeaderCell("Software", width:
                96),
            ConsoleHeaderCell("Phase", width:
                92),
            ConsoleHeaderCell("Detail"),
            ConsoleHeaderCell("Quarantined", width:
                88, trailing: true),
        ]) {
            ForEach(store.rows) { row in
                ConsoleTableRow(
                    isSelected: selection == row.pair,
                    select: { selection = row.pair }
                ) {
                    verdictCell(row)
                    ConsoleCell(text: row.product, width:
                        130, identifier: true, strong: true)
                    ConsoleCell(text: row.target, width:
                        140, identifier: true)
                    ConsoleCell(
                        text: row.report?.desiredVersion ?? "—",
                        width:
                            100,
                        identifier: true,
                        strong: true
                    )
                    ConsoleCell(
                        text: row.report?.observedVersion ?? (row.report == nil ? "—" : "none"),
                        width:
                            100,
                        identifier: true,
                        tone: observedTone(row)
                    )
                    softwareCell(row)
                    ConsoleCell(text: row.report?.phase ?? "—", width:
                        92)
                    ConsoleCell(text: rowDetail(row), tone: rowDetailTone(row))
                    ConsoleCell(
                        text: row.report.map { "\($0.quarantined.count)" } ?? "—",
                        width:
                            88,
                        trailing: true,
                        digits: true,
                        tone: (row.report?.quarantined.isEmpty == false) ? .warning : .neutral
                    )
                }
            }
        }
    }

    @ViewBuilder
    private func verdictCell(_ row: ReleaseRow) -> some View {
        HStack(spacing:
            0
        ) {
            switch row.diagnosis {
            case .pending:
                ConsoleCell(text: "reading…", width:
                    82, tone: .neutral)
            case let .diagnosed(report):
                WisentStatusChip(text: report.verdict.word, tone: tone(for: report.verdict))
            case .failed:
                WisentStatusChip(text: "no answer", tone: .danger)
            }
        }
        .frame(width:
            82, alignment: .leading)
    }

    /// Whether this host can be shown to run what the fleet declares for it.
    ///
    /// The CLI's own word, never a translation of it, and never blank. An empty
    /// cell reads as "fine" to every operator alive, and this column exists
    /// because a host that said nothing used to read exactly that way.
    @ViewBuilder
    private func softwareCell(_ row: ReleaseRow) -> some View {
        HStack(spacing:
            0
        ) {
            if let software = row.software {
                WisentStatusChip(
                    text: software.hasReport ? software.verdict : "never",
                    tone: software.failed ? .danger : .success
                )
            } else {
                // The CLI answered without a software block, so this console has
                // no verdict to show — which is not the same as a passing one.
                WisentStatusChip(text: "unreported", tone: .warning)
            }
        }
        .frame(width:
            96, alignment: .leading)
    }

    /// The row's own sentence. A blocked rollout shows its blockers verbatim,
    /// because the blocker is the reason and the phase detail beside it is
    /// usually the symptom. A software finding outranks the phase detail for the
    /// same reason: a phase is about the rollout, and the finding is about what
    /// the machine is running right now.
    private func rowDetail(_ row: ReleaseRow) -> String {
        if let problem = row.problem { return problem }
        if let report = row.report, !report.blockers.isEmpty {
            return report.blockers.joined(separator: ", ")
        }
        if let finding = row.software?.findings.first {
            return finding
        }
        guard let report = row.report else { return "waiting for the host" }
        return report.detail.isEmpty ? "—" : report.detail
    }

    private func rowDetailTone(_ row: ReleaseRow) -> WisentTone {
        if row.problem != nil { return .danger }
        if row.report?.blockers.isEmpty == false { return .danger }
        if row.software?.failed == true { return .danger }
        return .neutral
    }

    private func observedTone(_ row: ReleaseRow) -> WisentTone {
        guard let report = row.report else { return .neutral }
        if report.observedVersion == nil { return .warning }
        return report.isConverged ? .neutral : .warning
    }

    private func tone(for verdict: ReleaseVerdict) -> WisentTone {
        switch verdict {
        case .settled: .success
        case .rolling: .info
        case .blocked: .danger
        case .unrecognised: .warning
        }
    }
}
