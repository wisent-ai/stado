import SwiftUI
import WisentAuth
import WisentDesignSystem

/// The screen shell: the stores it reads, the state it holds, and the three
/// things the scroll area can be showing. What each of those renders, the
/// costly-decision dialog, and the provisioning run itself live beside this
/// file in `DeploymentSetup/`.
struct DeploymentSetupView: View {
    @ObservedObject var operationsStore: OperationsStore
    @ObservedObject var cleanupStore: CleanupStore
    @ObservedObject var deploymentStore: DeploymentStore
    @ObservedObject var fleetStore: FleetControlStore
    let identity: WisentIdentity?
    let onComplete: () -> Void

    @State var name = "My Stado"
    @State var selectedTargetID: String?
    @State var update: ProvisioningUpdate?
    @State var errorMessage: String?
    @State var isProvisioning = false
    @State var showsCreateDecision = false

    let provisioner = BackendProvisioner()

    var body: some View {
        VStack(alignment: .leading, spacing:
                0) {
            header
            Divider()
            ScrollView {
                Group {
                    if deploymentStore.isLoading {
                        WisentLoadingPanel(
                            title: "Reading the deployment registry",
                            detail: "The Stado deployments this account may create or read for the selected organization."
                        )
                    } else if let registryError = deploymentStore.errorMessage,
                              deploymentStore.deployments.isEmpty {
                        WisentErrorBanner(
                            title: "Deployment registry unavailable",
                            detail: registryError,
                            action: WisentAction("Retry", symbol: "arrow.clockwise") {
                                Task { await deploymentStore.load(identity: identity) }
                            }
                        )
                    } else if isProvisioning || update != nil {
                        provisioningContent
                    } else {
                        targetContent
                    }
                }
                .padding(WisentDesign.Space.x6)
                .frame(maxWidth:
                        760)
                .frame(maxWidth: .infinity)
            }
        }
        .frame(minWidth:
                760, minHeight:
                620)
        .background(WisentDesign.canvas)
        .task {
            chooseInitialTarget()
            if let deployment = deploymentStore.selectedDeployment,
               deployment.status != .ready {
                selectedTargetID = deployment.targetID
                name = deployment.name
            }
        }
        .onChange(of: deploymentStore.infrastructureTargets) { _, _ in
            chooseInitialTarget()
        }
        .sheet(isPresented: $showsCreateDecision) {
            if let target = selectedTarget {
                createDecision(target)
            }
        }
    }
}
