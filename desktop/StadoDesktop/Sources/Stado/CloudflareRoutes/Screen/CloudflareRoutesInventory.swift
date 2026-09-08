import SwiftUI
import WisentDesignSystem

/// What this tunnel already carries: the read routes, the connector signals
/// above them, and the warning shown when the fields no longer describe the
/// rows.
///
/// `inventorySection` is internal rather than private only because `body` sits
/// in `CloudflareRoutesView.swift`: Swift scopes `private` to one file, and the
/// split has to keep the frame and the inventory reachable to each other.
extension CloudflareRoutesView {
    @ViewBuilder
    func inventorySection(_ scopeProblems: [String]) -> some View {
        WisentSectionBox(
            title: "Managed routes",
            detail: "Ingress and exact tunnel CNAMEs are compared for every hostname in this zone. Connector state comes from Cloudflare; origin reachability is deliberately reported as not probed.",
            trailing: inventoryTrailing
        ) {
            if let loadedScope = store.inventoryScope {
                connectorSignals
                if loadedScope != draft.scope.normalized {
                    scopeChangedPanel(loadedScope)
                }
                if store.routes.isEmpty {
                    WisentEmptyPanel(
                        title: store.isRefreshingRoutes ? "Reading routes" : "No managed routes",
                        detail: store.isRefreshingRoutes
                            ? "Stado is reading tunnel ingress, DNS and active connector state."
                            : "This tunnel has no ingress or tunnel CNAME route inside \(loadedScope.zone).",
                        symbol: "network"
                    )
                } else {
                    routeRows(loadedScope)
                }
            } else {
                WisentEmptyPanel(
                    title: scopeProblems.isEmpty ? "Routes have not been read" : "Choose a tunnel and zone",
                    detail: scopeProblems.isEmpty
                        ? "Read routes to compare Cloudflare tunnel ingress, DNS and connector state."
                        : "A valid API credential id, tunnel credential id and lowercase zone are required.",
                    symbol: "network"
                )
            }
        }
    }

    private var connectorSignals: some View {
        WisentSignalStrip(signals: [
            WisentSignal(
                "Tunnel",
                value: store.tunnelConnected ? "connected" : "no active connection",
                tone: store.tunnelConnected ? .success : .warning
            ),
            WisentSignal(
                "Connectors",
                value: store.connectorCount.formatted(.number),
                tone: store.connectorCount > 0 ? .neutral : .warning
            ),
            WisentSignal(
                "Active connections",
                value: store.activeConnections.formatted(.number),
                tone: store.activeConnections > 0 ? .success : .warning
            ),
        ])
    }

    private var inventoryTrailing: String {
        if store.isRefreshingRoutes {
            return "Reading…"
        }
        guard store.inventoryScope != nil else {
            return "Not read"
        }
        return "\(store.routes.count.formatted(.number)) routes"
    }

    private func scopeChangedPanel(_ loadedScope: CloudflareRouteScope) -> some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x2) {
            Image(systemName: "exclamationmark.triangle")
                .foregroundStyle(WisentDesign.warning)
                .accessibilityHidden(true)
            Text("These rows are still \(loadedScope.zone) from tunnel \(store.tunnelID ?? "unknown"). Read routes again before treating edited fields as current state.")
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(WisentDesign.Space.x3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            WisentTone.warning.softColor,
            in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
        )
    }
}
