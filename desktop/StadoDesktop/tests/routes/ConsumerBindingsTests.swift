import CryptoKit
import Darwin
import Foundation
import XCTest
@testable import Stado

/// The Routes screen's actual operations, sent to a real isolated Stado API.
@MainActor
final class ConsumerBindingsTests: XCTestCase {
    func testConsumerActionsPersistBindingsRefuseCollisionsAndRemoveBoth() async throws {
        let host = try ConsumerHost()
        defer { host.stop() }
        try await host.waitUntilListening()
        let fleet = FleetControlStore()
        fleet.configureEndpoint(host.endpoint)
        let store = NativeCapabilityStore()
        let add = try XCTUnwrap(NativeRouteOperations.all.first { $0.id == "consumer-add" })
        let client = "desktop-directory-client"
        let bind = "127.0.0.1:\(try ConsumerHost.availablePort())"
        var values = ["service": host.service, "consumer": client, "target": host.name,
            "bind": bind, "capability": "object-store\ninspection"]
        let request = try add.request(host: host.name, values: values, content: "")
        let accepted = await store.run(request, fleet: fleet, expectedSource: fleet.requestGeneration)
        try host.retain(store.receipt, named: "declared.json")
        XCTAssertTrue(accepted, store.problem ?? "The declaration was not accepted")
        let adapters = try host.adapters()
        XCTAssertTrue(adapters.contains { $0["consumer"] as? String == client && $0["bind"] as? String == bind })
        let consumer = try XCTUnwrap(try host.consumers()[client] as? [String: Any])
        XCTAssertEqual(consumer["capabilities"] as? [String], ["object-store", "inspection"])

        let before = try Data(contentsOf: host.registry)
        values["bind"] = host.apiBind
        let invalid = try add.request(host: host.name, values: values, content: "")
        let refused = await store.run(invalid, fleet: fleet, expectedSource: fleet.requestGeneration)
        try host.retain(store.receipt, named: "collision.json")
        XCTAssertFalse(refused, "The resolver API socket was also assigned to a consumer")
        XCTAssertTrue(store.problem?.contains("duplicate resolver bind") == true, store.problem ?? "No refusal")
        XCTAssertEqual(try Data(contentsOf: host.registry), before)

        let remove = try XCTUnwrap(NativeRouteOperations.all.first { $0.id == "consumer-rm" })
        let removal = try remove.request(host: host.name,
            values: ["service": host.service, "consumer": client], content: "")
        let removed = await store.run(removal, fleet: fleet, expectedSource: fleet.requestGeneration)
        try host.retain(store.receipt, named: "removed.json")
        XCTAssertTrue(removed, store.problem ?? "The removal was not accepted")
        XCTAssertNil(try host.consumers()[client])
        XCTAssertFalse(try host.adapters().contains { $0["consumer"] as? String == client })
        XCTAssertNotNil(try host.consumers()[host.existingConsumer])
    }
}

/// A private registry and normal loopback API, with receipts retained in this checkout.
@MainActor
private final class ConsumerHost {
    let name = "desktop-consumer-test"
    let service = "stado-object-api"
    let existingConsumer = "desktop-existing-client"
    let root: URL
    let registry: URL
    let endpoint: String
    let apiBind: String
    private let process = Process()
    private let output: FileHandle
    private let errors: FileHandle

    init() throws {
        let caller = ProcessInfo.processInfo.environment
        let binary = URL(fileURLWithPath: try XCTUnwrap(caller["STADO_BIN"], "Set STADO_BIN to the real product"))
        let revision = try XCTUnwrap(caller["STADO_SOURCE_REVISION"], "Set the exact compiled source revision")
        let package = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        root = package.appendingPathComponent(".build/test-results/consumers-\(UUID().uuidString)")
        let home = root.appendingPathComponent("home")
        let storage = root.appendingPathComponent("storage")
        let bin = home.appendingPathComponent(".stado/bin")
        let temporary = root.appendingPathComponent("temporary")
        registry = storage.appendingPathComponent("registry.json")
        for directory in [home, storage, bin, temporary] {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        }
        let executable = bin.appendingPathComponent("stado")
        try FileManager.default.linkItem(at: binary, to: executable)
        let port = try Self.availablePort()
        endpoint = "http://127.0.0.1:\(port)"
        apiBind = "127.0.0.1:\(try Self.availablePort())"
        let target: [String: Any] = ["name": name, "kind": "local", "ssh": "nobody@127.0.0.1",
            "release_platform": "darwin-arm64", "hostnames": [ProcessInfo.processInfo.hostName.lowercased()],
            "services": [["name": service, "unit": "", "label": "com.wisent.compute.service.\(service)",
                "path": "/Library/LaunchAgents/com.wisent.compute.service.\(service).plist",
                "kind": "launchd", "managed_since": "2026-08-01T00:00:00+00:00"]],
            "service_resolver": ["api_bind": apiBind, "adapters": [["service": service,
                "consumer": existingConsumer, "bind": "127.0.0.1:\(try Self.availablePort())"]]]]
        let directory: [String: Any] = ["authority": ["target": name, "command": executable.path],
            "generation": 1, "services": [service: ["managed_service": service, "active_host": name,
                "endpoints": [name: ["url": endpoint]],
                "consumers": [existingConsumer: ["capabilities": ["object-store"]]]]]]
        try JSONSerialization.data(withJSONObject: ["schema_version": 2, "targets": [target],
            "coordinators": [], "service_directory": directory]).write(to: registry)
        let config = root.appendingPathComponent("config.json")
        try JSONSerialization.data(withJSONObject: ["storage": ["backend": "local", "local": ["path": storage.path]]])
            .write(to: config)
        let out = root.appendingPathComponent("server.stdout")
        let err = root.appendingPathComponent("server.stderr")
        _ = FileManager.default.createFile(atPath: out.path, contents: nil)
        _ = FileManager.default.createFile(atPath: err.path, contents: nil)
        output = try FileHandle(forWritingTo: out)
        errors = try FileHandle(forWritingTo: err)
        let digest = SHA256.hash(data: try Data(contentsOf: binary, options: .mappedIfSafe))
            .map { String(format: "%02x", $0) }.joined()
        try Data("binary=\(binary.path)\nsha256=\(digest)\nrevision=\(revision)\n".utf8)
            .write(to: root.appendingPathComponent("source.txt"))
        process.executableURL = executable
        process.arguments = ["dashboard", "--bind", "127.0.0.1", "--port", String(port)]
        process.environment = ["HOME": home.path, "TMPDIR": temporary.path,
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "STADO_CONFIG": config.path,
            "WC_STORAGE_BACKEND": "local", "WC_LOCAL_STORAGE_PATH": storage.path,
            "WC_PROVIDERS": "local", "NO_COLOR": "1"]
        process.standardOutput = output
        process.standardError = errors
        try process.run()
        print("Consumer native API evidence: \(root.path)")
    }

    func waitUntilListening() async throws {
        let deadline = Date().addingTimeInterval(30)
        while Date() < deadline, process.isRunning {
            if let (_, response) = try? await URLSession.shared.data(from: URL(string: endpoint + "/healthz")!),
               let response = response as? HTTPURLResponse, response.statusCode == 200 { return }
            try await Task.sleep(for: .milliseconds(100))
        }
        throw NSError(domain: "ConsumerTests", code: 1,
            userInfo: [NSLocalizedDescriptionKey: "The real Stado API did not start; logs: \(root.path)"])
    }

    func document() throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: registry)) as? [String: Any])
    }

    func consumers() throws -> [String: Any] {
        let directory = try XCTUnwrap(try document()["service_directory"] as? [String: Any])
        let services = try XCTUnwrap(directory["services"] as? [String: Any])
        let route = try XCTUnwrap(services[service] as? [String: Any])
        return try XCTUnwrap(route["consumers"] as? [String: Any])
    }

    func adapters() throws -> [[String: Any]] {
        let targets = try XCTUnwrap(try document()["targets"] as? [[String: Any]])
        let target = try XCTUnwrap(targets.first { $0["name"] as? String == name })
        let resolver = try XCTUnwrap(target["service_resolver"] as? [String: Any])
        return try XCTUnwrap(resolver["adapters"] as? [[String: Any]])
    }

    func retain(_ receipt: OperatorCommandResult?, named name: String) throws {
        let receipt = try XCTUnwrap(receipt)
        let record: [String: Any] = ["ok": receipt.ok, "exit_code": receipt.exitCode.map { $0 as Any } ?? NSNull(),
            "args": receipt.arguments, "stdout": receipt.standardOutput, "stderr": receipt.standardError,
            "registry": try document()]
        try JSONSerialization.data(withJSONObject: record, options: [.prettyPrinted, .sortedKeys])
            .write(to: root.appendingPathComponent(name))
    }

    func stop() {
        if process.isRunning { process.terminate(); process.waitUntilExit() }
        try? output.close()
        try? errors.close()
    }

    static func availablePort() throws -> UInt16 {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        defer { close(fd) }
        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
        var size = socklen_t(MemoryLayout<sockaddr_in>.size)
        let result = withUnsafeMutablePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(fd, $0, size) == 0 ? getsockname(fd, $0, &size) : -1
            }
        }
        guard result == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        return UInt16(bigEndian: address.sin_port)
    }
}
