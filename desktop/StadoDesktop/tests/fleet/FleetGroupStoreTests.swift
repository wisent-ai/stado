import Foundation
import XCTest
@testable import Stado

/// `FleetGroupStore` against the real product, with no stand-in anywhere. The
/// real `stado` binary serves its own operator API on loopback, the store's
/// reads and writes travel over it through the real `FleetControlClient`, the
/// API runs the real `stado fleet …` commands, and every assertion is the
/// registry document those commands left on disk or the sentence the command
/// itself printed. Isolation is data only: a temp storage root and a temp
/// `HOME`, reached by the API's children through the config file `stado config
/// init` writes in that `HOME`.
@MainActor
final class FleetGroupStoreTests: XCTestCase {
    /// A create through the store is a write on the canonical document.
    func testTheFleetCreatedThroughTheStoreIsTheFleetTheRegistryCarries() async throws {
        let fleet = try RealFleet()
        defer { fleet.stop() }
        let store = try await fleet.store()

        await store.create(name: "build", notes: "ci builders")

        guard case let .succeeded(created) = store.mutation else {
            return XCTFail("the create did not succeed: \(store.mutation)")
        }
        XCTAssertTrue(
            created.hasPrefix("fleet 'build' created (generation "),
            "the receipt is the CLI's own sentence: \(created)"
        )
        // The document on disk is the contract, not the sentence.
        XCTAssertEqual(try fleet.declaredFleets(), [["name": "build", "notes": "ci builders"]])
        XCTAssertEqual(store.fleets, [FleetGroup(name: "build", notes: "ci builders", members: [])])

        // A fleet declared with no notes reads as empty notes and no members,
        // because that is what `fleet list --json` prints for it.
        try fleet.cli(["fleet", "create", "edge"])
        await store.refresh()
        XCTAssertEqual(store.fleets.last, FleetGroup(name: "edge", notes: "", members: []))

        // Membership comes from the registry, never from this store's own
        // bookkeeping: the assignment below is made outside it.
        try fleet.cli(["fleet", "assign", "w1", "build"])
        await store.refresh()
        XCTAssertEqual(store.fleets.first?.members, ["w1"])
        XCTAssertEqual(try fleet.fleetOfRegisteredTarget(), "build")

        // The bridge rejects an empty argument, so the store's own create with
        // blank notes never reaches the registry — and says so in its words.
        await store.create(name: "spare", notes: "")
        guard case let .failed(blank) = store.mutation else {
            return XCTFail("blank notes must be refused: \(store.mutation)")
        }
        XCTAssertEqual(blank, "arguments must be non-empty, bounded strings without NUL bytes")
        // Declaring a fleet twice is refused, and the declared notes stay.
        await store.create(name: "build", notes: "a second opinion")
        guard case let .failed(duplicate) = store.mutation else {
            return XCTFail("a duplicate create must fail: \(store.mutation)")
        }
        XCTAssertEqual(RealFleet.sentence(of: duplicate), "Error: fleet 'build' already exists")
        XCTAssertEqual(try fleet.declaredFleets().compactMap { $0["name"] }, ["build", "edge"])
        XCTAssertEqual(try fleet.declaredFleets().first?["notes"], "ci builders")
    }

    /// A fleet a target still points at cannot be retired, and an empty one
    /// can. Both answers are the CLI's, and the document proves each.
    func testARefusedDeleteArrivesInTheCLIsOwnSentence() async throws {
        let fleet = try RealFleet()
        defer { fleet.stop() }
        let store = try await fleet.store()
        try fleet.cli(["fleet", "create", "build", "--notes", "ci builders"])
        try fleet.cli(["fleet", "create", "edge", "--notes", "spare"])
        try fleet.cli(["fleet", "assign", "w1", "build"])

        await store.delete(name: "build")

        guard case let .failed(refusal) = store.mutation else {
            return XCTFail("a refused delete must fail the mutation: \(store.mutation)")
        }
        XCTAssertEqual(
            RealFleet.sentence(of: refusal),
            "Error: fleet 'build' still has 1 member(s): w1; reassign them first"
        )
        // The refused delete retired nothing, on disk or on screen.
        XCTAssertEqual(try fleet.declaredFleets().compactMap { $0["name"] }, ["build", "edge"])
        XCTAssertEqual(try fleet.fleetOfRegisteredTarget(), "build")
        XCTAssertEqual(store.fleets.map(\.name), ["build", "edge"])

        await store.delete(name: "edge")

        guard case let .succeeded(deleted) = store.mutation else {
            return XCTFail("the unmanned fleet was not retired: \(store.mutation)")
        }
        XCTAssertTrue(
            deleted.hasPrefix("fleet 'edge' deleted (generation "),
            "the receipt is the CLI's own sentence: \(deleted)"
        )
        // The document lost exactly the fleet that was retired.
        XCTAssertEqual(try fleet.declaredFleets(), [["name": "build", "notes": "ci builders"]])
        XCTAssertEqual(store.fleets.map(\.name), ["build"])
    }

    /// With no registry object at all the screen carries the backend's
    /// sentence and no fleets — never an empty fleet list read from a failure.
    func testAReadFailureNamesTheBackendSentence() async throws {
        let fleet = try RealFleet()
        defer { fleet.stop() }
        let store = try await fleet.store()
        try fleet.removeRegistryDocument()

        await store.refresh()

        XCTAssertEqual(store.fleets, [])
        XCTAssertEqual(
            RealFleet.sentence(of: try XCTUnwrap(store.failure)),
            "Error: no registry document at local:registry.json"
        )
        XCTAssertNil(store.lastReadAt)
    }
}

/// One real Stado deployment: a temp storage root, a temp `HOME`, the registry
/// object `stado registry push` created inside it, and the product's own
/// operator API on a loopback port the kernel chose.
@MainActor
private final class RealFleet {
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
        root = URL(fileURLWithPath: NSTemporaryDirectory())
            .appending(path: "stado-desktop-fleet-\(UUID().uuidString)")
        home = root.appending(path: "home")
        log = root.appending(path: "operator-api.stderr")
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
    }

    /// The store, pointed at the operator API of a deployment holding one
    /// registered machine and no fleets, seeded by `stado registry push`.
    func store() async throws -> FleetGroupStore {
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
        let store = FleetGroupStore()
        store.configureEndpoint(try await startOperatorAPI())
        return store
    }

    /// Run the real CLI here; a nonzero exit throws with its own complaint.
    func cli(_ arguments: [String], input: Data? = nil, configured: Bool = false) throws {
        let process = Process()
        process.executableURL = binary
        process.arguments = arguments
        process.environment = environment(configured: configured)
        let errors = Pipe()
        let stdin = Pipe()
        process.standardOutput = FileHandle.nullDevice
        process.standardError = errors
        process.standardInput = input == nil ? FileHandle.nullDevice : stdin
        try process.run()
        if let input {
            stdin.fileHandleForWriting.write(input)
            try stdin.fileHandleForWriting.close()
        }
        let complained = errors.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
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
        try? FileManager.default.removeItem(at: root)
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

private struct RealFleetFailure: LocalizedError {
    let errorDescription: String?
    init(_ sentence: String) { errorDescription = sentence }
}
