import SwiftUI
import WisentAuth
import WisentDesignSystem

struct DeploymentAccessView: View {
    let deployment: StadoDeployment
    let organization: WisentOrganization

    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .top, spacing: WisentDesign.Space.x4) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                    Text("ORGANIZATION ACCESS")
                        .font(WisentTypeScale.eyebrow())
                        .tracking(0.8)
                        .foregroundStyle(WisentDesign.muted)
                    Text(deployment.name)
                        .font(WisentTypography.heading(20))
                        .foregroundStyle(WisentDesign.ink)
                    Text("Access follows verified organization membership; deployments have no local user or role grants.")
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                }
                Spacer(minLength: 0)
                Button("Done") { dismiss() }
                    .buttonStyle(WisentSecondaryButtonStyle())
            }
            .padding(WisentDesign.Space.x6)

            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
                    WisentSectionBox(
                        title: "Owning organization",
                        detail: "The organization in the signed-in Wisent identity and X-Wisent-Organization-ID must both match this deployment."
                    ) {
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                            WisentField(label: "Organization", value: organization.name)
                            WisentField(label: "Organization ID", value: deployment.organizationID)
                        }
                    }

                    WisentSectionBox(
                        title: "Policy",
                        detail: "Supabase verifies membership centrally before row-level security evaluates the deployment."
                    ) {
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                            policyRow(
                                title: "Read",
                                detail: "Owner, admin, and member roles may read organization deployments."
                            )
                            Divider()
                            policyRow(
                                title: "Change",
                                detail: "Only owner and admin roles may create, update, or delete deployments."
                            )
                        }
                    }
                }
                .padding(WisentDesign.Space.x6)
                .frame(maxWidth: 720)
                .frame(maxWidth: .infinity)
            }
        }
        .frame(minWidth: 720, minHeight: 500)
        .background(WisentDesign.canvas)
    }

    private func policyRow(title: String, detail: String) -> some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x4) {
            Text(title)
                .font(WisentTypeScale.bodyStrong())
                .foregroundStyle(WisentDesign.ink)
                .frame(width: 96, alignment: .leading)
            Text(detail)
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
            Spacer(minLength: 0)
        }
    }
}
