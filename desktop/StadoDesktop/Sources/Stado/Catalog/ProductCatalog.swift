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

    enum CodingKeys: String, CodingKey {
        case product, surface, status, host, release, readiness
        case sourceRevision = "source_revision"
        case installedAt = "installed_at"
        case installedPaths = "installed_paths"
    }
}

struct ProductReleaseReceipt: Decodable, Sendable {
    let coordinate: ProductReleaseCoordinate
}

struct ProductReleaseCoordinate: Decodable, Sendable {
    let version: String
    let sourceRevision: String
    enum CodingKeys: String, CodingKey {
        case version
        case sourceRevision = "source_revision"
    }
}

struct ProductReadiness: Decodable, Sendable {
    let ready: Bool
    let detail: String
}

@MainActor
final class ProductsStore: ObservableObject {
    @Published private(set) var products: [ProductCatalogEntry] = []
    @Published private(set) var isRefreshing = false
    @Published private(set) var failure: String?
    @Published private(set) var mutation: WisentMutationOutcome = .idle
    @Published private(set) var states: [String: ProductLifecycleState] = [:]

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) { self.cli = cli }

    static func catalogArguments() -> [String] { ["product", "catalog", "--json"] }
    static func lifecycleArguments(_ verb: String, product: String, surface: String, host: String?,
                                   releaseVersion: String? = nil, sourceCommit: String? = nil) -> [String] {
        var values = ["product", verb, product, "--surface", surface]
        if let host, !host.isEmpty { values += ["--host", host] }
        if let releaseVersion { values += ["--release-version", releaseVersion] }
        if let sourceCommit { values += ["--source-commit", sourceCommit] }
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
    }

    func status(product: String, surface: String, host: String?) async {
        do {
            let state = try await cli.json(
                ProductLifecycleState.self,
                arguments: Self.lifecycleArguments("status", product: product, surface: surface, host: host)
            )
            states[key(product, surface)] = state
        } catch {
            mutation = .failed(Self.message(error))
        }
    }

    func mutate(_ verb: String, product: String, surface: String, host: String?,
                releaseVersion: String? = nil, sourceCommit: String? = nil) async {
        guard !mutation.isWorking else { return }
        mutation = .working("\(verb.capitalized) \(product) \(surface)")
        do {
            let state = try await cli.json(
                ProductLifecycleState.self,
                arguments: Self.lifecycleArguments(verb, product: product, surface: surface, host: host,
                                                   releaseVersion: releaseVersion, sourceCommit: sourceCommit),
                timeoutSeconds: 900
            )
            states[key(product, surface)] = state
            mutation = .succeeded("\(product) \(surface): \(state.status)")
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
