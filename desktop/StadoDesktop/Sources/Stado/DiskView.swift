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
                WisentLoadingPanel(
                    title: "Reading the cleanup report",
                    detail: "Disk pressure, thresholds, and what the last registry-controlled pass reclaimed."
                )
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

        if presentation.severity == .critical || presentation.severity == .warning {
            WisentAlertPanel(
                tone: presentation.severity == .critical ? .danger : .warning,
                title: presentation.title,
                detail: report.errors.first ?? presentation.detail,
                command: "curl \(store.dashboardURLString)/api/cleanup.json"
            )
        }

        WisentSignalStrip(signals: signals(report))

        WisentCounterRow(counters: [
            WisentCounterRow.Counter(
                "Free now",
                value: DisplayFormat.bytes(report.freeBytesAfter),
                detail: "Reported after the last pass",
                tone: report.pressureActive == true ? .warning : .neutral
            ),
            WisentCounterRow.Counter(
                "Low threshold",
                value: DisplayFormat.bytes(report.lowBytes),
                detail: "Pressure starts below this"
            ),
            WisentCounterRow.Counter(
                "Target",
                value: DisplayFormat.bytes(report.targetBytes),
                detail: "A pass stops here"
            ),
            WisentCounterRow.Counter(
                "Reclaimed",
                value: DisplayFormat.bytes(report.reclaimedBytes),
                detail: "Measured, not estimated"
            ),
        ])

        WisentSectionBox(
            title: "Cleaners",
            detail: "Only the cleaners the canonical registry declares may run, and only within the registry's limits.",
            trailing: report.caps.activeLabels.isEmpty
                ? "unbounded pass"
                : "bounded by \(report.caps.activeLabels.joined(separator: ", "))"
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
                    ForEach(report.cleaners.namedReports, id: \.0) { item in
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
                "Mode",
                value: report.mode?.capitalized ?? "Not configured",
                tone: .neutral
            ),
            WisentSignal(
                "Active slots",
                value: report.activeSlotCount.formatted(.number),
                tone: report.activeSlotCount > 0 ? .warning : .neutral
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
            tone: report.mode == "enforce" ? .danger : .warning,
            title: "Run one registry-controlled cleanup pass?",
            lines: lines(for: report),
            reasonCode: report.outcome,
            listing: report.cleaners.namedReports.map { name, cleaner in
                "\(name) — \(cleaner.eligibleItems.formatted(.number)) eligible of \(cleaner.scannedItems.formatted(.number)) scanned"
            },
            footnote: "Mode \(report.mode ?? "not configured") · low \(DisplayFormat.bytes(report.lowBytes)) · target \(DisplayFormat.bytes(report.targetBytes)) · interval \(report.checkIntervalSeconds.map { "\($0) s" } ?? "not configured")",
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
        var lines: [String] = []
        switch report.mode {
        case "enforce":
            lines.append("Deletion is authorized on this host. The pass removes eligible cached items until free space reaches the registry target. Deleted items are not recoverable from this console.")
        case "report":
            lines.append("This host's policy is report mode: the pass records what it would delete and deletes nothing.")
        case "off":
            lines.append("This host's policy is off: the pass will do no work and no cleaner will run.")
        default:
            lines.append("The service did not report a cleanup mode, so the registry policy in force decides whether anything is deleted.")
        }
        lines.append("The pass is bounded by the registry's byte, item, scan, and time limits, and it refuses to run while compute slots are active.")
        return lines
    }
}
