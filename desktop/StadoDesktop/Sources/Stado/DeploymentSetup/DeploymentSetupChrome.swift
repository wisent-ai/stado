import SwiftUI
import WisentAuth
import WisentDesignSystem

/// The two pieces the screen shows before provisioning starts: the header that
/// names the account, and the name field, target list and create button.
///
/// `header` and `targetContent` are internal rather than private only because
/// the screen shell that reads them sits in a sibling file: Swift scopes
/// `private` to one file.
extension DeploymentSetupView {
    var header: some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x4) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text("NEW DEPLOYMENT")
                    .font(WisentTypeScale.eyebrow())
                    .tracking(0.8)
                    .foregroundStyle(WisentDesign.muted)
                Text("Choose where this Stado control plane runs")
                    .font(WisentTypography.heading(17))
                    .foregroundStyle(WisentDesign.ink)
                Text("The queue, the host registry, and the cleanup service all live wherever this deployment runs.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            }
            Spacer(minLength:
                0)
            if let identity {
                VStack(alignment: .trailing, spacing:
                        1) {
                    Text(identity.organization.name)
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.ink)
                    Text(identity.email)
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.secondary)
                }
            }
        }
        .padding(WisentDesign.Space.x6)
    }

    var targetContent: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x6) {
            WisentSectionBox(title: "Name", detail: "Shown in the source selector and in the deployment registry.") {
                TextField("My Stado", text: $name)
                    .textFieldStyle(.roundedBorder)
                    .font(WisentTypeScale.body())
            }

            WisentSectionBox(
                title: "Run the backend on",
                detail: "Infrastructure accounts come from Skarbiec, which shares account identifiers and never credentials.",
                trailing: "\(deploymentStore.infrastructureTargets.count.formatted(.number)) discovered"
            ) {
                if deploymentStore.infrastructureTargets.isEmpty {
                    WisentEmptyPanel(
                        title: "No infrastructure discovered",
                        detail: "Open Skarbiec, refresh Infrastructure, then retry. Stado never enumerates cloud accounts on its own.",
                        symbol: "network.slash",
                        action: WisentAction("Refresh infrastructure", symbol: "arrow.clockwise", kind: .primary) {
                            Task { await deploymentStore.load(identity: identity) }
                        }
                    )
                } else {
                    VStack(spacing: WisentDesign.Space.x2) {
                        ForEach(deploymentStore.infrastructureTargets) { target in
                            targetButton(target)
                        }
                    }
                }
            }

            if let identity {
                WisentSectionBox(
                    title: "Organization ownership",
                    detail: "This deployment belongs to \(identity.organization.name). Every member may read it; only organization owners and admins may change it."
                ) {
                    WisentField(label: "Organization", value: identity.organization.name)
                }
            }

            if let errorMessage {
                WisentErrorBanner(title: "Deployment could not be created", detail: errorMessage)
            }

            HStack(spacing: WisentDesign.Space.x2) {
                WisentActionButton(
                    action: WisentAction("Refresh infrastructure", symbol: "arrow.clockwise") {
                        Task { await deploymentStore.load(identity: identity) }
                    }
                )
                Spacer(minLength:
                    0)
                WisentActionButton(
                    action: WisentAction(
                        "Create and start Stado…",
                        symbol: "bolt.horizontal",
                        kind: .primary,
                        isEnabled: selectedTarget != nil && !trimmedName.isEmpty
                    ) {
                        showsCreateDecision = true
                    }
                )
            }
        }
    }
}
