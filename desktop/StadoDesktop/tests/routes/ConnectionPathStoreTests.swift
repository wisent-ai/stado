import CryptoKit
import WisentDesignSystem
import XCTest
@testable import Stado

/// The Hosts inspector's host-control routes, driven through the real product
/// binary against an isolated registry. Nothing is mocked and the operator's
/// canonical registry is never addressed.
@MainActor
final class ConnectionPathStoreTests: XCTestCase {
    private static let nebula = "operator@routes-nebula.example"

    func testDeclaringARouteWritesItAndRemovingItTakesItBack() async throws {
        let fixture = try RegistryFixture()
        let store = HostConnectionPathStore(cli: StadoCLI(executable: fixture.binary.path))

        let declared = await store.set(host: RegistryFixture.host, name: "nebula",
            destination: Self.nebula, priority: 1)
        XCTAssertTrue(declared, store.mutation.message ?? "no mutation reported")
        let receipt = try XCTUnwrap(store.mutation.message)
        XCTAssertTrue(receipt.contains("now reaches nebula at \(Self.nebula)"), receipt)
        XCTAssertTrue(try fixture.document().contains(Self.nebula),
            "the registry document does not carry the declared destination")
        XCTAssertEqual(try fixture.declaredRoutes(), [
            ["name": "primary", "destination": RegistryFixture.preferred, "preferred": "true"],
            ["name": "nebula", "destination": Self.nebula, "preferred": "false"],
        ])

        let removed = await store.remove(host: RegistryFixture.host, name: "nebula")
        try fixture.retain(store.mutation, named: "removed.txt")
        XCTAssertTrue(removed, store.mutation.message ?? "no mutation reported")
        let retired = try XCTUnwrap(store.mutation.message)
        XCTAssertTrue(retired.contains("alternate route is removed"), retired)
        XCTAssertFalse(try fixture.document().contains(Self.nebula),
            "the removed destination is still in the registry document")
        XCTAssertEqual(try fixture.declaredRoutes(), [
            ["name": "primary", "destination": RegistryFixture.preferred, "preferred": "true"],
        ])
    }

    func testADuplicateHostIdentityIsRefusedAndTheDocumentIsUntouched() async throws {
        let fixture = try RegistryFixture()
        let store = HostConnectionPathStore(cli: StadoCLI(executable: fixture.binary.path))
        let first = await store.set(host: RegistryFixture.host, name: "nebula",
            destination: Self.nebula, priority: 1)
        XCTAssertTrue(first, store.mutation.message ?? "no mutation reported")
        let before = try Data(contentsOf: fixture.registry)

        let accepted = await store.set(host: RegistryFixture.host, name: "third",
            destination: RegistryFixture.preferred, priority: nil)
        try fixture.retain(store.mutation, named: "duplicate-refusal.txt")
        XCTAssertFalse(accepted, "a duplicate host identity was accepted")
        let refusal = try XCTUnwrap(store.mutation.message)
        XCTAssertTrue(refusal.contains("host identity 'routes-preferred.example' is already declared"),
            refusal)
        XCTAssertEqual(try Data(contentsOf: fixture.registry), before,
            "a refused route change rewrote the registry document")
    }
}

/// What this fixture refuses, in its own words.
private struct RegistryFixtureRefusal: LocalizedError {
    let errorDescription: String?
    init(_ sentence: String) { errorDescription = sentence }
}

/// One isolated local registry the real binary reads and writes. The store
/// runs the CLI as a child process, so the store's own invocation is what
/// changes this document.
@MainActor
private final class RegistryFixture {
    static let host = "desktop-routes-test"
    static let preferred = "operator@routes-preferred.example"
    /// The only registry document version the product accepts.
    private static let registryDocumentVersion = 2
    /// `setenv` replaces an existing value when this is nonzero.
    private static let replaceEnvironmentValue: Int32 = 1

    let binary: URL
    let root: URL
    let registry: URL

    init() throws {
        let package = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let repo = package.deletingLastPathComponent().deletingLastPathComponent()
        let declared = ProcessInfo.processInfo.environment["STADO_BIN"]
        binary = declared.map { URL(fileURLWithPath: $0) }
            ?? repo.appendingPathComponent("stado-rs/target/debug/stado")
        guard FileManager.default.isExecutableFile(atPath: binary.path) else {
            throw RegistryFixtureRefusal(
                "Build the real Stado binary before running the Desktop routes flow: \(binary.path)")
        }
        let revision = try XCTUnwrap(ProcessInfo.processInfo.environment["STADO_SOURCE_REVISION"],
            "Set STADO_SOURCE_REVISION to the exact compiled product revision")
        root = package.appendingPathComponent(".wisent-output/routes-native/\(UUID().uuidString)")
        let storage = root.appendingPathComponent("storage")
        registry = storage.appendingPathComponent("registry.json")
        try FileManager.default.createDirectory(at: storage, withIntermediateDirectories: true)
        let document: [String: Any] = [
            "schema_version": Self.registryDocumentVersion,
            "targets": [[
                "name": Self.host,
                "kind": "local",
                "ssh": Self.preferred,
                "release_platform": "darwin-arm64",
                "hostnames": ["\(Self.host).example"],
            ]],
            "coordinators": [],
        ]
        try JSONSerialization.data(withJSONObject: document, options: [.prettyPrinted, .sortedKeys])
            .write(to: registry)
        let digest = SHA256.hash(data: try Data(contentsOf: binary, options: .mappedIfSafe))
            .map { String(format: "%02x", $0) }.joined()
        try Data("binary=\(binary.path)\nsha256=\(digest)\nrevision=\(revision)\n".utf8)
            .write(to: root.appendingPathComponent("source.txt"))
        // The store spawns the CLI without an environment of its own, so the
        // child reads these: one isolated local store and no configuration.
        setenv("WC_STORAGE_BACKEND", "local", Self.replaceEnvironmentValue)
        setenv("WC_LOCAL_STORAGE_PATH", storage.path, Self.replaceEnvironmentValue)
        setenv("STADO_CONFIG", root.appendingPathComponent("no-such-config.json").path,
            Self.replaceEnvironmentValue)
        setenv("NO_COLOR", "1", Self.replaceEnvironmentValue)
        for inherited in ["COMPUTE_API_KEY", "COMPUTE_API_URL", "WC_PROFILES_DIR", "STADO_API_URL"] {
            unsetenv(inherited)
        }
        print("Routes registry evidence: \(root.path)")
    }

    /// The registry document exactly as the product left it on disk.
    func document() throws -> String {
        try String(contentsOf: registry, encoding: .utf8)
    }

    /// The routes the product reports for this host, in the order its channel
    /// tries them, read through `registry host path list --json`.
    func declaredRoutes() throws -> [[String: String]] {
        let process = Process()
        let output = Pipe()
        process.executableURL = binary
        process.arguments = ["registry", "host", "path", "list", Self.host, "--json"]
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        try process.run()
        let printed = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0,
              let listed = try JSONSerialization.jsonObject(with: printed) as? [String: Any],
              let connections = listed["connections"] as? [[String: Any]]
        else { throw RegistryFixtureRefusal("the product did not report this host's routes") }
        try printed.write(to: root.appendingPathComponent("routes.json"))
        return connections.map { connection in
            [
                "name": connection["name"] as? String ?? "",
                "destination": connection["destination"] as? String ?? "",
                "preferred": String(connection["preferred"] as? Bool ?? false),
            ]
        }
    }

    func retain(_ outcome: WisentMutationOutcome, named name: String) throws {
        let reported = outcome.message ?? "no mutation reported"
        try Data("\(reported)\n".utf8).write(to: root.appendingPathComponent(name))
    }
}
