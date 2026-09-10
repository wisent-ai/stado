import CryptoKit
import XCTest
@testable import Stado

/// The interactive workload stream, driven through the real Swift client
/// against a real Stado API process and the installed Jeden runtime.
@MainActor
final class AttachmentTests: XCTestCase {
    func testAttachmentStreamsARealJedenSessionAndRecordsItsLedger() async throws {
        let host = try NativeWorkloadHost()
        try await host.waitUntilListening()
        let fleet = FleetControlStore()
        fleet.configureEndpoint(host.endpoint)
        let store = WorkloadAttachmentStore()

        await store.connect(kind: "jeden-session", target: host.name, workspace: "__home__",
            resume: "", fleet: fleet, expectedSource: fleet.requestGeneration)
        XCTAssertNil(store.problem, store.standardError)
        XCTAssertTrue(store.connected, store.status)
        await store.send(#"{"id":"initialize","method":"initialize"}"#)
        await store.send(
            #"{"id":"create","method":"session/new","params":{"cwd":"\#(host.home.path)"}}"#)
        await store.send(#"{"id":"shutdown","method":"shutdown"}"#)
        await store.finishInput()
        try await host.waitUntilFinished(store)
        try host.retain(named: "attached.txt", stdout: store.standardOutput,
            stderr: store.standardError, status: store.status, problem: store.problem)
        XCTAssertNil(store.problem, store.standardError)
        XCTAssertTrue(store.standardOutput.contains("jeden-rpc"),
            "the native stream carried no real Jeden readiness frame: \(store.standardOutput)")
        XCTAssertEqual(store.status, "Workload exited with status 0")

        let sessions = host.home.appendingPathComponent(".jeden/sessions")
        let ledgers = try FileManager.default.contentsOfDirectory(atPath: sessions.path)
        XCTAssertEqual(ledgers.count, 1, "the real attachment left \(ledgers.count) session ledgers")
        let ledger = sessions.appendingPathComponent(ledgers[0])
        let state = try Data(contentsOf: ledger.appendingPathComponent("state.json"))
        let document = try XCTUnwrap(try JSONSerialization.jsonObject(with: state) as? [String: Any])
        let recorded = URL(fileURLWithPath: try XCTUnwrap(document["cwd"] as? String))
        XCTAssertEqual(recorded.resolvingSymlinksInPath().path, host.home.resolvingSymlinksInPath().path)
        XCTAssertTrue(FileManager.default.fileExists(
            atPath: ledger.appendingPathComponent("transcript.jsonl").path))
    }

    func testAttachmentRefusesAnUndeclaredWorkloadAndLeavesNoSession() async throws {
        let host = try NativeWorkloadHost()
        try await host.waitUntilListening()
        let fleet = FleetControlStore()
        fleet.configureEndpoint(host.endpoint)
        let store = WorkloadAttachmentStore()
        await store.connect(kind: "no-such-workload", target: host.name, workspace: "__home__",
            resume: "", fleet: fleet, expectedSource: fleet.requestGeneration)
        try await host.waitUntilFinished(store)
        try host.retain(named: "undeclared-refusal.txt", stdout: store.standardOutput,
            stderr: store.standardError, status: store.status, problem: store.problem)
        let refusal = try XCTUnwrap(store.problem)
        XCTAssertTrue(refusal.contains("workload kind 'no-such-workload' is not declared"), refusal)
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: host.home.appendingPathComponent(".jeden/sessions").path))
    }
}

/// What this fixture refuses, in its own words.
private struct WorkloadHostRefusal: LocalizedError {
    let errorDescription: String?
    init(_ sentence: String) { errorDescription = sentence }
}

/// A real Stado listener with its own home, local registry and installed Jeden
/// binary. No provider, HTTP server, executable or response is mocked.
private final class NativeWorkloadHost {
    let name = "desktop-workload-test"
    let root: URL
    let home: URL
    let endpoint: String
    private let process = Process()
    private let stdout: FileHandle
    private let stderr: FileHandle

    init() throws {
        let package = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let repo = package.deletingLastPathComponent().deletingLastPathComponent()
        let declaredStado = ProcessInfo.processInfo.environment["STADO_BIN"]
        let binary = declaredStado.map { URL(fileURLWithPath: $0) }
            ?? repo.appendingPathComponent("stado-rs/target/debug/stado")
        guard FileManager.default.isExecutableFile(atPath: binary.path) else {
            throw WorkloadHostRefusal(
                "Build the real Stado binary before running the Desktop workload flow: \(binary.path)")
        }
        let declaredJeden = ProcessInfo.processInfo.environment["JEDEN_BIN"]
        let jeden = declaredJeden.map { URL(fileURLWithPath: $0) }
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".stado/bin/jeden")
        guard FileManager.default.isExecutableFile(atPath: jeden.path) else {
            throw WorkloadHostRefusal(
                "The real installed Jeden runtime is required for this flow: \(jeden.path)")
        }
        let revision = try XCTUnwrap(ProcessInfo.processInfo.environment["STADO_SOURCE_REVISION"],
            "Set STADO_SOURCE_REVISION to the exact compiled product revision")
        root = package.appendingPathComponent(".wisent-output/workload-native/\(UUID().uuidString)")
        home = root.appendingPathComponent("home")
        let storage = root.appendingPathComponent("storage")
        let bin = home.appendingPathComponent(".stado/bin")
        let temporary = root.appendingPathComponent("temporary")
        for directory in [home, storage, bin, temporary] {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        }
        try FileManager.default.createSymbolicLink(at: bin.appendingPathComponent("stado"),
            withDestinationURL: binary)
        try FileManager.default.createSymbolicLink(at: bin.appendingPathComponent("jeden"),
            withDestinationURL: jeden)
        var document: [String: Any] = [:]
        document["schema_version"] = 2
        document["targets"] = [["name": name, "kind": "local", "ssh": NSNull(),
            "release_platform": "darwin-arm64", "hostnames": [ProcessInfo.processInfo.hostName],
            "services": []]]
        document["coordinators"] = []
        try JSONSerialization.data(withJSONObject: document, options: [.prettyPrinted, .sortedKeys])
            .write(to: storage.appendingPathComponent("registry.json"))
        let config = root.appendingPathComponent("config.json")
        let profile: [String: Any] = ["storage": ["backend": "local", "local": ["path": storage.path]]]
        try JSONSerialization.data(withJSONObject: profile).write(to: config)
        let out = root.appendingPathComponent("server.stdout")
        let err = root.appendingPathComponent("server.stderr")
        _ = FileManager.default.createFile(atPath: out.path, contents: nil)
        _ = FileManager.default.createFile(atPath: err.path, contents: nil)
        stdout = try FileHandle(forWritingTo: out)
        stderr = try FileHandle(forWritingTo: err)
        let digest = SHA256.hash(data: try Data(contentsOf: binary, options: .mappedIfSafe))
            .map { String(format: "%02x", $0) }.joined()
        try Data("binary=\(binary.path)\nsha256=\(digest)\nrevision=\(revision)\njeden=\(jeden.path)\n".utf8)
            .write(to: root.appendingPathComponent("source.txt"))
        process.executableURL = bin.appendingPathComponent("stado")
        // The listener picks the port and prints the address it bound, so no
        // second process can take the port between selection and bind.
        process.arguments = ["dashboard", "--bind", "127.0.0.1", "--port", "0"]
        process.environment = ["HOME": home.path, "TMPDIR": temporary.path,
            "PATH": bin.path + ":/usr/bin:/bin:/usr/sbin:/sbin", "STADO_CONFIG": config.path,
            "WC_STORAGE_BACKEND": "local", "WC_LOCAL_STORAGE_PATH": storage.path,
            "WC_PROVIDERS": "local", "NO_COLOR": "1",
            "JEDEN_SESSION_ROOT": home.appendingPathComponent(".jeden/sessions").path]
        process.standardOutput = stdout
        process.standardError = stderr
        try process.run()
        endpoint = try Self.address(loggedIn: err, of: process, retained: root)
        print("Workload native API evidence: \(root.path)")
    }

    deinit {
        if process.isRunning { process.terminate() }
        try? stdout.close()
        try? stderr.close()
    }

    func waitUntilListening() async throws {
        let deadline = Date().addingTimeInterval(Self.readinessSeconds)
        while Date() < deadline, process.isRunning {
            if let (_, response) = try? await URLSession.shared.data(from: URL(string: endpoint + "/healthz")!),
               response is HTTPURLResponse { return }
            try await Task.sleep(for: .milliseconds(Self.pollMilliseconds))
        }
        throw WorkloadHostRefusal("The real Stado API did not answer; retained logs: \(root.path)")
    }

    /// Wait for the attached workload to reach its own terminal state.
    @MainActor
    func waitUntilFinished(_ store: WorkloadAttachmentStore) async throws {
        let deadline = Date().addingTimeInterval(Self.readinessSeconds)
        while Date() < deadline {
            if !store.active { return }
            try await Task.sleep(for: .milliseconds(Self.pollMilliseconds))
        }
        throw WorkloadHostRefusal(
            "The attached workload did not finish; retained evidence: \(root.path)")
    }

    func retain(named name: String, stdout: String, stderr: String, status: String,
                problem: String?) throws {
        let reported = problem ?? "no failure reported"
        let text = "status=\(status)\nproblem=\(reported)\n--- stdout ---\n\(stdout)\n--- stderr ---\n\(stderr)\n"
        try Data(text.utf8).write(to: root.appendingPathComponent(name))
    }

    /// Seconds this fixture waits for the listener, and its poll interval.
    private static let readinessSeconds: TimeInterval = 30
    private static let pollMilliseconds = 100

    /// The address the listener itself reported.
    private static func address(loggedIn log: URL, of process: Process, retained root: URL) throws -> String {
        let marker = "[dashboard] listening on "
        let deadline = Date().addingTimeInterval(readinessSeconds)
        while Date() < deadline, process.isRunning {
            let text = (try? String(contentsOf: log, encoding: .utf8)) ?? ""
            if let line = text.split(separator: "\n").first(where: { $0.contains(marker) }) {
                return String(line).replacingOccurrences(of: marker, with: "")
                    .trimmingCharacters(in: .whitespaces)
            }
            usleep(useconds_t(pollMilliseconds) * 1000)
        }
        throw WorkloadHostRefusal("The real Stado API never reported a bound address; retained logs: \(root.path)")
    }
}
