import SwiftUI
import WisentDesignSystem

enum StadoLayout {
    static let sidebarMaximumWidth: CGFloat = 280
    static let metricMinimumWidth: CGFloat = 170
    static let progressWidth: CGFloat = 116
    static let emptyStateMinimumHeight: CGFloat = 220
}

struct UnavailableNotice: View {
    let title: String
    let detail: String
    var symbol = "questionmark.circle"

    var body: some View {
        WisentPanel(padding: WisentDesign.Space.x3) {
            HStack(alignment: .top, spacing: WisentDesign.Space.x3) {
                Image(systemName: symbol)
                    .foregroundStyle(WisentDesign.muted)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                    Text(title)
                        .font(WisentTypography.bodyMedium(13))
                        .foregroundStyle(WisentDesign.ink)
                    Text(detail)
                        .font(WisentTypography.body(12))
                        .foregroundStyle(WisentDesign.secondary)
                }
                Spacer(minLength: 0)
            }
        }
        .accessibilityElement(children: .combine)
    }
}
