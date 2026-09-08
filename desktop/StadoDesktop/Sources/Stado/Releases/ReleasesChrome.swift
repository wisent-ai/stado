import SwiftUI
import WisentDesignSystem

/// The screen's frame: the freshness line, the two pane heights, the pipeline
/// run list, and the panel shown when nothing was read.
///
/// `freshness`, `tableHeight`, `runsHeight`, `pipelineRuns` and `placeholder`
/// are internal rather than private only because `body` sits in
/// `ReleasesView.swift`: Swift scopes `private` to one file. `runTone`,
/// `platformSummary` and `selectedRunFailure` are read only from here and stay
/// private.
extension ReleasesView {
    // MARK: Chrome

    var freshness: String {
        if store.isRefreshing, store.rows.isEmpty {
            return "Diagnosing"
        }
        guard store.lastUpdated != nil else {
            return store.inventoryProblem == nil ? "Not read yet" : "Not read"
        }
        let blocked = store.rows.count { $0.report?.verdict == .blocked }
        let unreadable = store.rows.count { $0.problem != nil }
        // Counted in the header because this is the state that used to be
        // invisible: a target nobody can show is running the declared build.
        let unaccounted = store.rows.count { $0.software?.failed == true }
        var parts = ["\(store.rows.count) rollouts"]
        if blocked > 0 { parts.append("\(blocked) blocked") }
        if unaccounted > 0 { parts.append("\(unaccounted) unaccounted") }
        if unreadable > 0 { parts.append("\(unreadable) undiagnosed") }
        parts.append("read \(ConsoleFormat.relative(store.lastUpdated))")
        return parts.joined(separator: " · ")
    }

    /// Eight rows and a head. The pane below carries the evidence, and it is
    /// the part an operator reads for minutes rather than seconds.
    var tableHeight: CGFloat {
        let rows = min(max(store.rows.count, 3), 8)
        return WisentAppLayout.denseRowHeight + CGFloat(rows) * WisentAppLayout.tableRowHeight
    }

    /// Room for the header and up to five runs; the failure text of the
    /// selected run lives in this pane and scrolls rather than growing it.
    var runsHeight: CGFloat {
        let rows = min(max(store.pipelineRuns.count, 1), 5)
        return WisentAppLayout.denseRowHeight * 2
            + CGFloat(rows) * WisentAppLayout.tableRowHeight
            + (selectedRunFailure == nil ? 0 : 96)
    }

    private var selectedRunFailure: (run: ReleasePipelineRunRecord, text: String)? {
        let run = store.pipelineRuns.first { $0.id == selectedRunID }
            ?? store.pipelineRuns.first { $0.failure != nil }
        guard let run else { return nil }
        let text = run.failure
            ?? run.platforms.values.compactMap(\.failure).first
        guard let text else { return nil }
        return (run, text)
    }

    /// The pipeline itself, newest run first: identity, run state, and each
    /// platform's leg with the queue's live word on its build job. A failed
    /// run's recorded failure — the failing platform, pinned host, and the
    /// job's own last output lines — is shown verbatim below the list.
    var pipelineRuns: some View {
        VStack(spacing:
            0
        ) {
            ConsoleTableHead(cells: [
                ConsoleHeaderCell("Run", width:
                    78),
                ConsoleHeaderCell("Product", width:
                    130),
                ConsoleHeaderCell("Version", width:
                    70),
                ConsoleHeaderCell("Channel", width:
                    82),
                ConsoleHeaderCell("State", width:
                    92),
                ConsoleHeaderCell("Platforms"),
                ConsoleHeaderCell("Updated", width:
                    110, trailing: true),
            ])
            ScrollView {
                LazyVStack(spacing:
                    0
                ) {
                    ForEach(store.pipelineRuns) { run in
                        ConsoleTableRow(
                            isSelected: selectedRunFailure?.run.id == run.id,
                            select: { selectedRunID = run.id }
                        ) {
                            ConsoleCell(text: String(run.runID.prefix(8)), width:
                                78, identifier: true)
                            ConsoleCell(text: run.product, width:
                                130, strong: true)
                            ConsoleCell(text: run.version, width:
                                70, identifier: true)
                            ConsoleCell(text: run.channel, width:
                                82)
                            ConsoleCell(text: run.state, width:
                                92, tone: runTone(run))
                            ConsoleCell(text: platformSummary(run))
                            ConsoleCell(
                                text: ConsoleFormat.relative(run.updated),
                                width:
                                    110,
                                trailing: true
                            )
                        }
                    }
                }
            }
            if let failure = selectedRunFailure {
                Divider()
                ScrollView {
                    Text(failure.text)
                        .font(.system(size:
                            11, design: .monospaced))
                        .foregroundStyle(WisentDesign.ink)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .topLeading)
                        .padding(WisentDesign.Space.x3)
                }
                .frame(height:
                    96)
                .background(WisentTone.danger.color.opacity(0.05))
            }
        }
        .background(WisentDesign.surface)
    }

    private func runTone(_ run: ReleasePipelineRunRecord) -> WisentTone {
        switch run.state {
        case "failed": .danger
        case "reconciled", "completed", "promoted": .success
        default: .warning
        }
    }

    /// "darwin-arm64 published · linux-amd64 submitted [running]" — the leg
    /// states the run object records, plus the queue's live word for a run
    /// still in flight. This is the line that answers "is anything actually
    /// happening" without a terminal.
    private func platformSummary(_ run: ReleasePipelineRunRecord) -> String {
        run.platforms
            .sorted { $0.key < $1.key }
            .map { platform, leg in
                var part = "\(platform) \(leg.state)"
                if let live = leg.jobState {
                    part += " [\(live)]"
                }
                if let compiled = leg.compiledCrates {
                    // An estimate, labelled as one: against the previous run.
                    if let percent = leg.compilePercent {
                        part += " · \(compiled) crates, ~\(percent)%"
                    } else {
                        part += " · \(compiled) crates"
                    }
                }
                return part
            }
            .joined(separator: " · ")
    }

    @ViewBuilder
    var placeholder: some View {
        if store.isRefreshing {
            WisentLoadingPanel(
                title: "Diagnosing every declared rollout",
                detail: "`stado release status` lists the product targets, then one `stado release doctor` per target reads the host itself: its state file, the staged candidate, and the gates that decide whether it claims work."
            )
        } else if store.inventoryProblem == nil {
            WisentEmptyPanel(
                title: "No rollout is declared",
                detail: "`stado release status --json` returned no product target. The canonical registry declares no release control policy for this fleet, so there is nothing to diagnose.",
                symbol: "shippingbox",
                action: WisentAction("Read again", symbol: "arrow.clockwise", kind: .primary) {
                    Task { await reload() }
                }
            )
        } else {
            WisentEmptyPanel(
                title: "Nothing was read",
                detail: "The inventory command failed, so this console does not know which rollouts exist. Nothing about release state is assumed while that is true.",
                symbol: "shippingbox"
            )
        }
    }
}
