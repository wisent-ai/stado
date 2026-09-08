import SwiftUI
import WisentDesignSystem

/// Selectable, bounded presentation for the host command's unedited stream.
/// Horizontal scrolling preserves timestamps and message layout instead of
/// wrapping a diagnostic line into something the host never printed.
private struct HostRetainedLogTranscript: View {
    let title: String
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text(title.uppercased())
                .font(WisentTypeScale.eyebrow())
                .foregroundStyle(WisentDesign.muted)
            ScrollView([.horizontal, .vertical]) {
                Text(verbatim: text)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(WisentDesign.secondary)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: true, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(WisentDesign.Space.x3)
            }
            .frame(minHeight: 120, maxHeight: 280)
            .background(
                WisentDesign.canvasMuted,
                in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
            )
        }
    }
}

extension HostsView {
    @ViewBuilder
    func tailscaleLogSection(for host: WorkerNode) -> some View {
        let target = host.targetName ?? host.displayName
        let reading = fleetStore.isReadingTailscaleLogs(from: target)
        let attempt = fleetStore.tailscaleLogAttempt(for: target)
        WisentSectionBox(
            title: "Retained Tailscale logs",
            detail: "Reads the previous hour from the host's durable native log through Stado. It does not watch live traffic, make a test connection, or claim that a public endpoint is healthy.",
            trailing: reading
                ? "Reading…"
                : attempt.map { "Read \(ConsoleFormat.relative($0.completedAt))" } ?? "Not read"
        ) {
            Text("This console's registry and capacity data do not declare the host operating system. Select the source the host actually uses; Desktop will not infer it from the host name or from this Mac.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Picker("Host platform", selection: $tailscaleLogSource) {
                Text("Choose the host platform").tag(nil as HostTailscaleLogSource?)
                ForEach(HostTailscaleLogSource.allCases) { source in
                    Text(source.title).tag(Optional(source))
                }
            }
            .pickerStyle(.menu)
            .disabled(reading)

            if let source = tailscaleLogSource {
                WisentField(
                    label: "Read-only command",
                    value: StadoCLI.commandLine(
                        FleetControlStore.tailscaleLogArguments(host: target, source: source)
                    )
                )
            }
            if !host.declared {
                WisentAlertPanel(
                    tone: .warning,
                    title: "This host is not a registry target",
                    detail: "Stado host exec resolves declared registry targets. No retained-log read is available for this undeclared capacity publisher."
                )
            }
            WisentActionButton(
                action: WisentAction(
                    reading ? "Reading retained Tailscale logs…" : "Read retained Tailscale logs",
                    symbol: "doc.text.magnifyingglass",
                    kind: .secondary,
                    isEnabled: host.declared
                        && fleetStore.isConfigured
                        && tailscaleLogSource != nil
                        && !reading
                ) {
                    guard let source = tailscaleLogSource else { return }
                    Task {
                        await fleetStore.readTailscaleLogs(host: target, source: source)
                    }
                }
            )

            if let attempt {
                retainedLogAttempt(attempt)
            }
        }
    }

    @ViewBuilder
    private func retainedLogAttempt(_ attempt: HostTailscaleLogAttempt) -> some View {
        WisentField(label: "Requested host", value: attempt.requestedHost)
        WisentField(label: "Selected source", value: attempt.source.title)
        WisentField(
            label: "Completed",
            value: attempt.completedAt.formatted(date: .abbreviated, time: .standard)
        )
        if let failure = attempt.failure {
            WisentAlertPanel(
                tone: .warning,
                title: "Retained Tailscale logs were not read",
                detail: "Stado did not return a completed retained-log read. Its exact refusal or error is shown below."
            )
            HostRetainedLogTranscript(title: "Refusal / error", text: failure)
        }
        if let receipt = attempt.receipt {
            WisentField(label: "Reported host", value: receipt.target)
            WisentField(
                label: "Route",
                value: receipt.usedConnection?.summary
                    ?? "This Stado did not report which route carried the read."
            )
            WisentField(label: "Reported command", value: receipt.command)
            WisentField(
                label: "Process result",
                value: "\(receipt.status) · exit \(receipt.exitCode)"
            )
            if let error = receipt.error, !error.isEmpty {
                HostRetainedLogTranscript(title: "Host error", text: error)
            }
            if receipt.standardOutput.isEmpty {
                WisentField(
                    label: "stdout",
                    value: "The command returned no retained log text."
                )
            } else {
                HostRetainedLogTranscript(
                    title: "Retained log stdout",
                    text: receipt.standardOutput
                )
            }
            if !receipt.standardError.isEmpty {
                HostRetainedLogTranscript(
                    title: "Retained log stderr",
                    text: receipt.standardError
                )
            }
        }
        if let result = attempt.result {
            if case nil = attempt.receipt, !result.standardOutput.isEmpty {
                HostRetainedLogTranscript(
                    title: "Stado stdout (unreadable receipt)",
                    text: result.standardOutput
                )
            }
            if !result.standardError.isEmpty {
                HostRetainedLogTranscript(
                    title: "Stado stderr / refusal",
                    text: result.standardError
                )
            }
            if result.standardOutputTruncated || result.standardErrorTruncated {
                WisentAlertPanel(
                    tone: .warning,
                    title: "Stado output was truncated",
                    detail: "The dashboard marked at least one returned stream as truncated. The text above is not the complete command result."
                )
            }
        }
    }
}
