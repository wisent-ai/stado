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
        var arguments = ["release", "version", "converge", "--host", host]
        if let binary, !binary.isEmpty {
            arguments.append(contentsOf: ["--binary", binary])
        }
        arguments.append("--json")
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

    /// Preserve a refused status read beside the beacon row rather than
    /// turning it into an absence of evidence. One name may have several
    /// hosts, so the same command refusal belongs to each matching row.
    nonisolated static func failureEvidence(
        for names: [String],
        using cli: StadoCLI
    ) async -> ([String: ServiceFailure], [String: String]) {
        guard !names.isEmpty else { return ([:], [:]) }
        return await withTaskGroup(
            of: (String, [String: ServiceFailure], String?).self
        ) { group in
            for name in names {
                group.addTask {
                    do {
                        let rows = try await cli.json(
                            FleetServiceList.self,
                            arguments: statusArguments(name: name)
                        )
                        var evidence: [String: ServiceFailure] = [:]
                        for row in rows where row.failure != nil {
                            evidence[row.id] = row.failure
                        }
                        return (name, evidence, nil)
                    } catch {
                        return (name, [:], message(for: error))
                    }
                }
            }
            var merged: [String: ServiceFailure] = [:]
            var errors: [String: String] = [:]
            for await (name, evidence, error) in group {
                merged.merge(evidence) { _, new in new }
                if let error { errors[name] = error }
            }
            return (merged, errors)
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
