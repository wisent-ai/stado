import Foundation
import XCTest
@testable import Stado

/// The Services screen's remove-file verb: what the store decodes, which rows
/// may show the button, and the exact argv the button runs.
@MainActor
final class RemoveFileStoreTests: XCTestCase {
    func testDecodesTheRemovalReport() throws {
        let payload = """
        {"target": "charless-mac-mini", "path": "/Users/charles/Library/LaunchAgents/com.wisent.weles-echo-api.plist", "status": "removed", "detail": ""}
        """
        let report = try JSONDecoder().decode(RemoveFileReport.self, from: Data(payload.utf8))
        XCTAssertEqual(report.status, "removed")
        XCTAssertTrue(report.succeeded)
    }

    func testAbsentCountsAsDoneNotAsFailure() throws {
        let payload = """
        {"target": "charless-mac-mini", "path": "/Users/charles/Library/LaunchAgents/gone.plist", "status": "absent", "detail": null}
        """
        let report = try JSONDecoder().decode(RemoveFileReport.self, from: Data(payload.utf8))
        XCTAssertTrue(report.succeeded, "absence is the goal state, not a failure")
    }

    func testARefusalCarriesThePrivilegedCommandVerbatim() throws {
        let payload = """
        {"target": "charless-mac-mini", "path": "/Library/LaunchDaemons/com.wisent.always-on.weles.plist", "status": "refused", "detail": "outside the managed home areas; remove it on the host with: sudo rm -- /Library/LaunchDaemons/com.wisent.always-on.weles.plist"}
        """
        let report = try JSONDecoder().decode(RemoveFileReport.self, from: Data(payload.utf8))
        XCTAssertFalse(report.succeeded)
        XCTAssertEqual(
            report.detail,
            "outside the managed home areas; remove it on the host with: sudo rm -- /Library/LaunchDaemons/com.wisent.always-on.weles.plist"
        )
    }

    func testTheButtonRunsTheExactCommand() {
        XCTAssertEqual(
            FleetServicesStore.removeFileArguments(
                host: "charless-mac-mini",
                path: "/Users/charles/Library/LaunchAgents/com.wisent.weles-echo-api.plist"
            ),
            ["host", "remove-file", "charless-mac-mini", "/Users/charles/Library/LaunchAgents/com.wisent.weles-echo-api.plist", "--json"]
        )
    }

    func testOnlyUserHomePathsEarnTheButton() {
        func entry(_ path: String) -> FleetServiceEntry {
            let payload = """
            {"host": "h", "name": "s", "unit": "", "label": "l", "unit_id": "l",
             "path": "\(path)", "kind": "launchd", "source": "registry",
             "managed_since": "2026-08-19T00:00:00Z", "state": "missing",
             "reported_at": "2026-08-19T00:00:00Z", "detail": ""}
            """
            return try! JSONDecoder().decode(FleetServiceEntry.self, from: Data(payload.utf8))
        }
        XCTAssertTrue(entry("/Users/charles/Library/LaunchAgents/a.plist").removableByRemoveFile)
        XCTAssertTrue(entry("/Users/charles/.stado/bin/helper.sh").removableByRemoveFile)
        XCTAssertFalse(entry("/Library/LaunchDaemons/a.plist").removableByRemoveFile)
        XCTAssertFalse(entry("").removableByRemoveFile)
        XCTAssertFalse(entry("/Users/charles/Library/LaunchAgents/../Secrets/a").removableByRemoveFile)
    }
}
