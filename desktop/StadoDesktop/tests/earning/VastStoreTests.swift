import CryptoKit
import XCTest
@testable import Stado

/// The Earning screen, driven through the real product binary on a machine
/// that holds no credential — the state the whole fleet was in on 2026-09-20,
/// when nothing in the console said the idle GPU was unlisted or why.
@MainActor
final class VastStoreTests: XCTestCase {
    func testTheScreenNamesWhyTheFleetIsNotEarning() async throws {
        let fixture = try EarningFixture()
        let store = VastStore(cli: StadoCLI(executable: fixture.binary.path))

        await store.refresh()
        try fixture.retain(store, named: "screen.txt")

        let readiness = try XCTUnwrap(store.readiness, store.problem ?? "no document")
        XCTAssertEqual(readiness.document, "stado.vast-readiness.v1")
        XCTAssertEqual(readiness.verdict, "no_channel",
            "a home with no bearer cannot ask Skarbiec anything")
        XCTAssertFalse(readiness.earning)
        XCTAssertEqual(readiness.item, "stado-vast")
        XCTAssertEqual(readiness.field, "api_key")
        XCTAssertTrue(readiness.headline.contains("No Skarbiec channel on this host"),
            "the screen's headline must be the verdict, not a generic failure: \(readiness.headline)")
        XCTAssertTrue(readiness.channel.summary.contains("no control-plane bearer at"),
            "the channel row must name the bearer that is missing: \(readiness.channel.summary)")
        XCTAssertFalse(readiness.remedy.isEmpty,
            "a screen that refuses without naming a way out sends the operator to a terminal")
    }

    func testThePreviewShowsADecisionWithoutACredential() async throws {
        let fixture = try EarningFixture()
        let store = VastStore(cli: StadoCLI(executable: fixture.binary.path))

        await store.previewDecision(
            idleWindowSeconds: EarningConstants.immediateIdleWindowSeconds,
            priceGPU: EarningConstants.defaultPriceGPU
        )
        let preview = try XCTUnwrap(store.preview, store.problem ?? "no preview")
        try fixture.write(preview, named: "preview.txt")

        XCTAssertTrue(preview.contains("startup: no Vast.ai credential"),
            "the preview must say it is deciding without a key: \(preview)")
        XCTAssertTrue(preview.contains("DRY-RUN would list"),
            "an empty queue must produce the listing decision: \(preview)")
        XCTAssertFalse(preview.contains("LISTED ("),
            "a preview must never publish an offer: \(preview)")
    }
}

private struct EarningFixtureRefusal: LocalizedError {
    let errorDescription: String?
    init(_ sentence: String) { errorDescription = sentence }
}

/// One isolated machine: a home holding no Skarbiec bearer, an empty local
/// queue store, and the real Stado binary whose digest and revision are
/// retained beside what the screen read.
@MainActor
private final class EarningFixture {
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
            throw EarningFixtureRefusal(
                "Build the real Stado binary before running the Desktop earning flow: \(binary.path)")
        }
        let revision = try XCTUnwrap(ProcessInfo.processInfo.environment["STADO_SOURCE_REVISION"],
            "Set STADO_SOURCE_REVISION to the exact compiled product revision")
        root = package.appendingPathComponent(".wisent-output/earning-native/\(UUID().uuidString)")
        let storage = root.appendingPathComponent("storage")
        try FileManager.default.createDirectory(at: storage, withIntermediateDirectories: true)
        let digest = SHA256.hash(data: try Data(contentsOf: binary, options: .mappedIfSafe))
            .map { String(format: "%02x", $0) }.joined()
        try Data("binary=\(binary.path)\nsha256=\(digest)\nrevision=\(revision)\n".utf8)
            .write(to: root.appendingPathComponent("source.txt"))
        // The home decides which Skarbiec bearer exists, so the case owns it:
        // no control-plane token file and no agent grant means the product
        // reports the channel-less verdict instead of reaching the operator's
        // own vault.
        setenv("HOME", root.path, Self.replaceEnvironmentValue)
        setenv("WC_STORAGE_BACKEND", "local", Self.replaceEnvironmentValue)
        setenv("WC_LOCAL_STORAGE_PATH", storage.path, Self.replaceEnvironmentValue)
        setenv("STADO_CONFIG", root.appendingPathComponent("no-such-config.json").path,
            Self.replaceEnvironmentValue)
        setenv("WC_VAST_AUTO_LIST", "false", Self.replaceEnvironmentValue)
        setenv("NO_COLOR", "1", Self.replaceEnvironmentValue)
        for inherited in ["COMPUTE_API_KEY", "COMPUTE_API_URL", "WC_PROFILES_DIR", "STADO_API_URL"] {
            unsetenv(inherited)
        }
        print("Earning screen evidence: \(root.path)")
    }

    /// What the screen would show, retained beside the machine it read.
    func retain(_ store: VastStore, named name: String) throws {
        var lines = ["problem=\(store.problem ?? "none")"]
        if let readiness = store.readiness {
            lines.append("verdict=\(readiness.verdict)")
            lines.append("headline=\(readiness.headline)")
            lines.append("channel=\(readiness.channel.summary)")
            lines.append(contentsOf: readiness.remedy.map { "remedy=\($0)" })
        }
        if let snapshot = store.snapshot {
            lines.append("queued=\(snapshot.wisentQueue) running=\(snapshot.wisentRunning)")
        } else {
            lines.append("snapshot=\(store.snapshotProblem ?? "not read")")
        }
        try write(lines.joined(separator: "\n"), named: name)
    }

    func write(_ text: String, named name: String) throws {
        try Data((text + "\n").utf8).write(to: root.appendingPathComponent(name))
    }
}
