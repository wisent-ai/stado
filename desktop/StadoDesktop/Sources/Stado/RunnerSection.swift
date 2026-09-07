import SwiftUI
import WisentDesignSystem

/// The declared runner capability for one host.
///
/// Scope comes from the host's registration record and listener connectivity
/// is a typed field in the CLI report; neither is inferred from the selected
/// profile or from raw script output.
struct RunnerSection: View {
    let host: WorkerNode
    @ObservedObject var fleetStore: FleetControlStore

    @State private var profile = "precheck"
    @State private var repository = ""

    private var target: String { host.targetName ?? host.displayName }

    private var report: HostRunnerReport? {
        guard fleetStore.runnerHost == target,
              fleetStore.runnerReport?.profile == profile else { return nil }
        return fleetStore.runnerReport
    }

    private var listenerLabel: String {
        guard let listener = report?.listener else { return "Not read" }
        switch listener.connected {
        case true: return "Connected"
        case false: return "Disconnected"
        case nil: return listener.state == "unknown" ? "Unknown" : listener.state
        }
    }

    var body: some View {
        WisentSectionBox(
            title: "GitHub runners",
            detail: "Profiles come from runner-profiles.json. Each host admits one job across every installed profile."
        ) {
            Picker("Profile", selection: $profile) {
                Text("Precheck").tag("precheck")
                Text("Publisher").tag("publisher")
            }
            .pickerStyle(.segmented)

            WisentField(label: "Profile", value: report?.profile ?? profile)
            WisentField(label: "Registration scope", value: report?.runnerScope ?? "Not read")
            WisentField(label: "Listener", value: listenerLabel)
            WisentField(label: "Host job slot", value: report?.hostJobSlot ?? "Not read")
            WisentField(label: "Labels", value: report?.runnerLabels ?? "Not read")

            WisentField(
                label: "Read-only command",
                value: StadoCLI.commandLine(
                    FleetControlStore.hostRunnerArguments(
                        action: "status",
                        host: target,
                        profile: profile,
                        repository: nil
                    )
                )
            )
            WisentActionButton(
                action: WisentAction(
                    "Read runner",
                    symbol: "arrow.clockwise",
                    isEnabled: !fleetStore.runnerMutation.isWorking
                ) {
                    Task { await fleetStore.readHostRunner(host: target, profile: profile) }
                }
            )

            TextField("Repository (optional)", text: $repository)
                .textFieldStyle(.roundedBorder)
                .font(WisentTypeScale.body())
            WisentField(
                label: "Command",
                value: StadoCLI.commandLine(
                    FleetControlStore.hostRunnerArguments(
                        action: "install",
                        host: target,
                        profile: profile,
                        repository: repository
                    )
                )
            )
            WisentActionButton(
                action: WisentAction(
                    "Install or reconcile",
                    symbol: "square.and.arrow.down",
                    isEnabled: !fleetStore.runnerMutation.isWorking
                ) {
                    Task {
                        await fleetStore.installHostRunner(
                            host: target,
                            profile: profile,
                            repository: repository
                        )
                    }
                }
            )
            WisentActionButton(
                action: WisentAction(
                    "Restart in place",
                    symbol: "arrow.triangle.2.circlepath",
                    isEnabled: !fleetStore.runnerMutation.isWorking
                ) {
                    Task { await fleetStore.restartHostRunner(host: target, profile: profile) }
                }
            )
            WisentActionButton(
                action: WisentAction(
                    "Remove",
                    symbol: "trash",
                    isEnabled: !fleetStore.runnerMutation.isWorking
                ) {
                    Task {
                        await fleetStore.removeHostRunner(
                            host: target,
                            profile: profile,
                            repository: repository
                        )
                    }
                }
            )

            if fleetStore.runnerHost == target {
                WisentMutationBar(outcome: fleetStore.runnerMutation) {
                    fleetStore.clearRunnerMutation()
                }
            }
            if let detail = report?.listener.state,
               !detail.isEmpty,
               detail != "unknown" {
                WisentField(label: "Listener detail", value: detail)
            }
            if let stderr = report?.stderr.trimmingCharacters(in: .whitespacesAndNewlines),
               !stderr.isEmpty {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The host reported this while answering",
                    detail: stderr
                )
            }
        }
    }
}
