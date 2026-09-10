import Foundation
import SwiftUI
import WisentDesignSystem

/// Cloudflare route management projected into Stado Desktop.
///
/// The screen calls the product CLI rather than duplicating Cloudflare API,
/// Stado host, service, or Skarbiec logic. Every mutation shows its exact argv
/// before it can run.
///
/// The screen's own parts live in `CloudflareRoutes/`: the inputs and the
/// draft in `CloudflareRoutes/CloudflareRouteInputs.swift`, the one caller of
/// the CLI in `CloudflareRoutes/CloudflareRoutesStore.swift`, the decoded
/// state and the receipt panels in `CloudflareRoutes/Receipts/`, and the
/// sections, rows and dialogs in `CloudflareRoutes/Screen/`.
struct CloudflareRoutesView: View {
    @ObservedObject var store: CloudflareRoutesStore
    let hosts: [String]
    let scope: String

    @State var draft = CloudflareRouteDraft()
    @State var pendingRoute: PendingRoute?
    @State var pendingRemoval: CloudflareRouteState?

    var body: some View {
        let routeProblems = draft.problems
        let scopeProblems = draft.scope.problems
        WisentScreen(
            title: "Cloudflare routes",
            scope: scope,
            freshness: store.lastInventoryAt.map { "Read \(ConsoleFormat.relative($0))" },
            actions: [
                WisentAction(
                    store.isReadingCredentials ? "Reading credentials…" : "Refresh credentials",
                    symbol: "key",
                    isEnabled: !store.isBusy
                ) {
                    Task { await store.refreshCredentials() }
                },
                WisentAction(
                    store.isRefreshingRoutes ? "Reading routes…" : "Read routes",
                    symbol: "arrow.clockwise",
                    kind: .primary,
                    isEnabled: scopeProblems.isEmpty && !store.isBusy
                ) {
                    Task { await store.refreshRoutes(draft.scope) }
                },
            ]
        ) {
            if let problem = store.mutationProblem {
                WisentErrorBanner(title: "The Cloudflare change was not completed", detail: problem)
            }
            if let problem = store.inventoryProblem {
                WisentErrorBanner(
                    title: "Cloudflare route state could not be read",
                    detail: problem,
                    action: scopeProblems.isEmpty
                        ? WisentAction("Retry", symbol: "arrow.clockwise") {
                            Task { await store.refreshRoutes(draft.scope) }
                        }
                        : nil
                )
            }
            if let problem = store.credentialsProblem {
                WisentErrorBanner(
                    title: "Credential names could not be read",
                    detail: "\(problem) You can still type an exact item id.",
                    action: WisentAction("Retry", symbol: "arrow.clockwise") {
                        Task { await store.refreshCredentials() }
                    }
                )
            }
            if let receipt = store.lastRouteReceipt {
                routeReceiptPanel(receipt)
            }
            if let receipt = store.lastRemovalReceipt {
                removalReceiptPanel(receipt)
            }
            scopeSection
            inventorySection(scopeProblems)
            publicRouteSection
            connectorSection
            advancedSection
            if !routeProblems.isEmpty {
                problemsPanel(routeProblems)
            }
            commandAndAction(routeProblems)
        }
        .task {
            if draft.host.isEmpty {
                draft.host = hosts.first ?? ""
            }
            await store.refreshCredentials()
        }
        .onChange(of: hosts) { _, values in
            if draft.host.isEmpty {
                draft.host = values.first ?? ""
            }
        }
        .sheet(item: $pendingRoute) { route in
            routeConfirmation(route.draft)
        }
        .sheet(item: $pendingRemoval) { route in
            removalConfirmation(route)
        }
    }

    struct PendingRoute: Identifiable {
        let id = UUID()
        let draft: CloudflareRouteDraft
    }
}
