import Foundation
import XCTest
@testable import Stado

/// The Services screen's remove verb: what the store decodes from
/// `stado service remove --json`, which rows may show the button, and the
/// exact argv the button runs.
@MainActor
final class RemoveFileStoreTests: XCTestCase {
    func testDecodesTheComposedReport() throws {
        let payload = """
        {"target": "control-host", "unit": "weles-echo-api", "action": "removed",
         "generation": "abc123",
         "file": {"path": "/Users/charles/Library/LaunchAgents/com.wisent.weles-echo-api.plist",
                  "status": "removed", "detail": ""},
         "report": {}}
        """
        let report = try JSONDecoder().decode(ServiceRemoveReport.self, from: Data(payload.utf8))
        XCTAssertEqual(report.unit, "weles-echo-api")
        XCTAssertEqual(report.generation, "abc123")
        XCTAssertTrue(report.succeeded)
    }

    func testAbsentFileCountsAsDoneNotAsFailure() throws {
        let payload = """
        {"target": "control-host", "unit": "ghost", "action": "removed",
         "generation": "abc123",
         "file": {"path": "/Users/charles/Library/LaunchAgents/ghost.plist",
                  "status": "absent", "detail": null},
         "report": {}}
        """
        let report = try JSONDecoder().decode(ServiceRemoveReport.self, from: Data(payload.utf8))
        XCTAssertTrue(report.succeeded, "absence is the goal state, not a failure")
    }

    func testARefusedFileIsNamedWithItsReasonVerbatim() throws {
        let payload = """
        {"target": "control-host", "unit": "weles", "action": "removed",
         "generation": "abc123",
         "file": {"path": "/Library/LaunchDaemons/com.wisent.always-on.weles.plist",
                  "status": "refused",
                  "detail": "outside the managed home areas; remove it on the host with: sudo rm -- /Library/LaunchDaemons/com.wisent.always-on.weles.plist"},
         "report": {}}
        """
        let report = try JSONDecoder().decode(ServiceRemoveReport.self, from: Data(payload.utf8))
        XCTAssertFalse(report.succeeded)
        XCTAssertEqual(
            report.fileSentence,
            "refused — outside the managed home areas; remove it on the host with: sudo rm -- /Library/LaunchDaemons/com.wisent.always-on.weles.plist"
        )
    }

    func testTheButtonRunsTheExactCommand() {
        XCTAssertEqual(
            FleetServicesStore.removeServiceArguments(
                name: "weles-echo-api",
                host: "control-host"
            ),
            ["service", "remove", "weles-echo-api", "--host", "control-host", "--json"]
        )
    }

    func testDeployUsesOnlyTheStoredDeclarationNameAndHost() {
        XCTAssertEqual(
            FleetServicesStore.deployArguments(name: "vllm-llama", host: "gpu-host-1"),
            ["service", "deploy", "vllm-llama", "--host", "gpu-host-1", "--json"]
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
