import SwiftUI
import WisentDesignSystem

/// Which clouds and GPU vendors rent agent machines for this fleet: the
/// same reads and writes `stado capabilities`, `stado instances list`,
/// `stado doctor`, `stado config` and `stado credentials item put` offer,
/// each answered with the CLI's own receipt.
struct ComputeProvidersView: View {
    @ObservedObject var fleetStore: FleetControlStore
    let scope: String

    var body: some View {
        WisentScreen(
            title: "Compute providers",
            scope: scope,
            freshness: nil,
            actions: [],
            scrolls: true,
            constrainsWidth: true
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
                WisentSectionBox(
                    title: "Providers",
                    detail: "GCP, Azure, AWS and fifteen GPU and cloud compute vendors — Arkane Cloud, Crusoe, Cudo Compute, Hyperstack, Lambda, Latitude.sh, Nebius, Oblivus, Oracle, RunPod, SaladCloud, Scaleway, Vast.ai (vast-rental: Stado rents the cheapest verified on-demand offer; selling this fleet's GPU on Vast.ai is the Earning screen), Voltage Park and Vultr. A vendor is enabled by naming it in providers, setting its required settings, storing its credential under role cloud-<provider>, and declaring how many of its machines may run in config/quotas.json."
                ) {
                    NativeCapabilityActions(host: "", fleet: fleetStore,
                        operations: NativeComputeProviderOperations.all)
                        .disabled(!fleetStore.isConfigured)
                }
            }
            .padding(WisentDesign.Space.x4)
        }
    }
}
