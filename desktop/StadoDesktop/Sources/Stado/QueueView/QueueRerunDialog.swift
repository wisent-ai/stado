import SwiftUI
import WisentDesignSystem

/// The confirmation a rerun has to pass: it spends real capacity and it does
/// not clear the failure it was opened from.
///
/// Internal rather than private only because the sheet is attached in
/// `QueueView.body`, in a sibling file: Swift scopes `private` to one file.
extension QueueView {
    // MARK: Irreversible decision

    func rerunDialog(_ record: QueueRecord) -> some View {
        WisentDecisionDialog(
            tone: .warning,
            title: "Resubmit job \(record.jobID)?",
            lines: [
                "Stado resubmits the exact recorded specification for this job. A worker admits it when live CPU, memory, disk, and accelerator capacity allow, and the provider that runs it is billed.",
                "The failed record stays in the dashboard's recent-failure list; a rerun does not clear it.",
            ],
            reasonCode: record.kind == .failed ? "job_failed" : nil,
            listing: (record.error ?? "The dashboard published this failure without a sanitized reason.")
                .split(separator: "\n", omittingEmptySubsequences: false)
                .map(String.init),
            footnote: "Runs stado job rerun \(record.jobID) through the dashboard's allowlisted command bridge, with the mutation confirmation it requires.",
            actions: [
                WisentAction("Keep the failure only", kind: .primary) { rerunCandidate = nil },
                WisentAction("Rerun job", symbol: "arrow.clockwise", kind: .destructive) {
                    let jobID = record.jobID
                    rerunCandidate = nil
                    Task { await fleetStore.rerunJob(jobID) }
                },
            ]
        )
    }
}
