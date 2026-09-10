import SwiftUI
import WisentDesignSystem

/// One clearance the operator has asked for and not yet authorized.
///
/// Internal rather than private because the "Clear…" button that builds one
/// sits in `Releases/Detail/ReleasesQuarantine.swift` and the sheet that reads
/// one in `Releases/ReleasesClearDialog.swift`: Swift scopes `private` to one
/// file.
struct PendingClearance: Identifiable {
    let pair: ReleaseInventoryPair
    let entry: ReleaseQuarantineEntry

    var id: String { "\(pair.id)/\(entry.digest)" }
}

/// Why a rollout is where it is, host by host.
///
/// One row per product target, carrying the verdict `stado release doctor`
/// reached, the desired and observed versions side by side, and — when the
/// fleet calls the rollout blocked — the blockers in the CLI's own words. The
/// pane below the table holds the three things that were previously only
/// reachable over SSH: the candidate's port, health and liveness, the tail of
/// the candidate's own stderr off the host, and the quarantined digests, one
/// of which is usually the reason the rollout will never finish.
///
/// Quarantine clearance and recorded-run resumption show the exact command
/// before mutation. Resumption keeps the original source and recorded jobs.
///
/// The screen's own parts live in `Releases/`: the frame, the pane heights and
/// the pipeline runs in `Releases/ReleasesChrome.swift`, the rows in
/// `Releases/ReleasesTable.swift`, the evidence pane in `Releases/Detail/`,
/// mutation dialogs in `Releases/ReleasesClearDialog.swift` and the reads in
/// `Releases/ReleasesLoading.swift`. The stored state stays here, because
/// `@State` belongs to the view rather than to any one of its sections.
struct ReleasesView: View {
    @ObservedObject var store: ReleaseEvidenceStore
    @ObservedObject var fleetStore: FleetControlStore
    let scope: String

    @State var selection: ReleaseInventoryPair?
    @State var stream: ReleaseLogStreamSelection = .err
    @State var lines = 40
    @State var clearance: PendingClearance?
    @State var reason = ""
    @State var selectedRunID: String?
    @State var resumption: ReleasePipelineRunRecord?

    var selectedRun: ReleasePipelineRunRecord? {
        store.pipelineRuns.first { $0.runID == selectedRunID }
    }

    static let lineChoices = [40, 100, 250, 500, 1_000]

    var body: some View {
        WisentScreen(
            title: "Releases",
            scope: scope,
            freshness: freshness,
            actions: [
                WisentAction(
                    "Re-diagnose",
                    symbol: "arrow.clockwise",
                    isEnabled: !store.isRefreshing
                ) {
                    Task { await reload() }
                },
                WisentAction(
                    "Resume selected run",
                    symbol: "play",
                    isEnabled: selectedRun != nil && fleetStore.isConfigured && !store.mutation.isWorking
                ) {
                    resumption = selectedRun
                }
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing:
                0
            ) {
                if let problem = store.inventoryProblem {
                    WisentErrorBanner(
                        title: store.rows.isEmpty
                            ? "No rollout could be listed"
                            : "Re-diagnosis failed — the rows below are the last reading",
                        detail: problem,
                        action: WisentAction("Retry", symbol: "arrow.clockwise") {
                            Task { await reload() }
                        }
                    )
                    .padding(WisentDesign.Space.x4)
                }

                if store.rows.isEmpty && store.pipelineRuns.isEmpty {
                    placeholder
                        .padding(WisentDesign.Space.x6)
                    Spacer(minLength:
                        0
                    )
                } else {
                    if !store.rows.isEmpty {
                        table
                            .frame(height: tableHeight)
                        Divider()
                    }
                    if !store.pipelineRuns.isEmpty {
                        pipelineRuns
                            .frame(height: runsHeight)
                        Divider()
                    }
                    detail
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                }

                if !store.resumeDetails.isEmpty {
                    DisclosureGroup("Release resume result") {
                        ScrollView {
                            Text(store.resumeDetails)
                                .font(WisentTypeScale.identifierSmall())
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .frame(maxHeight: tableHeight)
                    }
                    .padding(.horizontal, WisentDesign.Space.x4)
                }

                WisentMutationBar(outcome: store.mutation) { store.clearMutation() }
                    .padding(.horizontal, WisentDesign.Space.x4)
                    .padding(.bottom, store.mutation == .idle ? 0 : WisentDesign.Space.x3)
            }
        }
        .task {
            guard store.rows.isEmpty, store.inventoryProblem == nil else { return }
            await reload()
        }
        .onChange(of: selection) { _, pair in
            guard let pair else { return }
            Task { await loadEvidence(for: pair) }
        }
        .sheet(item: $clearance) { pending in
            clearDialog(pending)
        }
        .onChange(of: fleetStore.requestGeneration) { _, _ in
            resumption = nil
            store.clearMutation()
        }
        .sheet(item: $resumption) { run in
            resumeDialog(run)
        }
    }
}
