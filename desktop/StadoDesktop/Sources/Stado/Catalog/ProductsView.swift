import SwiftUI
import WisentDesignSystem

struct ProductsView: View {
    @ObservedObject var store: ProductsStore
    @ObservedObject var fleetStore: FleetControlStore
    let scope: String

    @State private var selection: String?
    @State private var selectedHost = ""
    @State private var decision: ProductDecision?
    @State private var releaseVersions: [String: String] = [:]
    @State private var sourceCommits: [String: String] = [:]

    var body: some View {
        WisentScreen(
            title: "Products",
            scope: scope,
            freshness: "\(store.products.count) products",
            actions: [
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task { await store.refresh() }
                }
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: 0) {
                if let failure = store.failure {
                    WisentErrorBanner(title: "Product catalog unavailable", detail: failure)
                        .padding(WisentDesign.Space.x4)
                }
                if store.products.isEmpty {
                    if store.isRefreshing {
                        Group {
                            let loadingTitle = "Reading products"
                            WisentSectionBox(title: loadingTitle, detail: "The canonical Wisent product catalog is read through stado product catalog.") {
                                WisentSkeletonList(label: loadingTitle)
                            }
                        }
                            .padding(WisentDesign.Space.x6)
                    } else {
                        WisentEmptyPanel(title: "No product catalog", detail: "The selected Stado API host answered with no products; it may run a Stado older than 0.22.0, which read the catalog from another program.", symbol: "shippingbox")
                            .padding(WisentDesign.Space.x6)
                    }
                    Spacer()
                } else {
                    HStack(spacing: 0) {
                        productList
                        inspector
                    }
                }
                Divider()
                ScrollView {
                    ProductLocksView(locks: store.locks, failure: store.lockFailure)
                        .padding(WisentDesign.Space.x4)
                    NativeCapabilityActions(host: scope, fleet: fleetStore,
                        operations: NativeProductOperations.all)
                        .disabled(!fleetStore.isConfigured)
                        .padding(WisentDesign.Space.x4)
                }
                .frame(maxHeight: ProductsLayout.operationsHeight)
                WisentMutationBar(outcome: store.mutation) { store.clearMutation() }
                    .padding(.horizontal, WisentDesign.Space.x4)
                    .padding(.bottom, store.mutation == .idle ? 0 : WisentDesign.Space.x3)
            }
        }
        .sheet(item: $decision) { value in decisionDialog(value) }
        .task {
            if selectedHost.isEmpty { selectedHost = fleetStore.targets.first?.name ?? "" }
            if store.products.isEmpty { await store.refresh() }
            else { await store.refreshLocks() }
        }
    }

    private var productList: some View {
        ConsoleTable(head: [
            ConsoleHeaderCell("Product", width: 180),
            ConsoleHeaderCell("Surfaces", width: 170),
            ConsoleHeaderCell("Description"),
        ]) {
            ForEach(store.products) { product in
                ConsoleTableRow(isSelected: selection == product.id, select: { selection = product.id }) {
                    ConsoleCell(text: product.name, width: 180, identifier: true, strong: true)
                    ConsoleCell(text: product.installations.map(\.surface).joined(separator: ", "), width: 170)
                    ConsoleCell(text: product.description)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    @ViewBuilder
    private var inspector: some View {
        if let product = store.products.first(where: { $0.id == selection }) {
            WisentInspector(eyebrow: "Product", title: product.name) {
                Text(product.description)
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if product.installations.contains(where: { $0.surface == "service" }) {
                    Picker("Service host", selection: $selectedHost) {
                        ForEach(fleetStore.targets.map(\.name), id: \.self) { Text($0).tag($0) }
                    }
                }
                ForEach(product.installations) { installation in
                    surfaceBox(product, installation)
                }
            }
        } else {
            WisentInspector(eyebrow: "Selection", title: "No product selected") {
                Text("Select a product to install, update, roll back or remove one of its declared surfaces.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            }
        }
    }

    private func surfaceBox(_ product: ProductCatalogEntry, _ installation: ProductInstallation) -> some View {
        let host = installation.surface == "service" ? selectedHost : nil
        let state = store.state(product: product.id, surface: installation.surface)
        return WisentSectionBox(
            title: installation.surface.capitalized,
            detail: "\(installation.kind) · \(installation.repository)",
            trailing: state?.status
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                if let keychain = installation.signingKeychain {
                    Text("Source installation signs with the catalog's local keychain: \(keychain). A missing keychain refuses before building; no Skarbiec signing role is read.")
                        .font(WisentTypeScale.identifierSmall())
                        .textSelection(.enabled)
                }
                if installation.surface == "cli" && installation.kind == "stado-release" {
                    TextField("Signed release version (optional)", text: Binding(
                        get: { releaseVersions[product.id, default: ""] },
                        set: { releaseVersions[product.id] = $0 }
                    ))
                    TextField("Accepted full source commit", text: Binding(
                        get: { sourceCommits[product.id, default: ""] },
                        set: { sourceCommits[product.id] = $0 }
                    ))
                    Text("Provide both fields to install verified published bytes without building. Both empty use the canonical recipe.")
                        .font(WisentTypeScale.identifierSmall())
                }
                if let release = state?.release {
                    Text("Release \(release.coordinate.version) · \(release.coordinate.sourceRevision)")
                        .font(WisentTypeScale.identifierSmall())
                        .textSelection(.enabled)
                }
                if let signing = state?.signing {
                    Text("Selected \(signing.source): \(signing.keychain); observed native signatures:")
                        .textSelection(.enabled)
                    ForEach(signing.signatures.keys.sorted(), id: \.self) { path in
                        if let signature = signing.signatures[path] {
                            Text("\(path): \(signature.identity) · team \(signature.team) · \(signature.identifier) · CDHash \(signature.cdhash)")
                                .font(WisentTypeScale.identifierSmall())
                                .textSelection(.enabled)
                        }
                    }
                }
                if let readiness = state?.readiness {
                    Text(readiness.detail).textSelection(.enabled)
                }
                if let operation = state?.operation {
                    Text("Installation \(operation.phase): pid \(operation.pid) is \(operation.running ? "running" : "no longer running")")
                        .textSelection(.enabled)
                    if let since = operation.startedAt ?? operation.acquiredAt {
                        Text("Since \(since)").textSelection(.enabled)
                    }
                    if let step = operation.step {
                        Text(step).textSelection(.enabled)
                    }
                    if let steps = operation.steps {
                        Text(steps.map { $0.joined(separator: " ") }.joined(separator: "\n"))
                            .textSelection(.enabled)
                    }
                }
                if let decision = state?.installDecision {
                    Text("\(decision.action): \(decision.reason)")
                        .textSelection(.enabled)
                }
                if let cache = state?.cargoCache {
                    Text("Cargo cache \(cache.reused ? "reused" : "created"): \(cache.target)")
                        .textSelection(.enabled)
                    Text("Lockfiles \(cache.lockfileSha256) · revision \(cache.sourceRevision)")
                        .font(WisentTypeScale.identifierSmall())
                        .textSelection(.enabled)
                }
                if let dependencies = state?.dependencies {
                    ForEach(dependencies) { dependency in
                        if let decision = dependency.installDecision {
                            Text("\(dependency.product)/\(dependency.surface): \(decision.action) — \(decision.reason)")
                                .textSelection(.enabled)
                        } else if let reason = dependency.reason ?? dependency.alternative {
                            Text("\(dependency.product)/\(dependency.surface): \(reason)")
                                .textSelection(.enabled)
                        }
                    }
                }
                if let state, !state.installedPaths.isEmpty {
                    Text(state.installedPaths.joined(separator: "\n"))
                        .font(WisentTypeScale.identifierSmall())
                        .textSelection(.enabled)
                }
                HStack {
                    action("Install", symbol: "arrow.down.circle", product, installation, host)
                    action("Update", symbol: "arrow.triangle.2.circlepath", product, installation, host)
                    action("Rollback", symbol: "arrow.uturn.backward", product, installation, host)
                    action("Remove", symbol: "trash", product, installation, host, destructive: true)
                    if state?.operation?.running == true {
                        action("Cancel", symbol: "xmark.circle", product, installation, host, destructive: true)
                    }
                }
                ProductPrerequisitesView(store: store, product: product.id,
                    surface: installation.surface, host: host,
                    releaseVersion: releaseVersions[product.id], sourceCommit: sourceCommits[product.id]) { version, commit in
                    decision = ProductDecision(verb: "install", product: product.id,
                        productName: product.name, surface: installation.surface,
                        host: host, destructive: false, releaseVersion: version,
                        sourceCommit: commit, withPrerequisites: true)
                }
            }
        }
    }

    private func action(
        _ verb: String,
        symbol: String,
        _ product: ProductCatalogEntry,
        _ installation: ProductInstallation,
        _ host: String?,
        destructive: Bool = false
    ) -> some View {
        WisentActionButton(
            action: WisentAction(
                verb,
                symbol: symbol,
                kind: destructive ? .plain : .secondary,
                isEnabled: !store.mutation.isWorking && (installation.surface != "service" || !(host ?? "").isEmpty)
            ) {
                let exact = installation.surface == "cli" && installation.kind == "stado-release"
                    && (verb == "Install" || verb == "Update")
                let version = releaseVersions[product.id, default: ""].trimmingCharacters(in: .whitespacesAndNewlines)
                let commit = sourceCommits[product.id, default: ""].trimmingCharacters(in: .whitespacesAndNewlines)
                decision = ProductDecision(
                    verb: verb.lowercased(), product: product.id,
                    productName: product.name, surface: installation.surface,
                    host: host, destructive: destructive,
                    releaseVersion: exact && !version.isEmpty ? version : nil,
                    sourceCommit: exact && !commit.isEmpty ? commit : nil,
                    withPrerequisites: false
                )
            }
        )
    }

    private func decisionDialog(_ value: ProductDecision) -> some View {
        WisentDecisionDialog(
            tone: value.destructive ? .danger : .warning,
            title: "\(value.verb.capitalized) \(value.productName) \(value.surface)?",
            lines: [
                value.releaseVersion == nil && value.sourceCommit == nil
                    ? "Stado executes the canonical recipe from its product catalog."
                    : "Stado verifies the signed published release and its accepted source. It does not compile or re-sign its bytes. Both coordinates are required.",
                value.surface == "service" ? "The service lifecycle is delegated back to Stado on \(value.host ?? "the selected host")." : "The installation is local to this Mac and its previous state is retained for rollback.",
                value.withPrerequisites ? "Qualified provider revisions in the plan are installed before this product." : "Missing verb prerequisites are refused rather than installed implicitly.",
            ],
            listing: [StadoCLI.commandLine(ProductsStore.lifecycleArguments(
                value.verb, product: value.product, surface: value.surface, host: value.host,
                releaseVersion: value.releaseVersion, sourceCommit: value.sourceCommit,
                withPrerequisites: value.withPrerequisites
            ))],
            actions: [
                WisentAction("Cancel", kind: .primary) { decision = nil },
                WisentAction(value.verb.capitalized, symbol: value.destructive ? "trash" : "checkmark", kind: value.destructive ? .destructive : .secondary) {
                    let selected = value
                    decision = nil
                    Task {
                        await store.mutate(selected.verb, product: selected.product,
                                           surface: selected.surface, host: selected.host,
                                           releaseVersion: selected.releaseVersion, sourceCommit: selected.sourceCommit,
                                           withPrerequisites: selected.withPrerequisites)
                    }
                },
            ]
        )
    }
}


private enum ProductsLayout {
    /// Room for the catalog, provisioning, reconciliation, compiler and
    /// documentation operations under the product table.
    static let operationsHeight: CGFloat = 220
}
