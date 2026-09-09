import SwiftUI
import WisentDesignSystem

/// The fleets of the canonical registry: what groups exist, which machines
/// point at each, and the three writes the CLI owns — create, assign,
/// delete. Every write is confirmed here and executed by the control plane;
/// this window never edits the registry document itself.
///
/// This is the screen's entry point: the observed stores, the selection and
/// sheet state, and the body that assembles the zones in `FleetsView/`.
///
/// The state below is internal rather than private because the rail, table,
/// inspector and delete dialog that read and write it sit in sibling files:
/// Swift scopes `private` to one file.
struct FleetsView: View {
    @ObservedObject var groupStore: FleetGroupStore
    @ObservedObject var fleetStore: FleetControlStore
    let scope: String

    @State var facet: FleetFacet = .all
    @State var selection: String?
    @State var showsCreate = false
    @State var assignTarget: SheetID?
    @State var deleteCandidate: FleetGroup?

    var body: some View {
        WisentScreen(
            title: "Fleets",
            scope: scope,
            freshness: freshness,
            actions: [
                WisentAction("New fleet…", symbol: "plus", kind: .primary) {
                    showsCreate = true
                },
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !groupStore.isReading) {
                    Task { await groupStore.refresh() }
                },
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing:
                0) {
                if let failure = groupStore.failure {
                    WisentErrorBanner(
                        title: groupStore.fleets.isEmpty
                            ? "Fleets could not be read"
                            : "Refresh failed — the fleets below are the last read that succeeded",
                        detail: failure,
                        action: WisentAction("Retry", symbol: "arrow.clockwise") {
                            Task { await groupStore.refresh() }
                        }
                    )
                    .padding(WisentDesign.Space.x4)
                }

                if !groupStore.fleets.isEmpty {
                    zones
                } else if groupStore.failure == nil {
                    placeholder
                        .padding(WisentDesign.Space.x6)
                    Spacer(minLength:
                        0)
                } else {
                    Spacer(minLength:
                        0)
                }

                WisentMutationBar(outcome: groupStore.mutation) { groupStore.clearMutation() }
                    .padding(.horizontal, WisentDesign.Space.x4)
                    .padding(.bottom, groupStore.mutation == .idle ? 0 : WisentDesign.Space.x3)
            }
        }
        .sheet(isPresented: $showsCreate) {
            FleetCreateSheet(groupStore: groupStore, isPresented: $showsCreate)
        }
        .sheet(item: $assignTarget) { sheet in
            FleetAssignSheet(
                groupStore: groupStore,
                fleetStore: fleetStore,
                fleetName: sheet.id,
                isPresented: $assignTarget
            )
        }
        .sheet(item: $deleteCandidate) { fleet in
            deleteDialog(fleet)
        }
        .task {
            if groupStore.fleets.isEmpty { await groupStore.refresh() }
        }
    }

    private var freshness: String {
        guard groupStore.lastReadAt != nil else {
            return groupStore.isConfigured ? "Not read yet" : "Not configured"
        }
        return "\(groupStore.fleets.count.formatted(.number)) fleets · read \(ConsoleFormat.relative(groupStore.lastReadAt))"
    }

    @ViewBuilder
    private var placeholder: some View {
        if groupStore.isReading {
            WisentLoadingPanel(
                title: "Reading the fleets",
                detail: "stado fleet list through the control plane's command bridge."
            )
        } else if !groupStore.isConfigured {
            WisentEmptyPanel(
                title: "No Stado endpoint",
                detail: "Choose a source in the sidebar to read the fleets this registry declares.",
                symbol: "rectangle.3.group"
            )
        } else {
            WisentEmptyPanel(
                title: "No fleets declared",
                detail: "A fleet is a named group of machines. Create the first one, then assign machines to it from its inspector.",
                symbol: "rectangle.3.group",
                action: WisentAction("New fleet…", symbol: "plus", kind: .primary) {
                    showsCreate = true
                }
            )
        }
    }
}
