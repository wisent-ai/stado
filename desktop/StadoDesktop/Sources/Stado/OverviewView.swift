import SwiftUI
import WisentDesignSystem

struct OverviewView: View {
    let snapshot: DashboardSnapshot
    let lastUpdated: Date?

    private let metricColumns = [
        GridItem(.adaptive(minimum: StadoLayout.metricMinimumWidth), spacing: WisentDesign.Space.x3),
    ]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x6) {
                header

                LazyVGrid(columns: metricColumns, alignment: .leading, spacing: WisentDesign.Space.x3) {
                    WisentMetricCard(
                        title: "Queued",
                        value: snapshot.counts.queue.formatted(),
                        detail: "Jobs waiting for capacity",
                        symbol: "clock",
                        tone: snapshot.counts.queue > 0 ? .warning : .neutral
                    )
                    WisentMetricCard(
                        title: "Running",
                        value: snapshot.counts.running.formatted(),
                        detail: "Jobs currently executing",
                        symbol: "play.circle.fill",
                        tone: snapshot.counts.running > 0 ? .success : .neutral
                    )
                    WisentMetricCard(
                        title: "Registered workers",
                        value: snapshot.workers.count.formatted(),
                        detail: workerMetricDetail,
                        symbol: "server.rack",
                        tone: unavailableWorkerCount > 0 ? .danger : (liveWorkerCount == 0 ? .warning : .success)
                    )
                    WisentMetricCard(
                        title: "Free slots",
                        value: snapshot.throughput.liveTotalFreeSlots.formatted(),
                        detail: snapshot.liveAgents.isEmpty ? "No live worker reports" : "Reported by live workers",
                        symbol: "gauge.with.dots.needle.50percent",
                        tone: snapshot.liveAgents.isEmpty ? .warning : (snapshot.throughput.liveTotalFreeSlots > 0 ? .success : .neutral)
                    )
                }

                workerAvailabilityCard

                HStack(alignment: .top, spacing: WisentDesign.Space.x4) {
                    capacityCard
                    throughputCard
                }

                recentActivity
            }
            .frame(maxWidth: WisentDesign.Layout.contentMaximumWidth, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(WisentDesign.Space.x6)
        }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text("Operations overview")
                    .font(.largeTitle.weight(.semibold))
                Text(sourceDescription)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            WisentBadge(operationalStatus.label, tone: operationalStatus.tone)
        }
    }

    private var sourceDescription: String {
        let bucket = snapshot.bucket?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let source: String
        if let bucket, !bucket.isEmpty {
            source = "Queue source: \(bucket)"
        } else {
            source = "Queue source: dashboard-managed storage"
        }
        guard let lastUpdated else { return source }
        return "\(source) · refreshed \(lastUpdated.formatted(.relative(presentation: .named)))"
    }

    private var liveWorkerCount: Int {
        snapshot.workers.count { $0.status == .live }
    }

    private var unavailableWorkerCount: Int {
        snapshot.workers.count { $0.status == .unavailable }
    }

    private var staleWorkerCount: Int {
        snapshot.workers.count { $0.status == .stale }
    }

    private var workerMetricDetail: String {
        var parts = ["\(liveWorkerCount) live"]
        if unavailableWorkerCount > 0 {
            parts.append("\(unavailableWorkerCount) unavailable")
        }
        if staleWorkerCount > 0 {
            parts.append("\(staleWorkerCount) stale")
        }
        return parts.joined(separator: " · ")
    }

    private var availabilityIssues: [WorkerNode] {
        snapshot.workers.filter { $0.status != .live || !$0.declared }
    }

    private var operationalStatus: (label: String, tone: WisentTone) {
        if snapshot.counts.queue > 0 && liveWorkerCount == 0 {
            return ("Queue blocked", .danger)
        }
        if unavailableWorkerCount > 0 {
            return ("Fleet degraded", .danger)
        }
        if snapshot.counts.failed > 0 || staleWorkerCount > 0 {
            return ("Attention required", .warning)
        }
        if liveWorkerCount == 0 {
            return ("No live workers", .warning)
        }
        return ("Fleet reporting", .success)
    }

    private var workerAvailabilityCard: some View {
        WisentPanel {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                HStack {
                    Label("Worker availability", systemImage: "server.rack")
                        .font(.headline)
                    Spacer()
                    Text("\(snapshot.workers.count) registered or observed")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }

                if snapshot.workers.isEmpty {
                    UnavailableNotice(
                        title: "Worker inventory unavailable",
                        detail: "The dashboard did not publish any registered workers or capacity identities."
                    )
                } else if availabilityIssues.isEmpty {
                    Label(
                        "Every registered worker has a current capacity report.",
                        systemImage: "checkmark.circle.fill"
                    )
                    .font(.subheadline)
                    .foregroundStyle(WisentTone.success.color)
                } else {
                    ForEach(availabilityIssues) { worker in
                        HStack(alignment: .top, spacing: WisentDesign.Space.x3) {
                            Image(systemName: worker.status == .unavailable ? "xmark.circle.fill" : "clock.badge.exclamationmark.fill")
                                .foregroundStyle(worker.status == .unavailable ? WisentTone.danger.color : WisentTone.warning.color)
                                .accessibilityHidden(true)
                            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                                Text(worker.displayName)
                                    .font(.subheadline.weight(.semibold))
                                    .textSelection(.enabled)
                                Text(worker.availabilityReason)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                    .fixedSize(horizontal: false, vertical: true)
                                if let age = worker.ageSeconds {
                                    Text("Last capacity report: \(StadoFormat.duration(age)) ago")
                                        .font(.caption)
                                        .foregroundStyle(.tertiary)
                                }
                            }
                            Spacer()
                            WisentBadge(
                                worker.status == .unavailable ? "Unavailable" : (worker.status == .stale ? "Stale" : "Unregistered"),
                                tone: worker.status == .unavailable ? .danger : .warning
                            )
                        }
                        .accessibilityElement(children: .combine)
                    }
                }
            }
        }
    }

    private var capacityCard: some View {
        WisentPanel {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                Label("Capacity", systemImage: "memorychip")
                    .font(.headline)

                if let memory = aggregateMemory {
                    HStack(alignment: .firstTextBaseline) {
                        Text("GPU memory allocated")
                            .font(.subheadline)
                        Spacer()
                        Text("\(StadoFormat.decimal(memory.used)) / \(StadoFormat.decimal(memory.total)) GB")
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(.secondary)
                    }
                    ProgressView(value: memory.used, total: memory.total)
                        .accessibilityLabel("GPU memory allocated")
                        .accessibilityValue("\(StadoFormat.decimal(memory.used)) of \(StadoFormat.decimal(memory.total)) gigabytes")
                    Text("Calculated only from live workers that publish total and free VRAM.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                } else {
                    UnavailableNotice(
                        title: "Utilization unavailable",
                        detail: "Live capacity reports do not currently include both total and free VRAM. Slot totals and busy-slot counts are not published by this state interface."
                    )
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .top)
    }

    private var throughputCard: some View {
        WisentPanel {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                Label("Throughput", systemImage: "chart.line.uptrend.xyaxis")
                    .font(.headline)

                LabeledContent("Average completion", value: StadoFormat.duration(snapshot.throughput.averageWallSecondsPerCompletedJob))
                LabeledContent("Samples", value: snapshot.throughput.samples.formatted())

                if let projection = snapshot.throughput.projectedRemainingSeconds {
                    LabeledContent("Queue projection", value: StadoFormat.duration(projection))
                    Text("Projection uses observed completion time, queue depth, and currently free slots.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                } else {
                    UnavailableNotice(
                        title: "Queue projection unavailable",
                        detail: "A projection requires completed-job timing samples and at least one free live-worker slot."
                    )
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .top)
    }

    private var recentActivity: some View {
        WisentPanel {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                HStack {
                    Label("Recent outcomes", systemImage: "clock.arrow.circlepath")
                        .font(.headline)
                    Spacer()
                    Text("Published by dashboard")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }

                if snapshot.completedRecent.isEmpty && snapshot.recentFailed.isEmpty {
                    UnavailableNotice(
                        title: "No recent outcomes",
                        detail: "The dashboard has not published recent completed or failed jobs."
                    )
                } else {
                    ForEach(Array(snapshot.recentFailed.prefix(3).enumerated()), id: \.offset) { _, job in
                        OutcomeRow(
                            symbol: "xmark.circle.fill",
                            tone: .danger,
                            title: "Job \(job.jobID) failed",
                            detail: job.task ?? job.model ?? "Task details unavailable",
                            date: nil
                        )
                    }
                    ForEach(Array(snapshot.completedRecent.prefix(3).enumerated()), id: \.offset) { _, job in
                        OutcomeRow(
                            symbol: "checkmark.circle.fill",
                            tone: .success,
                            title: "Job \(job.jobID) completed",
                            detail: job.task ?? job.model ?? "Task details unavailable",
                            date: StadoFormat.date(job.completedAt)
                        )
                    }
                }
            }
        }
    }

    private var aggregateMemory: (used: Double, total: Double)? {
        let reports = snapshot.liveAgents.compactMap { worker -> (Double, Double)? in
            guard let total = worker.totalVRAMGB,
                  let free = worker.freeVRAMGB,
                  total > 0,
                  free >= 0
            else { return nil }
            return (max(0, total - min(free, total)), total)
        }
        guard !reports.isEmpty else { return nil }
        return (
            reports.reduce(0) { $0 + $1.0 },
            reports.reduce(0) { $0 + $1.1 }
        )
    }
}

private struct OutcomeRow: View {
    let symbol: String
    let tone: WisentTone
    let title: String
    let detail: String
    let date: Date?

    var body: some View {
        HStack(spacing: WisentDesign.Space.x3) {
            Image(systemName: symbol)
                .foregroundStyle(tone.color)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text(title)
                    .font(.subheadline.weight(.medium))
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer()
            if let date {
                Text(date, style: .relative)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                Text("Time unavailable")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        }
        .accessibilityElement(children: .combine)
    }
}
