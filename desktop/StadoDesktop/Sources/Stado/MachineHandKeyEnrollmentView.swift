import SwiftUI
import WisentDesignSystem

/// Adding a machine nobody can reach, by carrying its key there.
///
/// This is the method the other three exist to avoid, and it is kept because
/// one machine in every fleet needs it: the one no operator can open a session
/// to and whose owner will not run a line they were sent. Its five steps are
/// ordered by what the work requires rather than by what reads well — a key
/// has to exist before its public half can be carried, the machine has to
/// accept that half before a channel opens, and a channel has to open before
/// enrollment can probe the machine and write it down. The walk between step
/// three and step four is why this screen is resumable, and why the steps
/// ahead of the operator say what they are waiting for instead of failing
/// blankly when opened early.
struct MachineHandKeyEnrollmentView: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: MachineEnrollmentStore
    /// Names the registry and the capacity store already know. Enrollment
    /// refuses a duplicate, and it refuses it after the operator has already
    /// been to the other machine.
    let existingNames: Set<String>
    let refresh: () async -> Void

    var body: some View {
        EnrollmentChrome(
            store: store,
            eyebrow: MachineEnrollmentFlow.handKey.eyebrow,
            title: store.draft.machineName.isEmpty
                ? "Carry a key to the machine"
                : "Add \(store.draft.machineName) by carrying its key",
            detail: "Every step here runs one allowlisted Stado command through the dashboard's authenticated bridge. This app never opens an SSH session itself, which is exactly why the middle of this method happens on the other machine.",
            trailing: store.draft.hasKey
                ? ("KEY MINTED", ConsoleFormat.relative(store.draft.keyMintedAt))
                : nil,
            guidance: guidance,
            actions: actions,
            rail: { rail },
            content: { content }
        )
    }

    // MARK: Rail

    private var rail: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            ForEach(MachineEnrollmentStep.allCases) { step in
                railRow(step)
            }
            Spacer(minLength: 0)
            Text("The registry is written only by the enroll step, and only after the machine has answered.")
                .font(WisentTypography.body(10))
                .foregroundStyle(WisentDesign.muted)
                .fixedSize(horizontal: false, vertical: true)
                .padding(WisentDesign.Space.x3)
        }
        .padding(.vertical, WisentDesign.Space.x4)
        .frame(width: 232, alignment: .leading)
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
                    .font(.system(size: 11, weight: .semibold))
                    .frame(width: 16)
                    .foregroundStyle(railSymbolTone(step, done: isDone, open: isOpen, current: isCurrent))
                VStack(alignment: .leading, spacing: 1) {
                    Text("\(step.ordinal). \(step.title)")
                        .font(isCurrent ? WisentTypography.bodyMedium(12) : WisentTypography.body(12))
                        .foregroundStyle(isOpen || isCurrent ? WisentDesign.ink : WisentDesign.muted)
                    Text(step.purpose)
                        .font(WisentTypography.body(10))
                        .foregroundStyle(WisentDesign.muted)
                        .fixedSize(horizontal: false, vertical: true)
                        .multilineTextAlignment(.leading)
                }
                Spacer(minLength: 0)
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

    /// One line for what closing and discarding cost. An attempt that already
    /// minted a key cannot be thrown away for free: the key outlives the
    /// attempt, and the operator should read that before pressing discard, not
    /// after finding an orphaned credential item.
    private var guidance: String {
        if store.draft.isEmpty {
            return "Nothing has been minted or written yet."
        }
        if store.draft.hasKey {
            return "Progress is kept: close this window, walk to the other machine, and reopen it here. Discarding leaves \(store.draft.credentialItem) in the credential store — remove it with stado fleet key rm \(store.draft.machineName)."
        }
        return "Progress is kept. Close this window, go to the other machine, and reopen it here."
    }

    // MARK: Steps

    @ViewBuilder
    private var content: some View {
        switch store.step {
        case .name: nameStep
        case .key: keyStep
        case .channel: channelStep
        case .enroll: enrollStep
        case .verify: verifyStep
        }
    }

    private var nameStep: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            EnrollmentNameSection(
                store: store,
                existingNames: existingNames,
                detail: "The identifier the canonical registry, the capacity reports, and every stado command will use for this machine. Lowercase letters, digits, and the characters . - _",
                isLocked: store.draft.hasKey
            )

            if store.draft.hasKey {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The name is fixed once a key exists for it",
                    detail: "The credential item is \(store.draft.credentialItem), named after this machine. To use a different name, start another attempt; the key already minted stays in the credential store until it is removed with stado fleet key rm.",
                    command: "stado fleet key rm \(store.draft.machineName)"
                )
            }

            WisentSectionBox(title: "What happens after this") {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    ForEach(MachineEnrollmentStep.allCases.dropFirst()) { step in
                        HStack(alignment: .top, spacing: WisentDesign.Space.x2) {
                            Text("\(step.ordinal).")
                                .font(WisentTypeScale.identifierSmall())
                                .foregroundStyle(WisentDesign.muted)
                                .frame(width: 16, alignment: .trailing)
                            Text(step.purpose)
                                .font(WisentTypeScale.body())
                                .foregroundStyle(WisentDesign.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }

            EnrollmentNote(
                title: "If you can reach this machine, or somebody there can",
                detail: "Adopt does the key install over a session you can already open, and Invite has the machine's owner do it with one line. Both skip the three steps after this one.",
                actions: [
                    WisentAction("Adopt instead", isEnabled: store.isPermitted(.adopt)) {
                        store.open(.adopt)
                    },
                    WisentAction("Invite instead", isEnabled: store.isPermitted(.invite)) {
                        store.open(.invite)
                    },
                ]
            )
        }
    }

    private var keyStep: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            if store.draft.hasKey {
                WisentSectionBox(
                    title: "Public key for \(store.draft.machineName)",
                    detail: "The private half stays in the credential store and is never shown here. Put this line into ~/.ssh/authorized_keys on the machine you are adding.",
                    trailing: store.draft.credentialItem
                ) {
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                        EnrollmentCopyBlock(text: store.draft.publicKey)
                        if !store.draft.keyFingerprint.isEmpty {
                            WisentField(label: "Fingerprint", value: store.draft.keyFingerprint)
                        }
                    }
                }

                WisentSectionBox(
                    title: "One line to run on the machine you are adding",
                    detail: "Paste it into a terminal on that machine. Stado does not run it for you: it has no way in until this key is accepted."
                ) {
                    EnrollmentCopyBlock(text: store.draft.authorizedKeysCommand)
                }

                WisentAlertPanel(
                    tone: .warning,
                    title: "Turn on Remote Login over there before the enroll step",
                    detail: "On macOS: System Settings, General, Sharing, Remote Login. On Linux: start and enable sshd. Enrollment opens an SSH channel as its first act, so a machine with Remote Login off fails at step 4 no matter how the key was installed.",
                    actions: [
                        WisentAction("Mint a replacement key", isEnabled: !store.isRunning) {
                            Task { await store.generateKey() }
                        }
                    ]
                )
            } else {
                WisentSectionBox(
                    title: "Mint the key pair",
                    detail: "Stado generates an ed25519 pair, stores both halves in the credential store as \("stado-ssh-\(store.draft.machineName)"), and prints only the public half. Nothing is written to the registry by this step and nothing is changed on the machine you are adding."
                ) {
                    WisentPanel {
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                            Text(verbatim: "stado fleet key generate \(store.draft.machineName)")
                                .font(WisentTypeScale.identifier())
                                .foregroundStyle(WisentDesign.ink)
                                .textSelection(.enabled)
                            Text("The public half is what you carry to the other machine. It is kept here afterwards, so closing this window does not lose it.")
                                .font(WisentTypeScale.caption())
                                .foregroundStyle(WisentDesign.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }
        }
    }

    private var channelStep: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            WisentSectionBox(
                title: "SSH address",
                detail: "How the machine running the Stado control plane reaches this one. A Bonjour name on the same network is as good as a tailnet name: the registry stores whatever answers, and requires no particular kind."
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

            if store.draft.hasChannel, !store.draft.sshTarget.contains("@") {
                WisentAlertPanel(
                    tone: .warning,
                    title: "No user name in the address",
                    detail: "Stado will connect as whichever user the control plane runs as. That is only right if the public key from the previous step is in that user's ~/.ssh/authorized_keys on \(store.draft.sshTarget). Write it as user@host to be sure."
                )
            }

            WisentSectionBox(title: "Before you continue") {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    EnrollmentChecklistRow(text: "Remote Login is on over there.")
                    EnrollmentChecklistRow(text: "The public key from step 2 is in ~/.ssh/authorized_keys of the user in this address.")
                    EnrollmentChecklistRow(text: "This address resolves from the machine running the Stado dashboard, not from this Mac.")
                }
            }
        }
    }

    private var enrollStep: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            if let blockade = store.blockade(before: .enroll) {
                WisentAlertPanel(
                    tone: .warning,
                    title: "Enrollment cannot start yet",
                    detail: blockade,
                    actions: [
                        WisentAction("Go to the key step", kind: .primary) { store.open(.key) }
                    ]
                )
            } else {
                WisentSectionBox(
                    title: "What this runs",
                    detail: "The machine is asked for its hostname, uname -s and uname -m over the channel. Only after it answers is an entry written to the canonical registry, so the entry records what the machine is rather than what was typed here."
                ) {
                    WisentPanel {
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                            Text(verbatim: store.draft.enrollCommand)
                                .font(WisentTypeScale.identifier())
                                .foregroundStyle(WisentDesign.ink)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                            Text("--bootstrap then installs the Stado agent on the machine. If that install fails, the entry that was just written is removed again: a failed enrollment leaves nothing behind to clean up.")
                                .font(WisentTypeScale.caption())
                                .foregroundStyle(WisentDesign.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }

                WisentSignalStrip(signals: [
                    WisentSignal("Machine", value: store.draft.machineName, tone: .neutral),
                    WisentSignal("Address", value: store.draft.sshTarget, tone: .neutral),
                    WisentSignal(
                        "Key",
                        value: store.draft.credentialItem.isEmpty ? "Minted" : store.draft.credentialItem,
                        tone: .success
                    ),
                ])
            }

            if store.draft.isEnrolled, !store.draft.enrollmentTranscript.isEmpty {
                WisentSectionBox(title: "Stado's answer", detail: "Verbatim, from the command that wrote the entry.") {
                    EnrollmentTranscript(text: store.draft.enrollmentTranscript)
                }
            }
        }
    }

    private var verifyStep: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            WisentSignalStrip(signals: [
                WisentSignal(
                    "Registry entry",
                    value: store.draft.isEnrolled ? "Written" : "Not written",
                    tone: store.draft.isEnrolled ? .success : .neutral
                ),
                WisentSignal(
                    "Channel",
                    value: checkValue(store.draft.channelCheck),
                    tone: checkTone(store.draft.channelCheck)
                ),
                WisentSignal(
                    "Agent",
                    value: checkValue(store.draft.agentRecovery),
                    tone: checkTone(store.draft.agentRecovery)
                ),
            ])

            EnrollmentProofSection(store: store)

            if store.draft.channelCheck?.ok == false {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The entry exists but the channel did not open on the stored key",
                    detail: "The registry entry is real and enrollment proved the machine once, so this is about the key rather than the machine. A freshly minted item is unreadable until the local-operator consumer is granted its fields, and a key installed under the wrong user on the other machine fails the same way."
                )
            }
        }
    }

    // MARK: Reading the draft

    private func checkValue(_ check: MachineEnrollmentCheck?) -> String {
        guard let check else { return "Not checked" }
        return check.ok ? "Verified" : "Refused"
    }

    private func checkTone(_ check: MachineEnrollmentCheck?) -> WisentTone {
        guard let check else { return .neutral }
        return check.ok ? .success : .danger
    }

    private func isSettled(_ step: MachineEnrollmentStep) -> Bool {
        switch step {
        case .name: MachineName.problem(with: store.draft.machineName) == nil
        case .key: store.draft.hasKey
        case .channel: store.draft.hasChannel
        case .enroll: store.draft.isEnrolled
        case .verify: store.draft.channelCheck?.ok == true && store.draft.agentRecovery?.ok == true
        }
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

    // MARK: Verbs

    private var actions: [WisentAction] {
        var actions: [WisentAction] = []
        if !store.draft.isEmpty {
            actions.append(
                WisentAction("Discard this attempt", kind: .plain, isEnabled: !store.isRunning) {
                    store.startAnother()
                }
            )
        }
        if store.step.previous != nil {
            actions.append(
                WisentAction("Back", isEnabled: !store.isRunning) { store.goBack() }
            )
        }
        actions.append(primaryAction)
        return actions
    }

    private var primaryAction: WisentAction {
        switch store.step {
        case .name:
            return WisentAction(
                "Continue",
                kind: .primary,
                isEnabled: store.canOpen(.key) && !existingNames.contains(store.draft.machineName)
            ) {
                store.open(.key)
            }
        case .key:
            if store.draft.hasKey {
                return WisentAction("Continue", kind: .primary, isEnabled: !store.isRunning) {
                    store.open(.channel)
                }
            }
            return WisentAction(
                "Mint the key",
                symbol: "key",
                kind: .primary,
                isEnabled: !store.isRunning && store.isConfigured
            ) {
                Task { await store.generateKey() }
            }
        case .channel:
            return WisentAction("Continue", kind: .primary, isEnabled: store.canOpen(.enroll)) {
                store.open(.enroll)
            }
        case .enroll:
            if store.draft.isEnrolled {
                return WisentAction("Continue", kind: .primary, isEnabled: !store.isRunning) {
                    store.open(.verify)
                }
            }
            return WisentAction(
                "Enroll \(store.draft.machineName)",
                symbol: "arrow.right.circle",
                kind: .primary,
                isEnabled: !store.isRunning && store.isConfigured && store.canOpen(.enroll)
            ) {
                Task {
                    await store.enroll()
                    await refresh()
                }
            }
        case .verify:
            if isSettled(.verify) {
                return WisentAction("Done", symbol: "checkmark", kind: .primary) {
                    store.startAnother()
                    dismiss()
                }
            }
            return WisentAction(
                "Run the checks",
                symbol: "checkmark.shield",
                kind: .primary,
                isEnabled: !store.isRunning && store.isConfigured && store.canOpen(.verify)
            ) {
                Task {
                    await store.verify()
                    await refresh()
                }
            }
        }
    }
}
