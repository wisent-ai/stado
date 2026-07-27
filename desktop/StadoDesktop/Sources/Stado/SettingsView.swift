import SwiftUI

struct SettingsView: View {
    @ObservedObject var deploymentStore: DeploymentStore

    var body: some View {
        Form {
            Section("Selected deployment") {
                if let deployment = deploymentStore.selectedDeployment {
                    LabeledContent("Name", value: deployment.name)
                    LabeledContent("Provider", value: deployment.provider.title)
                    LabeledContent("Status", value: deployment.status.rawValue.capitalized)
                    if let endpoint = deployment.endpoint {
                        LabeledContent("Endpoint") {
                            Text(endpoint)
                                .font(.caption.monospaced())
                                .textSelection(.enabled)
                        }
                    }
                } else {
                    Text("Create or select a deployment in the Stado console.")
                        .foregroundStyle(.secondary)
                }
            }

            Section("Configuration ownership") {
                Label(
                    "Deployment endpoints and team access are managed by the Wisent deployment registry. Cloud account identifiers come from Skarbiec; credentials remain in their native keychains and CLIs.",
                    systemImage: "lock.shield"
                )
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .frame(width: 560)
        .fixedSize(horizontal: false, vertical: true)
    }
}
