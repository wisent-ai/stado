import SwiftUI
import WisentDesignSystem

struct DiskView: View {
    @ObservedObject var store: OperationsStore
    @ObservedObject var cleanupStore: CleanupStore
    let scope: String

    @State private var showsCleanupDecision = false

    var body: some View {
        WisentScreen(
            title: "Disk",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(cleanupStore.lastUpdated))",
            actions: contextActions
        ) {
            if let message = cleanupStore.errorMessage {
                WisentErrorBanner(
                    title: cleanupStore.report == nil
                        ? "Cleanup state unavailable"
                        : "Refresh failed — the report below is the last one the service returned",
                    detail: message,
                    action: WisentAction("Retry", symbol: "arrow.clockwise") {
                        Task { await cleanupStore.refresh() }
                    }
                )
            }

            WisentMutationBar(outcome: cleanupStore.mutation) { cleanupStore.clearMutation() }

            if let report = cleanupStore.report {
                reportBody(report)
            } else if cleanupStore.isRefreshing {
                Group {
                    let loadingTitle = "Reading the cleanup report"
                    WisentSectionBox(title: loadingTitle, detail: "Disk pressure, thresholds, and what the last registry-controlled pass reclaimed.") {
                        WisentSkeletonList(label: loadingTitle)
                    }
                }
            } else {
                WisentEmptyPanel(
                    title: "No cleanup report",
                    detail: "The dashboard has not answered the cleanup interface yet. This screen never estimates free space.",
                    symbol: "externaldrive.badge.questionmark",
                    action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary) {
                        Task { await cleanupStore.refresh() }
                    }
                )
            }
        }
        .task { await cleanupStore.refresh() }
        .sheet(isPresented: $showsCleanupDecision) {
            if let report = cleanupStore.report {
                decisionDialog(report)
            }
        }
    }

    private var contextActions: [WisentAction] {
        [
            WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !cleanupStore.isRefreshing) {
                Task { await cleanupStore.refresh() }
            },
            WisentAction(
                "Run cleanup pass…",
                symbol: "sparkles",
                kind: .primary,
                isEnabled: canRunCleanup
            ) {
                showsCleanupDecision = true
            },
        ]
    }

    private var canRunCleanup: Bool {
        guard let report = cleanupStore.report else { return false }
        return !cleanupStore.isRunningCleanup
            && !cleanupStore.isRefreshing
            && !report.lockBusy
            && cleanupStore.dashboardAddress != nil
    }

    @ViewBuilder
    private func reportBody(_ report: CleanupReport) -> some View {
        let presentation = report.outcomePresentation
        let cleaners = report.cleaners?.namedReports ?? []

        if presentation.severity == .critical || presentation.severity == .warning {
            WisentAlertPanel(
                tone: presentation.severity == .critical ? .danger : .warning,
                title: presentation.title,
                detail: report.errors.first ?? presentation.detail
            )
        }

        WisentSignalStrip(signals: signals(report))

        WisentPanel {
            Text(report.rule.summary)
                .font(WisentTypeScale.body())
                .foregroundStyle(report.rule.triggered ? WisentDesign.danger : WisentDesign.ink)
                .fixedSize(horizontal: false, vertical: true)
        }

        WisentCounterRow(counters: [
            WisentCounterRow.Counter(
                "Free now",
                value: DisplayFormat.bytes(report.freeBytesAfter),
                detail: "Reported after the last pass",
                tone: report.pressureActive == true ? .warning : .neutral
            ),
            WisentCounterRow.Counter(
                "Volume",
                value: DisplayFormat.bytes(report.totalBytes),
                detail: "The volume holding the agent's home"
            ),
            WisentCounterRow.Counter(
                "Reclaimed",
                value: DisplayFormat.bytes(report.reclaimedBytes),
                detail: "Measured, not estimated"
            ),
        ])

        WisentSectionBox(
            title: "Cleaners",
            detail: "At 80% used every cleaner takes everything the fleet put in its area. User data, ~/.ssh, declared and installed releases and running jobs stay.",
            trailing: cleaners.isEmpty ? "no cleaner ran" : "\(cleaners.count) cleaners"
        ) {
            WisentTableFrame {
                VStack(spacing: 0) {
                    ConsoleTableHead(cells: [
                        ConsoleHeaderCell("Cleaner", width: 200),
                        ConsoleHeaderCell("Scanned", width: 84, trailing: true),
                        ConsoleHeaderCell("Eligible", width: 84, trailing: true),
                        ConsoleHeaderCell("Deleted", width: 84, trailing: true),
                        ConsoleHeaderCell("Freed", width: 96, trailing: true),
                    ])
                    ForEach(cleaners, id: \.0) { item in
                        let (name, cleaner) = item
                        ConsoleTableRow {
                            ConsoleCell(text: name, width: 200, strong: true)
                            ConsoleCell(text: cleaner.scannedItems.formatted(.number), width: 84, trailing: true, digits: true)
                            ConsoleCell(text: cleaner.eligibleItems.formatted(.number), width: 84, trailing: true, digits: true)
                            ConsoleCell(text: cleaner.deletedItems.formatted(.number), width: 84, trailing: true, digits: true)
                            ConsoleCell(text: DisplayFormat.bytes(cleaner.actualFreeDeltaBytes), width: 96, trailing: true, digits: true)
                        }
                    }
                }
            }
        }

        ForEach(cleaners, id: \.0) { name, cleaner in
            if !cleaner.skipped.isEmpty {
                WisentSectionBox(
                    title: "Skipped: \(name)",
                    detail: "Exact reasons and counts returned by this cleaner."
                ) {
                    WisentPanel {
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                            ForEach(cleaner.skipped.sorted { $0.key < $1.key }, id: \.key) { reason in
                                HStack(alignment: .firstTextBaseline) {
                                    Text(reason.key)
                                        .font(WisentTypeScale.identifier())
                                        .textSelection(.enabled)
                                        .fixedSize(horizontal: false, vertical: true)
                                    Spacer()
                                    Text(reason.value.formatted(.number))
                                        .monospacedDigit()
                                }
                            }
                        }
                    }
                }
            }
        }

        if !report.errors.isEmpty {
            WisentSectionBox(
                title: "Sanitized errors",
                detail: "Quoted exactly as the cleanup service returned them.",
                trailing: "\(report.errors.count) reported"
            ) {
                WisentPanel {
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                        ForEach(report.errors, id: \.self) { error in
                            Text(error)
                                .font(WisentTypeScale.identifier())
                                .foregroundStyle(WisentDesign.danger)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }
        }
    }

    private func signals(_ report: CleanupReport) -> [WisentSignal] {
        var values: [WisentSignal] = [
            WisentSignal(
                "Outcome",
                value: report.outcome.humanizedIdentifier,
                tone: tone(for: report.outcomePresentation.severity)
            ),
            WisentSignal(
                "Pressure",
                value: pressureLabel(report),
                tone: report.pressureActive == true ? .warning : .neutral
            ),
            WisentSignal(
                "Used",
                value: report.rule.usedPercent.map { String(format: "%.1f%%", $0) } ?? "Not read",
                tone: report.rule.triggered ? .warning : .neutral
            ),
            WisentSignal(
                "Running jobs",
                value: report.activeJobCount.formatted(.number),
                tone: report.activeJobCount > 0 ? .warning : .neutral
            ),
            WisentSignal(
                "Last success",
                value: ConsoleFormat.relative(DisplayFormat.date(report.lastSuccessAt)),
                tone: .neutral
            ),
        ]
        if report.lockBusy {
            values.append(WisentSignal("Lock", value: "Held by another pass", tone: .warning))
        }
        return values
    }

    private func pressureLabel(_ report: CleanupReport) -> String {
        switch report.pressureActive {
        case true: "Active"
        case false: "Clear"
        case nil: "Not reported"
        }
    }

    /// `never_run` and `not reported` are neutral. Red is reserved for a pass
    /// that actually failed.
    private func tone(for severity: OutcomePresentation.Severity) -> WisentTone {
        switch severity {
        case .healthy: .success
        case .neutral: .neutral
        case .warning: .warning
        case .critical: .danger
        }
    }

    // MARK: Irreversible decision

    private func decisionDialog(_ report: CleanupReport) -> some View {
        WisentDecisionDialog(
            tone: report.rule.triggered ? .danger : .warning,
            title: "Run one cleanup pass?",
            lines: lines(for: report),
            reasonCode: report.outcome,
            listing: (report.cleaners?.namedReports ?? []).map { name, cleaner in
                "\(name) — \(cleaner.eligibleItems.formatted(.number)) eligible of \(cleaner.scannedItems.formatted(.number)) scanned"
            },
            footnote: report.rule.summary,
            actions: [
                WisentAction("Keep current state", kind: .primary) { showsCleanupDecision = false },
                WisentAction("Run cleanup pass", symbol: "sparkles", kind: .destructive) {
                    showsCleanupDecision = false
                    Task { await cleanupStore.runCleanup() }
                },
            ]
        )
    }

    private func lines(for report: CleanupReport) -> [String] {
        if report.rule.triggered {
            return [
                "This host's volume is at or past 80% used. The pass deletes everything the fleet put on it — build caches, job outputs, proven backup copies, unused release versions, recordings, logs and local snapshots — with no limit. Deleted items are not recoverable from this console.",
                "User data, ~/.ssh, releases the fleet declares or this host runs, and running jobs' trees stay.",
            ]
        }
        return [
            "This host's volume is under 80% used, so the pass reads it and deletes nothing.",
        ]
    }
}
