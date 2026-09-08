import SwiftUI
import WisentDesignSystem

/// The one place a write can start, and the review it has to pass first.
///
/// Reading status runs straight away; every other phase opens the confirmation
/// dialog, which quotes the equivalent CLI invocation and says plainly that an
/// accepted response is not completion and that nothing follows automatically.
///
/// `footer` and `confirmation` are internal rather than private because `body`
/// sits in `../../StorageReconciliation.swift`, and Swift scopes `private` to
/// one file.
extension StorageReconciliationSheet {
    var footer: some View {
        HStack(spacing: WisentDesign.Space.x2) {
            WisentActionButton(
                action: WisentAction("Close", kind: .plain, isEnabled: !store.isRunning) {
                    dismiss()
                }
            )
            Spacer(minLength:
                0)
            WisentActionButton(
                action: WisentAction(
                    phase == .status ? "Read status" : "Review \(phase.title.lowercased())",
                    symbol: phase == .status ? "doc.text.magnifyingglass" : "externaldrive",
                    kind: phase == .status ? .primary : .destructive,
                    isEnabled: !store.isRunning && StorageReconciliationStore.transactionProblem(selectedTransaction) == nil
                ) {
                    if phase.isReadOnly {
                        let transaction = selectedTransaction
                        let requested = phase
                        Task { await store.invoke(requested, host: host, transaction: transaction, at: address) }
                    } else {
                        pendingPhase = phase
                    }
                }
            )
        }
    }

    func confirmation(for requested: StorageReconciliationPhase) -> WisentDecisionDialog {
        let transaction = selectedTransaction
        let arguments = StorageReconciliationStore.arguments(
            host: host,
            transaction: transaction,
            phase: requested
        )
        return WisentDecisionDialog(
            tone: requested == .rollback ? .danger : .warning,
            title: "\(requested.title) storage transaction \(transaction)?",
            lines: [
                requested.explanation,
                "Stado owns every filesystem, process, queue, locking, and evidence decision. Desktop sends the reviewed host, transaction and phase to this API and retains its answer.",
                "An accepted response is not completion. This window will not automatically issue status, resume, rollback, or finalize afterwards.",
            ],
            listing: [
                "dashboard: \(address.displayString)",
                "host: \(host)",
                "transaction: \(transaction)",
                "phase: \(requested.rawValue)",
            ],
            footnote: "Equivalent CLI: \(StadoCLI.commandLine(arguments)).",
            actions: [
                WisentAction("Back to transaction", kind: .secondary) { pendingPhase = nil },
                WisentAction(requested.title, symbol: "externaldrive", kind: .destructive) {
                    pendingPhase = nil
                    Task { await store.invoke(requested, host: host, transaction: transaction, at: address) }
                },
            ]
        )
    }
}
