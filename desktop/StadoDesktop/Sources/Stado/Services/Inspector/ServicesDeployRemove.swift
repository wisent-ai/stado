import SwiftUI
import WisentDesignSystem

/// The two writes that create and destroy a service, and their decisions.
///
/// The affordances are internal rather than private because
/// `fleetServiceInspector` sits in `ServicesInspector.swift`, and the dialogs
/// because `body` sits in `ServicesView.swift`: Swift scopes `private` to one
/// file.
extension ServicesView {
    /// A declaration writes a placeholder row whose state is `missing`.
    /// That is the one row that earns Deploy: an active/failed unit already
    /// exists and belongs to restart/update, while a declaration with no unit
    /// needs `service deploy` and nothing else.
    @ViewBuilder
    func deployAffordance(_ entry: FleetServiceEntry) -> some View {
        if entry.state.lowercased() == "missing" {
            WisentActionButton(
                action: WisentAction(
                    "Deploy service…",
                    symbol: "shippingbox.and.arrow.backward",
                    kind: .primary,
                    isEnabled: !fleetStore.mutation.isWorking
                ) {
                    deployCandidate = entry
                }
            )
        }
    }

    func deployDialog(_ entry: FleetServiceEntry) -> some View {
        WisentDecisionDialog(
            tone: .warning,
            title: "Deploy \(entry.name) on \(entry.host)?",
            lines: [
                "The service declaration already owns the immutable artifact, digest, arguments, endpoint, readiness check and consumers. This action supplies none of them and cannot override them.",
                "Stado installs that exact declaration, verifies the artifact before activation, creates the unit and records the new registry generation.",
            ],
            listing: [StadoCLI.commandLine(FleetServicesStore.deployArguments(name: entry.name, host: entry.host))],
            actions: [
                WisentAction("Keep it declared only", kind: .primary) { deployCandidate = nil },
                WisentAction("Deploy service", symbol: "shippingbox", kind: .secondary) {
                    let candidate = entry
                    deployCandidate = nil
                    Task { await fleetStore.deploy(candidate) }
                },
            ]
        )
    }

    /// The file-delete verb is offered only where the CLI's guards could
    /// pass: a unit file inside a user's own LaunchAgents or under .stado.
    /// Anything else — a system daemon path, an empty path — has no button,
    /// because `stado space file remove` would refuse it before deleting
    /// anything anyway.
    @ViewBuilder
    func removeFileAffordance(_ entry: FleetServiceEntry) -> some View {
        if entry.removableByRemoveFile {
            WisentActionButton(
                action: WisentAction(
                    "Remove this service…",
                    symbol: "trash",
                    kind: .plain,
                    isEnabled: !fleetStore.mutation.isWorking
                ) {
                    removeFileCandidate = entry
                }
            )
        }
    }

    func removeFileDialog(_ entry: FleetServiceEntry) -> some View {
        let unit = entry.unitID.isEmpty ? entry.name : entry.unitID
        return WisentDecisionDialog(
            tone: .danger,
            title: "Remove \(unit) on \(entry.host)?",
            lines: [
                "This is the whole of removing a service: it is stopped, forgotten by Stado, and its unit file is deleted from the host, in that order. A unit that will not stop keeps everything — its file is not deleted out from under a running process.",
                "The file's guards decide on the host: anything that is not a regular file owned by the login account is refused, and the refusal arrives in the CLI's own words.",
            ],
            footnote: "Runs \(StadoCLI.commandLine(FleetServicesStore.removeServiceArguments(name: entry.name, host: entry.host))).",
            actions: [
                WisentAction("Keep the service", kind: .primary) { removeFileCandidate = nil },
                WisentAction("Remove the service", symbol: "trash", kind: .destructive) {
                    let candidate = entry
                    removeFileCandidate = nil
                    Task { await fleetStore.removeService(candidate) }
                },
            ]
        )
    }
}
