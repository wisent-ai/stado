import Foundation
import SwiftUI
import WisentDesignSystem

/// One durable A/B storage-root transaction, operated a phase at a time.
///
/// Opening this sheet and typing in it run nothing. Status is read-only and
/// runs on request; every other phase is reviewed in a dialog that quotes the
/// equivalent CLI invocation before anything starts. An accepted response is
/// not completion, and Desktop never advances a transaction on its own.
///
/// The sheet's own parts live in `StorageReconciliation/Sheet/`: the title and
/// the transaction request in `StorageReconciliationRequest.swift`, the
/// reported-state readout in `StorageReconciliationSummary.swift`, the
/// retained attempts in `StorageReconciliationHistory.swift`, and the footer
/// and its confirmation dialog in `StorageReconciliationDecision.swift`. The
/// phase list, the retained JSON and the store they read sit one level up in
/// `StorageReconciliation/`.
struct StorageReconciliationSheet: View {
    let host: String
    let address: OperationsDashboardAddress
    @ObservedObject var store: StorageReconciliationStore

    @Environment(\.dismiss) var dismiss
    @State var phase: StorageReconciliationPhase = .status
    @State var pendingPhase: StorageReconciliationPhase?

    @State var selectedTransaction = ""

    var command: String {
        StadoCLI.commandLine(
            StorageReconciliationStore.arguments(
                host: host,
                transaction: selectedTransaction,
                phase: phase
            )
        )
    }

    var invocations: [StorageReconciliationInvocation] {
        store.invocations(host: host, transaction: selectedTransaction, at: address)
    }

    private var latestReceipt: StorageReconciliationJSON? {
        invocations.lazy.compactMap(\.receipt).first
    }

    var latestStatusReceipt: StorageReconciliationJSON? {
        invocations.lazy
            .filter { $0.phase == .status }
            .compactMap(\.receipt)
            .first
    }

    var body: some View {
        VStack(spacing:
            0) {
            ScrollView {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
                    header
                    requestSection
                    if let activeCommand = store.activeCommand {
                        WisentAlertPanel(
                            tone: .warning,
                            title: "Stado is still running this command",
                            detail: activeCommand
                        )
                    }
                    if let latestReceipt {
                        summary(receipt: latestReceipt)
                    }
                    history
                }
                .padding(WisentDesign.Space.x6)
            }
            Divider()
            footer
                .padding(WisentDesign.Space.x4)
        }
        .frame(width:
            820)
        .frame(minHeight:
            680)
        .background(WisentDesign.canvas)
        .onAppear { selectedTransaction = store.transaction(for: host) }
        .onChange(of: selectedTransaction) { _, value in
            store.retainTransaction(host: host, transaction: value)
        }
        .sheet(item: $pendingPhase) { requested in
            confirmation(for: requested)
        }
    }
}
