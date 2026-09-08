import SwiftUI
import WisentDesignSystem

/// The restart, the two cases where it cannot be offered, and the failure
/// evidence the inspector prints above them.
///
/// `fleetFailureDetail` and `restartAffordance` are internal rather than
/// private because `fleetServiceInspector` sits in `ServicesInspector.swift`,
/// and `restartDialog` because `body` sits in `ServicesView.swift`: Swift
/// scopes `private` to one file.
extension ServicesView {
    /// The failure evidence, composed as the CLI's `failure:` block composes
    /// it: the last launchd exit, then where the stderr tail came from, then
    /// the tail itself.
    func fleetFailureDetail(_ entry: FleetServiceEntry) -> String {
        guard let failure = entry.failure else {
            return entry.detail.isEmpty
                ? "The host reported the failure without evidence; stado service status could not read the last exit or the stderr tail."
                : entry.detail
        }
        var lines = [failure.lastExit.map { "last launchd exit \($0)" } ?? "last launchd exit unknown"]
        if let origin = failure.errorOrigin {
            lines.append("stderr: \(origin)")
        }
        lines.append(contentsOf: failure.errorLines)
        if let note = failure.note {
            lines.append("note: \(note)")
        }
        return lines.joined(separator: "\n")
    }

    /// The one write this screen allows, and an honest sentence where it is
    /// not allowed: a system LaunchDaemon loads as root, the approved channel
    /// is unprivileged, and a button that can only be refused is a lie.
    ///
    /// A unit declared where its host cannot load it is the same lie with a
    /// different cause. `stado service restart` on the mini's agent exits 1
    /// with the unit `not_loaded` and the postcondition unmet, every time,
    /// because there is no per-login domain on that machine to load it into.
    /// So this offers the command that changes the answer instead of a button
    /// that cannot.
    @ViewBuilder
    func restartAffordance(_ entry: FleetServiceEntry) -> some View {
        if let finding = entry.misdeclaredDomain {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text("Restarting it cannot help")
                    .font(WisentTypeScale.bodyStrong())
                    .foregroundStyle(WisentDesign.secondary)
                Text("Nobody is logged in on \(finding.host), so it has nowhere to start a unit registered as a user service: a restart is refused every time, with the unit left not loaded. Installing it as a machine service is what changes that, and it takes one privileged command run on the host itself.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                WisentField(label: "Run this on \(finding.host)", value: finding.installCommand)
            }
        } else if entry.domain.requiresPrivilegedBootstrap {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text("Privileged bootstrap required")
                    .font(WisentTypeScale.bodyStrong())
                    .foregroundStyle(WisentDesign.secondary)
                Text("This is a system LaunchDaemon; the approved channel is unprivileged and cannot bootstrap it. Use stado repair stado --step host --target \(entry.host) --apply, or load it as root on the host itself.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        } else {
            WisentActionButton(
                action: WisentAction(
                    "Restart…",
                    symbol: "arrow.clockwise",
                    kind: .primary,
                    isEnabled: !fleetStore.mutation.isWorking
                ) {
                    restartCandidate = entry
                }
            )
        }
    }

    func restartDialog(_ entry: FleetServiceEntry) -> some View {
        let unit = entry.unitID.isEmpty ? entry.name : entry.unitID
        return WisentDecisionDialog(
            tone: .warning,
            title: "Restart \(unit) on \(entry.host)?",
            lines: [
                "Stado restarts the unit over the approved channel and reads the host's state before the connection closes: the restart is only reported as done if the unit is left running.",
                "Whatever the unit was serving is interrupted until it is back.",
            ],
            footnote: "Runs \(StadoCLI.commandLine(FleetServicesStore.restartArguments(name: entry.name, host: entry.host))).",
            actions: [
                WisentAction("Keep it running as is", kind: .primary) { restartCandidate = nil },
                WisentAction("Restart", symbol: "arrow.clockwise", kind: .destructive) {
                    let candidate = entry
                    restartCandidate = nil
                    Task { await fleetStore.restart(candidate) }
                },
            ]
        )
    }
}
