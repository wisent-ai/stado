import Foundation
import SwiftUI
import WisentDesignSystem

/// What the newest answer and the newest retained status receipt say right now.
///
/// An accepted command is not a completed transaction, so the panel says so
/// until a durable receipt reports otherwise, and every field an operator has
/// to act on is read out of the report rather than summarised away.
///
/// `summary` is internal rather than private because `body` sits in
/// `../../StorageReconciliation.swift`, and Swift scopes `private` to one
/// file; the field readers it calls stay private to this one.
extension StorageReconciliationSheet {
    @ViewBuilder
    func summary(receipt: StorageReconciliationJSON) -> some View {
        let reportStatus = receipt["status"]?.stringValue
        let durableReport = reportStatus == "accepted" ? latestStatusReceipt ?? receipt : receipt
        let durableReceipt = durableReport["receipt"]
        let durableStatus = durableReceipt?["status"]?.stringValue
        if reportStatus == "accepted" {
            WisentAlertPanel(
                tone: .warning,
                title: "Accepted — completion is not yet known",
                detail: "The resident operation accepted this transaction. Explicitly select Status and run it to read the durable receipt; Desktop will not resume or advance it automatically."
            )
        } else if durableStatus != "complete" {
            WisentAlertPanel(
                tone: .warning,
                title: "Transaction is not complete",
                detail: durableStatus.map { "The durable receipt reports \($0)." }
                    ?? "No durable receipt status is present in this answer."
            )
        }

        WisentSectionBox(
            title: "Current reported state",
            detail: "Important fields from the newest answer and newest retained status receipt for this host and transaction. The complete documents remain below."
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                jsonField("Command status", receipt["status"])
                jsonField("Durable receipt status", durableReceipt?["status"])
                ownerFields(receipt)
                lifecycleFields(durableReport["lifecycle_fence"])
                evidenceFields(durableReport)
            }
        }
    }
    @ViewBuilder
    private func ownerFields(_ receipt: StorageReconciliationJSON) -> some View {
        let reportedOwner = receipt["operation_owner"]
        let owner = reportedOwner?.objectValue == nil
            ? receipt["lifecycle_fence"]?["resident_owner"]
            : reportedOwner
        if let owner, case .object = owner {
            Divider()
            Text("Resident owner")
                .font(WisentTypeScale.bodyStrong())
                .foregroundStyle(WisentDesign.ink)
            jsonField("Owner status", owner["status"])
            jsonField("Recorded owner status", owner["recorded_status"])
            jsonField("Owner PID", owner["process_pid"] ?? owner["pid"])
            if let manager = owner["native_manager"] {
                jsonField("Native manager", manager["manager"])
                jsonField("Native service", manager["service"])
                jsonField("Native PID", manager["pid"])
                jsonField("Native state", manager["state"] ?? manager["active_state"])
            }
            if let observation = owner["native_manager_observation"] {
                jsonField("Current manager observation", observation)
            }
        }
    }

    @ViewBuilder
    private func lifecycleFields(_ fence: StorageReconciliationJSON?) -> some View {
        if let fence, case .object = fence {
            Divider()
            Text("Lifecycle and write fence")
                .font(WisentTypeScale.bodyStrong())
                .foregroundStyle(WisentDesign.ink)
            jsonField("Lifecycle status", fence["status"])
            jsonField("Lifecycle schema", fence["schema"])
            if let queue = fence["queue"] {
                jsonField("Queue", queue)
            }
            if let writers = fence["writers"] {
                jsonField("Writers", writers)
            }
            if let writeFence = fence["write_fence"] {
                jsonField("Write-fence status", writeFence["status"])
                jsonField("Write-fence intent", writeFence["intent"])
                jsonField("Write fence acquired", writeFence["acquired_at"])
                jsonField("Write fence released", writeFence["released_at"])
            }
            if let roots = fence["roots"] {
                jsonField("Primary root", roots["primary"])
                jsonField("Backup root", roots["backup"])
                jsonField("Prior primary", roots["prior_primary"])
                jsonField("Prior backup", roots["prior_backup"])
                jsonField("Runtime root proof", roots["runtime"])
            }
            jsonField("Preflight evidence", fence["preflight_evidence"])
        }
    }

    @ViewBuilder
    private func evidenceFields(_ report: StorageReconciliationJSON) -> some View {
        if let receipt = report["receipt"], let fields = receipt.objectValue {
            let evidence = fields.keys
                .filter { $0.localizedCaseInsensitiveContains("evidence") || $0.localizedCaseInsensitiveContains("checkpoint") }
                .sorted()
            if !evidence.isEmpty {
                Divider()
                Text("Receipt evidence")
                    .font(WisentTypeScale.bodyStrong())
                    .foregroundStyle(WisentDesign.ink)
                ForEach(evidence, id: \.self) { key in
                    jsonField(key.replacingOccurrences(of: "_", with: " ").capitalized, fields[key])
                }
            }
        }
    }

    @ViewBuilder
    private func jsonField(_ label: String, _ value: StorageReconciliationJSON?) -> some View {
        if let value, case .null = value {
            WisentField(label: label, value: "Not captured")
        } else if let value {
            WisentField(label: label, value: value.displayValue)
        }
    }
}
