import Foundation
import WisentDesignSystem

struct ProductCatalogEnvelope: Decodable, Sendable {
    let products: [ProductCatalogEntry]
}

struct ProductCatalogEntry: Decodable, Identifiable, Sendable {
    let id: String
    let name: String
    let description: String
    let surfaces: [ProductSurface]
    let installations: [ProductInstallation]
}

struct ProductSurface: Decodable, Sendable {
    let kind: String
    let repository: String
}

struct ProductInstallation: Decodable, Identifiable, Sendable {
    let surface: String
    let kind: String
    let repository: String
    let signingKeychain: String?
    enum CodingKeys: String, CodingKey {
        case surface, kind, repository
        case signingKeychain = "signing_keychain"
    }
    var id: String { "\(surface)/\(repository)" }
}

struct ProductLifecycleState: Decodable, Sendable {
    let product: String
    let surface: String
    let status: String
    let installedAt: String?
    let installedPaths: [String]
    let host: String?
    let sourceRevision: String?
    let release: ProductReleaseReceipt?
    let readiness: ProductReadiness?
    let installDecision: ProductInstallDecision?
    let cargoCache: ProductCargoCache?
    let dependencies: [ProductDependencyResult]?
    let signing: ProductSigning?
    let operation: ProductOperation?

    enum CodingKeys: String, CodingKey {
        case product, surface, status, host, release, readiness, signing, operation
        case sourceRevision = "source_revision"
        case installedAt = "installed_at"
        case installedPaths = "installed_paths"
        case installDecision = "install_decision"
        case cargoCache = "cargo_cache"
        case dependencies
    }
}

struct ProductOperation: Decodable, Sendable {
    let pid: Int
    let running: Bool
    let phase: String
    let step: String?
    let startedAt: String?
    let acquiredAt: String?
    let steps: [[String]]?
    enum CodingKeys: String, CodingKey {
        case pid, running, phase, step, steps
        case startedAt = "started_at"
        case acquiredAt = "acquired_at"
    }
}

struct ProductCancellationReceipt: Decodable, Sendable {
    let cancelled: ProductCancellation
}

struct ProductCancellation: Decodable, Sendable {
    let runningAfterSignal: Bool?
    enum CodingKeys: String, CodingKey {
        case runningAfterSignal = "running_after_signal"
    }
}

struct ProductInstallDecision: Decodable, Sendable {
    let action: String
    let reason: String
    let sourceRevision: String?
    enum CodingKeys: String, CodingKey {
        case action, reason
        case sourceRevision = "source_revision"
    }
}

struct ProductCargoCache: Decodable, Sendable {
    let target: String
    let reused: Bool
    let lockfileSha256: String
    let sourceRevision: String
    enum CodingKeys: String, CodingKey {
        case target, reused
        case lockfileSha256 = "lockfile_sha256"
        case sourceRevision = "source_revision"
    }
}

struct ProductDependencyResult: Decodable, Identifiable, Sendable {
    let product: String
    let surface: String
    let installDecision: ProductInstallDecision?
    let reason: String?
    let alternative: String?
    var id: String { "\(product)/\(surface)" }
    enum CodingKeys: String, CodingKey {
        case product, surface, reason, alternative
        case installDecision = "install_decision"
    }
}

@MainActor
final class ProductsStore: ObservableObject {
    @Published private(set) var products: [ProductCatalogEntry] = []
    @Published private(set) var isRefreshing = false
    @Published private(set) var failure: String?
    @Published private(set) var mutation: WisentMutationOutcome = .idle
    @Published private(set) var states: [String: ProductLifecycleState] = [:]
    @Published private(set) var locks: [ProductLock] = []
    @Published private(set) var lockFailure: String?
    @Published private(set) var plans: [ProductPlanKey: ProductPrerequisitePlan] = [:]
    @Published private(set) var planFailures: [ProductPlanKey: String] = [:]

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) { self.cli = cli }

    static func catalogArguments() -> [String] { ["product", "catalog", "--json"] }
    static func lifecycleArguments(_ verb: String, product: String, surface: String, host: String?,
                                   releaseVersion: String? = nil, sourceCommit: String? = nil,
                                   withPrerequisites: Bool = false) -> [String] {
        var values = ["product", verb, product, "--surface", surface]
        if let host, !host.isEmpty { values += ["--host", host] }
        if let releaseVersion { values += ["--release-version", releaseVersion] }
        if let sourceCommit { values += ["--source-commit", sourceCommit] }
        if withPrerequisites { values.append("--with-prerequisites") }
        values.append("--json")
        return values
    }

    func refresh() async {
        guard !isRefreshing else { return }
        isRefreshing = true
        defer { isRefreshing = false }
        do {
            let catalog = try await cli.json(ProductCatalogEnvelope.self, arguments: Self.catalogArguments())
            products = catalog.products
            failure = nil
        } catch {
            failure = Self.message(error)
        }
        await refreshLocks()
    }

    func refreshLocks() async {
        do {
            let report = try await cli.json(ProductLocksEnvelope.self,
                                            arguments: ["product", "locks", "--json"])
            locks = report.locks
            lockFailure = nil
        } catch {
            lockFailure = Self.message(error)
        }
    }

    func plan(product: String, surface: String, host: String?,
              releaseVersion: String? = nil, sourceCommit: String? = nil) async {
        let name = ProductPlanKey(product: product, surface: surface, host: host,
                                  releaseVersion: releaseVersion, sourceCommit: sourceCommit)
        do {
            let report = try await cli.json(ProductPrerequisitePlan.self,
                arguments: Self.lifecycleArguments("install", product: product, surface: surface,
                                                   host: host, releaseVersion: releaseVersion,
                                                   sourceCommit: sourceCommit) + ["--plan"],
                destination: surface == "service" ? .selected : .localDevice)
            plans[name] = report
            planFailures.removeValue(forKey: name)
        } catch {
            plans.removeValue(forKey: name)
            planFailures[name] = Self.message(error)
        }
    }

    func status(product: String, surface: String, host: String?) async {
        do {
            let state = try await cli.json(
                ProductLifecycleState.self,
                arguments: Self.lifecycleArguments("status", product: product, surface: surface, host: host),
                destination: surface == "service" ? .selected : .localDevice
            )
            states[key(product, surface)] = state
        } catch {
            mutation = .failed(Self.message(error))
        }
    }

    func mutate(_ verb: String, product: String, surface: String, host: String?,
                releaseVersion: String? = nil, sourceCommit: String? = nil,
                withPrerequisites: Bool = false) async {
        guard !mutation.isWorking else { return }
        mutation = .working("\(verb.capitalized) \(product) \(surface)")
        do {
            if verb == "cancel" {
                let receipt = try await cli.json(
                    ProductCancellationReceipt.self,
                    arguments: Self.lifecycleArguments(verb, product: product, surface: surface, host: host),
                    confirmsMutation: true,
                    destination: surface == "service" ? .selected : .localDevice
                )
                mutation = .succeeded(receipt.cancelled.runningAfterSignal == true
                    ? "\(product)/\(surface): termination requested; process still running at read-back"
                    : "\(product)/\(surface): cancellation signalled; reading current state")
                await status(product: product, surface: surface, host: host)
                await refreshLocks()
                return
            }
            let state = try await cli.json(
                ProductLifecycleState.self,
                arguments: Self.lifecycleArguments(verb, product: product, surface: surface, host: host,
                                                   releaseVersion: releaseVersion, sourceCommit: sourceCommit,
                                                   withPrerequisites: withPrerequisites),
                confirmsMutation: true,
                destination: surface == "service" ? .selected : .localDevice
            )
            states[key(product, surface)] = state
            mutation = .succeeded("\(product) \(surface): \(state.status)")
            await refreshLocks()
            let planned = ProductPlanKey(product: product, surface: surface, host: host,
                                         releaseVersion: releaseVersion, sourceCommit: sourceCommit)
            if plans[planned] != nil {
                await plan(product: product, surface: surface, host: host,
                           releaseVersion: releaseVersion, sourceCommit: sourceCommit)
            }
        } catch {
            mutation = .failed(Self.message(error))
        }
    }

    func state(product: String, surface: String) -> ProductLifecycleState? {
        states[key(product, surface)]
    }

    func clearMutation() { mutation = .idle }

    private func key(_ product: String, _ surface: String) -> String { "\(product)/\(surface)" }
    private static func message(_ error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
    }
}
