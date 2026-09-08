import Combine
import Foundation
import WisentDesignSystem

/// The argv `FleetServicesStore` runs, and the decoding behind its reads.
///
/// Nothing here touches the store's own state: every member is a `nonisolated
/// static` that turns operator intent into the exact command a terminal would
/// run, or one recorded answer into typed rows.
extension FleetServicesStore {
    nonisolated static func listArguments() -> [String] {
        ["service", "list", "--json"]
    }

    nonisolated static func statusArguments(name: String) -> [String] {
        ["service", "status", name, "--json"]
    }

    nonisolated static func restartArguments(name: String, host: String) -> [String] {
        ["service", "restart", name, "--host", host, "--json"]
    }

    nonisolated static func removeServiceArguments(name: String, host: String) -> [String] {
        ["service", "remove", name, "--host", host, "--json"]
    }

    nonisolated static func deployArguments(name: String, host: String) -> [String] {
        ["service", "deploy", name, "--host", host, "--json"]
    }

    nonisolated static func repairRunnerRuntimeArguments(name: String, host: String) -> [String] {
        ["service", "repair-runner-runtime", name, "--host", host, "--json"]
    }

    nonisolated static func convergeApplyArguments(host: String, binary: String?) -> [String] {
        var arguments = ["service", "converge", host]
        if let binary, !binary.isEmpty {
            arguments.append(binary)
        }
        arguments.append(contentsOf: ["--apply", "--json"])
        return arguments
    }

    enum ListReading: Sendable {
        case listed([FleetServiceEntry])
        case failed(String)
    }

    nonisolated static func read(using cli: StadoCLI) async -> ListReading {
        do {
            return .listed(try await cli.json(FleetServiceList.self, arguments: listArguments()))
        } catch {
            return .failed(message(for: error))
        }
    }

    /// One `service status --json` per failed name, concurrently, keyed by
    /// the entry id the failure belongs to. A status read that fails costs
    /// that unit its evidence line, never the list it was annotating.
    nonisolated static func failureEvidence(
        for names: [String],
        using cli: StadoCLI
    ) async -> [String: ServiceFailure] {
        guard !names.isEmpty else { return [:] }
        return await withTaskGroup(of: [String: ServiceFailure].self) { group in
            for name in names {
                group.addTask {
                    guard let rows = try? await cli.json(
                        FleetServiceList.self,
                        arguments: statusArguments(name: name)
                    ) else { return [:] }
                    var evidence: [String: ServiceFailure] = [:]
                    for row in rows where row.failure != nil {
                        evidence[row.id] = row.failure
                    }
                    return evidence
                }
            }
            var merged: [String: ServiceFailure] = [:]
            for await evidence in group {
                merged.merge(evidence) { _, new in new }
            }
            return merged
        }
    }

    /// `service list --json` prints a bare array; naming the type keeps the
    /// decode site reading like the command it runs.
    private typealias FleetServiceList = [FleetServiceEntry]

    /// Decode one `service list --json` payload without running anything, so
    /// the shape this console reads can be exercised against a recorded
    /// answer — including the registry finding three of the fleet's rows
    /// carry today.
    nonisolated static func decode(from output: String) -> [FleetServiceEntry]? {
        let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let data = trimmed.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(FleetServiceList.self, from: data)
    }

    nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}
