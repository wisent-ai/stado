import AppKit
import SwiftUI
import WisentDesignSystem

// MARK: - Pieces

/// A value the operator has to move somewhere else, and the one button that
/// moves it.
///
/// Its copied state is its own. Threading that through a screen made every
/// block on it redraw whenever any one of them was pressed, and made the
/// secret block indistinguishable from the address block in the code.
struct EnrollmentCopyBlock: View {
    let text: String
    var caption: String?
    /// A secret is set slightly larger and never wrapped into prose: it is
    /// going to be read out loud or pasted, and a mistyped character in it
    /// fails with the same refusal as a revoked invitation.
    var isSecret = false

    @State private var copied = false

    var body: some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x3) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text(verbatim: text)
                    .font(isSecret ? WisentTypography.monoMedium(13) : WisentTypeScale.identifier())
                    .foregroundStyle(WisentDesign.ink)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                if let caption {
                    Text(caption)
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            WisentActionButton(
                action: WisentAction(
                    copied ? "Copied" : "Copy",
                    symbol: copied ? "checkmark" : "doc.on.doc"
                ) {
                    copy()
                }
            )
        }
        .padding(WisentDesign.Space.x4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(WisentDesign.canvasMuted, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small))
        .overlay {
            RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
                .stroke(
                    isSecret ? WisentDesign.warning.opacity(0.45) : WisentDesign.border,
                    lineWidth: WisentDesign.hairline
                )
        }
    }

    private func copy() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        copied = true
        Task {
            try? await Task.sleep(for: .seconds(2))
            copied = false
        }
    }
}

/// A command's own output, verbatim, in the size output belongs in.
struct EnrollmentTranscript: View {
    let text: String

    var body: some View {
        Text(verbatim: text)
            .font(WisentTypeScale.identifierSmall())
            .foregroundStyle(WisentDesign.secondary)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
            .padding(WisentDesign.Space.x3)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(WisentDesign.canvasMuted, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small))
    }
}

/// One thing that has to be true before the next button is worth pressing.
struct EnrollmentChecklistRow: View {
    let text: String

    var body: some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x2) {
            Image(systemName: "circle")
                .font(.system(
                    size:
                        9,
                    weight: .semibold
                ))
                .foregroundStyle(WisentDesign.muted)
                .padding(.top, 3)
                .accessibilityHidden(true)
            Text(text)
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// A pointer to a better door.
///
/// Deliberately not an alert: nothing is wrong, and a warning triangle beside
/// "there is an easier way to do this" is how operators learn to stop reading
/// warning triangles.
struct EnrollmentNote: View {
    let title: String
    let detail: String
    var actions: [WisentAction] = []

    var body: some View {
        WisentPanel {
            HStack(alignment: .top, spacing: WisentDesign.Space.x4) {
                Image(systemName: "arrow.turn.down.right")
                    .font(.system(
                        size:
                            13,
                        weight: .semibold
                    ))
                    .foregroundStyle(WisentDesign.brand)
                    .padding(.top, 1)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    Text(title)
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.ink)
                    Text(detail)
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: WisentDesign.Space.x4)
                if !actions.isEmpty {
                    HStack(spacing: WisentDesign.Space.x2) {
                        ForEach(actions) { WisentActionButton(action: $0) }
                    }
                }
            }
        }
    }
}
