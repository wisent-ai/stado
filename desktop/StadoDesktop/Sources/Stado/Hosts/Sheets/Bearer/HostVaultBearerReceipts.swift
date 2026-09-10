import SwiftUI
import WisentDesignSystem

extension HostVaultBearerSheet {
    func rawBearerSection(
        _ bearer: String,
        request: HostVaultBearerRequest
    ) -> some View {
        WisentSectionBox(
            title: "Generated bearer",
            detail: "Shown because Show generated bearer was enabled. This plaintext is returned once; Skarbiec stores only its hash.",
            trailing: "Sensitive"
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                EnrollmentCopyBlock(
                    text: bearer,
                    caption: "Copy it to its intended secret store before closing this sheet. Closing clears Desktop's displayed copy.",
                    isSecret: true
                )
                WisentField(label: "Target", value: request.host)
                WisentField(label: "Consumer", value: request.consumer)
                WisentField(label: "Audience", value: request.audience)
                WisentField(label: "Requested capabilities", value: request.capabilities)
            }
        }
    }


    func receiptSection(_ receipt: HostVaultBearerReceipt) -> some View {
        WisentSectionBox(
            title: receiptTitle(receipt),
            detail: "Non-secret metadata returned by Stado. No bearer value is included.",
            trailing: receipt.status.humanizedIdentifier
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                WisentField(label: "Target", value: receipt.target)
                WisentField(label: "Status", value: receipt.status)
                WisentField(label: "Consumer", value: receipt.skarbiec.consumer)
                WisentField(label: "Audience", value: receipt.skarbiec.audience)
                WisentField(
                    label: "Capabilities",
                    value: receipt.skarbiec.capabilities.isEmpty
                        ? "Not reported"
                        : receipt.skarbiec.capabilities.map(\.displayValue).joined(separator: "\n")
                )
                WisentField(label: "Expires", value: expiry(receipt.skarbiec.expiresAt))
                WisentField(
                    label: "Workload binding",
                    value: receipt.skarbiec.workloadBound ? "Workload-bound" : "Bearer-bound"
                )
                if let source = receipt.tokenSource {
                    WisentField(label: "Token source", value: "\(source.item)#\(source.field)")
                }
                if let tokenFile = receipt.skarbiec.tokenFile {
                    WisentField(label: "Bearer file on host", value: tokenFile)
                }
                if let detail = receipt.detail, !detail.isEmpty {
                    WisentField(label: "Detail", value: detail, tone: receipt.succeeded ? .neutral : .danger)
                }
            }
        }
    }

    private func receiptTitle(_ receipt: HostVaultBearerReceipt) -> String {
        switch receipt.status {
        case "token_registered": "Stored bearer registered"
        case "token_minted": "Bearer minted"
        default: "Bearer operation answer"
        }
    }

    private func expiry(_ epoch: UInt64?) -> String {
        guard let epoch else { return "Not reported" }
        return Date(timeIntervalSince1970: TimeInterval(epoch))
            .formatted(date: .abbreviated, time: .standard)
    }

    func confirmation(_ request: HostVaultBearerRequest) -> WisentDecisionDialog {
        let stored = request.tokenItem != nil
        var lines = [
            stored
                ? "Register the existing \(request.tokenItem ?? "")#\(request.tokenField) value for \(request.consumer)."
                : "Mint a new bearer for \(request.consumer).",
            "Grant \(request.capabilities) for audience \(request.audience).",
            request.showGeneratedBearer
                ? "Return the newly generated plaintext bearer once for display and copy; Skarbiec stores only its hash."
                : "Return non-secret target, status, and grant metadata without bearer bytes.",
        ]
        if let tokenFileName = request.tokenFileName {
            lines[0] = "Create or reuse ~/.stado/\(tokenFileName) on \(host) for \(request.consumer)."
        }
        if request.replaceCapabilities {
            lines.append("Replace the consumer's existing capability set.")
        } else {
            lines.append("Refuse rather than change a different existing capability set.")
        }
        return WisentDecisionDialog(
            tone: request.replaceCapabilities ? .danger : .warning,
            title: "\(stored ? "Register stored" : "Mint") bearer on \(host)?",
            lines: lines,
            listing: [StadoCLI.commandLine(HostVaultBearerStore.arguments(request))],
            footnote: stored
                ? "The owner-vault field stays on the target and is not returned to Desktop."
                : request.tokenFileName != nil
                    ? "The owner-only bearer file remains on this host for subsequent requests."
                    : request.showGeneratedBearer
                        ? "The generated bearer is shown after success because Show generated bearer is enabled."
                        : "The generated plaintext is discarded; only its hash and grant remain in the target vault.",
            actions: [
                WisentAction("Back to form", kind: .secondary) { reviewing = false },
                WisentAction(
                    stored ? "Register bearer" : "Mint bearer",
                    symbol: "key.horizontal",
                    kind: .primary
                ) {
                    Task {
                        await store.submit(request, fleet: fleet, expectedSource: sourceGeneration)
                        reviewing = false
                    }
                },
            ]
        )
    }
}
