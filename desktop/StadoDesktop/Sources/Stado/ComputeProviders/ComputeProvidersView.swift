import SwiftUI
import WisentDesignSystem

/// Which clouds and GPU vendors rent agent machines for this fleet: the
/// same reads and writes `stado capabilities`, `stado instances list`,
/// `stado doctor`, `stado config` and `stado credentials item put` offer,
/// and the Boat sandboxes `stado sandbox` drives, each answered with the
/// CLI's own receipt.
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
                    detail: "GCP, Azure, AWS and fifteen GPU and cloud compute vendors — Arkane Cloud, Crusoe, Cudo Compute, Hyperstack, Lambda, Latitude.sh, Nebius, Oblivus, Oracle, RunPod, SaladCloud, Scaleway, Vast.ai (vast-rental: Stado rents the cheapest verified on-demand offer; selling this fleet's GPU on Vast.ai is the Earning screen), Voltage Park and Vultr — and Boat, which runs a pinned job in one Boat sandbox per job and needs boat.ttl_seconds and boat.release_mode. A vendor is enabled by naming it in providers, setting its required settings, storing its credential under role cloud-<provider>, and declaring how many of its machines may run in config/quotas.json."
                ) {
                    NativeCapabilityActions(host: "", fleet: fleetStore,
                        operations: NativeComputeProviderOperations.all)
                        .disabled(!fleetStore.isConfigured)
                }
                WisentSectionBox(
                    title: "Fleet sandboxes",
                    detail: "Create gVisor sandboxes on a registered Linux server with caller-declared CPU, RAM and process ceilings. These are fleet-owned containers, not virtual machines or Boat rentals. The root filesystem is read-only; /workspace is private and writable; networking is disabled. Stop preserves the workspace; delete removes it permanently. Inspect prerequisites and complete receipts here."
                ) {
                    NativeCapabilityActions(host: "", fleet: fleetStore,
                        operations: NativeFleetSandboxOperations.all)
                        .disabled(!fleetStore.isConfigured)
                }
                WisentSectionBox(
                    title: "Sandboxes",
                    detail: "The Boat sandboxes this deployment's key reaches, driven as an agent drives them: create one, run a command line in it, read and write its files, fork it, stop and resume it, delete it. They use the boat provider's settings and its cloud-boat credential; a sandbox created here lives boat.ttl_seconds unless a lifetime is given."
                ) {
                    NativeCapabilityActions(host: "", fleet: fleetStore,
                        operations: NativeSandboxOperations.all)
                        .disabled(!fleetStore.isConfigured)
                }
            }
            .padding(WisentDesign.Space.x4)
        }
    }
}
