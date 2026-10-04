import SwiftUI
import WisentDesignSystem

/// The canonical fleet policy, target by target, and the one field of it this
/// console may write.
///
/// The screen's own parts live in `Registry/`: the facet rail and the three
/// zones in `Registry/RegistryFacets.swift`, the rows in
/// `Registry/RegistryTable.swift`, the policy pane and its write buttons in
/// `Registry/RegistryInspector.swift`, the confirmations in
/// `Registry/RegistryDialogs.swift`, and the filter and the pending write in
/// `Registry/RegistryPolicyDecision.swift`. The stored state stays here,
/// because `@State` belongs to the view rather than to any one of its
/// sections.
struct RegistryView: View {
    @ObservedObject var fleetStore: FleetControlStore
    let scope: String

    @State var facet: RegistryFacet = .all
    @State var selection: String?
    @State var decision: PolicyDecision?

    var body: some View {
        WisentScreen(
            title: "Registry",
            scope: scope,
            freshness: freshness,
            actions: [
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !fleetStore.isRefreshing) {
                    Task { await fleetStore.refresh() }
                }
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing:
                0) {
                WisentSectionBox(
                    title: "Registry documents",
                    detail: "Read the selected endpoint's registry, or validate and publish a document you supply. No source-checkout snapshot is selected for you."
                ) {
                    NativeCapabilityActions(host: "", fleet: fleetStore, operations: NativeRegistryOperations.all)
                }
                .padding(WisentDesign.Space.x4)

                if let message = fleetStore.errorMessage {
                    WisentErrorBanner(
                        title: fleetStore.isShowingStalePolicy
                            ? "Refresh failed — the policy below is the last projection that was read"
                            : "Canonical fleet policy unavailable",
                        detail: message,
                        action: WisentAction("Retry", symbol: "arrow.clockwise") {
                            Task { await fleetStore.refresh() }
                        }
                    )
                    .padding(WisentDesign.Space.x4)
                }

                if fleetStore.policy != nil {
                    zones
                } else {
                    placeholder
                        .padding(WisentDesign.Space.x6)
                    Spacer(minLength:
                        0)
                }

                WisentMutationBar(outcome: fleetStore.mutation) { fleetStore.clearMutation() }
                    .padding(.horizontal, WisentDesign.Space.x4)
                    .padding(.bottom, fleetStore.mutation == .idle ? 0 : WisentDesign.Space.x3)
            }
        }
        .sheet(item: $decision) { pending in
            dialog(for: pending)
        }
    }

    private var freshness: String {
        guard let policy = fleetStore.policy else {
            return fleetStore.isConfigured ? "Not read yet" : "Not configured"
        }
        return "Generation \(policy.generation) · read \(ConsoleFormat.relative(fleetStore.lastUpdated))"
    }

    @ViewBuilder
    private var placeholder: some View {
        if fleetStore.isRefreshing {
            Group {
                let loadingTitle = "Reading canonical fleet policy"
                WisentSectionBox(title: loadingTitle, detail: "The dashboard projects three policy fields per target. The Hosts screen reads control routes separately through stado host link; SSH credentials never cross this projection.") {
                    WisentSkeletonList(label: loadingTitle)
                }
            }
        } else if !fleetStore.isConfigured {
            WisentEmptyPanel(
                title: "No Stado endpoint",
                detail: "Choose a source in the sidebar to read the canonical policy this fleet runs on.",
                symbol: "book.closed"
            )
        } else {
            WisentEmptyPanel(
                title: "No policy projection",
                detail: "The dashboard has not returned the canonical registry projection. Nothing about fleet policy is assumed while it is missing.",
                symbol: "book.closed",
                action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary) {
                    Task { await fleetStore.refresh() }
                }
            )
        }
    }

    // MARK: Values

    var targets: [FleetPolicyTarget] {
        let targets = fleetStore.targets
        switch facet {
        case .all: return targets
        case .pinned: return targets.filter { $0.pinnedOnly == true }
        case .open: return targets.filter { $0.pinnedOnly != true }
        }
    }

    func badges(for target: FleetPolicyTarget) -> [(String, WisentTone)] {
        target.pinnedOnly == true ? [("Routed only", .neutral)] : []
    }
}

enum NativeRegistryOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "registry-document-read", title: "Read the canonical registry and its generation",
              path: ["registry", "pull"], hostPlacement: .none,
              fixedArguments: ["--with-generation"], mutates: false, jsonOutput: false),
        .init(id: "registry-document-validate", title: "Validate a supplied registry document",
              path: ["registry", "validate"], hostPlacement: .none,
              payload: .file(option: nil, label: "Registry JSON document", initial: ""),
              mutates: false, jsonOutput: false),
        .init(id: "registry-document-push", title: "Replace the registry from a supplied document",
              path: ["registry", "push"], hostPlacement: .none, fields: [
                  .init(id: "generation", label: "Generation read before editing", option: "--if-generation"),
                  .init(id: "force", label: "Authorize key removal or generation rollback", option: "--force", flag: true),
                  .init(id: "empty", label: "Authorize removal of every target", option: "--allow-empty-fleet", flag: true),
              ], fixedArguments: ["-"],
              payload: .standardInput(label: "Registry JSON document", initial: "")),
    ]
}
