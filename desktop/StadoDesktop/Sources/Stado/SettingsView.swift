import SwiftUI
import WisentDesignSystem

struct SettingsView: View {
    @ObservedObject var deploymentStore: DeploymentStore
    @ObservedObject var operationsStore: OperationsStore
    @ObservedObject var journey: StadoFirstUseJourney
    @State private var walkthrough: WisentMutationOutcome = .idle

    var body: some View {
        Form {
            Section("Active source") {
                if let deployment = deploymentStore.selectedDeployment {
                    LabeledContent("Name", value: deployment.name)
                    LabeledContent("Provider", value: deployment.provider.title)
                    LabeledContent("Connection", value: connectionStatus)
                    LabeledContent("Deployment", value: deployment.status.rawValue.capitalized)
                    if let endpoint = deployment.endpoint {
                        endpointRow(endpoint)
                    }
                } else {
                    LabeledContent("Name", value: "Local Stado")
                    LabeledContent("Provider", value: "This Mac")
                    LabeledContent("Connection", value: connectionStatus)
                    endpointRow(operationsStore.dashboardURLString)
                }
            }

            Section("Configuration ownership") {
                Label(
                    "The local CLI dashboard is used by default. Remote deployment endpoints and team access are managed by the Wisent deployment registry; credentials remain in their native keychains and CLIs.",
                    systemImage: "lock.shield"
                )
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
            }

            Section("First-run walkthrough") {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                    Text("See the walkthrough this product shows on a first run.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.secondary)
                    WisentActionButton(
                        action: WisentAction(
                            "Show it again",
                            symbol: "arrow.counterclockwise",
                            isEnabled: !walkthrough.isWorking
                        ) { showWalkthroughAgain() }
                    )
                    WisentMutationBar(outcome: walkthrough) { walkthrough = .idle }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .formStyle(.grouped)
        .frame(width: 560)
        .fixedSize(horizontal: false, vertical: true)
    }

    private var connectionStatus: String {
        if operationsStore.snapshot?.ready == true { return "Connected" }
        if operationsStore.errorMessage != nil { return "Unavailable" }
        return operationsStore.isRefreshing ? "Connecting" : "Configured"
    }

    private func showWalkthroughAgain() {
        guard !walkthrough.isWorking else { return }
        walkthrough = .working("Starting the walkthrough…")
        Task { walkthrough = await journey.replay() }
    }

    private func endpointRow(_ endpoint: String) -> some View {
        LabeledContent("Endpoint") {
            Text(endpoint)
                .font(WisentTypeScale.identifier())
                .textSelection(.enabled)
        }
    }
}
