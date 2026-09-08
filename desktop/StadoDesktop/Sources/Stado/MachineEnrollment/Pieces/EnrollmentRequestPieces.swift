import SwiftUI
import WisentDesignSystem

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
