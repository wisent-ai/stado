import SwiftUI
import WisentDesignSystem

/// What the host had left of its memory when the pass read it, and whether
/// that reading has stopped the host from taking work.
struct MemoryReadingSection: View {
    let state: MemoryPolicyState

    var body: some View {
        if state.isRefusingPlacement {
            WisentAlertPanel(
                tone: .danger,
                title: "\(state.target) is not accepting new jobs",
                detail: "The last memory pass on this host recorded refuse_placement while the host was over its watermark, so its capacity publication withholds it from selection. The recorded admission reason is \(MemoryReclaimReport.admissionReason)."
            )
        }

        if state.isDefaulted {
            WisentAlertPanel(
                tone: .warning,
                title: "\(state.target) declares no memory_reclaim",
                detail: "It is measured against the reporting default the writer resolved: memory is read and reported, and no repair is armed. Nothing on this host is restarted or terminated until the registry names a repair."
            )
        }

        if let report = state.report {
            WisentSignalStrip(signals: signals(report))
            counters(report)
            if let reading = report.currentReading, reading.reportsCompressor {
                compressor(reading)
            }
        } else {
            WisentAlertPanel(
                tone: .warning,
                title: "No memory reading for \(state.target)",
                detail: "This console read the declaration for this target, but the dashboard published no memory pass report for it. The declared fields below are what the host will be measured against once it runs one."
            )
        }
    }

    private func signals(_ report: MemoryReclaimReport) -> [WisentSignal] {
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
                "Declared mode",
                value: state.isDefaulted ? "\(state.modeLabel) (default)" : state.modeLabel,
                tone: state.mode == .enforce ? .warning : .neutral
            ),
            WisentSignal(
                "Placement",
                value: placementLabel,
                tone: state.isRefusingPlacement ? .danger : .neutral
            ),
            WisentSignal(
                "Running jobs",
                value: report.activeJobCount.map { $0.formatted(.number) } ?? "Not reported",
                tone: (report.activeJobCount ?? Int.zero) > Int.zero ? .warning : .neutral
            ),
            WisentSignal(
                "Last success",
                value: ConsoleFormat.relative(DisplayFormat.date(report.lastSuccessAt)),
                tone: .neutral
            ),
        ]
        if report.lockBusy {
            values.append(WisentSignal("Pass lock", value: "Held by another writer", tone: .warning))
        }
        return values
    }

    private func counters(_ report: MemoryReclaimReport) -> some View {
        let reading = report.currentReading
        return WisentCounterRow(counters: [
            WisentCounterRow.Counter(
                "Available",
                value: DisplayFormat.bytes(reading?.availableBytes),
                detail: reading?.availableMB.map { "\($0.formatted(.number)) MiB, as the host read it" }
                    ?? "The host did not answer this field",
                tone: report.pressureActive == true ? .warning : .neutral
            ),
            WisentCounterRow.Counter(
                "Installed",
                value: DisplayFormat.bytes(reading?.totalBytes),
                detail: "Physical memory on this host"
            ),
            WisentCounterRow.Counter(
                "Swap used",
                value: DisplayFormat.bytes(reading?.swapUsedBytes),
                detail: "Of \(DisplayFormat.bytes(reading?.swapTotalBytes)) configured"
            ),
            WisentCounterRow.Counter(
                "Swap utilisation",
                value: reading?.swapUsedPct.map { "\($0)%" } ?? "Not reported",
                detail: state.highSwapUsedPct.map { "Pressure at \($0)% or above" }
                    ?? "No swap watermark is in force",
                tone: swapTone(reading)
            ),
        ])
    }

    /// macOS answers two counters no free-page figure carries: a machine with
    /// pages in the compressor and a lifetime swapout count is not merely
    /// busy. Absent on Linux, and absent is not zero.
    private func compressor(_ reading: MemoryReadingSnapshot) -> some View {
        WisentSectionBox(
            title: "Compressor",
            detail: "macOS reports these beside the page classes; a Linux host answers neither and they are left out rather than shown as zero."
        ) {
            WisentPanel {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    WisentField(
                        label: "Pages in the compressor",
                        value: reading.compressorPages.map { $0.formatted(.number) } ?? "Not reported"
                    )
                    WisentField(
                        label: "Lifetime swapouts",
                        value: reading.swapouts.map { $0.formatted(.number) } ?? "Not reported"
                    )
                }
            }
        }
    }

    private var placementLabel: String {
        guard state.publishedRefusePlacement else { return "Accepting jobs" }
        return state.isRefusingPlacement ? "Refused" : "Refuses under pressure"
    }

    private func pressureLabel(_ report: MemoryReclaimReport) -> String {
        switch report.pressureActive {
        case true: "Active"
        case false: "Clear"
        case nil: "Not reported"
        }
    }

    private func swapTone(_ reading: MemoryReadingSnapshot?) -> WisentTone {
        guard let used = reading?.swapUsedPct, let watermark = state.highSwapUsedPct else { return .neutral }
        return used >= watermark ? .warning : .neutral
    }

    /// `never_run` and an unreported field are neutral. Red is reserved for a
    /// pass that actually failed.
    private func tone(for severity: OutcomePresentation.Severity) -> WisentTone {
        switch severity {
        case .healthy: .success
        case .neutral: .neutral
        case .warning: .warning
        case .critical: .danger
        }
    }
}
