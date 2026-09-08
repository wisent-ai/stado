import SwiftUI
import WisentDesignSystem

/// The rail down the left of the hand-key screen, and the mark it puts beside
/// each of the five steps.
///
/// Internal rather than private only because `body` sits in the file above
/// this folder: Swift scopes `private` to one file, and the halves of one
/// screen have to stay reachable to each other.
extension MachineHandKeyEnrollmentView {
    // MARK: Rail

    var rail: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            ForEach(MachineEnrollmentStep.allCases) { step in
                railRow(step)
            }
            Spacer(minLength:
                0)
            Text("The registry is written only by the enroll step, and only after the machine has answered.")
                .font(WisentTypography.body(10))
                .foregroundStyle(WisentDesign.muted)
                .fixedSize(horizontal: false, vertical: true)
                .padding(WisentDesign.Space.x3)
        }
        .padding(.vertical, WisentDesign.Space.x4)
        .frame(width:
            232, alignment: .leading)
        .background(WisentDesign.canvasMuted)
        .overlay(alignment: .trailing) {
            Rectangle()
                .fill(WisentDesign.border)
                .frame(width: WisentDesign.hairline)
        }
    }

    private func railRow(_ step: MachineEnrollmentStep) -> some View {
        let isCurrent = store.step == step
        let isDone = isSettled(step)
        let isOpen = store.canOpen(step)
        return Button {
            store.open(step)
        } label: {
            HStack(alignment: .top, spacing: WisentDesign.Space.x3) {
                Image(systemName: railSymbol(step, done: isDone, open: isOpen))
                    .font(.system(size:
                        11, weight: .semibold))
                    .frame(width:
                        16)
                    .foregroundStyle(railSymbolTone(step, done: isDone, open: isOpen, current: isCurrent))
                VStack(alignment: .leading, spacing:
                    1) {
                    Text("\(step.ordinal). \(step.title)")
                        .font(isCurrent ? WisentTypography.bodyMedium(12) : WisentTypography.body(12))
                        .foregroundStyle(isOpen || isCurrent ? WisentDesign.ink : WisentDesign.muted)
                    Text(step.purpose)
                        .font(WisentTypography.body(10))
                        .foregroundStyle(WisentDesign.muted)
                        .fixedSize(horizontal: false, vertical: true)
                        .multilineTextAlignment(.leading)
                }
                Spacer(minLength:
                    0)
            }
            .padding(.horizontal, WisentDesign.Space.x3)
            .padding(.vertical, WisentDesign.Space.x2)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background {
                if isCurrent {
                    RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
                        .fill(WisentDesign.surface)
                        .overlay {
                            RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
                                .stroke(WisentDesign.border, lineWidth: WisentDesign.hairline)
                        }
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.horizontal, WisentDesign.Space.x2)
        .accessibilityAddTraits(isCurrent ? [.isSelected] : [])
    }

    private func railSymbol(_ step: MachineEnrollmentStep, done: Bool, open: Bool) -> String {
        if done { return "checkmark.circle.fill" }
        if !open { return "lock" }
        return store.step == step ? "circle.inset.filled" : "circle"
    }

    private func railSymbolTone(
        _ step: MachineEnrollmentStep,
        done: Bool,
        open: Bool,
        current: Bool
    ) -> Color {
        if done { return WisentDesign.success }
        if !open { return WisentDesign.muted }
        return current ? WisentDesign.brand : WisentDesign.muted
    }
}
