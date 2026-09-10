import SwiftUI
import WisentDesignSystem

/// The pane below the table: the rollout itself and the host's installed
/// software, above the candidate and the quarantine map.
///
/// `detail` is internal rather than private only because `body` sits in
/// `ReleasesView.swift`, and it reaches `candidate` in
/// `Detail/ReleasesCandidate.swift` and `quarantine` in
/// `Detail/ReleasesQuarantine.swift`: Swift scopes `private` to one file.
/// `rollout`, `software` and their two sentences are read only from here and
/// stay private.
extension ReleasesView {
    // MARK: Detail

    @ViewBuilder
    var detail: some View {
        if let row = store.row(for: selection) {
            ScrollView {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x6) {
                    rollout(row)
                    software(row)
                    candidate(row)
                    quarantine(row)
                }
                .padding(WisentDesign.Space.x5)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(WisentDesign.surface)
        } else {
            VStack(spacing: WisentDesign.Space.x3) {
                WisentEmptyPanel(
                    title: "No rollout selected",
                    detail: "Select a product target to read the candidate the host staged, the candidate's own stderr, and the digests the host refuses to roll out again.",
                    symbol: "shippingbox"
                )
            }
            .padding(WisentDesign.Space.x6)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .background(WisentDesign.surface)
        }
    }

    /// Desired against observed, the phase, the agent's sentence, the blockers
    /// and the host gates the verdict was computed from.
    @ViewBuilder
    private func rollout(_ row: ReleaseRow) -> some View {
        WisentSectionBox(
            title: "\(row.product) on \(row.target)",
            detail: rolloutDetail(row),
            trailing: row.report.map(\.verdict.word) ?? (row.problem == nil ? "reading" : "no answer")
        ) {
            if let problem = row.problem {
                WisentAlertPanel(
                    tone: .danger,
                    title: "This rollout could not be diagnosed",
                    detail: problem,
                    actions: [
                        WisentAction("Diagnose again", symbol: "arrow.clockwise") {
                            Task { await store.diagnose(row.pair) }
                        }
                    ]
                )
            } else if let report = row.report {
                HStack(alignment: .top, spacing: WisentDesign.Space.x6) {
                    WisentField(
                        label: "Desired version",
                        value: report.desiredVersion ?? "The registry declares none"
                    )
                    WisentField(
                        label: "Observed version",
                        value: report.observedVersion ?? "The host has recorded none",
                        tone: report.isConverged ? .success : .warning
                    )
                    WisentField(label: "Phase", value: report.phase)
                }
                WisentField(
                    label: "Detail",
                    value: report.detail.isEmpty ? "The agent recorded no detail for this phase." : report.detail
                )
                if report.blockers.isEmpty {
                    WisentField(label: "Blockers", value: "None. Nothing is holding this rollout.")
                } else {
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                        Text("BLOCKERS")
                            .font(WisentTypeScale.eyebrow())
                            .tracking(0.6)
                            .foregroundStyle(WisentDesign.muted)
                        ForEach(report.blockers, id: \.self) { blocker in
                            Text(blocker)
                                .font(WisentTypeScale.identifier())
                                .foregroundStyle(WisentTone.danger.color)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                HStack(alignment: .top, spacing: WisentDesign.Space.x6) {
                    WisentField(
                        label: "Disk pressure",
                        value: report.gates.diskPressureUnresolved
                            ? "Unresolved — the host is not claiming work"
                            : "Resolved",
                        tone: report.gates.diskPressureUnresolved ? .danger : .neutral
                    )
                    WisentField(label: "Free space", value: ConsoleFormat.gigabytes(report.gates.freeGB))
                    WisentField(
                        label: "Low watermark",
                        value: ConsoleFormat.gigabytes(report.gates.lowWatermarkGB)
                    )
                    WisentField(
                        label: "Memory",
                        value: memorySummary(report.gates),
                        tone: report.gates.memoryPressureActive ? .danger : .neutral
                    )
                }
                Text(StadoCLI.commandLine(ReleaseEvidenceStore.doctorArguments(pair: row.pair)))
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
                    .textSelection(.enabled)
            } else {
                WisentLoadingPanel(
                    title: "Reading \(row.target)",
                    detail: "The diagnosis reads the host's rollout state file, probes the staged candidate, and asks the host for its claiming gates."
                )
            }
        }
    }

    /// The memory half of the claiming gates, in the CLI's own words: the
    /// reading, the watermark it was measured against, and whether this host
    /// is withholding itself from the rollout's build.
    private func memorySummary(_ gates: ReleaseGates) -> String {
        var clauses: [String] = []
        if let available = gates.memoryAvailableGB {
            clauses.append(
                gates.memoryLowWatermarkGB.map { "\(StadoFormat.decimal(available)) GB against a \(StadoFormat.decimal($0)) GB watermark" }
                    ?? "\(StadoFormat.decimal(available)) GB available"
            )
        }
        if let swap = gates.memorySwapUsedPct {
            clauses.append("swap \(swap)%")
        }
        if gates.memoryPressureActive {
            clauses.append("refusing placement")
        }
        return clauses.isEmpty ? "Not observed" : clauses.joined(separator: " · ")
    }

    /// What the host itself says it runs, and every disagreement the CLI found.
    ///
    /// The sentences are the CLI's, printed unaltered and in its order. This pane
    /// re-words nothing and re-derives nothing: `stado release status` already
    /// decided which of these is a failure, and a console that re-decided it
    /// would be a second source of truth about the one question the fleet spent
    /// a day being wrong about.
    @ViewBuilder
    private func software(_ row: ReleaseRow) -> some View {
        WisentSectionBox(
            title: "Installed software on \(row.target)",
            detail: softwareDetail(row),
            trailing: row.software.map { $0.hasReport ? $0.verdict : "never" } ?? "unreported"
        ) {
            if let software = row.software {
                HStack(alignment: .top, spacing: WisentDesign.Space.x6) {
                    WisentField(
                        label: "Report",
                        value: software.hasReport ? software.observed : "Never taken",
                        tone: software.hasReport ? .neutral : .danger
                    )
                    WisentField(label: "Programs", value: "\(software.reported)")
                    WisentField(
                        label: "From a release",
                        value: "\(software.release)",
                        tone: software.release == 0 && software.reported > 0 ? .warning : .neutral
                    )
                    WisentField(
                        label: "Unmanaged",
                        value: "\(software.unmanaged)",
                        tone: software.unmanaged > 0 ? .warning : .neutral
                    )
                }
                if software.findings.isEmpty {
                    WisentField(
                        label: "Findings",
                        value: "None. Every program the fleet declares for this host is accounted for."
                    )
                } else {
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                        Text("FINDINGS")
                            .font(WisentTypeScale.eyebrow())
                            .tracking(0.6)
                            .foregroundStyle(WisentDesign.muted)
                        ForEach(software.findings, id: \.self) { finding in
                            Text(finding)
                                .font(WisentTypeScale.identifier())
                                .foregroundStyle(WisentTone.danger.color)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                Text(StadoCLI.commandLine(["host", "software", row.target]))
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
                    .textSelection(.enabled)
            } else {
                WisentAlertPanel(
                    tone: .warning,
                    title: "This console read no software report",
                    detail: "`stado release status --json` answered without the software block, so nothing here states what \(row.target) is running. That is an unanswered question, not a passing one."
                )
            }
        }
    }

    private func softwareDetail(_ row: ReleaseRow) -> String {
        guard let software = row.software else {
            return "The status command said nothing about installed software."
        }
        if !software.hasReport {
            return "This host has never reported what it runs, so every version claimed for it is a declaration nothing confirms."
        }
        if software.state != "observed" {
            return "The last attempt to read this host's software did not complete, so what is below is history."
        }
        if software.failed {
            return "The host answered, and what it runs does not match what the fleet declares for it."
        }
        return "Every program the fleet declares for this host came out of a release and is at the declared version."
    }

    private func rolloutDetail(_ row: ReleaseRow) -> String {
        guard let report = row.report else {
            return row.problem == nil
                ? "Waiting for the host to answer."
                : "The diagnosis command failed; nothing below is assumed."
        }
        switch report.verdict {
        case .settled:
            return "The host runs the version the registry desires and nothing is in flight."
        case .rolling:
            return "A rollout is in flight, or the host has not yet reached the desired version."
        case .blocked:
            return "The fleet will not finish this rollout until the blockers below are gone."
        case let .unrecognised(word):
            return "The command answered a verdict this console does not classify: \(word)."
        }
    }
}
