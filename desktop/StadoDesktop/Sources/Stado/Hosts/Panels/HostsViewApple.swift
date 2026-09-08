import SwiftUI
import WisentDesignSystem

extension HostsView {
    @ViewBuilder
    func appleChallengeSection(for host: WorkerNode) -> some View {
        let target = host.targetName ?? host.displayName
        WisentSectionBox(
            title: "Apple code capture",
            detail: "Prepares the signed Apple helper in the registry-bound macOS session. It leaves CuaDriver unchanged and does not sign in to Apple."
        ) {
            WisentField(
                label: "Read-only command",
                value: StadoCLI.commandLine(FleetControlStore.appleChallengeStatusArguments(host: target))
            )
            WisentActionButton(
                action: WisentAction(
                    "Read Apple readiness",
                    symbol: "arrow.clockwise",
                    isEnabled: !fleetStore.appleChallengeMutation.isWorking
                ) {
                    Task { await fleetStore.readAppleChallenge(host: target) }
                }
            )
            WisentField(
                label: "Command",
                value: StadoCLI.commandLine(FleetControlStore.appleChallengeArguments(host: target))
            )
            WisentActionButton(
                action: WisentAction(
                    "Prepare Apple code capture",
                    symbol: "key",
                    isEnabled: !fleetStore.appleChallengeMutation.isWorking
                ) {
                    Task { await fleetStore.prepareAppleChallenge(host: target) }
                }
            )
            if fleetStore.appleChallengeHost == target {
                WisentMutationBar(outcome: fleetStore.appleChallengeMutation) {
                    fleetStore.clearAppleChallengeMutation()
                }
                if let receipt = fleetStore.appleChallengeReceipt {
                    WisentField(label: "Reported host", value: receipt.target)
                    WisentField(label: "Host-control destination", value: receipt.sshTarget)
                    ForEach(receipt.items.indices, id: \.self) { index in
                        Text(receipt.items[index].joined(separator: ": "))
                            .font(WisentTypeScale.body())
                            .textSelection(.enabled)
                    }
                    if let error = receipt.error {
                        WisentAlertPanel(
                            tone: .warning,
                            title: "Apple code capture is unavailable",
                            detail: error
                        )
                    }
                }
            }
        }
    }
}
