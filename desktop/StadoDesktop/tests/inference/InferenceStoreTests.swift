import CryptoKit
import XCTest
@testable import Stado

/// The Inference screen, driven through the real product binary against an
/// isolated registry. The operator asked which model the chat used and the
/// desktop could not say; this is the store that now says it.
@MainActor
final class InferenceStoreTests: XCTestCase {
    func testTheScreenNamesTheModelBehindEveryAlias() async throws {
        let fixture = try InferenceRegistryFixture()
        let store = InferenceStore(cli: StadoCLI(executable: fixture.binary.path))

        await store.refresh()
        try fixture.retain(store, named: "screen.txt")

        XCTAssertNil(store.problem, store.problem ?? "")
        XCTAssertEqual(store.gatewayTarget, InferenceRegistryFixture.host)
        XCTAssertEqual(store.deployments.map(\.name), ["chat-primary"])
        let deployment = try XCTUnwrap(store.deployments.first)
        XCTAssertEqual(deployment.model.coordinate,
            "\(InferenceRegistryFixture.repository)@\(InferenceRegistryFixture.revision)")
        XCTAssertEqual(deployment.target, InferenceRegistryFixture.host)

        XCTAssertEqual(store.routes.map(\.alias), ["model-review", "wisent-backend"])
        let local = try XCTUnwrap(store.routes.first { $0.alias == "wisent-backend" })
        XCTAssertEqual(local.deployment?.name, "chat-primary")
        XCTAssertEqual(local.model,
            "\(InferenceRegistryFixture.repository)@\(InferenceRegistryFixture.revision)",
            "the alias that lands on a deployment must name that deployment's exact revision")
        let remote = try XCTUnwrap(store.routes.first { $0.alias == "model-review" })
        XCTAssertNil(remote.deployment)
        XCTAssertEqual(remote.model, "openai/gpt-4o-mini",
            "an alias that leaves the fleet names the remote model as the router does")

        // No host in this registry publishes a beacon, so the row must say so
        // by name rather than show a state it never read.
        let beacon = store.beacons["chat-primary"]
        let beaconProblem = store.beaconProblems["chat-primary"]
        XCTAssertTrue(beacon != nil || beaconProblem != nil,
            "the deployment row has neither a beacon nor a reason there is none")
        if let beacon {
            XCTAssertNotEqual(beacon.state, "running",
                "a host that never reported was shown as running: \(beacon)")
        }
    }
}

private struct InferenceFixtureRefusal: LocalizedError {
    let errorDescription: String?
    init(_ sentence: String) { errorDescription = sentence }
}

/// One isolated local registry declaring a GPU host, one deployment on it and
/// two routes. The store runs the CLI as a child process, so the product's own
/// list and status commands are what this document is read through.
@MainActor
private final class InferenceRegistryFixture {
    static let host = "desktop-inference-test"
    static let repository = "TheDrummer/Cydonia-24B-v4.3"
    static let revision = "db0426d39d4bd4a6d34fdc71db97569da68f55e1"
    /// The only registry document version the product accepts.
    private static let registryDocumentVersion = 2
    private static let hostVramGb = 96
    private static let endpointPort = 8001
    private static let contextLength = 32768
    private static let replaceEnvironmentValue: Int32 = 1

    let binary: URL
    let root: URL

    init() throws {
        let package = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let repo = package.deletingLastPathComponent().deletingLastPathComponent()
        let declared = ProcessInfo.processInfo.environment["STADO_BIN"]
        binary = declared.map { URL(fileURLWithPath: $0) }
            ?? repo.appendingPathComponent("stado-rs/target/debug/stado")
        guard FileManager.default.isExecutableFile(atPath: binary.path) else {
            throw InferenceFixtureRefusal(
                "Build the real Stado binary before running the Desktop inference flow: \(binary.path)")
        }
        let revision = try XCTUnwrap(ProcessInfo.processInfo.environment["STADO_SOURCE_REVISION"],
            "Set STADO_SOURCE_REVISION to the exact compiled product revision")
        root = package.appendingPathComponent(".wisent-output/inference-native/\(UUID().uuidString)")
        let storage = root.appendingPathComponent("storage")
        try FileManager.default.createDirectory(at: storage, withIntermediateDirectories: true)
        let document: [String: Any] = [
            "schema_version": Self.registryDocumentVersion,
            "targets": [[
                "name": Self.host,
                "kind": "local",
                "ssh": "operator@inference.example",
                "release_platform": "linux-amd64",
                "hostnames": ["\(Self.host).example"],
                "vram_gb": Self.hostVramGb,
            ]],
            "coordinators": [],
            "inference": [
                "gateway_target": Self.host,
                "deployments": [[
                    "name": "chat-primary",
                    "target": Self.host,
                    "desired_state": "running",
                    "engine": ["name": "vllm", "image": "vllm/vllm-openai@sha256:0000"],
                    "model": ["repository": Self.repository, "revision": Self.revision],
                    "resources": ["gpu_mode": "exclusive", "gpus": 1, "max_model_len": Self.contextLength],
                    "endpoint": [
                        "host": "100.64.0.1", "visibility": "tailscale",
                        "port": Self.endpointPort, "protocol": "openai-chat",
                    ],
                    "credential_item": "provider:local-openai",
                ]],
                "routes": ["wisent-backend": "chat-primary", "model-review": "openai/gpt-4o-mini"],
            ],
        ]
        try JSONSerialization.data(withJSONObject: document, options: [.prettyPrinted, .sortedKeys])
            .write(to: storage.appendingPathComponent("registry.json"))
        let digest = SHA256.hash(data: try Data(contentsOf: binary, options: .mappedIfSafe))
            .map { String(format: "%02x", $0) }.joined()
        try Data("binary=\(binary.path)\nsha256=\(digest)\nrevision=\(revision)\n".utf8)
            .write(to: root.appendingPathComponent("source.txt"))
        setenv("WC_STORAGE_BACKEND", "local", Self.replaceEnvironmentValue)
        setenv("WC_LOCAL_STORAGE_PATH", storage.path, Self.replaceEnvironmentValue)
        setenv("STADO_CONFIG", root.appendingPathComponent("no-such-config.json").path,
            Self.replaceEnvironmentValue)
        setenv("NO_COLOR", "1", Self.replaceEnvironmentValue)
        for inherited in ["COMPUTE_API_KEY", "COMPUTE_API_URL", "WC_PROFILES_DIR", "STADO_API_URL"] {
            unsetenv(inherited)
        }
        print("Inference registry evidence: \(root.path)")
    }

    /// What the screen would show, retained beside the registry it read.
    func retain(_ store: InferenceStore, named name: String) throws {
        var lines = ["problem=\(store.problem ?? "none")", "gateway=\(store.gatewayTarget ?? "none")"]
        for route in store.routes {
            lines.append("route \(route.alias) -> \(route.destination) = \(route.model)")
        }
        for deployment in store.deployments {
            let beacon = store.beacons[deployment.name].map { "\($0.state)" }
                ?? "beacon not read: \(store.beaconProblems[deployment.name] ?? "no reason")"
            lines.append("deployment \(deployment.name) \(deployment.model.coordinate) on \(deployment.target): \(beacon)")
        }
        try Data((lines.joined(separator: "\n") + "\n").utf8)
            .write(to: root.appendingPathComponent(name))
    }
}
