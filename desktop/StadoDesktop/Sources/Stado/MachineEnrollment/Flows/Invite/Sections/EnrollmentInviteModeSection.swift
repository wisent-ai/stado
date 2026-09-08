import SwiftUI
import WisentDesignSystem

/// The one decision that has to be made before an invitation is minted.
///
/// It is a choice about the machine, not a preference, so both options say what
/// they need rather than what they are called. Two rows rather than a segmented
/// control: the difference between them is a sentence each, and a segmented
/// control has room for neither.
struct EnrollmentInviteModeSection: View {
    @ObservedObject var store: MachineEnrollmentStore

    var body: some View {
        WisentSectionBox(
            title: "Which invitation",
            detail: "The one line is less work for you and needs the machine to reach this fleet's control point. The fragment needs nothing of the sort and costs you one message more. If the control point does not answer when you mint, the one line is not offered at all — an invitation that cannot be redeemed is worse than none."
        ) {
            VStack(
                alignment: .leading,
                spacing:
                    0
            ) {
                ForEach(Array([MachineInviteMode.online, .offline].enumerated()), id: \.element) { index, mode in
                    if index > 0 {
                        Divider()
                    }
                    row(mode)
                }
            }
            .background(WisentDesign.surface, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.medium))
            .overlay {
                RoundedRectangle(cornerRadius: WisentDesign.Radius.medium)
                    .stroke(WisentDesign.border, lineWidth: WisentDesign.hairline)
            }
        }
    }

    private func row(_ mode: MachineInviteMode) -> some View {
        let isChosen = store.plan.inviteMode == mode
        return Button {
            store.setInviteMode(mode)
        } label: {
            HStack(alignment: .top, spacing: WisentDesign.Space.x4) {
                Image(systemName: isChosen ? "largecircle.fill.circle" : "circle")
                    .font(.system(
                        size:
                            13,
                        weight: .regular
                    ))
                    .foregroundStyle(isChosen ? WisentDesign.brand : WisentDesign.muted)
                    .padding(.top, 1)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    Text(mode.title)
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.ink)
                    Text(mode.summary)
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    HStack(alignment: .top, spacing: WisentDesign.Space.x3) {
                        Text("NEEDS")
                            .font(WisentTypeScale.eyebrow())
                            .tracking(0.6)
                            .foregroundStyle(WisentDesign.muted)
                            .frame(
                                width:
                                    44,
                                alignment: .leading
                            )
                            .padding(.top, 2)
                        Text(mode.requires)
                            .font(WisentTypeScale.body())
                            .foregroundStyle(WisentDesign.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Text(verbatim: mode == .offline
                        ? "stado fleet invite --name \(name) --offline"
                        : "stado fleet invite --name \(name)")
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.muted)
                        .textSelection(.enabled)
                }
                Spacer(
                    minLength:
                        0
                )
            }
            .padding(WisentDesign.Space.x5)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(isChosen ? WisentTone.brand.softColor : Color.clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(store.isRunning)
        .accessibilityAddTraits(isChosen ? [.isButton, .isSelected] : .isButton)
    }

    private var name: String {
        store.draft.machineName.isEmpty ? "NAME" : store.draft.machineName
    }
}
