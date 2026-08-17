import AppKit
import SwiftUI
import WisentDesignSystem

/// Adding a machine to the fleet, as a screen instead of tribal knowledge.
///
/// The five steps are ordered by what the work actually requires, not by what
/// reads well: a key has to exist before its public half can be carried, the
/// machine has to accept that half before a channel opens, and a channel has
/// to open before enrollment can probe the machine and write it down. The
/// third and fourth steps are separated by a walk to another computer, so this
/// screen is resumable and the steps ahead of the operator say what they are
/// waiting for instead of failing blankly when opened early.
struct MachineEnrollmentView: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var store: MachineEnrollmentStore
    /// Names the registry and the capacity store already know. Enrollment
    /// refuses a duplicate, and it refuses it after the operator has already
    /// been to the other machine.
    let existingNames: Set<String>
    let refresh: () async -> Void

    @State private var copiedField: String?

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            HStack(spacing: 0) {
                rail
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

    // MARK: Chrome

    private var header: some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x4) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text("ADD A MACHINE")
                    .font(WisentTypeScale.eyebrow())
                    .tracking(0.8)
                    .foregroundStyle(WisentDesign.muted)
                Text(store.draft.machineName.isEmpty
                    ? "Add a machine to the fleet"
                    : "Add \(store.draft.machineName) to the fleet")
                    .font(WisentTypography.heading(17))
                    .foregroundStyle(WisentDesign.ink)
                Text("Every step here runs one allowlisted Stado command through the dashboard's authenticated bridge. This app never opens an SSH session itself.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
            if store.draft.hasKey {
                VStack(alignment: .trailing, spacing: 1) {
                    Text("KEY MINTED")
                        .font(WisentTypeScale.eyebrow())
                        .tracking(0.8)
                        .foregroundStyle(WisentDesign.muted)
                    Text(ConsoleFormat.relative(store.draft.keyMintedAt))
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.secondary)
                }
            }
        }
        .padding(WisentDesign.Space.x6)
        .background(WisentDesign.canvas)
    }

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

    private var footer: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            WisentMutationBar(outcome: store.outcome, clear: { store.clearOutcome() })
            Text(guidance)
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: WisentDesign.Space.x2) {
                WisentActionButton(
                    action: WisentAction("Close", kind: .plain) { dismiss() }
                )
                if !store.draft.isEmpty {
                    WisentActionButton(
                        action: WisentAction("Discard this attempt", kind: .plain, isEnabled: !store.isRunning) {
                            store.startAnother()
                        }
                    )
                }
                Spacer(minLength: WisentDesign.Space.x4)
                if store.step.previous != nil {
                    WisentActionButton(
                        action: WisentAction("Back", isEnabled: !store.isRunning) { store.goBack() }
                    )
                }
                WisentActionButton(action: primaryAction)
            }
        }
        .padding(WisentDesign.Space.x5)
        .background(WisentDesign.canvasMuted)
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
            WisentSectionBox(
                title: "Machine name",
                detail: "The identifier the canonical registry, the capacity reports, and every stado command will use for this machine. Lowercase letters, digits, and the characters . - _"
            ) {
                TextField(
                    "studio",
                    text: Binding(
                        get: { store.draft.machineName },
                        set: { store.setMachineName($0) }
                    )
                )
                .textFieldStyle(.roundedBorder)
                .font(WisentTypeScale.body())
                .disabled(store.draft.hasKey)
            }

            if store.draft.hasKey {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The name is fixed once a key exists for it",
                    detail: "The credential item is \(store.draft.credentialItem), named after this machine. To use a different name, start another enrollment; the key already minted stays in the credential store until it is removed with stado fleet key rm.",
                    command: "stado fleet key rm \(store.draft.machineName)"
                )
            } else if let problem = MachineName.problem(with: store.draft.machineName),
                      !store.draft.machineName.isEmpty {
                WisentAlertPanel(tone: .warning, title: "The registry will refuse this name", detail: problem)
            } else if existingNames.contains(store.draft.machineName) {
                WisentAlertPanel(
                    tone: .danger,
                    title: "\(store.draft.machineName) is already in this fleet",
                    detail: "Enrollment refuses to overwrite a machine that already has a channel or a health beacon, so this attempt would fail at the last step. Pick a different name."
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
                        copyBlock(id: "public-key", text: store.draft.publicKey)
                        if !store.draft.keyFingerprint.isEmpty {
                            WisentField(label: "Fingerprint", value: store.draft.keyFingerprint)
                        }
                    }
                }

                WisentSectionBox(
                    title: "One line to run on the machine you are adding",
                    detail: "Paste it into a terminal on that machine. Stado does not run it for you: it has no way in until this key is accepted."
                ) {
                    copyBlock(id: "authorized-keys", text: store.draft.authorizedKeysCommand)
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
                            Text("stado fleet key generate \(store.draft.machineName)")
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

            failurePanel
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
                    Text("Examples: lukasz@studio.local, lukasz@100.92.4.11, lukasz@studio.tailnet-name.ts.net")
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
                    checklistRow("Remote Login is on over there.")
                    checklistRow("The public key from step 2 is in ~/.ssh/authorized_keys of the user in this address.")
                    checklistRow("This address resolves from the machine running the Stado dashboard, not from this Mac.")
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
                            Text(store.draft.enrollCommand)
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
                    transcript(store.draft.enrollmentTranscript)
                }
            }

            failurePanel
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

            WisentSectionBox(
                title: "The two proofs",
                detail: "The first opens the channel with the stored key and reads back the hostname the machine answers with. The second asks Stado to recover the host, which is what makes it start publishing capacity reports the Hosts table can read."
            ) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                    checkPanel(
                        store.draft.channelCheck,
                        fallbackCommand: "stado fleet key check \(store.draft.machineName)"
                    )
                    checkPanel(
                        store.draft.agentRecovery,
                        fallbackCommand: "stado host recover \(store.draft.machineName)"
                    )
                }
            }

            if store.draft.channelCheck?.ok == false {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The entry exists but the channel did not open on the stored key",
                    detail: "The registry entry is real and enrollment proved the machine once, so this is about the key rather than the machine. A freshly minted item is unreadable until the local-operator consumer is granted its fields, and a key installed under the wrong user on the other machine fails the same way."
                )
            }

            failurePanel
        }
    }

    // MARK: Pieces

    private var failurePanel: some View {
        Group {
            if let failure = store.failure {
                WisentAlertPanel(
                    tone: .danger,
                    title: failure.title,
                    detail: failure.detail,
                    command: failure.backendMessage.isEmpty ? nil : failure.backendMessage
                )
            }
        }
    }

    private func checklistRow(_ text: String) -> some View {
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

    private func copyBlock(id: String, text: String) -> some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x3) {
            Text(text)
                .font(WisentTypeScale.identifier())
                .foregroundStyle(WisentDesign.ink)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
            WisentActionButton(
                action: WisentAction(
                    copiedField == id ? "Copied" : "Copy",
                    symbol: copiedField == id ? "checkmark" : "doc.on.doc"
                ) {
                    copy(text, id: id)
                }
            )
        }
        .padding(WisentDesign.Space.x4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(WisentDesign.canvasMuted, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small))
        .overlay {
            RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
                .stroke(WisentDesign.border, lineWidth: WisentDesign.hairline)
        }
    }

    private func transcript(_ text: String) -> some View {
        Text(text)
            .font(WisentTypeScale.identifierSmall())
            .foregroundStyle(WisentDesign.secondary)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
            .padding(WisentDesign.Space.x3)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(WisentDesign.canvasMuted, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small))
    }

    @ViewBuilder
    private func checkPanel(_ check: MachineEnrollmentCheck?, fallbackCommand: String) -> some View {
        WisentPanel {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                HStack(spacing: WisentDesign.Space.x2) {
                    Text(check?.command ?? fallbackCommand)
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
                    transcript(check.output)
                }
            }
        }
    }

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

    private func copy(_ text: String, id: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        copiedField = id
        Task {
            try? await Task.sleep(for: .seconds(2))
            if copiedField == id { copiedField = nil }
        }
    }
}
