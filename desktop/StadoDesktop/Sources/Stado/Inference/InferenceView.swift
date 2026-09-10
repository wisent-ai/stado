import SwiftUI
import WisentDesignSystem

/// The fleet's local inference: which model each router alias reaches, and
/// the declared deployments behind them, with what each host's beacon last
/// reported.
///
/// Read-only on purpose. Until this screen existed the approved chat model
/// was visible only in `stado inference list`; an operator asked which model
/// the chat used and the desktop could not say. The screen runs that same
/// command and `stado inference status` and shows their answers, so the model
/// an operator approved is the model this window names.
struct InferenceView: View {
    @ObservedObject var store: InferenceStore
    let scope: String

    var body: some View {
        WisentScreen(
            title: "Inference",
            scope: scope,
            freshness: store.lastReadAt.map { "Read \(ConsoleFormat.relative($0))" },
            actions: [
                WisentAction(
                    store.isReading ? "Reading…" : "Refresh",
                    symbol: "arrow.clockwise",
                    kind: .primary,
                    isEnabled: !store.isReading
                ) {
                    Task { await store.refresh() }
                },
            ]
        ) {
            if let problem = store.problem {
                WisentErrorBanner(
                    title: "The inference declarations could not be read",
                    detail: problem,
                    action: WisentAction("Retry", symbol: "arrow.clockwise") {
                        Task { await store.refresh() }
                    }
                )
            }
            routesSection
            deploymentsSection
        }
        .task { await store.refresh() }
    }

    private var routesSection: some View {
        WisentSectionBox(
            title: "Routes",
            detail: "Each alias the model router serves, and the model it reaches. An alias that lands on a declared deployment reaches that deployment's exact model revision.",
            trailing: store.gatewayTarget.map { "gateway \($0)" } ?? "no gateway declared"
        ) {
            if store.routes.isEmpty {
                WisentEmptyPanel(
                    title: store.isReading ? "Reading routes" : "No routes declared",
                    detail: store.isReading
                        ? "stado inference list --json against the canonical registry. Nothing is written."
                        : "The registry's inference section names no alias.",
                    symbol: "arrow.triangle.branch"
                )
            } else {
                VStack(spacing: .zero) {
                    ForEach(store.routes) { route in
                        routeRow(route)
                        if route.id != store.routes.last?.id {
                            Divider()
                        }
                    }
                }
            }
        }
    }

    private func routeRow(_ route: InferenceRoute) -> some View {
        HStack(alignment: .center, spacing: WisentDesign.Space.x3) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text(route.alias)
                    .font(WisentTypeScale.bodyStrong())
                    .foregroundStyle(WisentDesign.ink)
                    .textSelection(.enabled)
                Text(route.model)
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.secondary)
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if let deployment = route.deployment {
                WisentBadge("deployment \(deployment.name) on \(deployment.target)", tone: .success)
            } else {
                WisentBadge("remote", tone: .neutral)
            }
        }
        .padding(.vertical, WisentDesign.Space.x3)
    }

    private var deploymentsSection: some View {
        WisentSectionBox(
            title: "Declared deployments",
            detail: "What the registry declares per host, and what the host's beacon last reported. GPU memory is the beacon's figure, not a plan.",
            trailing: store.isReading ? "Reading…" : "\(store.deployments.count.formatted(.number)) declared"
        ) {
            if store.deployments.isEmpty {
                WisentEmptyPanel(
                    title: store.isReading ? "Reading deployments" : "No deployments declared",
                    detail: store.isReading
                        ? "stado inference list --json against the canonical registry."
                        : "Declare one with stado inference plan and apply.",
                    symbol: "cpu"
                )
            } else {
                VStack(spacing: .zero) {
                    ForEach(store.deployments) { deployment in
                        InferenceDeploymentRow(
                            deployment: deployment,
                            beacon: store.beacons[deployment.name],
                            beaconProblem: store.beaconProblems[deployment.name]
                        )
                        if deployment.id != store.deployments.last?.id {
                            Divider()
                        }
                    }
                }
            }
        }
    }
}
