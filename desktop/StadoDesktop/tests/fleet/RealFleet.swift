import Foundation
@testable import Stado

/// One real Stado deployment in the package's ignored build directory.
/// Command receipts and persisted state survive for inspection after the API stops.
@MainActor
final class RealFleet {
    private let binary: URL
    private let root: URL
    private let home: URL
    private let log: URL
    private var api: Process?
    private static let readyPrefix = "[dashboard] listening on "
    /// registry-v2 is the only document shape `stado registry push` accepts
    /// and the CLI refuses any other version: the product's own constant.
    private static let registryDocumentVersion = 2

    init() throws {
        binary = try Self.resolveBinary()
        let evidence = ProcessInfo.processInfo.environment["STADO_EXPANSION_EVIDENCE_DIR"]
            .map { URL(fileURLWithPath: $0) }
            ?? URL(fileURLWithPath: #filePath)
                .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
                .appending(path: ".build/fleet-runs")
        root = evidence.appending(path: "desktop-\(UUID().uuidString)")
        home = root.appending(path: "home")
        log = root.appending(path: "operator-api.stderr")
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        if let source = ProcessInfo.processInfo.environment["WISENT_SOURCE_COMMIT"] {
            guard source.count == 40, source.allSatisfy(\.isHexDigit),
                  let digest = ProcessInfo.processInfo.environment["WISENT_SOURCE_SHA256"] else {
                throw RealFleetFailure("release source revision or archive digest is missing")
            }
            try source.write(to: root.appending(path: "source-revision.txt"), atomically: true, encoding: .utf8)
            try digest.write(to: root.appending(path: "source.sha256"), atomically: true, encoding: .utf8)
            return
        }
        let revision = Process()
        revision.executableURL = URL(fileURLWithPath: "/usr/bin/git")
        revision.arguments = ["-C", root.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().path, "rev-parse", "HEAD"]
        let revisionFile = root.appending(path: "source-revision.txt")
        FileManager.default.createFile(atPath: revisionFile.path, contents: nil)
        let revisionOutput = try FileHandle(forWritingTo: revisionFile)
        defer { try? revisionOutput.close() }
        revision.standardOutput = revisionOutput
        try revision.run()
        revision.waitUntilExit()
        guard revision.terminationStatus == 0 else { throw RealFleetFailure("could not retain source revision") }
    }

    /// The store, pointed at the operator API of a deployment holding one
    /// registered machine and no fleets, seeded by `stado registry push`.
    func store() async throws -> FleetGroupStore {
        let store = FleetGroupStore()
        store.configureEndpoint(try await deployment())
        return store
    }

    /// The control store the capability operations send through, on the
    /// same deployment.
    func control() async throws -> FleetControlStore {
        let store = FleetControlStore()
        store.configureEndpoint(try await deployment())
        return store
    }

    private func deployment() async throws -> String {
        let seed: [String: Any] = [
            "schema_version": Self.registryDocumentVersion,
            "targets": [[
                "name": "w1",
                "kind": "local",
                "ssh": "u@10.0.0.1",
                "release_platform": "linux-amd64",
                "hostnames": ["w1.local"],
            ]],
            "coordinators": [],
        ]
        try cli(["registry", "push", "-"], input: JSONSerialization.data(withJSONObject: seed))
        try cli(["config", "init"], configured: true)
        try cli(["config", "set", "storage.backend", "local"], configured: true)
        try cli(["config", "set", "storage.local.path", root.path], configured: true)
        return try await startOperatorAPI()
    }

    /// Run the real CLI here; a nonzero exit throws with its own complaint.
    func cli(_ arguments: [String], input: Data? = nil, configured: Bool = false) throws {
        let process = Process()
        process.executableURL = binary
        process.arguments = arguments
        process.environment = environment(configured: configured)
        let receipt = root.appending(path: "command-\(UUID().uuidString)")
        try JSONSerialization.data(withJSONObject: ["binary": binary.path, "arguments": arguments])
            .write(to: receipt.appendingPathExtension("json"))
        let outputFile = receipt.appendingPathExtension("stdout")
        let errorFile = receipt.appendingPathExtension("stderr")
        FileManager.default.createFile(atPath: outputFile.path, contents: nil)
        FileManager.default.createFile(atPath: errorFile.path, contents: nil)
        let output = try FileHandle(forWritingTo: outputFile)
        let errors = try FileHandle(forWritingTo: errorFile)
        defer { try? output.close(); try? errors.close() }
        let stdin = Pipe()
        process.standardOutput = output
        process.standardError = errors
        process.standardInput = input == nil ? FileHandle.nullDevice : stdin
        try process.run()
        if let input {
            stdin.fileHandleForWriting.write(input)
            try stdin.fileHandleForWriting.close()
        }
        if let input { try input.write(to: receipt.appendingPathExtension("stdin")) }
        process.waitUntilExit()
        try String(process.terminationStatus).write(to: receipt.appendingPathExtension("exit"), atomically: true, encoding: .utf8)
        let complained = try Data(contentsOf: errorFile)
        guard process.terminationStatus == 0 else {
            throw RealFleetFailure(
                "stado \(arguments.joined(separator: " ")) exited \(process.terminationStatus): "
                    + String(decoding: complained, as: UTF8.self)
            )
        }
    }

    func document() throws -> [String: Any] {
        let data = try Data(contentsOf: root.appending(path: "registry.json"))
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw RealFleetFailure("the registry object on disk is not a JSON document")
        }
        return object
    }

    func expansionPlan(id: String) throws -> [String: Any] {
        guard UUID(uuidString: id) != nil else { throw RealFleetFailure("plan id is not a UUID") }
        let data = try Data(contentsOf: root.appending(path: "state/fleet/expansion/plans/\(id).json"))
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw RealFleetFailure("persisted expansion plan is not a JSON document")
        }
        return object
    }

    func recordMissingMacDemand() throws {
        let plan = root.appending(path: "gui-plan.json")
        try JSONSerialization.data(withJSONObject: [
            "schema": "wisent.gui-automation-plan.v1", "operation": "enable",
        ]).write(to: plan)
        do {
            try cli(["workload", "run", "gui-automation", "--plan", plan.path])
        } catch let error as RealFleetFailure {
            guard error.errorDescription?.contains("the fleet declares no gui-automation") == true else { throw error }
            return
        }
        throw RealFleetFailure("the isolated Linux-only fleet unexpectedly accepted a macOS workload")
    }

    func declaredFleets() throws -> [[String: String]] {
        let declared = try document()["fleets"] as? [[String: Any]] ?? []
        return declared.map { row in row.compactMapValues { $0 as? String } }
    }

    func fleetOfRegisteredTarget() throws -> String? {
        (try document()["targets"] as? [[String: Any]])?.first?["fleet"] as? String
    }

    func removeRegistryDocument() throws {
        try FileManager.default.removeItem(at: root.appending(path: "registry.json"))
    }

    /// A command's sentence, without the lines that follow it.
    static func sentence(of message: String) -> String {
        String(message.split(separator: "\n", omittingEmptySubsequences: false).first ?? "")
    }

    func stop() {
        if let api, api.isRunning { api.terminate(); api.waitUntilExit() }
        api = nil
        print("Retained real fleet evidence: \(root.path)")
    }

    /// Start the product's own operator API and wait for the line it prints
    /// once its socket accepts; port 0 lets the kernel choose the port.
    private func startOperatorAPI() async throws -> String {
        FileManager.default.createFile(atPath: log.path, contents: nil)
        let process = Process()
        process.executableURL = binary
        process.arguments = ["dashboard", "--bind", "127.0.0.1", "--port", "0"]
        process.environment = environment(configured: true)
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        process.standardError = try FileHandle(forWritingTo: log)
        try process.run()
        api = process
        // It answers in milliseconds; this patience is for a loaded machine.
        let deadline = Date().addingTimeInterval(60)
        while Date() < deadline {
            let printed = (try? String(contentsOf: log, encoding: .utf8)) ?? ""
            let ready = printed.split(separator: "\n").first { $0.hasPrefix(Self.readyPrefix) }
            if let ready { return String(ready.dropFirst(Self.readyPrefix.count)) }
            guard process.isRunning else {
                throw RealFleetFailure("the operator API exited before it listened: \(printed)")
            }
            try await Task.sleep(for: .milliseconds(25))
        }
        throw RealFleetFailure("the operator API never reported a listening socket")
    }

    private func environment(configured: Bool) -> [String: String] {
        var environment = [
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
            "HOME": home.path,
            "TMPDIR": root.path,
            "NO_COLOR": "1",
        ]
        if configured {
            environment["STADO_CONFIG"] = home.appending(path: ".stado/config.json").path
        } else {
            environment["STADO_CONFIG"] = root.appending(path: "no-such-config.json").path
            environment["WC_STORAGE_BACKEND"] = "local"
            environment["WC_LOCAL_STORAGE_PATH"] = root.path
        }
        return environment
    }

    /// `STADO_BIN`, then `PATH`, then `~/.stado/bin/stado`; with none of them
    /// these tests fail by name rather than skipping.
    private static func resolveBinary() throws -> URL {
        let manager = FileManager.default
        if let configured = ProcessInfo.processInfo.environment["STADO_BIN"], !configured.isEmpty {
            guard manager.isExecutableFile(atPath: configured) else {
                throw RealFleetFailure(
                    "STADO_BIN=\(configured) is not an executable stado binary; build one with `cargo build --release -p stado` or unset STADO_BIN to use the installed CLI."
                )
            }
            return URL(fileURLWithPath: configured)
        }
        for directory in (ProcessInfo.processInfo.environment["PATH"] ?? "").split(separator: ":") {
            let candidate = URL(fileURLWithPath: String(directory)).appending(path: "stado")
            if manager.isExecutableFile(atPath: candidate.path) { return candidate }
        }
        let installed = URL(fileURLWithPath: NSHomeDirectory()).appending(path: ".stado/bin/stado")
        guard manager.isExecutableFile(atPath: installed.path) else {
            throw RealFleetFailure(
                "no stado binary to test against: STADO_BIN is unset, none is on PATH, and ~/.stado/bin/stado is absent; install one with `cargo install --path stado-rs` or point STADO_BIN at a build."
            )
        }
        return installed
    }
}

struct RealFleetFailure: LocalizedError {
    let errorDescription: String?
    init(_ sentence: String) { errorDescription = sentence }
}
