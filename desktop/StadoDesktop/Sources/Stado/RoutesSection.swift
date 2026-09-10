import SwiftUI
import WisentDesignSystem

struct RouteDirectoryReport: Decodable, Sendable {
    let authority: RouteDirectoryAuthority
    let services: [RouteDirectoryService]
}

struct RouteDirectoryAuthority: Decodable, Sendable {
    let target: String
    let command: String
}

struct RouteDirectoryService: Decodable, Sendable, Identifiable {
    let service: String
    let authority: RouteDirectoryAuthority
    let activeHost: String
    let endpoints: [RouteDirectoryEndpoint]
    let localForward: RouteOpenForward?

    var id: String { service }

    enum CodingKeys: String, CodingKey {
        case service, authority, endpoints
        case activeHost = "active_host"
        case localForward = "local_forward"
    }
}

struct RouteDirectoryEndpoint: Decodable, Sendable, Identifiable {
    let target: String
    let url: String?

    var id: String { target }
}

struct RouteOpenForward: Decodable, Sendable {
    let service: String
    let url: String
    let marker: String
    let location: String
}

@MainActor
final class RoutesStore: ObservableObject {
    @Published private(set) var report: RouteDirectoryReport?
    @Published private(set) var isLoading = false
    @Published private(set) var problem: String?
    @Published private(set) var receipt: OperatorCommandResult?
    private var generation = 0

    func load(fleet: FleetControlStore) async {
        generation += 1
        let current = generation
        let source = fleet.requestGeneration
        report = nil
        receipt = nil
        problem = nil
        guard let address = fleet.address else {
            problem = "No Stado API is configured."
            isLoading = false
            return
        }
        isLoading = true
        defer { if current == generation { isLoading = false } }
        do {
            let result = try await fleet.client.run(arguments: ["route", "list", "--json"],
                confirmsMutation: false, at: address, authorizationToken: fleet.authorizationToken)
            guard current == generation, source == fleet.requestGeneration else { return }
            receipt = result
            guard result.ok else { problem = result.message; return }
            report = try JSONDecoder().decode(RouteDirectoryReport.self, from: Data(result.standardOutput.utf8))
        } catch {
            guard current == generation, source == fleet.requestGeneration else { return }
            problem = error.localizedDescription
        }
    }
}

struct RoutesSection: View {
    @ObservedObject var store: RoutesStore
    let selectedHost: String
    @ObservedObject var fleet: FleetControlStore

    var body: some View {
        WisentSectionBox(
            title: "Routes",
            detail: "Every endpoint and active host comes from the fleet service directory. Open forwards are the marker files consumers on this machine read.",
            trailing: store.isLoading ? "Reading…" : store.report.map { "\($0.services.count) declared" }
        ) {
            if let problem = store.problem {
                WisentAlertPanel(
                    tone: .warning,
                    title: "Routes were not read",
                    detail: problem
                )
            } else if let report = store.report {
                WisentField(
                    label: "Directory authority",
                    value: "\(report.authority.target) · \(report.authority.command)"
                )
                ForEach(report.services) { route in
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                        HStack(spacing: WisentDesign.Space.x2) {
                            Text(route.service)
                                .font(WisentTypeScale.bodyStrong())
                                .foregroundStyle(WisentDesign.ink)
                            Spacer(minLength: 0)
                            Text(route.activeHost == selectedHost ? "Served here" : "Served by \(route.activeHost)")
                                .font(WisentTypeScale.caption())
                                .foregroundStyle(WisentDesign.secondary)
                        }
                        Text(endpointDescription(route))
                            .font(WisentTypeScale.caption())
                            .foregroundStyle(WisentDesign.secondary)
                        if let forward = route.localForward {
                            Text("Open forward · \(forward.url) · \(forward.marker)")
                                .font(WisentTypeScale.caption())
                                .foregroundStyle(WisentDesign.success)
                        } else {
                            Text("No local forward open")
                                .font(WisentTypeScale.caption())
                                .foregroundStyle(WisentDesign.secondary)
                        }
                    }
                    .padding(.vertical, WisentDesign.Space.x1)
                }
            } else {
                Text("Route declarations have not been read yet.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            }
            NativeCapabilityActions(host: selectedHost, fleet: fleet, operations: NativeRouteOperations.all)
            if let receipt = store.receipt {
                DisclosureGroup("Complete directory read receipt") {
                    Text(receipt.standardOutput).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    Text(receipt.standardError).font(WisentTypeScale.identifier()).textSelection(.enabled)
                }
            }
        }
        .task(id: "\(selectedHost)|\(fleet.requestGeneration)") {
            await store.load(fleet: fleet)
        }
    }

    private func endpointDescription(_ route: RouteDirectoryService) -> String {
        guard let endpoint = route.endpoints.first(where: { $0.target == selectedHost }) else {
            return "No endpoint declared for \(selectedHost)"
        }
        return "\(selectedHost) calls \(endpoint.url ?? "an undeclared address")"
    }
}
