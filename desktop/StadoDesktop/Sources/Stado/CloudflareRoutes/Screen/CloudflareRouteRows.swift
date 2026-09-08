import SwiftUI
import WisentDesignSystem

/// One row per managed hostname, and the badge vocabulary the rows read from.
///
/// `routeRows` is internal rather than private only because the inventory
/// section sits in `CloudflareRoutesInventory.swift`: Swift scopes `private`
/// to one file, and the split has to keep the section and its rows reachable
/// to each other.
extension CloudflareRoutesView {
    func routeRows(_ loadedScope: CloudflareRouteScope) -> some View {
        VStack(spacing:
            0
        ) {
            ForEach(store.routes) { route in
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    HStack(alignment: .center, spacing: WisentDesign.Space.x3) {
                        VStack(alignment: .leading, spacing:
                            2
                        ) {
                            Text(route.hostname)
                                .font(WisentTypeScale.bodyStrong())
                                .foregroundStyle(WisentDesign.ink)
                                .textSelection(.enabled)
                            Text(route.origin ?? "No exact ingress origin")
                                .font(WisentTypeScale.identifierSmall())
                                .foregroundStyle(WisentDesign.muted)
                                .lineLimit(1)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)

                        WisentBadge(routeStateLabel(route.state), tone: routeStateTone(route.state))

                        Menu {
                            Button(store.isInspecting == route.hostname ? "Reading status…" : "Read status") {
                                Task { await store.inspect(route, in: loadedScope) }
                            }
                            .disabled(store.isBusy)
                            Divider()
                            Button("Remove…", role: .destructive) {
                                pendingRemoval = route
                            }
                            .disabled(store.isBusy)
                        } label: {
                            Image(systemName: "ellipsis.circle")
                        }
                        .accessibilityLabel("Actions for \(route.hostname)")
                    }

                    HStack(spacing: WisentDesign.Space.x4) {
                        Text("\(route.ingressRules) ingress")
                        Text("\(route.dnsRecords) tunnel DNS")
                        if route.conflictingDNSRecords > 0 {
                            Text("\(route.conflictingDNSRecords) conflicting DNS")
                                .foregroundStyle(WisentDesign.warning)
                        }
                        Text(route.proxied ? "proxied" : "not proxied")
                        Text(route.tunnelConnected ? "connector active" : "connector down")
                        Text("origin \(route.originReachability.replacingOccurrences(of: "_", with: " "))")
                    }
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                }
                .padding(.vertical, WisentDesign.Space.x3)

                if route.id != store.routes.last?.id {
                    Divider()
                }
            }
        }
    }

    private func routeStateLabel(_ state: String) -> String {
        switch state {
        case "connector_down": "connector down"
        default: state
        }
    }

    private func routeStateTone(_ state: String) -> WisentTone {
        switch state {
        case "routed": .success
        case "drifted", "connector_down": .warning
        case "absent": .neutral
        default: .danger
        }
    }
}
