import Foundation
import XCTest
@testable import Stado

/// The gates decode path decides whether a host that takes no work is a
/// failure or a declared registry pin. The distinction is what keeps a
/// by-design pinned host out of the red alarm — and what put "44 free slots
/// of 2 declared" reading back into its own shape on the CLI side.
final class HostGatesTests: XCTestCase {
    func testPinnedOnlyAloneIsDeclaredPolicyNotARefusal() throws {
        let gates = try decode(
            claiming: false,
            blockers: ["pinned_only"]
        )
        XCTAssertTrue(gates.pinnedByDesign)
        XCTAssertFalse(gates.refusingUnpinned)
    }

    func testAnyOtherBlockerBesideThePinIsARefusal() throws {
        let gates = try decode(
            claiming: false,
            blockers: ["pinned_only", "disk_pressure_unresolved"]
        )
        XCTAssertFalse(gates.pinnedByDesign)
        XCTAssertTrue(gates.refusingUnpinned)
    }

    func testANamelessRefusalIsNeverDeclaredPolicy() throws {
        let gates = try decode(claiming: false, blockers: [])
        XCTAssertFalse(gates.pinnedByDesign)
        XCTAssertTrue(gates.refusingUnpinned)
    }

    func testAClaimingHostIsNeither() throws {
        let gates = try decode(claiming: true, blockers: [])
        XCTAssertFalse(gates.pinnedByDesign)
        XCTAssertFalse(gates.refusingUnpinned)
    }

    private func decode(claiming: Bool, blockers: [String]) throws -> HostGates {
        let payload: [String: Any] = [
            "host": "charless-mac-mini",
            "claiming": claiming,
            "blockers": blockers,
            "waiting_jobs": [],
        ]
        return try JSONDecoder().decode(
            HostGates.self,
            from: JSONSerialization.data(withJSONObject: payload)
        )
    }
}
