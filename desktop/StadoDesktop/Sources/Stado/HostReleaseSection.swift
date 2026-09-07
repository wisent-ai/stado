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

struct HostReleaseState: Decodable, Sendable {
    let target: String
    let state: String
    let applied: Bool
    let binaries: [HostReleaseBinary]
}

@MainActor
final class HostReleaseStore: ObservableObject {
    @Published private(set) var report: HostReleaseState?
    @Published private(set) var refusal: String?
    @Published private(set) var isRefreshing = false

    private let cli: StadoCLI
    private var generation = 0

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func arguments(host: String) -> [String] {
        ["release", "host-state", "--host", host, "--json"]
    }

    func refresh(host: String) async {
        generation += 1
        let current = generation
        isRefreshing = true
        defer {
            if current == generation { isRefreshing = false }
        }
        do {
            let answer = try await cli.jsonResult(
                HostReleaseState.self,
                arguments: Self.arguments(host: host)
            )
            guard current == generation else { return }
            report = answer.value
            refusal = answer.refusal
        } catch {
            guard current == generation else { return }
            report = nil
            refusal = error.localizedDescription
        }
    }
}

struct HostReleaseSection: View {
    @ObservedObject var store: HostReleaseStore
    let host: String

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
            }
            WisentActionButton(
                action: WisentAction(
                    "Read release state",
                    symbol: "arrow.clockwise",
                    isEnabled: !store.isRefreshing
                ) {
                    Task { await store.refresh(host: host) }
                }
            )
        }
        .task(id: host) {
            await store.refresh(host: host)
        }
    }
}
