import Foundation
import XCTest
@testable import Stado

/// The Products screen's catalog-wide operations, sent to a real isolated
/// Stado API that runs the real `stado product`.
///
/// Nothing here writes to a catalog or provisions a repository: a read-only
/// operation must come back with the CLI's JSON, and provisioning without the
/// explicit grant must come back with the CLI's own refusal.
@MainActor
final class ProductOperationsTests: XCTestCase {
    func testProductOperationsReadAndRefuseThroughTheRealAPI() async throws {
        let fleet = try RealFleet()
        defer { fleet.stop() }
        let control = try await fleet.control()
        let store = NativeCapabilityStore()
        let operations = NativeProductOperations.all

        let paths = try XCTUnwrap(operations.first { $0.id == "paths" })
        let read = await store.run(
            try paths.request(host: "", values: [:], content: ""),
            fleet: control, expectedSource: control.requestGeneration)
        XCTAssertTrue(read, store.problem ?? "reading executable ownership was refused")
        let receipt = try XCTUnwrap(store.receipt)
        XCTAssertNoThrow(
            try JSONSerialization.jsonObject(with: Data(receipt.standardOutput.utf8)),
            "stado product paths --json answered with something other than JSON: \(receipt.standardOutput)")

        let create = try XCTUnwrap(operations.first { $0.id == "create" })
        let created = await store.run(
            try create.request(host: "", values: [:], content: "{}"),
            fleet: control, expectedSource: control.requestGeneration)
        XCTAssertFalse(created, "provisioning ran without --allow-create")
        XCTAssertTrue(
            store.problem?.contains("--allow-create is required") == true,
            store.problem ?? "no refusal")
    }
}
