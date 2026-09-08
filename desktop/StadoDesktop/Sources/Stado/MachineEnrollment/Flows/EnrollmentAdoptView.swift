import SwiftUI
import WisentDesignSystem

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
