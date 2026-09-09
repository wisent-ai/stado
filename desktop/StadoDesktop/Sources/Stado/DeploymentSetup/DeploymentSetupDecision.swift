import SwiftUI
import WisentAuth
import WisentDesignSystem

/// The dialog that stands between the create button and the first billed API
/// call, and the two lists it reads from.
///
/// `createDecision(_:)` is internal rather than private only because the sheet
/// that presents it sits in a sibling file: Swift scopes `private` to one file.
extension DeploymentSetupView {
    // MARK: Costly decision

    /// Creating a cloud deployment bills the operator's own account from the
    /// first step. The dialog names every resource before the first API call,
    /// and the destructive verb is the one that spends money.
    func createDecision(_ target: InfrastructureTarget) -> some View {
        WisentDecisionDialog(
            tone: target.provider == .local ? .warning : .danger,
            title: "Create \(trimmedName) on \(target.displayName)?",
            lines: decisionLines(target),
            reasonCode: nil,
            listing: decisionListing(target),
            footnote: "\(identity?.organization.name ?? "The selected organization") owns the deployment. Members may read it; only owners and admins may change it.",
            actions: [
                WisentAction("Do not create it", kind: .primary) { showsCreateDecision = false },
                WisentAction(
                    target.provider == .local ? "Install locally" : "Create and bill this account",
                    kind: .destructive
                ) {
                    showsCreateDecision = false
                    Task { await beginProvisioning() }
                },
            ]
        )
    }

    func decisionLines(_ target: InfrastructureTarget) -> [String] {
        switch target.provider {
        case .local:
            return [
                "Stado installs a control plane on this Mac and registers a launch agent that keeps it running after logout.",
                "Storage stays on this device. Removing the deployment later is a separate manual step.",
            ]
        case .gcp:
            return [
                "Stado enables Cloud Run, Cloud Build, Artifact Registry, and storage APIs in \(target.externalID), then builds an image and deploys a service.",
                "Every resource it creates bills that Google Cloud account until it is deleted, and this console cannot delete them.",
            ]
        case .aws:
            return [
                "Stado creates the storage, identity, and container resources this control plane needs in AWS account \(target.externalID), then starts the service.",
                "Every resource it creates bills that AWS account until it is deleted, and this console cannot delete them.",
            ]
        case .azure:
            return [
                "Stado creates the resource group, registry, storage, and container app this control plane needs in Azure subscription \(target.externalID).",
                "Every resource it creates bills that subscription until it is deleted, and this console cannot delete them.",
            ]
        }
    }

    func decisionListing(_ target: InfrastructureTarget) -> [String] {
        [
            "provider: \(target.provider.rawValue)",
            "account: \(target.externalID)",
            "deployment name: \(trimmedName)",
            "organization: \(identity?.organization.name ?? "Sign in required")",
        ]
    }
}
