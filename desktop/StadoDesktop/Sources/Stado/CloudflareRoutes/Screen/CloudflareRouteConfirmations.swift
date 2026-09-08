import SwiftUI
import WisentDesignSystem

/// What the operator is shown before anything is written, and before anything
/// is deleted.
///
/// Both are internal rather than private only because `body` presents them as
/// sheets from `CloudflareRoutesView.swift`: Swift scopes `private` to one
/// file, and the split has to keep the frame and its dialogs reachable to each
/// other.
extension CloudflareRoutesView {
    func routeConfirmation(_ value: CloudflareRouteDraft) -> WisentDecisionDialog {
        WisentDecisionDialog(
            tone: .warning,
            title: "Route \(value.hostname) through Cloudflare?",
            lines: [
                "Stado first writes the tunnel ingress rule \(value.hostname) -> \(value.origin). Public DNS is not moved if that write is refused.",
                "It then installs the connector token from \(value.tunnelCredential) on \(value.host) and restarts the declared service \(value.connectorService).",
                "Only after the connector restart succeeds does Stado create or update the proxied CNAME for \(value.hostname). Existing traffic for that hostname may move to this tunnel.",
            ],
            listing: [
                "zone: \(value.zone)",
                "hostname: \(value.hostname)",
                "origin: \(value.origin)",
                "host: \(value.host)",
                "service: \(value.connectorService)",
                "API credential: \(value.apiCredential)",
                "tunnel credential: \(value.tunnelCredential)",
                "token field: \(value.connectorTokenField)",
                "secret filename: \(value.connectorSecretName)",
            ],
            footnote: "Runs \(StadoCLI.commandLine(value.arguments)). Secret values are never rendered.",
            actions: [
                WisentAction("Back to the form", kind: .secondary) { pendingRoute = nil },
                WisentAction("Route hostname", symbol: "network", kind: .primary) {
                    pendingRoute = nil
                    Task { await store.route(value) }
                },
            ]
        )
    }

    func removalConfirmation(_ route: CloudflareRouteState) -> WisentDecisionDialog {
        let loadedScope = store.inventoryScope ?? draft.scope.normalized
        return WisentDecisionDialog(
            tone: .danger,
            title: "Remove \(route.hostname) from this tunnel?",
            lines: [
                "Stado deletes only CNAME records for \(route.hostname) that point to \(route.dnsContent), then removes every exact ingress rule for this hostname.",
                "A refused DNS deletion leaves ingress in place. If ingress cannot be updated after DNS is gone, Stado reports the partial removal explicitly.",
                "The cloudflared connector, its service and its credential stay because this tunnel may carry other hostnames.",
            ],
            listing: [
                "zone: \(loadedScope.zone)",
                "hostname: \(route.hostname)",
                "matching ingress rules: \(route.ingressRules)",
                "matching tunnel DNS records: \(route.dnsRecords)",
                "conflicting DNS records left alone: \(route.conflictingDNSRecords)",
            ],
            footnote: "Runs \(StadoCLI.commandLine(loadedScope.removeArguments(hostname: route.hostname))).",
            actions: [
                WisentAction("Keep route", kind: .secondary) { pendingRemoval = nil },
                WisentAction("Remove route", symbol: "trash", kind: .primary) {
                    pendingRemoval = nil
                    Task { await store.remove(route, from: loadedScope) }
                },
            ]
        )
    }
}
