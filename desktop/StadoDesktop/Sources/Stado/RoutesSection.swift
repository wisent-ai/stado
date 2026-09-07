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

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    func load() async {
        guard !isLoading else { return }
        isLoading = true
        problem = nil
        do {
            report = try await cli.json(
                RouteDirectoryReport.self,
                arguments: ["route", "list", "--json"]
            )
        } catch {
            report = nil
            if let localized = error as? LocalizedError,
               let description = localized.errorDescription {
                problem = description
            } else {
                problem = error.localizedDescription
            }
        }
        isLoading = false
    }
}

struct RoutesSection: View {
    @ObservedObject var store: RoutesStore
    let selectedHost: String

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
        }
        .task {
            await store.load()
        }
    }

    private func endpointDescription(_ route: RouteDirectoryService) -> String {
        guard let endpoint = route.endpoints.first(where: { $0.target == selectedHost }) else {
            return "No endpoint declared for \(selectedHost)"
        }
        return "\(selectedHost) calls \(endpoint.url ?? "an undeclared address")"
    }
}
