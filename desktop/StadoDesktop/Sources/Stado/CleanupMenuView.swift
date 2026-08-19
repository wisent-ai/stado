import AppKit
import SwiftUI
import WisentDesignSystem

/// The menu bar states disk posture and hands the decision to the console.
///
/// Running an irreversible pass from a popover meant the operator never saw
/// what the pass would delete, and a refusal disappeared with the popover. The
/// decision, its dialog, and the service's verbatim answer now live on one
/// screen instead of two surfaces.
struct CleanupMenuView: View {
    @Environment(\.openWindow) private var openWindow
    @ObservedObject var store: CleanupStore
    @ObservedObject var router: ConsoleRouter

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            header

            if let message = store.errorMessage {
                WisentAlertPanel(
                    tone: .danger,
                    title: "Cleanup state unavailable",
                    detail: message
                )
            }

            if let report = store.report {
                WisentSignalStrip(signals: signals(report))
                if report.outcomePresentation.severity == .critical || report.outcomePresentation.severity == .warning {
                    WisentAlertPanel(
                        tone: report.outcomePresentation.severity == .critical ? .danger : .warning,
                        title: report.outcomePresentation.title,
                        detail: report.errors.first ?? report.outcomePresentation.detail
                    )
                }
            } else if store.isRefreshing {
                WisentLoadingPanel(
                    title: "Reading the cleanup report",
                    detail: "Disk pressure and the outcome of the last registry-controlled pass."
                )
            } else {
                WisentEmptyPanel(
                    title: "No cleanup report",
                    detail: "The Stado dashboard has not answered the cleanup interface yet.",
                    symbol: "externaldrive.badge.questionmark"
                )
            }

            footer
        }
        .padding(WisentDesign.Space.x5)
        .frame(width: 380)
        .background(WisentDesign.canvas)
    }

    private var header: some View {
        HStack(spacing: WisentDesign.Space.x3) {
            VStack(alignment: .leading, spacing: 1) {
                Text("STADO · DISK")
                    .font(WisentTypeScale.eyebrow())
                    .tracking(0.8)
                    .foregroundStyle(WisentDesign.muted)
                Text(store.report?.outcomePresentation.title ?? "Disk cleanup")
                    .font(WisentTypeScale.screenTitle())
                    .foregroundStyle(WisentDesign.ink)
            }
            Spacer(minLength: 0)
            Text("Read \(ConsoleFormat.relative(store.lastUpdated))")
                .font(WisentTypeScale.identifierSmall())
                .foregroundStyle(WisentDesign.muted)
        }
    }

    private var footer: some View {
        HStack(spacing: WisentDesign.Space.x2) {
            WisentActionButton(
                action: WisentAction("Open Disk", symbol: "externaldrive", kind: .primary) {
                    router.show(.disk)
                    openWindow(id: "operations-console")
                }
            )
            WisentActionButton(
                action: WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task { await store.refresh() }
                }
            )
            Spacer(minLength: 0)
            WisentActionButton(
                action: WisentAction("Quit", kind: .plain) {
                    NSApplication.shared.terminate(nil)
                }
            )
        }
    }

    private func signals(_ report: CleanupReport) -> [WisentSignal] {
        [
            WisentSignal(
                "Free",
                value: DisplayFormat.bytes(report.freeBytesAfter),
                tone: report.pressureActive == true ? .warning : .success
            ),
            WisentSignal(
                "Mode",
                value: report.mode?.capitalized ?? "Not configured",
                tone: .neutral
            ),
            WisentSignal(
                "Reclaimed",
                value: DisplayFormat.bytes(report.reclaimedBytes),
                tone: .neutral
            ),
        ]
    }
}
