import AppKit
import SwiftUI
import WisentDesignSystem

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
        VStack(spacing: 0) {
            header
            Divider()
            HStack(spacing: 0) {
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
                                detail: failure.detail,
                                command: failure.backendMessage.isEmpty ? nil : failure.backendMessage
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
        .frame(minWidth: 900, minHeight: 660)
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
            Spacer(minLength: 0)
            if let trailing {
                VStack(alignment: .trailing, spacing: 1) {
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
                .font(.system(size: 9, weight: .semibold))
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
                    .font(.system(size: 13, weight: .semibold))
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

/// One proof, with the command that produced it and whatever it printed.
struct EnrollmentCheckPanel: View {
    let check: MachineEnrollmentCheck?
    let fallbackCommand: String

    var body: some View {
        WisentPanel {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                HStack(spacing: WisentDesign.Space.x2) {
                    Text(verbatim: check?.command ?? fallbackCommand)
                        .font(WisentTypeScale.identifier())
                        .foregroundStyle(WisentDesign.ink)
                        .textSelection(.enabled)
                    Spacer(minLength: WisentDesign.Space.x3)
                    if let check {
                        WisentStatusChip(
                            text: check.ok ? "Answered" : "Refused",
                            tone: check.ok ? .success : .danger
                        )
                    } else {
                        Text("Not run yet")
                            .font(WisentTypeScale.identifierSmall())
                            .foregroundStyle(WisentDesign.muted)
                    }
                }
                if let check, !check.output.isEmpty {
                    EnrollmentTranscript(text: check.output)
                }
            }
        }
    }
}

/// The two proofs that a registry entry is a working machine and not a row.
///
/// They are the same two commands whichever way the machine got in, so they
/// are one view rather than one per method.
struct EnrollmentProofSection: View {
    @ObservedObject var store: MachineEnrollmentStore

    var body: some View {
        WisentSectionBox(
            title: "The two proofs",
            detail: "The first opens the channel with the stored key and reads back the hostname the machine answers with. The second asks Stado to recover the host, which is what makes it start publishing the capacity reports the Hosts table reads."
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                EnrollmentCheckPanel(
                    check: store.draft.channelCheck,
                    fallbackCommand: "stado fleet key check \(store.draft.machineName)"
                )
                EnrollmentCheckPanel(
                    check: store.draft.agentRecovery,
                    fallbackCommand: "stado host recover \(store.draft.machineName)"
                )
            }
        }
    }
}

/// A machine waiting for a decision, with the decision.
///
/// The address is the fact worth reading twice: it came from the machine, not
/// from the operator, and approval is going to connect to it.
struct EnrollmentRequestPanel: View {
    @ObservedObject var store: MachineEnrollmentStore
    let request: FleetPendingRequest
    let refresh: () async -> Void

    var body: some View {
        WisentPanel {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
                HStack(alignment: .firstTextBaseline, spacing: WisentDesign.Space.x3) {
                    Text(request.registryName)
                        .font(WisentTypography.heading(14))
                        .foregroundStyle(WisentDesign.ink)
                        .textSelection(.enabled)
                    WisentStatusChip(
                        text: request.inviteID == nil ? "Reported itself" : "Answered an invitation",
                        tone: request.inviteID == nil ? .neutral : .brand
                    )
                    Spacer(minLength: WisentDesign.Space.x3)
                    Text(ConsoleFormat.relative(request.requestedDate))
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.muted)
                }

                HStack(alignment: .top, spacing: WisentDesign.Space.x5) {
                    WisentField(
                        label: "Hostname it reported",
                        value: request.hostname
                    )
                    WisentField(
                        label: "Address it reported",
                        value: request.isReachable ? (request.destination ?? "") : "none",
                        tone: request.isReachable ? .neutral : .warning
                    )
                }

                HStack(alignment: .top, spacing: WisentDesign.Space.x5) {
                    WisentField(label: "Platform", value: request.platform)
                    WisentField(
                        label: "Key it installed",
                        value: request.installedKeyFingerprint ?? "not reported"
                    )
                }

                if request.targetName != nil, request.targetName != request.hostname {
                    Text("The entry will be called \(request.registryName), from the invitation — not \(request.hostname), which is what the machine calls itself. That is the name to look for in the Hosts table and to type after every stado command.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }

                if request.isReachable {
                    Text("Approval opens a channel to that address, asks the machine for its hostname and platform, and writes the registry entry only after it answers. If the agent install then fails, the entry is removed again.")
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                } else {
                    Text("This machine reported itself without an address, so approval has nothing to connect back to. Add it with Adopt or the hand-installed key instead, using an address you know reaches it.")
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.warning)
                        .fixedSize(horizontal: false, vertical: true)
                }

                HStack(spacing: WisentDesign.Space.x2) {
                    WisentActionButton(
                        action: WisentAction(
                            "Approve \(request.registryName)",
                            symbol: "checkmark.shield",
                            kind: .primary,
                            isEnabled: !store.isRunning && request.isReachable
                        ) {
                            Task {
                                await store.approve(request)
                                await refresh()
                            }
                        }
                    )
                    WisentActionButton(
                        action: WisentAction(
                            "Reject",
                            kind: .destructive,
                            isEnabled: !store.isRunning
                        ) {
                            Task { await store.reject(request) }
                        }
                    )
                }
            }
        }
    }
}

/// Every machine waiting for a decision, or the reason there are none.
struct EnrollmentRequestList: View {
    @ObservedObject var store: MachineEnrollmentStore
    let refresh: () async -> Void

    var body: some View {
        WisentSectionBox(
            title: "Waiting for a decision",
            detail: "Read from the request store, not from the registry. Nothing here is in the fleet yet.",
            trailing: store.plan.pendingReadAt == nil
                ? "not read yet"
                : "read \(ConsoleFormat.relative(store.plan.pendingReadAt))"
        ) {
            if store.waitingRequests.isEmpty {
                WisentPanel {
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                        Text("No machine is waiting.")
                            .font(WisentTypeScale.bodyStrong())
                            .foregroundStyle(WisentDesign.ink)
                        Text("A request appears here within seconds of the machine running its join command. This screen keeps reading the store while it is open, so there is nothing to press.")
                            .font(WisentTypeScale.body())
                            .foregroundStyle(WisentDesign.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            } else {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                    ForEach(store.waitingRequests) { request in
                        EnrollmentRequestPanel(store: store, request: request, refresh: refresh)
                    }
                }
            }
        }
    }
}

/// What approval or rejection actually did, in its own words.
struct EnrollmentDecisionSection: View {
    let decision: MachineEnrollmentCheck

    var body: some View {
        WisentSectionBox(
            title: decision.ok ? "The last decision" : "The last decision did not land",
            detail: "Verbatim, from the command the button ran.",
            trailing: ConsoleFormat.relative(decision.ranAt)
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text(verbatim: decision.command)
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(WisentDesign.ink)
                    .textSelection(.enabled)
                if !decision.output.isEmpty {
                    EnrollmentTranscript(text: decision.output)
                }
            }
        }
    }
}

/// The name field, with the two refusals that would otherwise arrive after the
/// operator had already done the work.
struct EnrollmentNameSection: View {
    @ObservedObject var store: MachineEnrollmentStore
    let existingNames: Set<String>
    let detail: String
    var isLocked = false

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            WisentSectionBox(title: "Machine name", detail: detail) {
                TextField(
                    "studio",
                    text: Binding(
                        get: { store.draft.machineName },
                        set: { store.setMachineName($0) }
                    )
                )
                .textFieldStyle(.roundedBorder)
                .font(WisentTypeScale.body())
                .disabled(isLocked)
            }
            if let problem = MachineName.problem(with: store.draft.machineName),
               !store.draft.machineName.isEmpty {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The registry will refuse this name",
                    detail: problem
                )
            } else if existingNames.contains(store.draft.machineName) {
                WisentAlertPanel(
                    tone: .danger,
                    title: "\(store.draft.machineName) is already in this fleet",
                    detail: "Enrollment refuses to overwrite a machine that already has a channel or a health beacon, so this attempt would fail at its last step. Pick a different name."
                )
            }
        }
    }
}

// MARK: - Invite

/// One code, sent to whoever has the machine.
///
/// This is the method that exists because the operator cannot always reach the
/// machine and should not have to. It has two halves separated by somebody
/// else's attention span: the code is minted and sent in a minute, and the
/// answer arrives whenever that person gets to it. The second half is why this
/// screen is written down rather than held in the window.
struct EnrollmentInviteView: View {
    @ObservedObject var store: MachineEnrollmentStore
    let existingNames: Set<String>
    let refresh: () async -> Void

    var body: some View {
        EnrollmentChrome(
            store: store,
            eyebrow: MachineEnrollmentFlow.invite.eyebrow,
            title: title,
            detail: detail,
            trailing: trailing,
            guidance: guidance,
            actions: actions
        ) {
            if let invite = store.mintedInvite {
                code(invite)
            }
            if let record = store.plan.invite {
                outstanding(record)
            } else if let approved = store.plan.approvedName {
                settled(approved)
            } else {
                EnrollmentNameSection(
                    store: store,
                    existingNames: existingNames,
                    detail: "The name the canonical registry will use for this machine once you approve it. The invitation carries it, so the person you send the code to does not get to choose it."
                )
                expectations
            }
            if let decision = store.plan.decision {
                EnrollmentDecisionSection(decision: decision)
            }
        }
        .task(id: store.plan.invite?.id) {
            guard store.plan.invite != nil else { return }
            await store.watchPending()
        }
    }

    private var title: String {
        if let record = store.plan.invite { return "Invitation for \(record.targetName)" }
        if let approved = store.plan.approvedName { return "\(approved) is in the fleet" }
        return "Invite a machine into the fleet"
    }

    /// The header says what this screen is for right now, not what the method
    /// is in general. An operator looking at a machine that already joined does
    /// not need the pitch again.
    private var detail: String {
        if store.plan.approvedName != nil, store.plan.invite == nil {
            return "The invitation was answered and approved. Everything below is what that took: what the machine reported, what approval ran, and the two proofs that turn the entry into a working machine."
        }
        if store.plan.invitedRequest != nil {
            return "The machine ran the line and installed the key. Approving it opens the channel, asks it what it is, and writes the registry entry only after it answers. Rejecting writes nothing at all."
        }
        return "You mint one code and send one line. Whoever has the machine runs it there, which puts the fleet's public key into their authorized_keys and makes the machine report itself back here. Then you approve it. You never open a session to the machine and the code carries nothing that can reach into this fleet."
    }

    private var trailing: (label: String, value: String)? {
        guard let record = store.plan.invite else { return nil }
        if record.isExpired {
            return ("INVITATION", "expired")
        }
        guard let expiry = record.expiryDate else {
            return ("INVITATION", record.id)
        }
        return ("EXPIRES", ConsoleFormat.relative(expiry))
    }

    private var guidance: String {
        if let request = store.plan.invitedRequest {
            return "\(request.hostname) is waiting for your decision. Nothing has been written to the registry for it yet."
        }
        if store.mintedInvite != nil {
            return "Send the code before you leave this screen: this app cannot show it again. If it is lost, revoke the invitation and mint another."
        }
        if let record = store.plan.invite {
            return record.isExpired
                ? "This invitation has expired. A machine answering it now is refused. Revoke it and mint another when you are ready."
                : "Waiting for \(record.targetName) to answer. This screen reads the request store while it is open, and remembers what it is waiting for when it is not."
        }
        if store.plan.approvedName != nil {
            return "The invitation is spent and cannot be answered again. Inviting another machine mints a new code for a new name."
        }
        return "Minting writes one invitation into the store and one key pair into the credential store. Nothing is written to the registry until you approve the machine that answers."
    }

    private var actions: [WisentAction] {
        if store.plan.invite == nil, store.plan.approvedName != nil {
            return [
                WisentAction(
                    "Invite another machine",
                    kind: .plain,
                    isEnabled: !store.isRunning
                ) {
                    store.startAnother(keepingMethod: true)
                },
                WisentAction(
                    "Run the checks",
                    symbol: "checkmark.shield",
                    kind: .primary,
                    isEnabled: !store.isRunning && store.isConfigured
                ) {
                    Task {
                        await store.verify()
                        await refresh()
                    }
                },
            ]
        }
        guard store.plan.invite != nil else {
            return [
                WisentAction(
                    "Mint an invitation",
                    symbol: "envelope",
                    kind: .primary,
                    isEnabled: !store.isRunning
                        && store.isConfigured
                        && MachineName.problem(with: store.draft.machineName) == nil
                        && !existingNames.contains(store.draft.machineName)
                ) {
                    Task { await store.mintInvite() }
                },
            ]
        }
        return [
            WisentAction("Revoke this invitation", kind: .destructive, isEnabled: !store.isRunning) {
                Task { await store.revokeInvite() }
            },
            WisentAction(
                "Check for the reply now",
                symbol: "arrow.clockwise",
                // Secondary once the reply is on screen: the decision in the
                // panel above is the action then, and two filled buttons would
                // compete for it.
                kind: store.plan.invitedRequest == nil ? .primary : .secondary,
                isEnabled: !store.isRunning
            ) {
                Task { await store.refreshPending(announce: true) }
            },
        ]
    }

    @ViewBuilder
    private func code(_ invite: MachineInvite) -> some View {
        WisentAlertPanel(
            tone: .warning,
            title: "This code is shown once",
            detail: "It is not written to disk, not kept in this window's saved state, and cannot be printed again by any command. Send it now. If it is lost, revoke this invitation and mint another — that costs one message, and reading a secret back from a file would cost the fleet."
        )

        WisentSectionBox(
            title: "One line to send to whoever has the machine",
            detail: "They run it on the machine being added. It fetches the join script from this control plane, installs the fleet's public key into their authorized_keys, and reports the machine back here.",
            trailing: "uses \(invite.usesAllowed)"
        ) {
            EnrollmentCopyBlock(
                text: invite.joinCommand,
                caption: "Assembled by the control plane against its own public address, so it is right from wherever that person is.",
                isSecret: true
            )
        }

        WisentSectionBox(
            title: "The code on its own",
            detail: "For a person who would rather paste the code into a prompt than run a line they did not write."
        ) {
            EnrollmentCopyBlock(text: invite.token, isSecret: true)
        }

        if !invite.authorizedKeysLine.isEmpty {
            WisentSectionBox(
                title: "Or the key, by hand",
                detail: "The same public key the script installs. Somebody who will not run a script can append this line to ~/.ssh/authorized_keys on the machine instead, and you can then adopt it by address."
            ) {
                EnrollmentCopyBlock(text: invite.authorizedKeysLine)
            }
        }
    }

    @ViewBuilder
    private func outstanding(_ record: MachineInviteRecord) -> some View {
        WisentSignalStrip(signals: [
            WisentSignal("Invitation", value: record.id, tone: .neutral),
            WisentSignal("Machine", value: record.targetName, tone: .neutral),
            WisentSignal(
                "State",
                value: record.isExpired ? "Expired" : (store.plan.invitedRequest == nil ? "Open" : "Answered"),
                tone: record.isExpired ? .warning : (store.plan.invitedRequest == nil ? .brand : .success)
            ),
            WisentSignal("Uses", value: "\(record.usesAllowed)", tone: .neutral),
        ])

        if let request = store.plan.invitedRequest {
            WisentSectionBox(
                title: "\(request.hostname) answered for \(record.targetName)",
                detail: "The machine ran the line, installed the key, and reported itself. Approving it runs the same probing enrollment as any other way in."
            ) {
                EnrollmentRequestPanel(store: store, request: request, refresh: refresh)
            }
        } else {
            WisentSectionBox(
                title: "Waiting for the machine to answer",
                detail: "Nothing is expected of you until it does. This screen reads the request store every few seconds while it is open, and closing the app does not lose the invitation.",
                trailing: store.plan.pendingReadAt == nil
                    ? "not read yet"
                    : "read \(ConsoleFormat.relative(store.plan.pendingReadAt))"
            ) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                    EnrollmentChecklistRow(
                        text: record.isExpired
                            ? "Minted \(ConsoleFormat.relative(record.mintedAt)) and expired since. A machine answering it now is refused."
                            : "Minted \(ConsoleFormat.relative(record.mintedAt)). A machine answering after it expires is refused, and the expiry is in the corner above."
                    )
                    if store.mintedInvite == nil {
                        EnrollmentChecklistRow(
                            text: "The code for this invitation is not kept anywhere in this app. If the person you sent it to lost it, revoke this invitation and mint another."
                        )
                    }
                    if !record.publicKey.isEmpty {
                        EnrollmentChecklistRow(
                            text: "The key pair for \(record.targetName) already exists in the credential store. Revoking the invitation does not remove it; stado fleet key rm \(record.targetName) does."
                        )
                    }
                }
            }
        }

        if othersWaiting > 0 {
            EnrollmentNote(
                title: "\(othersWaiting) other machine\(othersWaiting == 1 ? "" : "s") waiting for a decision",
                detail: "They did not answer this invitation. The Join method lists every request in the store, including those.",
                actions: [WisentAction("Go to Join") { store.open(.join) }]
            )
        }
    }

    /// The end of the method: a machine that answered, was probed, and is now
    /// a registry entry. Said plainly, with the one thing left to look at.
    @ViewBuilder
    private func settled(_ approved: String) -> some View {
        WisentSignalStrip(signals: [
            WisentSignal("Machine", value: approved, tone: .success),
            WisentSignal("Invitation", value: "Spent", tone: .neutral),
            WisentSignal(
                "Registry entry",
                value: store.draft.isEnrolled ? "Written" : "Not written",
                tone: store.draft.isEnrolled ? .success : .neutral
            ),
        ])

        WisentSectionBox(
            title: "Nothing else is waiting",
            detail: "The invitation was answered once and cannot be answered again. Approval opened the channel, asked \(approved) what it was, and wrote the entry only after it answered — what that command printed is below, verbatim."
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                EnrollmentChecklistRow(text: "The Hosts table is where \(approved) shows up next, with the capacity reports its agent publishes.")
                EnrollmentChecklistRow(text: "The key pair for it stays in the credential store. Nothing about the private half ever left this control plane.")
            }
        }

        EnrollmentProofSection(store: store)
    }

    private var othersWaiting: Int {
        guard let record = store.plan.invite else { return 0 }
        return store.waitingRequests.filter { $0.inviteID != record.id }.count
    }

    private var expectations: some View {
        WisentSectionBox(title: "What the other person has to do") {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                EnrollmentChecklistRow(text: "Turn on Remote Login on the machine. On macOS: System Settings, General, Sharing, Remote Login. On Linux: enable sshd.")
                EnrollmentChecklistRow(text: "Paste one line into a terminal on that machine and press return.")
                EnrollmentChecklistRow(text: "Nothing else. They do not install Stado, do not hold any credential for this fleet, and cannot read anything in it with the code.")
            }
        }
    }
}

// MARK: - Adopt

/// A machine the operator can already open a session to.
///
/// The whole method is the one flag the old path did not have. Stado installs
/// the public key over the session that already works, and the walk to another
/// computer stops existing.
struct EnrollmentAdoptView: View {
    @ObservedObject var store: MachineEnrollmentStore
    let existingNames: Set<String>
    let refresh: () async -> Void

    var body: some View {
        EnrollmentChrome(
            store: store,
            eyebrow: MachineEnrollmentFlow.adopt.eyebrow,
            title: adoptTitle,
            detail: store.draft.isEnrolled
                ? "The key went on over the session the control plane could already open, the machine answered the probe, and the entry was written. The two proofs below are what turn that entry into a machine the rest of the console can read."
                : "One command, for a machine the control plane can already open a session to — a key of yours already on it, or a credential in an SSH agent it can reach. Stado installs the fleet's own public key over that session, then probes the machine and writes the entry only if it answers.",
            trailing: store.draft.isEnrolled ? ("ENROLLED", ConsoleFormat.relative(store.draft.enrolledAt)) : nil,
            guidance: guidance,
            actions: actions
        ) {
            if store.draft.isEnrolled {
                WisentSignalStrip(signals: [
                    WisentSignal("Machine", value: store.draft.machineName, tone: .success),
                    WisentSignal("Address", value: store.draft.sshTarget, tone: .neutral),
                    WisentSignal("Registry entry", value: "Written", tone: .success),
                ])

                if !store.draft.enrollmentTranscript.isEmpty {
                    WisentSectionBox(
                        title: "What Stado did",
                        detail: "Verbatim, from the command that installed the key and wrote the entry."
                    ) {
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                            Text(verbatim: store.draft.adoptCommand)
                                .font(WisentTypeScale.identifier())
                                .foregroundStyle(WisentDesign.ink)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                            EnrollmentTranscript(text: store.draft.enrollmentTranscript)
                        }
                    }
                }

                EnrollmentProofSection(store: store)
            } else {
                EnrollmentNameSection(
                    store: store,
                    existingNames: existingNames,
                    detail: "The identifier the canonical registry, the capacity reports, and every stado command will use for this machine. Lowercase letters, digits, and the characters . - _"
                )

                WisentSectionBox(
                    title: "SSH address",
                    detail: "The address the machine running the Stado control plane reaches this one at, written as it would be typed after ssh. A Bonjour name on the same network is as good as a tailnet name."
                ) {
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                        TextField(
                            "lukasz@studio.local",
                            text: Binding(
                                get: { store.draft.sshTarget },
                                set: { store.setSSHTarget($0) }
                            )
                        )
                        .textFieldStyle(.roundedBorder)
                        .font(WisentTypeScale.body())
                        Text(verbatim: "Examples: lukasz@studio.local, lukasz@100.92.4.11, lukasz@studio.tailnet-name.ts.net")
                            .font(WisentTypeScale.identifierSmall())
                            .foregroundStyle(WisentDesign.muted)
                    }
                }

                WisentAlertPanel(
                    tone: .warning,
                    title: "A password cannot be typed into this window",
                    detail: "The session is opened by the machine hosting the Stado control plane, with its own ssh, and that process has no terminal. OpenSSH there cannot prompt for a password or a key passphrase, and this app has nothing to capture: what works from here is a key already on \(store.draft.sshTarget.isEmpty ? "the machine" : store.draft.sshTarget), or a credential loaded into an SSH agent that control-plane host can reach. If a password is the only credential you have, run the command below in a terminal on that host and answer it there — or send an invitation instead, which needs no credential from you at all.",
                    actions: [
                        WisentAction("Invite instead", isEnabled: store.isPermitted(.invite) && !store.isRunning) {
                            store.open(.invite)
                        },
                    ]
                )

                WisentSectionBox(
                    title: "What this runs",
                    detail: "The key install happens first, over the session you can already open. Everything after it is the same enrollment every other way in performs."
                ) {
                    WisentPanel {
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                            Text(verbatim: store.draft.adoptCommand)
                                .font(WisentTypeScale.identifier())
                                .foregroundStyle(WisentDesign.ink)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                            Text("--install-key appends the fleet's public key to ~/.ssh/authorized_keys of the user in that address. The private half stays in the credential store here and is never sent. --bootstrap then installs the Stado agent, and if that install fails the registry entry written a moment earlier is removed again.")
                                .font(WisentTypeScale.caption())
                                .foregroundStyle(WisentDesign.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }
        }
    }

    private var adoptTitle: String {
        if store.draft.isEnrolled { return "\(store.draft.machineName) is in the fleet" }
        return store.draft.machineName.isEmpty
            ? "Adopt a machine you can already reach"
            : "Adopt \(store.draft.machineName)"
    }

    private var isReady: Bool {
        !store.isRunning
            && store.isConfigured
            && MachineName.problem(with: store.draft.machineName) == nil
            && !existingNames.contains(store.draft.machineName)
            && store.draft.hasChannel
    }

    private var guidance: String {
        if store.draft.isEnrolled {
            return "\(store.draft.machineName) has a registry entry. The two proofs below are what turn that entry into a machine the Hosts table can read."
        }
        return "Nothing is written until the machine answers the probe, and a failed agent install takes the entry away again. There is no half-added machine to hunt for after a failure here."
    }

    private var actions: [WisentAction] {
        if store.draft.isEnrolled {
            return [
                WisentAction("Add another machine", kind: .plain, isEnabled: !store.isRunning) {
                    store.startAnother()
                },
                WisentAction(
                    "Run the checks",
                    symbol: "checkmark.shield",
                    kind: .primary,
                    isEnabled: !store.isRunning && store.isConfigured
                ) {
                    Task {
                        await store.verify()
                        await refresh()
                    }
                },
            ]
        }
        return [
            WisentAction(
                store.draft.machineName.isEmpty ? "Adopt this machine" : "Adopt \(store.draft.machineName)",
                symbol: "arrow.right.circle",
                kind: .primary,
                isEnabled: isReady
            ) {
                Task {
                    await store.adopt()
                    await refresh()
                }
            },
        ]
    }
}

// MARK: - Join

/// A machine that comes to the fleet on its own.
///
/// The oldest of the four and still the right one when the machine is already
/// trusted enough to hold credentials for this fleet's store: a build agent, a
/// rented box provisioned from an image, anything that runs Stado before
/// anyone thinks about adding it.
struct EnrollmentJoinView: View {
    @ObservedObject var store: MachineEnrollmentStore
    let refresh: () async -> Void

    var body: some View {
        EnrollmentChrome(
            store: store,
            eyebrow: MachineEnrollmentFlow.join.eyebrow,
            title: "Approve a machine that reported itself",
            detail: "The machine puts its own hand up. It needs Stado and credentials for this fleet's store to do that, which is exactly why this method suits a machine you provisioned and not somebody's laptop. Your part is the decision at the end.",
            guidance: "Approving runs the probing enrollment: the channel opens, the machine is asked what it is, and only then is the registry written. Rejecting writes nothing at all.",
            actions: [
                WisentAction(
                    "Check for requests now",
                    symbol: "arrow.clockwise",
                    kind: store.waitingRequests.isEmpty ? .primary : .secondary,
                    isEnabled: !store.isRunning
                ) {
                    Task { await store.refreshPending(announce: true) }
                },
            ]
        ) {
            WisentSectionBox(
                title: "What has to be true on that machine",
                detail: "This method asks more of the machine than the others and less of you. Nothing here is done from this window."
            ) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    EnrollmentChecklistRow(text: "Stado is installed on it and can read this fleet's store — the same credentials any fleet member holds.")
                    EnrollmentChecklistRow(text: "Remote Login is on, because approval opens a channel back to it.")
                    EnrollmentChecklistRow(text: "Somebody runs the join command there. It writes a request into the store and waits.")
                }
            }

            if let method = store.method(named: "join"), !method.command.isEmpty {
                WisentSectionBox(
                    title: "The command that machine runs",
                    detail: "Reported by this control plane, so it is the spelling this release accepts."
                ) {
                    EnrollmentCopyBlock(text: method.command)
                }
            }

            EnrollmentRequestList(store: store, refresh: refresh)

            if let decision = store.plan.decision {
                EnrollmentDecisionSection(decision: decision)
            }
        }
        .task {
            await store.watchPending()
        }
    }
}

// MARK: - Declare

/// A row in the registry and nothing more.
///
/// It is a way in because sometimes a row is all that is wanted: a machine
/// somebody else administers, a placeholder a schedule refers to, a target that
/// will be filled in later. Saying that plainly is more useful than dressing it
/// up as enrollment, because a declared machine answers nothing.
struct EnrollmentDeclareView: View {
    @ObservedObject var store: MachineEnrollmentStore

    var body: some View {
        EnrollmentChrome(
            store: store,
            eyebrow: MachineEnrollmentFlow.declare.eyebrow,
            title: "Declare a machine in the registry",
            detail: "One entry, written from what you type. No session is opened, no key is minted, no agent is installed, and nothing about the machine is checked. It is the only way in that can add a machine which is switched off.",
            guidance: "A declared machine appears in the Hosts table with no capacity reports behind it. That is not a fault to chase: nothing has ever spoken to it.",
            actions: [
                WisentAction("Adopt instead", kind: .plain, isEnabled: store.isPermitted(.adopt)) {
                    store.open(.adopt)
                },
            ]
        ) {
            if let method = store.method(named: "declare") {
                WisentSectionBox(
                    title: "The command",
                    detail: "Reported by this control plane, so it is the spelling this release accepts. Replace the placeholders with the machine's name and address."
                ) {
                    EnrollmentCopyBlock(text: method.command)
                }
            }

            WisentSectionBox(title: "What you get, and what you do not") {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    EnrollmentChecklistRow(text: "You get a registry entry: schedules, policies, and reports can refer to the machine by name.")
                    EnrollmentChecklistRow(text: "You do not get a channel. Nothing has proved the address, the hostname, or the platform.")
                    EnrollmentChecklistRow(text: "You do not get an agent, so no capacity report will arrive and the Hosts table will show the machine as never having reported.")
                }
            }

            EnrollmentNote(
                title: "This window does not run it",
                detail: "Declaring is the one way in that proves nothing, so it belongs beside the registry document it edits rather than behind a button here. Copy the command above and run it where you can see that document. Every other method on the list writes the registry only after a machine has answered, and those are driven from here.",
                actions: [
                    WisentAction("Invite instead", isEnabled: store.isPermitted(.invite)) {
                        store.open(.invite)
                    },
                ]
            )
        }
    }
}
