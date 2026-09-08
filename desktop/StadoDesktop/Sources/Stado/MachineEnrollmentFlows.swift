import SwiftUI
import WisentDesignSystem

/// The frame every way into the fleet is drawn in, and the entry point to the
/// four methods themselves.
///
/// The methods and the parts they share sit beside this file, one folder per
/// concern:
///
/// - `MachineEnrollment/Pieces/` — the blocks every method draws with.
/// - `MachineEnrollment/Flows/` — Adopt, Join, Declare, and `Invite/`.
///
/// Swift has no per-directory module and no re-export, so every type there is
/// in this same internal namespace: callers keep the names they already use.

// MARK: - Chrome

/// The one shape every way into the fleet is shown in.
///
/// Each method is a different screen because each one asks the operator for
/// different things, but what surrounds them never differs: the name of the
/// method, the work, the last command's own answer, and the way back to the
/// list. Deciding that once here is what keeps four methods from becoming four
/// dialects.
struct EnrollmentChrome<Content: View, Rail: View>: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: MachineEnrollmentStore

    private let eyebrow: String
    private let title: String
    private let detail: String
    private let trailing: (label: String, value: String)?
    private let showsWaysIn: Bool
    private let guidance: String
    private let actions: [WisentAction]
    private let rail: Rail
    private let content: Content

    init(
        store: MachineEnrollmentStore,
        eyebrow: String,
        title: String,
        detail: String,
        trailing: (label: String, value: String)? = nil,
        showsWaysIn: Bool = true,
        guidance: String = "",
        actions: [WisentAction] = [],
        @ViewBuilder rail: () -> Rail,
        @ViewBuilder content: () -> Content
    ) {
        self.store = store
        self.eyebrow = eyebrow
        self.title = title
        self.detail = detail
        self.trailing = trailing
        self.showsWaysIn = showsWaysIn
        self.guidance = guidance
        self.actions = actions
        self.rail = rail()
        self.content = content()
    }

    var body: some View {
        VStack(
            spacing:
                0
        ) {
            header
            Divider()
            HStack(
                spacing:
                    0
            ) {
                if Rail.self != EmptyView.self {
                    rail
                }
                ScrollView {
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
                        if let blockade = store.navigationBlock {
                            WisentAlertPanel(
                                tone: .warning,
                                title: "Not yet",
                                detail: blockade,
                                actions: [
                                    WisentAction("Understood") { store.clearNavigationBlock() }
                                ]
                            )
                        }
                        if let failure = store.failure {
                            WisentAlertPanel(
                                tone: .danger,
                                title: failure.title,
                                // The backend's own sentence sits in the detail
                                // now that the panel has no separate slot for
                                // it. Dropping it would leave the operator with
                                // our paraphrase of a refusal we did not write.
                                detail: failure.backendMessage.isEmpty
                                    ? failure.detail
                                    : "\(failure.detail)\n\n\(failure.backendMessage)"
                            )
                        }
                        content
                    }
                    .padding(WisentDesign.Space.x6)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .background(WisentDesign.canvas)
            }
            Divider()
            footer
        }
        .frame(
            minWidth:
                900,
            minHeight:
                660
        )
        .background(WisentDesign.canvas)
    }

    private var header: some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x4) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text(eyebrow)
                    .font(WisentTypeScale.eyebrow())
                    .tracking(0.8)
                    .foregroundStyle(WisentDesign.muted)
                Text(title)
                    .font(WisentTypography.heading(17))
                    .foregroundStyle(WisentDesign.ink)
                Text(detail)
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(
                minLength:
                    0
            )
            if let trailing {
                VStack(
                    alignment: .trailing,
                    spacing:
                        1
                ) {
                    Text(trailing.label)
                        .font(WisentTypeScale.eyebrow())
                        .tracking(0.8)
                        .foregroundStyle(WisentDesign.muted)
                    Text(trailing.value)
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.secondary)
                }
            }
        }
        .padding(WisentDesign.Space.x6)
        .background(WisentDesign.canvas)
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            WisentMutationBar(outcome: store.outcome, clear: { store.clearOutcome() })
            if !guidance.isEmpty {
                Text(guidance)
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(spacing: WisentDesign.Space.x2) {
                WisentActionButton(action: WisentAction("Close", kind: .plain) { dismiss() })
                if showsWaysIn {
                    WisentActionButton(
                        action: WisentAction("Other ways in", kind: .plain, isEnabled: !store.isRunning) {
                            store.returnToMethods()
                        }
                    )
                }
                Spacer(minLength: WisentDesign.Space.x4)
                ForEach(actions) { WisentActionButton(action: $0) }
            }
        }
        .padding(WisentDesign.Space.x5)
        .background(WisentDesign.canvasMuted)
    }
}

extension EnrollmentChrome where Rail == EmptyView {
    init(
        store: MachineEnrollmentStore,
        eyebrow: String,
        title: String,
        detail: String,
        trailing: (label: String, value: String)? = nil,
        showsWaysIn: Bool = true,
        guidance: String = "",
        actions: [WisentAction] = [],
        @ViewBuilder content: () -> Content
    ) {
        self.init(
            store: store,
            eyebrow: eyebrow,
            title: title,
            detail: detail,
            trailing: trailing,
            showsWaysIn: showsWaysIn,
            guidance: guidance,
            actions: actions,
            rail: { EmptyView() },
            content: content
        )
    }
}
