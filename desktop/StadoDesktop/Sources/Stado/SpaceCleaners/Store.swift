import Foundation
import WisentDesignSystem

struct HostCleaner: Decodable, Sendable, Identifiable {
    struct Declaration: Decodable, Sendable {
        let root: String?
        let minAgeSeconds: Int64
        let keepNewest: Int64?
        let allowMissingUploadProof: Bool?
        enum CodingKeys: String, CodingKey {
            case root
            case minAgeSeconds = "min_age_seconds"
            case keepNewest = "keep_newest"
            case allowMissingUploadProof = "allow_missing_upload_proof"
        }
    }
    let cleaner: String
    let declared: Bool
    let declaration: Declaration?
    let sweeps: String
    let defaultRoot: String
    let since: String
    let supported: Bool
    let detail: String
    let minAgeFloorSeconds: Int64?
    var id: String { cleaner }
    enum CodingKeys: String, CodingKey {
        case cleaner, declared, declaration, sweeps, since, detail
        case defaultRoot = "default_root"
        case supported = "supported_by_installed_binary"
        case minAgeFloorSeconds = "min_age_floor_seconds"
    }
}

struct HostCleanerListing: Decodable, Sendable {
    let target: String
    let installedStado: String
    let installedReadError: String?
    let policyMode: String?
    let declaresPolicy: Bool
    let cleaners: [HostCleaner]
    enum CodingKeys: String, CodingKey {
        case target, cleaners
        case installedStado = "installed_stado"
        case installedReadError = "installed_read_error"
        case policyMode = "policy_mode"
        case declaresPolicy = "declares_policy"
    }
}

@MainActor
final class HostCleanersStore: ObservableObject {
    @Published private(set) var listing: HostCleanerListing?
    @Published private(set) var isLoading = false
    @Published private(set) var working: String?
    @Published private(set) var problem: String?
    @Published private(set) var receipt: OperatorCommandResult?
    @Published private(set) var mutation: WisentMutationOutcome = .idle
    private var generation = 0

    func load(host: String, fleet: FleetControlStore) async {
        generation += 1
        let request = generation
        let source = fleet.requestGeneration
        listing = nil
        receipt = nil
        problem = nil
        guard let address = fleet.address else {
            problem = "No Stado API is configured."
            isLoading = false
            return
        }
        isLoading = true
        defer { if request == generation { isLoading = false } }
        do {
            let result = try await fleet.client.run(
                arguments: ["space", "cleaners", "list", host, "--json"],
                confirmsMutation: false, at: address,
                authorizationToken: fleet.authorizationToken
            )
            guard request == generation, source == fleet.requestGeneration else { return }
            receipt = result
            guard result.ok else { problem = result.message; return }
            listing = try JSONDecoder().decode(HostCleanerListing.self, from: Data(result.standardOutput.utf8))
            if listing?.target != host {
                listing = nil
                problem = "The cleaner report names a different host."
            }
        } catch {
            guard request == generation, source == fleet.requestGeneration else { return }
            problem = error.localizedDescription
        }
    }

    func declare(host: String, cleaner: String, fields: [String], fleet: FleetControlStore) async -> Bool {
        await write(host: host, cleaner: cleaner,
            arguments: ["space", "cleaners", "declare", host, "--cleaner", cleaner] + fields + ["--json"],
            fleet: fleet)
    }

    func withdraw(host: String, cleaner: String, fleet: FleetControlStore) async {
        _ = await write(host: host, cleaner: cleaner,
            arguments: ["space", "cleaners", "remove", host, "--cleaner", cleaner, "--json"], fleet: fleet)
    }

    func clearMutation() { mutation = .idle }

    private func write(host: String, cleaner: String, arguments: [String], fleet: FleetControlStore) async -> Bool {
        guard working == nil, let address = fleet.address else { return false }
        let source = fleet.requestGeneration
        let requested = generation
        working = cleaner
        mutation = .working("Saving \(cleaner) on \(host)")
        defer { working = nil }
        do {
            let result = try await fleet.client.run(arguments: arguments, confirmsMutation: true,
                at: address, authorizationToken: fleet.authorizationToken)
            guard requested == generation, source == fleet.requestGeneration else { return false }
            receipt = result
            guard result.ok else {
                problem = result.message
                mutation = .failed(result.message)
                return false
            }
            await load(host: host, fleet: fleet)
            guard source == fleet.requestGeneration else { return false }
            receipt = result
            if let problem { mutation = .failed("Write returned success, but read-back failed: \(problem)"); return false }
            mutation = .succeeded("Saved \(cleaner) on \(host) and read back the registry declaration.")
            return true
        } catch {
            guard requested == generation, source == fleet.requestGeneration else { return false }
            problem = error.localizedDescription
            mutation = .failed(error.localizedDescription)
            return false
        }
    }
}
