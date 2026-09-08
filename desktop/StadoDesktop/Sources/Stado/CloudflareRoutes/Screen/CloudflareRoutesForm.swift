import SwiftUI
import WisentDesignSystem

/// The hostname the operator is adding, the connector that will carry it, the
/// secret it reads, and the exact command the screen would run.
///
/// Every member here is internal rather than private only because `body` sits
/// in `CloudflareRoutesView.swift`: Swift scopes `private` to one file, and the
/// split has to keep the frame and the form reachable to each other.
extension CloudflareRoutesView {
    var publicRouteSection: some View {
        WisentSectionBox(
            title: "Add or update a hostname",
            detail: "The exact public hostname and the HTTP(S) service seen from the connector host. The zone and credentials above are shared with the inventory."
        ) {
            VStack(spacing: WisentDesign.Space.x3) {
                LabeledContent("Hostname") {
                    TextField("api.bobloo.com", text: $draft.hostname)
                        .textFieldStyle(.roundedBorder)
                }
                LabeledContent("Connector-local origin") {
                    TextField(CloudflareRouteConstants.defaultOrigin, text: $draft.origin)
                        .textFieldStyle(.roundedBorder)
                }
            }
        }
    }

    var connectorSection: some View {
        WisentSectionBox(
            title: "Managed connector",
            detail: "A registry host and its declared cloudflared service. Stado installs the connector token there and restarts this unit before DNS moves.",
            trailing: hosts.isEmpty ? "No hosts read" : "\(hosts.count.formatted(.number)) hosts"
        ) {
            VStack(spacing: WisentDesign.Space.x3) {
                selectionInput(
                    "Registry host",
                    placeholder: "charless-mac-mini",
                    values: hosts,
                    selection: $draft.host
                )
                LabeledContent("Connector service") {
                    TextField("cloudflared", text: $draft.connectorService)
                        .textFieldStyle(.roundedBorder)
                }
            }
        }
    }

    var advancedSection: some View {
        WisentSectionBox(
            title: "Connector secret",
            detail: "The named field Stado reads and the owner-only filename it writes under the connector service user's ~/.stado directory."
        ) {
            VStack(spacing: WisentDesign.Space.x3) {
                LabeledContent("Token field") {
                    TextField("token", text: $draft.connectorTokenField)
                        .textFieldStyle(.roundedBorder)
                }
                LabeledContent("Secret filename") {
                    TextField("cloudflared-token", text: $draft.connectorSecretName)
                        .textFieldStyle(.roundedBorder)
                }
            }
        }
    }

    func problemsPanel(_ problems: [String]) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("stado cloudflare route-tunnel would refuse this as it stands:")
                .font(WisentTypeScale.bodyStrong())
                .foregroundStyle(WisentDesign.ink)
            ForEach(problems, id: \.self) { problem in
                HStack(alignment: .top, spacing: WisentDesign.Space.x2) {
                    Image(systemName: "exclamationmark.circle")
                        .font(.system(size:
                            11, weight: .semibold
                        ))
                        .foregroundStyle(WisentTone.warning.color)
                        .accessibilityHidden(true)
                    Text(problem)
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .padding(WisentDesign.Space.x3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            WisentTone.warning.softColor,
            in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
        )
    }

    func commandAndAction(_ problems: [String]) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            commandLine(draft.arguments)
            HStack {
                Spacer(minLength:
                    0
                )
                WisentActionButton(
                    action: WisentAction(
                        store.isRouting ? "Routing…" : "Review route…",
                        symbol: "arrow.right.circle",
                        kind: .primary,
                        isEnabled: problems.isEmpty && !store.isBusy
                    ) {
                        pendingRoute = PendingRoute(draft: draft.normalized)
                    }
                )
            }
        }
    }
}
