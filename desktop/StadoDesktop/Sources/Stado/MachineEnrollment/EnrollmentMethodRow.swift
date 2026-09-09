import SwiftUI
import WisentDesignSystem

/// One way in, as a row: what it is, what it wants from you, what you get, and
/// the command it runs.
///
/// The command is shown because the screen and the terminal have to be visibly
/// the same thing. An operator who cannot see which command a button runs
/// eventually stops trusting the button.
struct EnrollmentMethodRow: View {
    let title: String
    let summary: String
    let requires: String
    let provides: String
    let command: String
    let refusal: String?
    /// Whether this is the row that gets the one filled button.
    let isRecommended: Bool
    let open: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x5) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                HStack(alignment: .firstTextBaseline, spacing: WisentDesign.Space.x3) {
                    Text(title)
                        .font(WisentTypography.heading(14))
                        .foregroundStyle(refusal == nil ? WisentDesign.ink : WisentDesign.muted)
                    if refusal != nil {
                        WisentStatusChip(text: "Not available", tone: .warning)
                    }
                }
                if !summary.isEmpty {
                    Text(summary)
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    if !requires.isEmpty {
                        labelled("Needs", requires)
                    }
                    if !provides.isEmpty {
                        labelled("Gives", provides)
                    }
                }
                if !command.isEmpty {
                    Text(verbatim: command)
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.muted)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let refusal {
                    Text(refusal)
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.warning)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            VStack(alignment: .trailing, spacing: WisentDesign.Space.x2) {
                WisentActionButton(
                    action: WisentAction(
                        "Use this",
                        kind: isRecommended ? .primary : .secondary,
                        isEnabled: refusal == nil,
                        perform: open
                    )
                )
                if isRecommended {
                    Text("least work")
                        .font(WisentTypeScale.eyebrow())
                        .tracking(0.6)
                        .foregroundStyle(WisentDesign.muted)
                }
            }
        }
        .padding(WisentDesign.Space.x5)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
    }

    /// A label wide enough to line the two values up, because "needs" and
    /// "gives" are read as a pair or not at all.
    private func labelled(_ label: String, _ value: String) -> some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x3) {
            Text(label.uppercased())
                .font(WisentTypeScale.eyebrow())
                .tracking(0.6)
                .foregroundStyle(WisentDesign.muted)
                .frame(width:
                    44, alignment: .leading)
                .padding(.top, 2)
            Text(value)
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
