import SwiftUI
import WisentDesignSystem

/// What the screen shows while a deployment is being created, and the row a
/// single infrastructure target renders as.
///
/// `provisioningContent` and `targetButton(_:)` are internal rather than
/// private only because their callers sit in sibling files: Swift scopes
/// `private` to one file.
extension DeploymentSetupView {
    var provisioningContent: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            WisentSectionBox(
                title: "Creating \(name)",
                detail: "Stado verifies the health endpoint before the console reads anything from it.",
                trailing: update.map { "\(Int($0.fraction * 100))%" }
            ) {
                WisentPanel {
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                        ProgressView(value: update?.fraction ?? 0)
                        Text(update?.phase ?? "Starting")
                            .font(WisentTypeScale.bodyStrong())
                            .foregroundStyle(WisentDesign.ink)
                        Text(update?.detail ?? "Waiting for the first provisioning step to report.")
                            .font(WisentTypeScale.body())
                            .foregroundStyle(WisentDesign.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }

            if let errorMessage {
                WisentErrorBanner(
                    title: "Provisioning stopped",
                    detail: errorMessage,
                    action: WisentAction("Try again", symbol: "arrow.clockwise", kind: .primary) {
                        update = nil
                        self.errorMessage = nil
                        Task { await resumeProvisioning() }
                    }
                )
            }
        }
    }

    func targetButton(_ target: InfrastructureTarget) -> some View {
        let selected = selectedTargetID == target.id
        return Button {
            selectedTargetID = target.id
        } label: {
            HStack(spacing: WisentDesign.Space.x3) {
                Image(systemName: target.provider.symbol)
                    .font(.system(size:
                            13, weight: .semibold))
                    .foregroundStyle(selected ? WisentDesign.brand : WisentDesign.muted)
                    .frame(width:
                            24)
                VStack(alignment: .leading, spacing:
                        1) {
                    Text(target.displayName)
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.ink)
                    Text(targetDetail(target))
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.secondary)
                        .lineLimit(2)
                }
                Spacer(minLength:
                        0)
                if target.provider == .local {
                    WisentBadge("This device", tone: .neutral)
                }
                Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                    .foregroundStyle(selected ? WisentDesign.brand : WisentDesign.muted)
            }
            .padding(WisentDesign.Space.x3)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                selected ? WisentDesign.brandSoft : WisentDesign.surface,
                in: RoundedRectangle(cornerRadius: WisentDesign.Radius.medium)
            )
            .overlay {
                RoundedRectangle(cornerRadius: WisentDesign.Radius.medium)
                    .stroke(selected ? WisentDesign.brand : WisentDesign.border, lineWidth: WisentDesign.hairline)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
