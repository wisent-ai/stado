import SwiftUI
import WisentDesignSystem

private enum QueueFacet: String, Hashable {
    case allOutcomes
    case failed
    case completed
    case models
}

private struct QueueRecord: Identifiable {
    enum Kind {
        case failed
        case completed

        var label: String {
            switch self {
            case .failed: "Failed"
            case .completed: "Completed"
            }
        }

        var tone: WisentTone {
            switch self {
            case .failed: .danger
            case .completed: .success
            }
        }
    }

    let id: String
    let kind: Kind
    let jobID: String
    let model: String?
    let task: String?
    let wallSeconds: Double?
    let completedAt: String?
    let error: String?
}

private struct ModelRecord: Identifiable {
    let id: String
    let model: String
    let counts: JobCounts

    var active: Int { counts.queue + counts.running }
}

struct QueueView: View {
    @ObservedObject var store: OperationsStore
    @ObservedObject var fleetStore: FleetControlStore
    let scope: String

    @State private var facet: QueueFacet = .allOutcomes
    @State private var selection: String?
    @State private var rerunCandidate: QueueRecord?

    var body: some View {
        WisentScreen(
            title: "Queue",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(store.lastUpdated))",
            actions: [
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task { await store.refresh() }
                }
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: 0) {
                if let message = store.errorMessage {
                    WisentErrorBanner(
                        title: store.isShowingStaleSnapshot
                            ? "Refresh failed — the rows below are the last good snapshot"
                            : "Queue state unavailable",
                        detail: message,
                        action: WisentAction("Retry", symbol: "arrow.clockwise") {
                            Task { await store.refresh() }
                        }
                    )
                    .padding(WisentDesign.Space.x4)
                }

                if let snapshot = store.snapshot, snapshot.ready {
                    zones(snapshot)
                } else {
                    placeholder
                        .padding(WisentDesign.Space.x6)
                    Spacer(minLength: 0)
                }

                WisentMutationBar(outcome: fleetStore.mutation) { fleetStore.clearMutation() }
                    .padding(.horizontal, WisentDesign.Space.x4)
                    .padding(.bottom, fleetStore.mutation == .idle ? 0 : WisentDesign.Space.x3)
            }
        }
        .sheet(item: $rerunCandidate) { record in
            rerunDialog(record)
        }
    }

    @ViewBuilder
    private var placeholder: some View {
        if store.isRefreshing {
            WisentLoadingPanel(
                title: "Reading queue state",
                detail: "Queued and running work by model, plus the recent completed and failed records the dashboard publishes."
            )
        } else {
            WisentEmptyPanel(
                title: "No queue state",
                detail: "The dashboard has not published a ready snapshot. Individual queued job records are not part of this interface; only per-model aggregates and recent outcomes are.",
                symbol: "tray",
                action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary) {
                    Task { await store.refresh() }
                }
            )
        }
    }

    // MARK: Three zones

    private func zones(_ snapshot: DashboardSnapshot) -> some View {
        HStack(spacing: 0) {
            WisentFacetRail(
                groups: facetGroups(snapshot),
                footerTitle: "Queue source",
                footerDetail: snapshot.bucket?.isEmpty == false ? snapshot.bucket! : "Dashboard-managed storage"
            )
            table(snapshot)
            inspector(snapshot)
        }
        .frame(maxHeight: .infinity)
    }

    private func facetGroups(_ snapshot: DashboardSnapshot) -> [WisentFacetGroup] {
        let records = allRecords(snapshot)
        let failed = records.count { $0.kind == .failed }
        let completed = records.count { $0.kind == .completed }
        return [
            WisentFacetGroup(
                "Outcomes",
                facets: [
                    WisentFacet(
                        id: QueueFacet.allOutcomes.rawValue,
                        label: "All outcomes",
                        count: records.count,
                        isSelected: facet == .allOutcomes,
                        select: { select(.allOutcomes) }
                    ),
                    WisentFacet(
                        id: QueueFacet.failed.rawValue,
                        label: "Failed",
                        count: failed,
                        tone: failed > 0 ? .danger : .neutral,
                        isSelected: facet == .failed,
                        select: { select(.failed) }
                    ),
                    WisentFacet(
                        id: QueueFacet.completed.rawValue,
                        label: "Completed",
                        count: completed,
                        isSelected: facet == .completed,
                        select: { select(.completed) }
                    ),
                ]
            ),
            WisentFacetGroup(
                "Current work",
                facets: [
                    WisentFacet(
                        id: QueueFacet.models.rawValue,
                        label: "By model",
                        count: modelRecords(snapshot).count,
                        tone: snapshot.counts.queue > 0 ? .warning : .neutral,
                        isSelected: facet == .models,
                        select: { select(.models) }
                    )
                ]
            ),
        ]
    }

    @ViewBuilder
    private func table(_ snapshot: DashboardSnapshot) -> some View {
        if facet == .models {
            let rows = modelRecords(snapshot)
            if rows.isEmpty {
                emptyTable(
                    title: "No queued or running work",
                    detail: "The latest snapshot reports no model group with queued or running jobs."
                )
            } else {
                ConsoleTable(head: [
                    ConsoleHeaderCell("Model"),
                    ConsoleHeaderCell("Queued", width: 74, trailing: true),
                    ConsoleHeaderCell("Running", width: 74, trailing: true),
                    ConsoleHeaderCell("Completed", width: 84, trailing: true),
                    ConsoleHeaderCell("Failed", width: 68, trailing: true),
                ]) {
                    ForEach(rows) { row in
                        ConsoleTableRow(isSelected: selection == row.id, select: { selection = row.id }) {
                            ConsoleCell(text: row.model, identifier: true, strong: true)
                            ConsoleCell(text: row.counts.queue.formatted(.number), width: 74, trailing: true, digits: true, tone: row.counts.queue > 0 ? .warning : .neutral)
                            ConsoleCell(text: row.counts.running.formatted(.number), width: 74, trailing: true, digits: true, tone: row.counts.running > 0 ? .success : .neutral)
                            ConsoleCell(text: row.counts.completed.formatted(.number), width: 84, trailing: true, digits: true)
                            ConsoleCell(text: row.counts.failed.formatted(.number), width: 68, trailing: true, digits: true, tone: row.counts.failed > 0 ? .danger : .neutral)
                        }
                    }
                }
            }
        } else {
            let rows = records(snapshot)
            if rows.isEmpty {
                if facet == .allOutcomes {
                    emptyTable(
                        title: "No recent outcomes",
                        detail: "The dashboard has not published a completed or failed job in this snapshot."
                    )
                } else {
                    filteredEmptyTable
                }
            } else {
                let minority = minorityKind(in: rows)
                ConsoleTable(head: [
                    ConsoleHeaderCell("Job", width: 220),
                    ConsoleHeaderCell("Model"),
                    ConsoleHeaderCell("Task"),
                    ConsoleHeaderCell("Wall", width: 72, trailing: true),
                    ConsoleHeaderCell("State", width: 92, trailing: true),
                ]) {
                    ForEach(rows) { row in
                        ConsoleTableRow(isSelected: selection == row.id, select: { selection = row.id }) {
                            ConsoleCell(text: row.jobID, width: 220, identifier: true, strong: true)
                            ConsoleCell(text: row.model ?? "—")
                            ConsoleCell(text: row.task ?? "—")
                            ConsoleCell(text: StadoFormat.duration(row.wallSeconds), width: 72, trailing: true, digits: true)
                            stateCell(row, minority: minority)
                        }
                    }
                }
            }
        }
    }

    /// A pill only for the minority state. When every visible row completed,
    /// the count belongs in the facet rail and no row wears a badge.
    @ViewBuilder
    private func stateCell(_ record: QueueRecord, minority: QueueRecord.Kind?) -> some View {
        if record.kind == minority {
            HStack {
                Spacer(minLength: 0)
                WisentStatusChip(text: record.kind.label, tone: record.kind.tone)
            }
            .frame(width: 92)
        } else {
            ConsoleCell(text: "", width: 92)
        }
    }

    private func emptyTable(title: String, detail: String) -> some View {
        VStack {
            WisentEmptyPanel(title: title, detail: detail, symbol: "tray")
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(WisentDesign.surface)
    }

    /// Empty because a filter says so is a different state, with a different
    /// remedy, from empty because there is nothing.
    private var filteredEmptyTable: some View {
        VStack {
            WisentEmptyPanel(
                title: "No rows in this filter",
                detail: "The snapshot has outcomes, but none of them match the selected facet.",
                symbol: "line.3.horizontal.decrease.circle",
                action: WisentAction("Clear filters", kind: .primary) { select(.allOutcomes) }
            )
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(WisentDesign.surface)
    }

    @ViewBuilder
    private func inspector(_ snapshot: DashboardSnapshot) -> some View {
        if facet == .models, let model = modelRecords(snapshot).first(where: { $0.id == selection }) {
            WisentInspector(
                eyebrow: "Model group",
                title: model.model,
                badges: model.counts.queue > 0 ? [("Queued", .warning)] : []
            ) {
                WisentField(label: "Queued", value: model.counts.queue.formatted(.number))
                WisentField(label: "Running", value: model.counts.running.formatted(.number))
                WisentField(label: "Completed", value: model.counts.completed.formatted(.number))
                WisentField(
                    label: "Failed",
                    value: model.counts.failed.formatted(.number),
                    tone: model.counts.failed > 0 ? .danger : .neutral
                )
                WisentCapabilityList(
                    title: "Not published for this group",
                    items: [
                        "Individual queued job records",
                        "Per-job placement decisions",
                        "Submitted job payloads",
                    ],
                    isAvailable: false
                )
            }
        } else if let record = records(snapshot).first(where: { $0.id == selection }) {
            WisentInspector(
                eyebrow: record.kind == .failed ? "Failed job" : "Completed job",
                title: record.jobID,
                badges: [(record.kind.label, record.kind.tone)]
            ) {
                WisentField(label: "Model", value: record.model ?? "Not reported")
                WisentField(label: "Task", value: record.task ?? "Not reported")
                WisentField(label: "Wall time", value: StadoFormat.duration(record.wallSeconds))
                WisentField(
                    label: "Completed at",
                    value: StadoFormat.date(record.completedAt)?.formatted(date: .abbreviated, time: .standard)
                        ?? "Not reported"
                )
                if record.kind == .failed {
                    WisentAlertPanel(
                        tone: .danger,
                        title: "Backend failure",
                        detail: record.error ?? "The dashboard published this failure without a sanitized reason.",
                        command: "stado job watch \(record.jobID)"
                    )
                    WisentActionButton(
                        action: WisentAction(
                            "Rerun job…",
                            symbol: "arrow.clockwise",
                            kind: .primary,
                            isEnabled: !fleetStore.mutation.isWorking && fleetStore.isConfigured
                        ) {
                            rerunCandidate = record
                        }
                    )
                }
            }
        } else {
            WisentInspector(eyebrow: "Selection", title: "No row selected") {
                Text("Select a job or a model group to read its full state. The list stays visible so the selected row can be compared with the rest.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            }
        }
    }

    // MARK: Irreversible decision

    private func rerunDialog(_ record: QueueRecord) -> some View {
        WisentDecisionDialog(
            tone: .warning,
            title: "Resubmit job \(record.jobID)?",
            lines: [
                "Stado resubmits the exact recorded specification for this job. It claims a worker slot and bills whichever provider runs it.",
                "The failed record stays in the dashboard's recent-failure list; a rerun does not clear it.",
            ],
            reasonCode: record.kind == .failed ? "job_failed" : nil,
            listing: (record.error ?? "The dashboard published this failure without a sanitized reason.")
                .split(separator: "\n", omittingEmptySubsequences: false)
                .map(String.init),
            footnote: "Runs stado job rerun \(record.jobID) through the dashboard's allowlisted command bridge, with the mutation confirmation it requires.",
            actions: [
                WisentAction("Keep the failure only", kind: .primary) { rerunCandidate = nil },
                WisentAction("Rerun job", symbol: "arrow.clockwise", kind: .destructive) {
                    let jobID = record.jobID
                    rerunCandidate = nil
                    Task { await fleetStore.rerunJob(jobID) }
                },
            ]
        )
    }

    // MARK: Records

    private func allRecords(_ snapshot: DashboardSnapshot) -> [QueueRecord] {
        let failed = snapshot.recentFailed.enumerated().map { index, job in
            QueueRecord(
                id: "failed-\(index)-\(job.jobID)",
                kind: .failed,
                jobID: job.jobID,
                model: job.model,
                task: job.task,
                wallSeconds: nil,
                completedAt: nil,
                error: job.error
            )
        }
        let completed = snapshot.completedRecent.enumerated().map { index, job in
            QueueRecord(
                id: "completed-\(index)-\(job.jobID)",
                kind: .completed,
                jobID: job.jobID,
                model: job.model,
                task: job.task,
                wallSeconds: job.wallSeconds,
                completedAt: job.completedAt,
                error: nil
            )
        }
        return failed + completed
    }

    private func records(_ snapshot: DashboardSnapshot) -> [QueueRecord] {
        let records = allRecords(snapshot)
        switch facet {
        case .failed: return records.filter { $0.kind == .failed }
        case .completed: return records.filter { $0.kind == .completed }
        case .allOutcomes, .models: return records
        }
    }

    private func modelRecords(_ snapshot: DashboardSnapshot) -> [ModelRecord] {
        snapshot.byModelState
            .filter { $0.value.queue > 0 || $0.value.running > 0 }
            .map { ModelRecord(id: "model-\($0.key)", model: $0.key, counts: $0.value) }
            .sorted { $0.active == $1.active ? $0.model < $1.model : $0.active > $1.active }
    }

    private func minorityKind(in rows: [QueueRecord]) -> QueueRecord.Kind? {
        let failed = rows.count { $0.kind == .failed }
        let completed = rows.count - failed
        guard failed > 0, completed > 0 else { return nil }
        return failed <= completed ? .failed : .completed
    }

    private func select(_ value: QueueFacet) {
        facet = value
        selection = nil
    }
}
