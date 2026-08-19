import SwiftUI
import WisentDesignSystem

/// One clearance the operator has asked for and not yet authorized.
private struct PendingClearance: Identifiable {
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
/// The single write is `release quarantine clear`, and it asks for a typed
/// reason and shows the exact command first. Clearing a digest starts nothing:
/// the release agent picks it up on its next tick.
struct ReleasesView: View {
    @ObservedObject var store: ReleaseEvidenceStore
    let scope: String

    @State private var selection: ReleaseInventoryPair?
    @State private var stream: ReleaseLogStreamSelection = .err
    @State private var lines = 40
    @State private var clearance: PendingClearance?
    @State private var reason = ""

    private static let lineChoices = [40, 100, 250, 500, 1_000]

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
                }
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: 0) {
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

                if store.rows.isEmpty {
                    placeholder
                        .padding(WisentDesign.Space.x6)
                    Spacer(minLength: 0)
                } else {
                    table
                        .frame(height: tableHeight)
                    Divider()
                    detail
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
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
    }

    // MARK: Chrome

    private var freshness: String {
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
    private var tableHeight: CGFloat {
        let rows = min(max(store.rows.count, 3), 8)
        return WisentAppLayout.denseRowHeight + CGFloat(rows) * WisentAppLayout.tableRowHeight
    }

    @ViewBuilder
    private var placeholder: some View {
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

    // MARK: Table

    private var table: some View {
        ConsoleTable(head: [
            ConsoleHeaderCell("Verdict", width: 82),
            ConsoleHeaderCell("Product", width: 130),
            ConsoleHeaderCell("Target", width: 140),
            ConsoleHeaderCell("Desired", width: 100),
            ConsoleHeaderCell("Observed", width: 100),
            ConsoleHeaderCell("Software", width: 96),
            ConsoleHeaderCell("Phase", width: 92),
            ConsoleHeaderCell("Detail"),
            ConsoleHeaderCell("Quarantined", width: 88, trailing: true),
        ]) {
            ForEach(store.rows) { row in
                ConsoleTableRow(
                    isSelected: selection == row.pair,
                    select: { selection = row.pair }
                ) {
                    verdictCell(row)
                    ConsoleCell(text: row.product, width: 130, identifier: true, strong: true)
                    ConsoleCell(text: row.target, width: 140, identifier: true)
                    ConsoleCell(
                        text: row.report?.desiredVersion ?? "—",
                        width: 100,
                        identifier: true,
                        strong: true
                    )
                    ConsoleCell(
                        text: row.report?.observedVersion ?? (row.report == nil ? "—" : "none"),
                        width: 100,
                        identifier: true,
                        tone: observedTone(row)
                    )
                    softwareCell(row)
                    ConsoleCell(text: row.report?.phase ?? "—", width: 92)
                    ConsoleCell(text: rowDetail(row), tone: rowDetailTone(row))
                    ConsoleCell(
                        text: row.report.map { "\($0.quarantined.count)" } ?? "—",
                        width: 88,
                        trailing: true,
                        digits: true,
                        tone: (row.report?.quarantined.isEmpty == false) ? .warning : .neutral
                    )
                }
            }
        }
    }

    @ViewBuilder
    private func verdictCell(_ row: ReleaseRow) -> some View {
        HStack(spacing: 0) {
            switch row.diagnosis {
            case .pending:
                ConsoleCell(text: "reading…", width: 82, tone: .neutral)
            case let .diagnosed(report):
                WisentStatusChip(text: report.verdict.word, tone: tone(for: report.verdict))
            case .failed:
                WisentStatusChip(text: "no answer", tone: .danger)
            }
        }
        .frame(width: 82, alignment: .leading)
    }

    /// Whether this host can be shown to run what the fleet declares for it.
    ///
    /// The CLI's own word, never a translation of it, and never blank. An empty
    /// cell reads as "fine" to every operator alive, and this column exists
    /// because a host that said nothing used to read exactly that way.
    @ViewBuilder
    private func softwareCell(_ row: ReleaseRow) -> some View {
        HStack(spacing: 0) {
            if let software = row.software {
                WisentStatusChip(
                    text: software.hasReport ? software.verdict : "never",
                    tone: software.failed ? .danger : .success
                )
            } else {
                // The CLI answered without a software block, so this console has
                // no verdict to show — which is not the same as a passing one.
                WisentStatusChip(text: "unreported", tone: .warning)
            }
        }
        .frame(width: 96, alignment: .leading)
    }

    /// The row's own sentence. A blocked rollout shows its blockers verbatim,
    /// because the blocker is the reason and the phase detail beside it is
    /// usually the symptom. A software finding outranks the phase detail for the
    /// same reason: a phase is about the rollout, and the finding is about what
    /// the machine is running right now.
    private func rowDetail(_ row: ReleaseRow) -> String {
        if let problem = row.problem { return problem }
        if let report = row.report, !report.blockers.isEmpty {
            return report.blockers.joined(separator: ", ")
        }
        if let finding = row.software?.findings.first {
            return finding
        }
        guard let report = row.report else { return "waiting for the host" }
        return report.detail.isEmpty ? "—" : report.detail
    }

    private func rowDetailTone(_ row: ReleaseRow) -> WisentTone {
        if row.problem != nil { return .danger }
        if row.report?.blockers.isEmpty == false { return .danger }
        if row.software?.failed == true { return .danger }
        return .neutral
    }

    private func observedTone(_ row: ReleaseRow) -> WisentTone {
        guard let report = row.report else { return .neutral }
        if report.observedVersion == nil { return .warning }
        return report.isConverged ? .neutral : .warning
    }

    private func tone(for verdict: ReleaseVerdict) -> WisentTone {
        switch verdict {
        case .settled: .success
        case .rolling: .info
        case .blocked: .danger
        case .unrecognised: .warning
        }
    }

    // MARK: Detail

    @ViewBuilder
    private var detail: some View {
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
                    command: StadoCLI.commandLine(
                        ReleaseEvidenceStore.doctorArguments(pair: row.pair)
                    ),
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
                    detail: "`stado release status --json` answered without the software block, so nothing here states what \(row.target) is running. That is an unanswered question, not a passing one.",
                    command: StadoCLI.commandLine(["host", "software", row.target])
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

    // MARK: Candidate and logs

    @ViewBuilder
    private func candidate(_ row: ReleaseRow) -> some View {
        WisentSectionBox(
            title: "Candidate",
            detail: candidateDetail(row),
            trailing: store.logsPair == row.pair
                ? store.logs.map { "\($0.product) \($0.version) on \($0.target)" }
                : nil
        ) {
            if let report = row.report {
                HStack(alignment: .top, spacing: WisentDesign.Space.x6) {
                    WisentField(
                        label: "Port",
                        value: report.candidate.port.map(String.init) ?? "None recorded"
                    )
                    WisentField(
                        label: "Health",
                        value: report.candidate.healthStatus,
                        tone: healthTone(report.candidate.healthStatus)
                    )
                    WisentField(
                        label: "Recorded pid",
                        value: pidText(report.candidate.pidAlive),
                        tone: report.candidate.pidAlive == false ? .danger : .neutral
                    )
                }
            }
            logs(row)
        }
    }

    private func candidateDetail(_ row: ReleaseRow) -> String {
        guard let report = row.report else {
            return "The candidate is read by the same diagnosis as the rollout above."
        }
        if !report.candidate.exists {
            return "The agent has staged no candidate on this host; the logs below are the last ones it wrote for this version."
        }
        if report.candidate.pidAlive == false {
            return "The recorded process is gone. Whatever it printed before it died is in the streams below."
        }
        return "The staged process the release agent is observing before it routes traffic to it."
    }

    private func pidText(_ alive: Bool?) -> String {
        switch alive {
        case .some(true): "Alive on the host"
        case .some(false): "Gone — the recorded pid is no longer running"
        case .none: "Nothing to probe"
        }
    }

    private func healthTone(_ status: String) -> WisentTone {
        switch status {
        case "ok": .success
        case "no_candidate", "unprobed": .neutral
        default: .danger
        }
    }

    @ViewBuilder
    private func logs(_ row: ReleaseRow) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            HStack(spacing: WisentDesign.Space.x3) {
                Picker("Stream", selection: $stream) {
                    ForEach(ReleaseLogStreamSelection.allCases) { choice in
                        Text(choice.title).tag(choice)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(width: 220)

                Picker("Lines", selection: $lines) {
                    ForEach(Self.lineChoices, id: \.self) { count in
                        Text("\(count) lines").tag(count)
                    }
                }
                .pickerStyle(.menu)
                .labelsHidden()
                .frame(width: 120)

                if store.isLoadingLogs {
                    ProgressView()
                        .controlSize(.small)
                }
                Spacer(minLength: 0)
            }
            .onChange(of: stream) { _, _ in reloadLogs(row.pair) }
            .onChange(of: lines) { _, _ in reloadLogs(row.pair) }

            Text(StadoCLI.commandLine(
                ReleaseEvidenceStore.logsArguments(pair: row.pair, stream: stream, lines: lines)
            ))
            .font(WisentTypeScale.identifierSmall())
            .foregroundStyle(WisentDesign.muted)
            .textSelection(.enabled)

            if let problem = store.logsProblem {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The candidate's logs could not be read",
                    detail: problem,
                    actions: [
                        WisentAction("Read again", symbol: "arrow.clockwise") {
                            reloadLogs(row.pair)
                        }
                    ]
                )
            } else if let report = store.logs, store.logsPair == row.pair {
                if report.streams.isEmpty {
                    Text("The command returned no stream for version \(report.version).")
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                } else {
                    ForEach(report.streams) { logStream in
                        streamPane(logStream)
                    }
                }
            } else if store.isLoadingLogs {
                Text("Reading the tail off \(row.target)…")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// A stream with nothing in it says which file it looked at and why it is
    /// empty. A blank pane here is how a candidate's own stderr stayed unread
    /// on a host while an operator watched a screen that showed nothing.
    private func streamPane(_ logStream: ReleaseLogStream) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            HStack(alignment: .firstTextBaseline, spacing: WisentDesign.Space.x2) {
                Text(logStream.stream.uppercased())
                    .font(WisentTypeScale.eyebrow())
                    .tracking(0.6)
                    .foregroundStyle(logStream.stream == "err" ? WisentTone.danger.color : WisentDesign.muted)
                Text(logStream.path)
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.secondary)
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: WisentDesign.Space.x2)
                Text(streamMeasure(logStream))
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
                    .monospacedDigit()
            }
            if logStream.isMissing {
                Text("No such file on the host. The release agent never opened this path for this version, so the product wrote nothing here.")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            } else if logStream.lines.isEmpty {
                Text("Present and empty. The agent opened this file and the product wrote nothing to it.")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                ScrollView([.vertical, .horizontal]) {
                    VStack(alignment: .leading, spacing: 1) {
                        ForEach(Array(logStream.lines.enumerated()), id: \.offset) { _, line in
                            Text(line)
                                .font(WisentTypography.mono(10))
                                .foregroundStyle(WisentDesign.ink)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: true, vertical: false)
                        }
                    }
                    .padding(WisentDesign.Space.x3)
                    .frame(maxWidth: .infinity, alignment: .topLeading)
                }
                .frame(maxWidth: .infinity)
                .frame(height: 220)
                .background(WisentDesign.canvasMuted)
                .clipShape(RoundedRectangle(cornerRadius: WisentDesign.Radius.small))
            }
        }
        .padding(WisentDesign.Space.x3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay {
            RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
                .stroke(WisentDesign.border, lineWidth: WisentDesign.hairline)
        }
    }

    private func streamMeasure(_ logStream: ReleaseLogStream) -> String {
        let bytes = logStream.bytes.map { "\($0.formatted(.number)) bytes" } ?? "no file"
        let count = logStream.lines.isEmpty ? "0 lines" : "last \(logStream.lines.count) lines"
        return "\(bytes) · \(count)"
    }

    // MARK: Quarantine

    @ViewBuilder
    private func quarantine(_ row: ReleaseRow) -> some View {
        WisentSectionBox(
            title: "Quarantine",
            detail: "Digests this host refuses to roll out again. The agent skips a quarantined digest on every pass, so the one the registry desires is a rollout that never finishes on its own.",
            trailing: store.quarantinePair == row.pair
                ? store.quarantine.map { "\($0.entries.count) held" }
                : nil
        ) {
            Text(StadoCLI.commandLine(ReleaseEvidenceStore.quarantineArguments(pair: row.pair)))
                .font(WisentTypeScale.identifierSmall())
                .foregroundStyle(WisentDesign.muted)
                .textSelection(.enabled)

            if let problem = store.quarantineProblem {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The quarantine map could not be read",
                    detail: problem,
                    actions: [
                        WisentAction("Read again", symbol: "arrow.clockwise") {
                            Task { await store.loadQuarantine(for: row.pair) }
                        }
                    ]
                )
                // The diagnosis already carried the host's quarantine map, so
                // a failed second read degrades to a list without the desired
                // flag rather than to nothing at all.
                if let held = row.report?.quarantined, !held.isEmpty {
                    Text("The diagnosis above read \(held.count) quarantined \(held.count == 1 ? "digest" : "digests") on this host. Clearing is unavailable until the map itself can be read.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    ForEach(held.desiredFirst) { entry in
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                            Text(entry.digest)
                                .font(WisentTypeScale.identifier())
                                .foregroundStyle(WisentDesign.ink)
                                .textSelection(.enabled)
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Text("\(entry.quarantinedAt ?? "no timestamp recorded") — \(entry.reason.isEmpty ? "the host recorded no reason" : entry.reason)")
                                .font(WisentTypeScale.caption())
                                .foregroundStyle(WisentDesign.secondary)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        .padding(WisentDesign.Space.x3)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(WisentDesign.canvasMuted, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small))
                    }
                }
            } else if store.isLoadingQuarantine, store.quarantine == nil {
                Text("Reading the host's quarantine map…")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            } else if let report = store.quarantine, store.quarantinePair == row.pair {
                if report.entries.isEmpty {
                    Text("Nothing is quarantined for \(report.product) on \(report.target). No digest is being skipped here.")
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                } else {
                    VStack(spacing: WisentDesign.Space.x2) {
                        ForEach(report.entries.desiredFirst) { entry in
                            quarantineRow(entry, pair: row.pair)
                        }
                    }
                }
            }

            if let record = store.clearance, store.quarantinePair == row.pair {
                clearanceRecord(record)
            }
        }
    }

    private func quarantineRow(_ entry: ReleaseQuarantineEntry, pair: ReleaseInventoryPair) -> some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x4) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                HStack(spacing: WisentDesign.Space.x2) {
                    Text(entry.shortDigest)
                        .font(WisentTypeScale.identifier())
                        .foregroundStyle(WisentDesign.ink)
                        .textSelection(.enabled)
                    if entry.isDesiredDigest {
                        WisentStatusChip(text: "Desired — blocks the rollout", tone: .danger)
                    }
                    Text(quarantinedAge(entry))
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.muted)
                }
                Text(entry.digest)
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(entry.reason.isEmpty ? "The host recorded no reason." : entry.reason)
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: WisentDesign.Space.x2)
            WisentActionButton(
                action: WisentAction(
                    "Clear…",
                    symbol: "arrow.uturn.forward",
                    isEnabled: !store.mutation.isWorking
                ) {
                    reason = ""
                    clearance = PendingClearance(pair: pair, entry: entry)
                }
            )
        }
        .padding(WisentDesign.Space.x3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            entry.isDesiredDigest ? WisentTone.danger.softColor : WisentDesign.canvasMuted,
            in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
        )
    }

    private func quarantinedAge(_ entry: ReleaseQuarantineEntry) -> String {
        guard let at = entry.quarantinedAt, !at.isEmpty else { return "no timestamp recorded" }
        guard let age = entry.quarantinedAge else { return at }
        return "\(at) · \(ConsoleFormat.age(age))"
    }

    private func clearanceRecord(_ record: ReleaseQuarantineClearance) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text("LAST CLEARANCE")
                .font(WisentTypeScale.eyebrow())
                .tracking(0.6)
                .foregroundStyle(WisentDesign.muted)
            Text("\(record.digest) on \(record.target), recorded at \(record.auditedAt ?? "an unreported time") because: \(record.reason)")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            Text("Previous state backed up on the host at \(record.stateBackup ?? "a path the command did not report").")
                .font(WisentTypeScale.identifierSmall())
                .foregroundStyle(WisentDesign.muted)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(WisentDesign.Space.x3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(WisentTone.success.softColor, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small))
    }

    // MARK: The one write

    /// The reason is not optional and not defaulted. The command records it
    /// beside the state file, and a cleared digest with no recorded reason is
    /// an audit trail that answers nothing.
    private func clearDialog(_ pending: PendingClearance) -> some View {
        let typed = reason.trimmingCharacters(in: .whitespacesAndNewlines)
        let command = StadoCLI.commandLine(
            ReleaseEvidenceStore.clearArguments(
                pair: pending.pair,
                digest: pending.entry.digest,
                reason: typed.isEmpty ? "<reason>" : typed
            )
        )
        return VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            HStack(alignment: .top, spacing: WisentDesign.Space.x3) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(WisentTone.warning.color)
                    .frame(width: 34, height: 34)
                    .background(WisentTone.warning.softColor, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small))
                    .accessibilityHidden(true)
                Text("Clear \(pending.entry.shortDigest) for \(pending.pair.product) on \(pending.pair.target)?")
                    .font(WisentTypography.heading(17))
                    .foregroundStyle(WisentDesign.ink)
                    .fixedSize(horizontal: false, vertical: true)
            }

            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text(pending.entry.isDesiredDigest
                    ? "This is the digest the registry currently desires, so every pass of the release agent skips it and the rollout never finishes. Clearing it is what lets the next pass try again."
                    : "The registry does not currently desire this digest. Clearing it retires the refusal; nothing rolls out until this digest is desired again.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Text("This console starts, stops and restarts nothing. The command rewrites the host's rollout state after backing it up, and the release agent picks the digest up on its next tick.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Text("quarantined at \(pending.entry.quarantinedAt ?? "an unreported time") because: \(pending.entry.reason.isEmpty ? "the host recorded no reason" : pending.entry.reason)")
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(WisentTone.warning.color)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }

            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text("REASON — REQUIRED, RECORDED IN THE AUDIT TRAIL")
                    .font(WisentTypeScale.eyebrow())
                    .tracking(0.6)
                    .foregroundStyle(WisentDesign.muted)
                TextField("rebuilt 0.9.14 after the signing key rotation", text: $reason, axis: .vertical)
                    .textFieldStyle(.roundedBorder)
                    .font(WisentTypeScale.body())
                    .lineLimit(2...4)
                    .disabled(store.mutation.isWorking)
                if typed.isEmpty {
                    Text("Without a reason this command does not run. It is what an audit reads months from now, when nobody remembers why the digest was given another chance.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text(command)
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.ink)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(WisentDesign.Space.x3)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(WisentDesign.canvasMuted, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small))

            HStack(spacing: WisentDesign.Space.x2) {
                Image(systemName: "doc.badge.clock")
                    .font(.system(size: 11))
                    .foregroundStyle(WisentDesign.muted)
                    .accessibilityHidden(true)
                Text("The host writes a backup of its rollout state document before the rewrite, and the clearance is appended to the audit trail beside it.")
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }

            HStack(spacing: WisentDesign.Space.x2) {
                Spacer(minLength: 0)
                WisentActionButton(
                    action: WisentAction("Leave it quarantined", kind: .primary) {
                        clearance = nil
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Clear digest",
                        kind: .destructive,
                        isEnabled: !typed.isEmpty && !store.mutation.isWorking
                    ) {
                        let pair = pending.pair
                        let digest = pending.entry.digest
                        clearance = nil
                        Task {
                            await store.clearQuarantine(pair: pair, digest: digest, reason: typed)
                        }
                    }
                )
            }
            .padding(.top, WisentDesign.Space.x2)
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: WisentAppLayout.dialogWidth, alignment: .leading)
        .background(WisentDesign.surface)
    }

    // MARK: Loading

    private func reload() async {
        await store.refresh()
        let pair = selection.flatMap { current in
            store.rows.first { $0.pair == current }?.pair
        } ?? store.rows.first?.pair
        guard let pair else { return }
        if selection == pair {
            await loadEvidence(for: pair)
        } else {
            // Selecting drives the evidence read through onChange, so the
            // logs and the quarantine map are never read twice for one row.
            selection = pair
        }
    }

    private func loadEvidence(for pair: ReleaseInventoryPair) async {
        await store.loadLogs(for: pair, stream: stream, lines: lines)
        await store.loadQuarantine(for: pair)
    }

    private func reloadLogs(_ pair: ReleaseInventoryPair) {
        Task { await store.loadLogs(for: pair, stream: stream, lines: lines) }
    }
}
