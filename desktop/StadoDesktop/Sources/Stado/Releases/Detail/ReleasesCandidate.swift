import SwiftUI
import WisentDesignSystem

/// The staged candidate the release agent is observing, and the tail of its
/// own streams read off the host.
///
/// `candidate` is internal rather than private only because `detail` sits in
/// `Detail/ReleasesDetail.swift`: Swift scopes `private` to one file. The
/// stream panes, the tones and the two sentences are read only from here and
/// stay private.
extension ReleasesView {
    // MARK: Candidate and logs

    @ViewBuilder
    func candidate(_ row: ReleaseRow) -> some View {
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
                .frame(width:
                    220)

                Picker("Lines", selection: $lines) {
                    ForEach(Self.lineChoices, id: \.self) { count in
                        Text("\(count) lines").tag(count)
                    }
                }
                .pickerStyle(.menu)
                .labelsHidden()
                .frame(width:
                    120)

                if store.isLoadingLogs {
                    // The pickers keep their place and the bar sits where a
                    // stream label lands, so the header says the read is in
                    // flight without a spinning circle.
                    WisentSkeleton(.pill, width:
                        120, height:
                        14)
                }
                Spacer(minLength:
                    0
                )
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
                // A tail is a block of monospaced rows, so the wait is that
                // block: bars in the pane the lines are about to fill.
                WisentSkeletonGroup(
                    label: "Reading the tail off \(row.target)",
                    spacing: WisentDesign.Space.x2
                ) {
                    ForEach(0 ..< 8, id: \.self) { _ in
                        WisentSkeleton(.line)
                    }
                }
                .padding(WisentDesign.Space.x3)
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
                    VStack(alignment: .leading, spacing:
                        1
                    ) {
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
                .frame(height:
                    220)
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
}
