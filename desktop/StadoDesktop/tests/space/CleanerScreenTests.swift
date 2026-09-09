import CryptoKit
import Darwin
import Foundation
import XCTest
@testable import Stado

@MainActor
final class CleanerScreenTests: XCTestCase {
    func testDesktopPolicyWritesReachTheRegistryAndKeepRefusalEvidence() async throws {
        let host = try NativeSpaceHost()
        defer { host.stop() }
        try await host.waitUntilListening()
        let fleet = FleetControlStore()
        fleet.configureEndpoint(host.endpoint)
        let store = HostCleanersStore()
        await store.load(host: host.name, fleet: fleet)
        XCTAssertNil(store.problem)
        XCTAssertEqual(store.listing?.target, host.name)
        let root = host.home.appendingPathComponent("release-data").path
        try FileManager.default.createDirectory(atPath: root, withIntermediateDirectories: true)
        let saved = await store.declare(host: host.name, cleaner: "release_store",
            fields: ["--root", root, "--keep-newest", "2", "--min-age-seconds", "60"], fleet: fleet)
        XCTAssertTrue(saved, store.problem ?? "no saved receipt")
        let first = try host.cleaner("release_store")
        XCTAssertEqual(first["root"] as? String, root)
        XCTAssertEqual(first["keep_newest"] as? Int, 2)
        XCTAssertEqual(first["min_age_seconds"] as? Int, 60)
        let edited = await store.declare(host: host.name, cleaner: "release_store",
            fields: ["--keep-newest", "3"], fleet: fleet)
        XCTAssertTrue(edited, store.problem ?? "edit did not complete")
        let second = try host.cleaner("release_store")
        XCTAssertEqual(second["root"] as? String, root)
        XCTAssertEqual(second["min_age_seconds"] as? Int, 60)
        XCTAssertEqual(second["keep_newest"] as? Int, 3)
        let unchanged = try Data(contentsOf: host.registry)
        let refused = await store.declare(host: host.name, cleaner: "release_store",
            fields: ["--keep-newest", "0"], fleet: fleet)
        XCTAssertFalse(refused)
        XCTAssertEqual(try Data(contentsOf: host.registry), unchanged)
        XCTAssertFalse(try XCTUnwrap(store.receipt).ok)
        XCTAssertNotNil(store.problem)
        try host.retain(store.receipt, named: "refusal.json")
        await store.withdraw(host: host.name, cleaner: "release_store", fleet: fleet)
        XCTAssertNil(store.problem)
        XCTAssertNil(try host.cleaners()["release_store"])
        try host.retain(store.receipt, named: "withdraw.json")
        await store.load(host: "not-a-registered-host", fleet: fleet)
        XCTAssertNil(store.listing, "an error on a new host must not show the previous host's data")
        XCTAssertNotNil(store.problem)
    }
}

/// An actual Stado listener and installed binary, with a separate home and
/// local registry. No provider, HTTP server, executable, or response is mocked.
private final class NativeSpaceHost {
    let name = "desktop-space-test"
    let root: URL
    let home: URL
    let registry: URL
    let endpoint: String
    private let process = Process()
    private let stdout: FileHandle
    private let stderr: FileHandle

    init() throws {
        let package = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let repo = package.deletingLastPathComponent().deletingLastPathComponent()
        let binary = ProcessInfo.processInfo.environment["STADO_BIN"].map { URL(fileURLWithPath: $0) }
            ?? repo.appendingPathComponent("stado-rs/target/debug/stado")
        guard FileManager.default.isExecutableFile(atPath: binary.path) else {
            throw NSError(domain: "SpaceTests", code: 1,
                userInfo: [NSLocalizedDescriptionKey: "Build the real Stado binary before running the Desktop space flow: \(binary.path)"])
        }
        let revision = try XCTUnwrap(ProcessInfo.processInfo.environment["STADO_SOURCE_REVISION"],
            "Set STADO_SOURCE_REVISION to the exact compiled product revision")
        root = package.appendingPathComponent(".wisent-output/space-native/\(UUID().uuidString)")
        home = root.appendingPathComponent("home")
        let storage = root.appendingPathComponent("storage")
        registry = storage.appendingPathComponent("registry.json")
        let bin = home.appendingPathComponent(".stado/bin")
        let temporary = root.appendingPathComponent("temporary")
        for directory in [home, storage, bin, temporary] {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        }
        try FileManager.default.linkItem(at: binary, to: bin.appendingPathComponent("stado"))
        var document: [String: Any] = [:]
        document["schema_version"] = 2
        document["targets"] = [["name": name, "kind": "local", "ssh": NSNull(),
            "release_platform": "darwin-arm64", "hostnames": [ProcessInfo.processInfo.hostName],
            "services": []]]
        document["coordinators"] = []
        try JSONSerialization.data(withJSONObject: document, options: [.prettyPrinted, .sortedKeys]).write(to: registry)
        let config = root.appendingPathComponent("config.json")
        try Data("{}\n".utf8).write(to: config)
        let port = try Self.availablePort()
        endpoint = "http://127.0.0.1:\(port)"
        let out = root.appendingPathComponent("server.stdout")
        let err = root.appendingPathComponent("server.stderr")
        _ = FileManager.default.createFile(atPath: out.path, contents: nil)
        _ = FileManager.default.createFile(atPath: err.path, contents: nil)
        stdout = try FileHandle(forWritingTo: out)
        stderr = try FileHandle(forWritingTo: err)
        let digest = SHA256.hash(data: try Data(contentsOf: binary, options: .mappedIfSafe))
            .map { String(format: "%02x", $0) }.joined()
        let identity = "binary=\(binary.path)\nsha256=\(digest)\nrevision=\(revision)\n"
        try Data(identity.utf8).write(to: root.appendingPathComponent("source.txt"))
        process.executableURL = bin.appendingPathComponent("stado")
        process.arguments = ["dashboard", "--bind", "127.0.0.1", "--port", String(port)]
        process.environment = ["HOME": home.path, "TMPDIR": temporary.path,
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "STADO_CONFIG": config.path,
            "WC_STORAGE_BACKEND": "local", "WC_LOCAL_STORAGE_PATH": storage.path,
            "WC_PROVIDERS": "local", "NO_COLOR": "1"]
        process.standardOutput = stdout
        process.standardError = stderr
        try process.run()
        print("Space native API evidence: \(root.path)")
    }

    func waitUntilListening() async throws {
        let deadline = Date().addingTimeInterval(30)
        while Date() < deadline, process.isRunning {
            if let (_, response) = try? await URLSession.shared.data(from: URL(string: endpoint + "/healthz")!),
               response is HTTPURLResponse { return }
            try await Task.sleep(for: .milliseconds(100))
        }
        throw NSError(domain: "SpaceTests", code: 2,
            userInfo: [NSLocalizedDescriptionKey: "The real Stado API did not start; retained logs: \(root.path)"])
    }

    func cleaners() throws -> [String: Any] {
        let document = try JSONSerialization.jsonObject(with: Data(contentsOf: registry)) as! [String: Any]
        let target = (document["targets"] as! [[String: Any]])[0]
        return (target["disk_cleanup"] as? [String: Any])?["cleaners"] as? [String: Any] ?? [:]
    }

    func cleaner(_ name: String) throws -> [String: Any] {
        try XCTUnwrap(try cleaners()[name] as? [String: Any])
    }

    func retain(_ receipt: OperatorCommandResult?, named name: String) throws {
        let receipt = try XCTUnwrap(receipt)
        let document: [String: Any] = ["ok": receipt.ok, "exit_code": receipt.exitCode.map { $0 as Any } ?? NSNull(),
            "args": receipt.arguments, "stdout": receipt.standardOutput, "stderr": receipt.standardError]
        try JSONSerialization.data(withJSONObject: document, options: .prettyPrinted)
            .write(to: root.appendingPathComponent(name))
    }

    func stop() {
        if process.isRunning { process.terminate(); process.waitUntilExit() }
        try? stdout.close()
        try? stderr.close()
    }

    private static func availablePort() throws -> UInt16 {
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
