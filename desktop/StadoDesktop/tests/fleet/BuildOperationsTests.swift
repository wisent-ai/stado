import Foundation
import XCTest
@testable import Stado

/// The Releases screen's build operations, sent to a real isolated Stado API.
///
/// A build is not a release: the screen reads and lists builds and releases
/// one that passed, and the API answers with the CLI's own sentences. Nothing
/// here needs a builder: the store starts empty, and what is proved is that
/// every operation reaches the product and comes back with its verdict.
@MainActor
final class BuildOperationsTests: XCTestCase {
    /// A well-formed build id — the same 32 lowercase hexadecimal characters
    /// a release run id has — that no store holds.
    private static let absentBuild = "0123456789abcdef0123456789abcdef"

    func testBuildOperationsListReadAndRefuseThroughTheRealAPI() async throws {
        let fleet = try RealFleet()
        defer { fleet.stop() }
        let control = try await fleet.control()
        let store = NativeCapabilityStore()
        let operations = NativeReleaseSourceOperations.all

        let list = try XCTUnwrap(operations.first { $0.id == "build-list" })
        let listed = await store.run(
            try list.request(host: "", values: [:], content: ""),
            fleet: control, expectedSource: control.requestGeneration)
        XCTAssertTrue(listed, store.problem ?? "listing the builds was refused")
        let receipt = try XCTUnwrap(store.receipt)
        XCTAssertEqual(
            receipt.standardOutput.trimmingCharacters(in: .whitespacesAndNewlines), "[]",
            "a fresh store lists no builds")

        let missing = Self.absentBuild
        let status = try XCTUnwrap(operations.first { $0.id == "build-status" })
        let read = await store.run(
            try status.request(host: "", values: ["build": missing], content: ""),
            fleet: control, expectedSource: control.requestGeneration)
        XCTAssertFalse(read, "a build the store does not hold was read")
        XCTAssertTrue(
            store.problem?.contains("build \(missing) does not exist") == true,
            store.problem ?? "no refusal")

        let release = try XCTUnwrap(operations.first { $0.id == "release-build" })
        let released = await store.run(
            try release.request(host: "", values: ["build": missing], content: ""),
            fleet: control, expectedSource: control.requestGeneration)
        XCTAssertFalse(released, "a build the store does not hold was released")
        XCTAssertTrue(
            store.problem?.contains("build \(missing) does not exist") == true,
            store.problem ?? "no refusal")
    }
}
