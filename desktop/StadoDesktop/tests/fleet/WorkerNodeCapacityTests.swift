import Foundation
import XCTest
@testable import Stado

final class WorkerNodeCapacityTests: XCTestCase {
    func testWorkerNodeUsesLiveResourcesAndIgnoresRemovedFixedCounts() throws {
        let node = try JSONDecoder().decode(
            WorkerNode.self,
            from: Data(
                """
                {
                  "targetName": "charless-mac-mini",
                  "declared": true,
                  "status": "live",
                  "availabilityReason": "fresh capacity publication",
                  "acceptingJobs": true,
                  "runningJobs": 3,
                  "availableCpuCores": 6,
                  "totalCpuCores": 12,
                  "availableAccelerators": {"apple-m2-max": 1},
                  "freeRamGb": 18.5,
                  "totalRamGb": 64.0,
                  "freeVramGb": 7.5,
                  "totalVramGb": 32.0,
                  "slots": 0,
                  "freeSlots": 0
                }
                """.utf8
            )
        )

        XCTAssertTrue(try XCTUnwrap(node.acceptingJobs))
        XCTAssertEqual(node.runningJobs, 3)
        XCTAssertEqual(node.availableCPUCores, 6)
        XCTAssertEqual(node.totalCPUCores, 12)
        XCTAssertEqual(node.availableAccelerators, ["apple-m2-max": 1])
        XCTAssertEqual(node.freeRAMGB, 18.5)
        XCTAssertEqual(node.totalRAMGB, 64.0)
        XCTAssertEqual(node.freeVRAMGB, 7.5)
        XCTAssertEqual(node.totalVRAMGB, 32.0)
    }
}
