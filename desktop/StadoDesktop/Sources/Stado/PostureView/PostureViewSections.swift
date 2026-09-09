import SwiftUI
import WisentDesignSystem

/// Everything the screen renders once a snapshot is ready: the alarms, the
/// strip, the counters and the decision section.
///
/// `body(for:)` is internal rather than private only because the screen shell
/// that calls it sits in a sibling file: Swift scopes `private` to one file.
extension PostureView {
    @ViewBuilder
    func body(for snapshot: DashboardSnapshot) -> some View {
        let posture = FleetPosture(
            snapshot: snapshot,
            report: cleanupStore.report,
            links: linkStore.links
        )

        if posture.queueBlocked {
            WisentAlertPanel(
                tone: .danger,
                title: "The queue is blocked",
                detail: "\(snapshot.counts.queue.formatted(.number)) jobs are queued and no host reports live capacity. Until one host publishes a current capacity report, nothing in this queue can start.",
                actions: [
                    WisentAction("Open Hosts", symbol: "server.rack", kind: .primary) { route(.hosts) }
                ]
            )
        }

        if let failure = posture.newestFailure {
            WisentAlertPanel(
                tone: .danger,
                title: "Job \(failure.jobID) failed",
                detail: failure.error ?? "The dashboard published this failure without a sanitized reason.",
                actions: [
                    WisentAction("Open Queue", symbol: "list.bullet.rectangle", kind: .primary) { route(.queue) }
                ]
            )
        }

        if let report = cleanupStore.report, report.outcomePresentation.severity == .critical {
            WisentAlertPanel(
                tone: .danger,
                title: report.outcomePresentation.title,
                detail: report.errors.first ?? report.outcomePresentation.detail,
                actions: [
                    WisentAction("Open Disk", symbol: "externaldrive", kind: .primary) { route(.disk) }
                ]
            )
        }

        if fleetStore.policy == nil, let message = fleetStore.errorMessage {
            WisentAlertPanel(
                tone: .danger,
                title: "Canonical fleet policy unavailable",
                detail: message,
                actions: [
                    WisentAction("Open Registry", symbol: "book.closed") { route(.registry) }
                ]
            )
        }

        WisentSignalStrip(signals: signals(posture))
        WisentCounterRow(counters: posture.counters)

        WisentSectionBox(
            title: "Needs a decision",
            detail: "Every row is a host or a job the fleet cannot resolve on its own, quoted as the backend reported it.",
            trailing: posture.decisionCount > 0 ? "\(posture.decisionCount) open" : "clear"
        ) {
            if posture.decisions.isEmpty {
                WisentPanel(padding:
                    0) {
                    WisentQueueRow(
                        symbol: "checkmark.circle.fill",
                        tone: .success,
                        title: "Nothing is waiting for you",
                        detail: "Every registered host has a current capacity report and no recent job failed.",
                        meta: "\(posture.liveHosts.count.formatted(.number)) live"
                    )
                }
            } else {
                WisentPanel(padding:
                    0) {
                    VStack(spacing:
                        0) {
                        ForEach(Array(posture.decisions.enumerated()), id: \.element.id) { index, decision in
                            if index > 0 {
                                Divider()
                            }
                            WisentQueueRow(
                                symbol: decision.symbol,
                                tone: decision.tone,
                                title: decision.title,
                                detail: decision.detail,
                                meta: decision.meta,
                                action: WisentAction(decision.destination.title, symbol: decision.destination.symbol) {
                                    if let host = decision.host {
                                        routeToHost(host)
                                    } else {
                                        route(decision.destination)
                                    }
                                }
                            )
                        }
                    }
                }
            }
            if let command = posture.silenceCommand {
                // The footer, once, under every row that reproduces from it —
                // rather than the same command repeated on each row.
                Text(command)
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
                    .textSelection(.enabled)
                    .padding(.top, WisentDesign.Space.x2)
            }
        }
    }

    private func signals(_ posture: FleetPosture) -> [WisentSignal] {
        var values = posture.signals
        if let policy = fleetStore.policy {
            values.append(
                WisentSignal("Registry", value: "Generation \(policy.generation)", tone: .neutral)
            )
        }
        if let firstRunNotice {
            values.append(WisentSignal("First run", value: firstRunNotice, tone: .neutral))
        }
        return values
    }
}
