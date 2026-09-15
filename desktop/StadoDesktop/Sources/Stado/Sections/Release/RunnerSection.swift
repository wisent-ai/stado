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

    @StateObject private var runner = RunnerStore()
    @State private var profile = ""
    @State private var repository = ""

    private var target: String { host.targetName ?? host.displayName }

    private var canRunProfile: Bool {
        !runner.isLoading && !fleetStore.runnerMutation.isWorking && runner.profiles.contains(profile)
    }

    private func readProfiles() async {
        await runner.load(fleet: fleetStore)
        if !runner.profiles.contains(profile) { profile = runner.profiles.first ?? "" }
    }

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
            Button("Read profiles") {
                Task { await readProfiles() }
            }.disabled(runner.isLoading || fleetStore.runnerMutation.isWorking)
            Picker("Profile", selection: $profile) {
                Text("Choose a declared profile…").tag("")
                ForEach(runner.profiles, id: \.self) { Text($0).tag($0) }
            }
            .disabled(runner.isLoading || fleetStore.runnerMutation.isWorking)
            if runner.isLoading { ProgressView("Reading runner…") }

            WisentField(label: "Profile", value: report?.profile ?? profile)
            WisentField(label: "Registration scope", value: report?.runnerScope ?? "Not read")
            WisentField(label: "Listener", value: listenerLabel)
            WisentField(label: "Host job slot", value: report?.hostJobSlot ?? "Not read")
            WisentField(label: "Labels", value: report?.runnerLabels ?? "Not read")
            if let registration = report?.registration {
                WisentField(label: "GitHub runner", value: registration.runner)
                WisentField(label: "GitHub scope", value: registration.scope)
                WisentField(label: "GitHub status", value: registration.status ?? "Unknown")
                WisentField(label: "Registration changed", value: registration.reconfigured ? "Yes" : "No")
            }
            if let review = report?.modelReview {
                WisentField(label: "Model review", value: review.state)
                WisentField(label: "Repository secret", value: review.secret)
            }

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
                    isEnabled: canRunProfile
                ) {
                    Task { await fleetStore.readHostRunner(host: target, profile: profile) }
                }
            )
            WisentActionButton(
                action: WisentAction(
                    "Read diagnostics",
                    symbol: "doc.text.magnifyingglass",
                    isEnabled: canRunProfile
                ) {
                    Task { await runner.readDiagnostics(target: target, profile: profile, fleet: fleetStore) }
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
                    isEnabled: canRunProfile
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
                    isEnabled: canRunProfile
                ) {
                    Task { await fleetStore.restartHostRunner(host: target, profile: profile) }
                }
            )
            WisentActionButton(
                action: WisentAction(
                    "Remove",
                    symbol: "trash",
                    isEnabled: canRunProfile
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
            WisentActionButton(
                action: WisentAction(
                    "Check GitHub credential",
                    symbol: "key",
                    isEnabled: !fleetStore.runnerMutation.isWorking
                ) {
                    Task { await fleetStore.checkRunnerCredential(host: target) }
                }
            )
            WisentActionButton(
                action: WisentAction(
                    "Configure model review",
                    symbol: "text.badge.checkmark",
                    isEnabled: !fleetStore.runnerMutation.isWorking
                        && !repository.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                ) {
                    Task { await fleetStore.configureRunnerModelReview(host: target, repository: repository) }
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
            if let failure = runner.failure {
                WisentAlertPanel(tone: .danger, title: "Runner read failed", detail: failure)
            }
            if let diagnostic = runner.diagnostic,
               diagnostic.target == target, diagnostic.profile == profile {
                WisentField(label: "Diagnostic read", value: diagnostic.read)
                WisentField(label: "Diagnostic log", value: diagnostic.log)
                Text(diagnostic.tail).font(WisentTypeScale.identifier()).textSelection(.enabled)
                if !diagnostic.stderr.isEmpty {
                    WisentAlertPanel(tone: .warning, title: "Diagnostic read errors", detail: diagnostic.stderr)
                }
            }
            if let receipt = runner.lastReceipt {
                DisclosureGroup("Complete runner receipt") {
                    Text(receipt.standardOutput).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    Text(receipt.standardError).font(WisentTypeScale.identifier()).textSelection(.enabled)
                }
            }
        }
        .task(id: "\(target)|\(fleetStore.requestGeneration)") {
            profile = ""
            await readProfiles()
        }
    }
}
