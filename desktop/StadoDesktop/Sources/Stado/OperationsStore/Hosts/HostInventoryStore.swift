import Combine
import Foundation
import WisentDesignSystem

/// Live fixed-path filesystem inventory for the selected Hosts inspector row.
///
/// Reads go through the dashboard's authenticated typed HTTP API. One host is
/// requested at a time: a full inventory reaches the target and must not become
/// an implicit fleet-wide poll when the dashboard refreshes.
@MainActor
final class HostInventoryStore: ObservableObject {
    @Published private(set) var cargoByHost: [String: HostCargoInventory] = [:]
    @Published private(set) var failures: [String: String] = [:]
    @Published private(set) var readingHosts: Set<String> = []

    private let client: OperationsClient
    private var addressString = DashboardEndpointPreference.localURL
    private var requestGeneration = 0

    init(client: OperationsClient = OperationsClient()) {
        self.client = client
    }

    func configureEndpoint(_ endpoint: String?) {
        let normalized = endpoint?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard normalized != addressString else { return }
        requestGeneration &+= 1
        addressString = normalized
        cargoByHost = [:]
        failures = [:]
        readingHosts = []
    }

    func cargo(for host: String) -> HostCargoInventory? {
        cargoByHost[host]
    }

    func failure(for host: String) -> String? {
        failures[host]
    }

    func isReading(_ host: String) -> Bool {
        readingHosts.contains(host)
    }

    func refresh(host: String) async {
        guard !host.isEmpty, !readingHosts.contains(host) else { return }
        guard let address = try? OperationsDashboardAddress(addressString) else {
            failures[host] = "No Stado endpoint is configured, so the inventory was not read."
            return
        }
        let generation = requestGeneration
        readingHosts.insert(host)
        defer {
            if requestGeneration == generation {
                readingHosts.remove(host)
            }
        }
        do {
            let report = try await client.fetchHostInventory(
                target: host,
                from: address
            )
            guard requestGeneration == generation, !Task.isCancelled else { return }
            guard report.target == host else {
                failures[host] = "inventory for \(host) answered for \(report.target)"
                return
            }
            guard report.status == "inventory", let cargo = report.cargo else {
                failures[host] = report.error ?? "\(host) did not complete its inventory"
                return
            }
            cargoByHost[host] = cargo
            failures.removeValue(forKey: host)
        } catch is CancellationError {
            return
        } catch let error as URLError where error.code == .cancelled {
            return
        } catch {
            guard requestGeneration == generation else { return }
            failures[host] = Self.message(for: error)
        }
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}

/// The forward markers one host carries, read through `stado host inventory`.
///
/// Read-only, and per host on demand: the inventory read crosses the managed
/// channel, so it runs when an operator opens a host rather than on every
/// refresh of the list. A failure is shown as itself; the console never
/// renders an empty marker list for a read that did not happen.
@MainActor
final class HostForwardStore: ObservableObject {
    @Published private(set) var host: String?
    @Published private(set) var markers: [HostForwardMarker] = []
    @Published private(set) var problem: String?
    @Published private(set) var isLoading = false

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func arguments(host: String) -> [String] {
        ["host", "inventory", host, "--json"]
    }

    func load(host name: String) async {
        host = name
        isLoading = true
        problem = nil
        do {
            let report = try await cli.json(
                InventoryReport.self,
                arguments: Self.arguments(host: name),
                timeoutSeconds:
                    240
            )
            markers = report.forwards
        } catch {
            markers = []
            problem = Self.message(for: error)
        }
        isLoading = false
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }

    private struct InventoryReport: Decodable, Sendable {
        let forwards: [HostForwardMarker]
    }
}
