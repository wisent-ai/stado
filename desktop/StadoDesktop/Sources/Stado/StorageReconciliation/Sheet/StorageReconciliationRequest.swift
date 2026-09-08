import SwiftUI
import WisentDesignSystem

/// The two zones above the receipt: what this sheet is about, and the durable
/// transaction an operator is about to name.
///
/// `header` and `requestSection` are internal rather than private because
/// `body` sits in `../../StorageReconciliation.swift`, and Swift scopes
/// `private` to one file.
extension StorageReconciliationSheet {
    var header: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("Reconcile storage roots on \(host)")
                .font(WisentTypography.heading(17))
                .foregroundStyle(WisentDesign.ink)
            Text("Operate one durable A/B storage transaction through the configured Stado API. Opening this sheet and changing fields run nothing; status is read only, and every write is reviewed before it starts.")
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
            WisentField(label: "Dashboard", value: address.displayString)
        }
    }

    var requestSection: some View {
        WisentSectionBox(
            title: "Durable transaction",
            detail: phase.explanation
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                    Text("Transaction ID")
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.secondary)
                    TextField("desktop-transaction-id", text: $selectedTransaction)
                        .textFieldStyle(.roundedBorder)
                        .font(WisentTypeScale.identifier())
                        .disabled(store.isRunning)
                }
                if let problem = StorageReconciliationStore.transactionProblem(selectedTransaction) {
                    Text(problem)
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.danger)
                }
                Picker("Phase", selection: $phase) {
                    ForEach(StorageReconciliationPhase.allCases) { value in
                        Text(value.title).tag(value)
                    }
                }
                .pickerStyle(.segmented)
                .disabled(store.isRunning)
                Text("Equivalent CLI command")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                Text(verbatim: command)
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}
