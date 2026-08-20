import Foundation
import XCTest
@testable import Stado

final class CatalogEntryTests: XCTestCase {
    func testInstallableProductIsEnabled() throws {
        let entry = try JSONDecoder().decode(
            WisentCatalogEntry.self,
            from: Data(#"{"name":"weles","summary":"worker","program":"$HOME/.stado/bin/weles-worker","args":[],"available":true,"unavailable_reason":null}"#.utf8)
        )
        XCTAssertTrue(entry.isAvailable)
        XCTAssertNil(entry.unavailableReason)
    }

    func testProductWithoutDeliveryContractStaysVisibleButDisabled() throws {
        let reason = "No published host-service artifact or Stado install contract."
        let data = try JSONSerialization.data(withJSONObject: [
            "name": "skarbiec-hub",
            "summary": "paid control plane",
            "program": "",
            "args": [],
            "available": false,
            "unavailable_reason": reason,
        ])
        let entry = try JSONDecoder().decode(WisentCatalogEntry.self, from: data)
        XCTAssertFalse(entry.isAvailable)
        XCTAssertEqual(entry.unavailableReason, reason)
    }
}
