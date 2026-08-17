import SwiftUI
import WisentDesignSystem

/// Aggregation happens before rendering: the screen asks one value object what
/// needs a human, instead of every view re-deriving the same counts.
@MainActor
struct FleetPosture {
    struct Decision: Identifiable {
        let id: String
        let symbol: String
        let tone: WisentTone
        let title: String
        /// The backend's own sentence, never a paraphrase.
        let detail: String
        let meta: String
        let destination: ConsoleDestination
    }

    let snapshot: DashboardSnapshot
    let report: CleanupReport?

    var liveHosts: [WorkerNode] { snapshot.workers.filter { $0.status == .live } }
    var staleHosts: [WorkerNode] { snapshot.workers.filter { $0.status == .stale } }
    var unavailableHosts: [WorkerNode] { snapshot.workers.filter { $0.status == .unavailable } }
    var undeclaredHosts: [WorkerNode] { snapshot.workers.filter { $0.status == .live && !$0.declared } }

    var queueBlocked: Bool {
        snapshot.counts.queue > 0 && liveHosts.isEmpty
    }

    var newestFailure: FailedJob? { snapshot.recentFailed.first }

    var decisions: [Decision] {
        var items: [Decision] = []
        for host in unavailableHosts {
            items.append(
                Decision(
                    id: "unavailable-\(host.id)",
                    symbol: "xmark.octagon.fill",
                    tone: .danger,
                    title: host.displayName,
                    detail: host.availabilityReason,
                    meta: ConsoleFormat.age(host.ageSeconds),
                    destination: .hosts
                )
            )
        }
        for host in staleHosts {
            items.append(
                Decision(
                    id: "stale-\(host.id)",
                    symbol: "clock.badge.exclamationmark.fill",
                    tone: .warning,
                    title: host.displayName,
                    detail: host.availabilityReason,
                    meta: ConsoleFormat.age(host.ageSeconds),
                    destination: .hosts
                )
            )
        }
        for host in undeclaredHosts {
            items.append(
                Decision(
                    id: "undeclared-\(host.id)",
                    symbol: "questionmark.square.dashed",
                    tone: .warning,
                    title: host.displayName,
                    detail: "Publishes capacity but is not declared in the canonical registry.",
                    meta: "Undeclared",
                    destination: .registry
                )
            )
        }
        for job in snapshot.recentFailed {
            items.append(
                Decision(
                    id: "failed-\(job.jobID)",
                    symbol: "exclamationmark.triangle.fill",
                    tone: .danger,
                    title: "Job \(job.jobID)",
                    detail: job.error ?? "The dashboard published this failure without a sanitized reason.",
                    meta: job.model ?? "No model",
                    destination: .queue
                )
            )
        }
        if let report, report.pressureActive == true {
            items.append(
                Decision(
                    id: "disk-pressure",
                    symbol: "externaldrive.fill.badge.exclamationmark",
                    tone: report.outcomePresentation.severity == .critical ? .danger : .warning,
                    title: "Disk pressure on the dashboard host",
                    detail: report.errors.first ?? report.outcomePresentation.detail,
                    meta: DisplayFormat.bytes(report.freeBytesAfter),
                    destination: .disk
                )
            )
        }
        return items
    }

    var decisionCount: Int { decisions.count }

    /// A healthy fact is one line. It never grows into a panel just because
    /// there was room for one.
    var signals: [WisentSignal] {
        var values: [WisentSignal] = [
            WisentSignal(
                "Queued",
                value: snapshot.counts.queue.formatted(.number),
                tone: snapshot.counts.queue > 0 ? .warning : .neutral
            ),
            WisentSignal(
                "Running",
                value: snapshot.counts.running.formatted(.number),
                tone: snapshot.counts.running > 0 ? .success : .neutral
            ),
            WisentSignal(
                "Live hosts",
                value: "\(liveHosts.count) of \(snapshot.workers.count)",
                tone: liveHosts.isEmpty ? .warning : .success
            ),
            WisentSignal(
                "Free slots",
                value: snapshot.throughput.liveTotalFreeSlots.formatted(.number),
                tone: .neutral
            ),
        ]
        if let report {
            values.append(
                WisentSignal(
                    "Free disk",
                    value: DisplayFormat.bytes(report.freeBytesAfter),
                    tone: report.pressureActive == true ? .warning : .success
                )
            )
        }
        return values
    }

    var counters: [WisentCounterRow.Counter] {
        [
            WisentCounterRow.Counter(
                "Queued",
                value: snapshot.counts.queue.formatted(.number),
                detail: "Waiting for capacity",
                tone: queueBlocked ? .danger : .neutral
            ),
            WisentCounterRow.Counter(
                "Running",
                value: snapshot.counts.running.formatted(.number),
                detail: "Executing on live hosts"
            ),
            WisentCounterRow.Counter(
                "Recent failures",
                value: snapshot.recentFailed.count.formatted(.number),
                detail: "Published in this snapshot",
                tone: snapshot.recentFailed.isEmpty ? .neutral : .danger
            ),
            WisentCounterRow.Counter(
                "Average completion",
                value: StadoFormat.duration(snapshot.throughput.averageWallSecondsPerCompletedJob),
                detail: "\(snapshot.throughput.samples.formatted(.number)) samples"
            ),
        ]
    }
}

struct PostureView: View {
    @ObservedObject var store: OperationsStore
    @ObservedObject var cleanupStore: CleanupStore
    @ObservedObject var fleetStore: FleetControlStore
    let scope: String
    let firstRunNotice: String?
    let route: (ConsoleDestination) -> Void
    let refresh: () async -> Void

    var body: some View {
        WisentScreen(
            title: "Posture",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(store.lastUpdated))",
            actions: [
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task { await refresh() }
                }
            ]
        ) {
            if let message = store.errorMessage {
                WisentErrorBanner(
                    title: store.isShowingStaleSnapshot
                        ? "Refresh failed — every row below is the last good snapshot"
                        : "Fleet state unavailable",
                    detail: message,
                    action: WisentAction("Retry", symbol: "arrow.clockwise") {
                        Task { await refresh() }
                    }
                )
            }

            if let snapshot = store.snapshot {
                if snapshot.ready {
                    body(for: snapshot)
                } else {
                    WisentAlertPanel(
                        tone: .warning,
                        title: "The dashboard has not published its first snapshot",
                        detail: "The endpoint answered, but its background scan has not produced queue state yet. Nothing on this screen is estimated while that scan is incomplete.",
                        command: "curl \(store.dashboardURLString)/api/state.json"
                    )
                }
            } else if store.isRefreshing {
                WisentLoadingPanel(
                    title: "Reading fleet state",
                    detail: "Queue depth, host capacity reports, and recent job outcomes from /api/state.json."
                )
            } else {
                WisentEmptyPanel(
                    title: "No fleet state",
                    detail: "The configured endpoint has not answered yet. No operational data is fabricated while the source is unavailable.",
                    symbol: "network.slash",
                    action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary) {
                        Task { await refresh() }
                    }
                )
            }
        }
    }

    @ViewBuilder
    private func body(for snapshot: DashboardSnapshot) -> some View {
        let posture = FleetPosture(snapshot: snapshot, report: cleanupStore.report)

        if posture.queueBlocked {
            WisentAlertPanel(
                tone: .danger,
                title: "The queue is blocked",
                detail: "\(snapshot.counts.queue.formatted(.number)) jobs are queued and no host reports live capacity. Until one host publishes a current capacity report, nothing in this queue can start.",
                command: "curl \(store.dashboardURLString)/api/state.json",
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
                command: "stado job watch \(failure.jobID)",
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
                command: "curl \(store.dashboardURLString)/api/cleanup.json",
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
                command: "curl \(store.dashboardURLString)/api/registry.json",
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
                WisentPanel(padding: 0) {
                    WisentQueueRow(
                        symbol: "checkmark.circle.fill",
                        tone: .success,
                        title: "Nothing is waiting for you",
                        detail: "Every registered host has a current capacity report and no recent job failed.",
                        meta: "\(posture.liveHosts.count.formatted(.number)) live"
                    )
                }
            } else {
                WisentPanel(padding: 0) {
                    VStack(spacing: 0) {
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
                                    route(decision.destination)
                                }
                            )
                        }
                    }
                }
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
