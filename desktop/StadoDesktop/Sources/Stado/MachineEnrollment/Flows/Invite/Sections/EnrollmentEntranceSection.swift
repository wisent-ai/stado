import SwiftUI
import WisentDesignSystem

/// The public entrance the one-line invitation stands on, and the two verbs it
/// has: stand it up, tear it down.
///
/// Shown only while the online mode is chosen and nothing is minted yet,
/// because that is the one moment the operator can still change what the line
/// will be built against. When nothing serves the line and no permanent
/// address is configured, the honest thing to say is that an online mint will
/// only fall to the offline mode — and to offer the one button that changes
/// that, instead of letting the mint demonstrate it.
struct EnrollmentEntranceSection: View {
    @ObservedObject var store: MachineEnrollmentStore

    var body: some View {
        WisentSectionBox(
            title: "The entrance the line stands on",
            detail: "The one line fetches the join script from this fleet's control point, so something has to serve it where the machine being added can reach it. This section is that something.",
            trailing: trailing
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
                if let busy = store.entranceBusy {
                    EnrollmentNote(title: "Working", detail: busy)
                } else if let ingress = store.ingress, ingress.published {
                    standing(ingress)
                } else if store.ingress != nil {
                    absent
                } else {
                    EnrollmentNote(
                        title: "Reading",
                        detail: "Asking the control plane whether an entrance is standing."
                    )
                }
            }
        }
    }

    private var trailing: String? {
        guard let ingress = store.ingress, ingress.published else { return nil }
        return ingress.temporary ? "temporary" : ingress.mode
    }

    @ViewBuilder
    private func standing(_ ingress: FleetIngressStatus) -> some View {
        WisentSignalStrip(signals: [
            WisentSignal(
                "Answers",
                value: ingress.reachable ? "Yes, from the internet" : "Not right now",
                tone: ingress.reachable ? .success : .warning
            ),
            WisentSignal(
                "Standing",
                value: ingress.standingSeconds.map { "\($0 / 60) min" } ?? "unknown",
                tone: .neutral
            ),
            WisentSignal(
                "Lifetime",
                value: ingress.temporary ? "Dies with ingress down" : "Configured",
                tone: ingress.temporary ? .warning : .success
            ),
        ])
        EnrollmentCopyBlock(
            text: ingress.baseURL,
            caption: ingress.temporary
                ? "A Cloudflare quick-tunnel address: it stops answering the moment the ingress is torn down, and a restarted ingress comes back under a different name. Lines minted against it die with it."
                : "The address every one-line invitation is built against."
        )
        if !ingress.reachable, !ingress.detail.isEmpty {
            EnrollmentNote(title: "The address stopped answering", detail: ingress.detail)
        }
        EnrollmentNote(
            title: "Tearing it down kills every line minted against it",
            detail: "The CLI says the same at mint time. Machines already enrolled are untouched; only unredeemed one-line invitations die.",
            actions: [
                WisentAction("Tear the entrance down", kind: .destructive, isEnabled: !store.isRunning && store.entranceBusy == nil) {
                    Task { await store.tearDownEntrance() }
                },
            ]
        )
    }

    @ViewBuilder
    private var absent: some View {
        if store.enrollmentURLConfigured == true {
            EnrollmentNote(
                title: "A permanent address is configured",
                detail: "No temporary entrance is standing, and none is needed: the configuration names a permanent enrollment address, and the line will be built against it."
            )
        } else {
            EnrollmentNote(
                title: "Nothing serves the one line today",
                detail: "No entrance is standing and no permanent enrollment address is configured, so minting now falls to the offline fragment — honestly, but at the cost of one more paste for the machine's owner. Standing the entrance up takes about a minute and no Cloudflare account.",
                actions: [
                    WisentAction("Stand the entrance up", symbol: "antenna.radiowaves.left.and.right", kind: .primary, isEnabled: !store.isRunning && store.entranceBusy == nil) {
                        Task { await store.standUpEntrance() }
                    },
                ]
            )
        }
    }
}
