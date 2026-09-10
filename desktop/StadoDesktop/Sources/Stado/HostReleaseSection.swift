import Foundation
import SwiftUI
import WisentDesignSystem

struct HostReleaseBinary: Decodable, Identifiable, Sendable {
    let binary: String
    let version: String?
    let root: String
    let unit: String
    let state: String
    let attestation: String
    let receipt: String
    let declaredVersion: String
    let verdict: String
    let detail: String

    var id: String { binary }

    private enum CodingKeys: String, CodingKey {
        case binary, version, root, unit, state, attestation, receipt, verdict, detail
        case declaredVersion = "declared_version"
    }
}

/// The software report the visit left on file for `stado release status`, as
/// the command reports it: the state of the look, its age, the counts, and
/// the refusal when the host could not be read.
struct HostSoftwareReport: Decodable, Sendable {
    let state: String
    let observed: String
    let detail: String
    let reported: Int
    let release: Int
    let unmanaged: Int
    let scripts: Int

    var summary: String {
        "\(reported) program(s), \(release) release, \(unmanaged) unmanaged, \(scripts) script(s)"
    }
}

struct HostReleaseState: Decodable, Sendable {
    let target: String
    let state: String
    let applied: Bool
    let binaries: [HostReleaseBinary]
    let software: HostSoftwareReport?
}

@MainActor
final class HostReleaseStore: ObservableObject {
    @Published private(set) var report: HostReleaseState?
    @Published private(set) var refusal: String?
    @Published private(set) var receipt: OperatorCommandResult?
    @Published private(set) var isRefreshing = false
    private var generation = 0

    nonisolated static func arguments(host: String) -> [String] {
        ["release", "host-state", "--host", host, "--json"]
    }

    func refresh(host: String, fleet: FleetControlStore) async {
        generation += 1
        let current = generation
        let source = fleet.requestGeneration
        report = nil
        receipt = nil
        refusal = nil
        guard let address = fleet.address else {
            refusal = "No Stado API is configured."
            isRefreshing = false
            return
        }
        isRefreshing = true
        defer { if current == generation { isRefreshing = false } }
        do {
            let result = try await fleet.client.run(arguments: Self.arguments(host: host),
                confirmsMutation: false, at: address, authorizationToken: fleet.authorizationToken)
            guard current == generation, source == fleet.requestGeneration else { return }
            receipt = result
            refusal = result.ok ? nil : result.message
            do {
                report = try JSONDecoder().decode(HostReleaseState.self, from: Data(result.standardOutput.utf8))
            } catch {
                if refusal == nil { refusal = error.localizedDescription }
            }
        } catch {
            guard current == generation, source == fleet.requestGeneration else { return }
            refusal = error.localizedDescription
        }
    }
}

struct HostReleaseSection: View {
    @ObservedObject var store: HostReleaseStore
    let host: String
    @ObservedObject var fleet: FleetControlStore

    var body: some View {
        WisentSectionBox(
            title: "Release state",
            detail: "Reads the release manifest and this host's targets[].managed_versions declaration through Stado's release capability. No desired version is inferred by Desktop.",
            trailing: store.isRefreshing ? "Reading…" : store.report?.state.humanizedIdentifier
        ) {
            WisentField(
                label: "Command",
                value: StadoCLI.commandLine(HostReleaseStore.arguments(host: host))
            )
            if let refusal = store.refusal, !refusal.isEmpty {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The release command refused this state",
                    detail: refusal
                )
            }
            if let report = store.report {
                if report.state == "undeclared" {
                    WisentField(
                        label: "Desired state",
                        value: "undeclared — add managed versions to targets[].managed_versions"
                    )
                }
                ForEach(report.binaries) { row in
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                        WisentField(
                            label: row.binary,
                            value: "declared=\(row.declaredVersion) verdict=\(row.verdict)"
                        )
                        WisentField(label: "binary", value: row.binary)
                        WisentField(label: "version", value: row.version ?? "unknown")
                        WisentField(label: "root", value: row.root)
                        WisentField(label: "unit", value: row.unit)
                        WisentField(label: "state", value: row.state)
                        WisentField(label: "attestation", value: row.attestation)
                        WisentField(label: "receipt", value: row.receipt)
                        if !row.detail.isEmpty && row.detail != "-" {
                            WisentField(label: "Detail", value: row.detail)
                        }
                    }
                }
                if let software = report.software {
                    if software.state == "observed" {
                        WisentField(
                            label: "Software report",
                            value: "recorded \(software.observed): \(software.summary)"
                        )
                    } else {
                        WisentAlertPanel(
                            tone: .warning,
                            title: "The software report was not refreshed (\(software.state))",
                            detail: software.detail
                        )
                    }
                }
            }
            WisentActionButton(
                action: WisentAction(
                    "Read release state",
                    symbol: "arrow.clockwise",
                    isEnabled: !store.isRefreshing
                ) {
                    Task { await store.refresh(host: host, fleet: fleet) }
                }
            )
            NativeCapabilityActions(host: host, fleet: fleet, operations: NativeHostReleaseOperations.all)
            if let receipt = store.receipt {
                DisclosureGroup("Complete release state receipt") {
                    Text(receipt.standardOutput).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    Text(receipt.standardError).font(WisentTypeScale.identifier()).textSelection(.enabled)
                }
            }
        }
        .task(id: "\(host)|\(fleet.requestGeneration)") {
            await store.refresh(host: host, fleet: fleet)
        }
    }
}
