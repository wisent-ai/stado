import Foundation
import XCTest
@testable import Stado

/// `ScratchStore` against the real product, with no stand-in anywhere. The
/// store runs the real `stado scratch …` commands through the real `StadoCLI`,
/// the declaration it renders is the one compiled into that binary, and the
/// lease it takes is a throwaway account on a machine the fleet itself named.
/// Every assertion is either the document the command printed or the host's own
/// account of what it holds afterwards.
///
/// The host is never hardcoded and never read from the environment: `stado
/// scratch hosts --json` says which registry targets are leasable, and a run
/// where none is fails by name instead of passing.
@MainActor
final class ScratchStoreTests: XCTestCase {
    /// The declaration reaches the form: choosing a profile seeds the lifetime
    /// with the duration that profile declares, in the declaration's own words.
    func testTheFormTakesItsLifetimeFromTheDeclaredProfile() async throws {
        let product = try RealScratch()
        let store = ScratchStore(cli: product.cli)

        await store.loadProfiles()

        XCTAssertNil(store.refusal, "the declaration read cleanly: \(store.refusal ?? "")")
        let profiles = store.profiles
        XCTAssertFalse(profiles.isEmpty, "the compiled declaration carries profiles")
        for profile in profiles {
            XCTAssertFalse(profile.platforms.isEmpty, "\(profile.name) declares platforms")
            XCTAssertFalse(profile.defaultTTL.isEmpty, "\(profile.name) declares a default lifetime")
            XCTAssertFalse(profile.maxTTL.isEmpty, "\(profile.name) declares a ceiling")
        }
        // The first profile is selected on load, so the field already holds a
        // declared duration before anyone types.
        XCTAssertEqual(store.form.ttl, profiles.first?.defaultTTL)

        guard let other = profiles.last, other.name != profiles.first?.name else { return }
        store.selectProfile(named: other.name)
        XCTAssertEqual(store.form.profile, other.name)
        XCTAssertEqual(store.form.ttl, other.defaultTTL, "the field follows the declaration")
        XCTAssertEqual(
            store.createArguments(host: "some-host"),
            ["scratch", "create", "--host", "some-host", "--profile", other.name,
             "--ttl", other.defaultTTL, "--json"],
            "the button runs exactly what the form holds"
        )
    }

    /// The whole section's story: lease through the store, see the account the
    /// host reports, then destroy it and see all three parts gone.
    func testALeaseTakenThroughTheStoreIsTheLeaseTheHostReportsAndLoses() async throws {
        let product = try RealScratch()
        let host = try product.leasableHost()
        let store = ScratchStore(cli: product.cli)
        await store.loadProfiles()
        store.selectProfile(named: host.profile)
        store.form.ttl = "15m"

        await store.create(host: host.target)

        guard let receipt = store.lease else {
            return XCTFail("no lease receipt: \(store.refusal ?? "no refusal either")")
        }
        XCTAssertEqual(receipt.status, "leased")
        XCTAssertEqual(receipt.target, host.target)
        XCTAssertEqual(receipt.account.exists, true)
        XCTAssertEqual(receipt.verifiedLogin, receipt.username, "the lease was entered as itself")
        XCTAssertEqual(receipt.ttl, "15m")
        XCTAssertFalse(receipt.homePath.isEmpty, "the receipt names the account's home directory")
        XCTAssertTrue(
            FileManager.default.fileExists(atPath: receipt.registryPath),
            "the emitted registry is on disk at \(receipt.registryPath)"
        )

        await store.read(host: host.target)
        guard let row = store.leases.first(where: { $0.name == receipt.name }) else {
            return XCTFail("the host does not report the lease it just took: \(store.leases)")
        }
        XCTAssertEqual(row.account.exists, true, "the account is on the machine")
        XCTAssertEqual(row.profile, host.profile)
        XCTAssertFalse(row.expired)
        XCTAssertEqual(row.homePath, receipt.homePath, "one home directory, two reports")

        await store.destroy(name: receipt.name, host: host.target)

        guard let gone = store.destroyed else {
            return XCTFail("no destroy receipt: \(store.refusal ?? "no refusal either")")
        }
        XCTAssertEqual(gone.account.exists, false)
        XCTAssertEqual(gone.home.exists, false)
        XCTAssertEqual(gone.record.exists, false)
        XCTAssertFalse(gone.destroyedAt.isEmpty, "the destroy is stamped")
        XCTAssertFalse(
            store.leases.contains { $0.name == receipt.name },
            "the section no longer lists the destroyed lease"
        )
        XCTAssertFalse(
            FileManager.default.fileExists(atPath: receipt.storageRoot),
            "the emitted registry root went with the lease"
        )
    }

    /// A refusal reaches the screen in the command's own sentence. A console
    /// that paraphrases sends the operator to a terminal to run the command it
    /// just ran.
    func testAnUndeclaredProfileSurfacesTheCommandsOwnSentence() async throws {
        let product = try RealScratch()
        let host = try product.leasableHost()
        let store = ScratchStore(cli: product.cli)
        store.form.profile = "macos-vm"
        store.form.ttl = ""

        await store.create(host: host.target)

        XCTAssertNil(store.lease, "no lease was taken")
        let refusal = store.refusal ?? ""
        XCTAssertTrue(
            refusal.contains(
                "scratch profile 'macos-vm' is not declared in stado-rs/data/scratch-profiles.json"
            ),
            "the screen shows the command's sentence: \(refusal)"
        )
        XCTAssertTrue(
            refusal.contains("declared profiles:"),
            "including the part that says what to use instead: \(refusal)"
        )
    }
}

/// The real product, and the fleet's own answer to where a lease may be taken.
private struct RealScratch {
    let binary: URL
    let cli: StadoCLI

    init() throws {
        binary = try Self.resolveBinary()
        cli = StadoCLI(executable: binary.path)
    }

    struct Host {
        let target: String
        let profile: String
    }

    /// `stado scratch hosts --json`, read through the same binary the store
    /// uses. A run with no leasable target fails here, carrying the report.
    func leasableHost() throws -> Host {
        let report = try json(["scratch", "hosts", "--json"])
        let hosts = report["hosts"] as? [[String: Any]] ?? []
        let leasable = hosts.filter { $0["eligible"] as? Bool == true }
        guard let chosen = leasable.first(where: { ($0["target"] as? String) != Self.localName() })
            ?? leasable.first
        else {
            throw RealScratchFailure(
                "no registry target is leasable, so this run is blocked rather than passed: \(hosts)"
            )
        }
        return Host(
            target: chosen["target"] as? String ?? "",
            profile: chosen["profile"] as? String ?? ""
        )
    }

    private func json(_ arguments: [String]) throws -> [String: Any] {
        let process = Process()
        process.executableURL = binary
        process.arguments = arguments
        let out = Pipe()
        let err = Pipe()
        process.standardOutput = out
        process.standardError = err
        try process.run()
        let stdout = out.fileHandleForReading.readDataToEndOfFile()
        let stderr = err.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            throw RealScratchFailure(
                "stado \(arguments.joined(separator: " ")) failed: "
                    + String(decoding: stderr, as: UTF8.self)
            )
        }
        guard let document = try JSONSerialization.jsonObject(with: stdout) as? [String: Any] else {
            throw RealScratchFailure("stado \(arguments.joined(separator: " ")) printed no document")
        }
        return document
    }

    private static func localName() -> String {
        ProcessInfo.processInfo.hostName
            .lowercased()
            .split(separator: ".")
            .first
            .map(String.init) ?? ""
    }

    /// `STADO_BIN`, then `PATH`, then `~/.stado/bin/stado`; with none of them
    /// these tests fail by name rather than skipping.
    private static func resolveBinary() throws -> URL {
        let manager = FileManager.default
        if let configured = ProcessInfo.processInfo.environment["STADO_BIN"], !configured.isEmpty {
            guard manager.isExecutableFile(atPath: configured) else {
                throw RealScratchFailure(
                    "STADO_BIN=\(configured) is not an executable stado binary; build one with `cargo build -p stado` or unset STADO_BIN to use the installed CLI."
                )
            }
            return URL(fileURLWithPath: configured)
        }
        let candidates = (ProcessInfo.processInfo.environment["PATH"] ?? "")
            .split(separator: ":")
            .map { URL(fileURLWithPath: String($0)).appendingPathComponent("stado") }
            + [manager.homeDirectoryForCurrentUser.appending(path: ".stado/bin/stado")]
        guard let found = candidates.first(where: { manager.isExecutableFile(atPath: $0.path) })
        else {
            throw RealScratchFailure(
                "no stado binary to test against: STADO_BIN is unset, none is on PATH, and ~/.stado/bin/stado is absent; point STADO_BIN at a build."
            )
        }
        return found
    }
}

private struct RealScratchFailure: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}
