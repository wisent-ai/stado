import Foundation
import SwiftUI
import WisentDesignSystem

/// One host's answer in `stado release failed --json`: the Stado releases it
/// recorded as failed and rolled back from, or why its beacon could not read
/// its retained releases.
struct FailedReleaseHost: Decodable, Identifiable, Sendable {
    let host: String
    let failed: [FailedRelease]?
    let unreadable: String?

    var id: String { host }
}

/// A release as the host's beacon publishes it, with the record the host
/// wrote when the release refused its configuration.
struct FailedRelease: Decodable, Identifiable, Sendable {
    let version: String
    let platform: String
    let record: FailedReleaseRecord

    var id: String { "\(version)|\(platform)" }
}

struct FailedReleaseRecord: Decodable, Sendable {
    let at: String
    let cause: String
}

struct FailedReleasesReport: Decodable, Sendable {
    let hosts: [FailedReleaseHost]
}

@MainActor
final class FailedReleasesStore: ObservableObject {
    @Published private(set) var report: FailedReleasesReport?
    @Published private(set) var refusal: String?
    @Published private(set) var reading: UUID?

    nonisolated static let arguments = ["release", "failed", "--json"]

    func refresh(fleet: FleetControlStore) async {
        let current = UUID()
        let source = fleet.requestGeneration
        report = nil
        refusal = nil
        guard let address = fleet.address else {
            refusal = "No Stado API is configured."
            return
        }
        reading = current
        defer { if reading == current { reading = nil } }
        do {
            let result = try await fleet.client.run(arguments: Self.arguments,
                confirmsMutation: false, at: address, authorizationToken: fleet.authorizationToken)
            guard reading == current, source == fleet.requestGeneration else { return }
            guard result.ok else {
                refusal = [result.message, result.standardError]
                    .filter { !$0.isEmpty }.joined(separator: "\n")
                return
            }
            report = try JSONDecoder().decode(FailedReleasesReport.self, from: Data(result.standardOutput.utf8))
        } catch {
            guard reading == current, source == fleet.requestGeneration else { return }
            refusal = error.localizedDescription
        }
    }
}

/// The Stado releases a host recorded as failed and rolled back from, as
/// `stado release failed` reads them from the hosts' beacons. A release shown
/// here is refused by every delivery to another host until its record is
/// removed on the host that failed it.
struct FailedReleasesSection: View {
    @StateObject private var store = FailedReleasesStore()
    @ObservedObject var fleet: FleetControlStore

    var body: some View {
        WisentSectionBox(
            title: "Releases rolled back on a host",
            detail: "Read from every host's health beacon. A release here refused that host's configuration, was replaced by the release before it, and is not delivered to any other host.",
            trailing: store.reading == nil ? nil : "Reading…"
        ) {
            WisentField(label: "Command", value: StadoCLI.commandLine(FailedReleasesStore.arguments))
            if let refusal = store.refusal {
                WisentAlertPanel(tone: .warning, title: "The failed releases could not be read", detail: refusal)
            }
            if let report = store.report {
                if report.hosts.isEmpty {
                    WisentField(label: "Fleet", value: "no host publishes a failed Stado release")
                }
                ForEach(report.hosts) { host in
                    if let unreadable = host.unreadable {
                        WisentAlertPanel(
                            tone: .warning,
                            title: "\(host.host) could not read its retained releases",
                            detail: unreadable
                        )
                    }
                    if let failed = host.failed {
                        ForEach(failed) { release in
                            WisentAlertPanel(
                                tone: .danger,
                                title: "\(host.host): Stado \(release.version) (\(release.platform)) was rolled back at \(release.record.at)",
                                detail: release.record.cause
                            )
                        }
                    }
                }
            }
            WisentActionButton(
                action: WisentAction("Read failed releases", symbol: "arrow.clockwise",
                    isEnabled: store.reading == nil) {
                    Task { await store.refresh(fleet: fleet) }
                }
            )
        }
        .task(id: fleet.requestGeneration) {
            await store.refresh(fleet: fleet)
        }
    }
}
