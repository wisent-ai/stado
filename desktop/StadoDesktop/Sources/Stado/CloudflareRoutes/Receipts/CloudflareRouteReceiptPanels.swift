import SwiftUI
import WisentDesignSystem

/// The two panels that render what Stado last changed.
///
/// Both are internal rather than private only because `body` sits in
/// `CloudflareRoutesView.swift`: Swift scopes `private` to one file, and the
/// split has to keep the frame and its panels reachable to each other.
extension CloudflareRoutesView {
    func routeReceiptPanel(_ receipt: CloudflareRouteReceipt) -> some View {
        WisentSectionBox(
            title: "Last completed route",
            detail: "The nonsecret receipt returned by Stado after ingress, connector and DNS all completed.",
            trailing: receipt.status
        ) {
            WisentSignalStrip(signals: [
                WisentSignal("Hostname", value: receipt.hostname, tone: .success),
                WisentSignal(
                    "DNS",
                    value: "\(receipt.action), \(receipt.proxied ? "proxied" : "unproxied")",
                    tone: .success
                ),
                WisentSignal("Connector", value: receipt.connectorRestart, tone: .success),
            ])
            HStack(alignment: .top, spacing: WisentDesign.Space.x5) {
                WisentField(label: "Origin", value: receipt.origin)
                WisentField(label: "DNS target", value: receipt.dnsContent)
                WisentField(label: "Host / service", value: "\(receipt.connectorHost) / \(receipt.connectorService)")
            }
            HStack(alignment: .top, spacing: WisentDesign.Space.x5) {
                WisentField(label: "Zone", value: receipt.zone)
                WisentField(label: "Connector unit", value: receipt.connectorUnit)
                WisentField(label: "Secret path", value: receipt.connectorSecretPath)
            }
        }
    }

    func removalReceiptPanel(_ receipt: CloudflareRouteRemovalReceipt) -> some View {
        WisentSectionBox(
            title: "Last removed route",
            detail: "Stado removed only this tunnel's exact DNS and ingress entries. The shared connector, service and credential remained.",
            trailing: receipt.status
        ) {
            WisentSignalStrip(signals: [
                WisentSignal("Hostname", value: receipt.hostname, tone: .neutral),
                WisentSignal("DNS removed", value: receipt.removedDNSRecords.formatted(.number), tone: .success),
                WisentSignal("Ingress removed", value: receipt.removedIngressRules.formatted(.number), tone: .success),
            ])
            HStack(alignment: .top, spacing: WisentDesign.Space.x5) {
                WisentField(label: "Zone", value: receipt.zone)
                WisentField(label: "DNS target", value: receipt.dnsContent)
                WisentField(label: "Connector", value: receipt.connectorPreserved ? "Preserved" : "Not preserved")
            }
        }
    }
}
