import Foundation
import XCTest
@testable import Stado

/// The same client and draft the Memory screen uses, against real services.
@MainActor
final class MemoryDeclarationTests: XCTestCase {
    func testRepairEditsPersistAndInvalidPoliciesPreserveTheRegistry() async throws {
        let host = try NativeMemoryHost()
        defer { host.stop() }
        try await host.waitUntilListening()
        let client = FleetControlClient()
        let address = try OperationsDashboardAddress(host.endpoint)
        let initial = try await state(client, address, host.name)
        var draft = MemoryPolicyDraft(state: initial)
        draft.mode = .enforce
        draft.numbers[.lowFreeMB] = "512"
        draft.numbers[.targetFreeMB] = "1024"
        draft.numbers[.maxPassSeconds] = "10"
        draft.repairsText = "{\"restart_unit\":{\"units\":[\"com.wisent.memory-native-test\"]}}"
        let patch = try XCTUnwrap(MemoryReclaimPatch(draft: draft, current: initial))
        let generation = try await client.updatePolicy(at: address, target: host.name, patch: .memoryReclaim(patch))
        let persisted = try host.policy()
        try host.record(["generation": generation, "policy": persisted], named: "declared.json")
        XCTAssertEqual(persisted["mode"] as? String, "enforce")
        XCTAssertEqual(persisted["max_pass_seconds"] as? Int, 10)
        let repairs = try XCTUnwrap(persisted["repairs"] as? [String: Any])
        let restart = try XCTUnwrap(repairs["restart_unit"] as? [String: Any])
        XCTAssertEqual(restart["units"] as? [String], ["com.wisent.memory-native-test"])

        let savedState = try await state(client, address, host.name)
        XCTAssertEqual(savedState.declared?.repairs["restart_unit"]?.units, ["com.wisent.memory-native-test"])
        XCTAssertEqual(savedState.declared?.maxPassSeconds, 10)
        let beforeRefusal = try Data(contentsOf: host.registry)
        var refused = MemoryPolicyDraft(state: savedState)
        refused.repairsText = "{}"
        let refusedPatch = try XCTUnwrap(MemoryReclaimPatch(draft: refused, current: savedState))
        do {
            _ = try await client.updatePolicy(at: address, target: host.name, patch: .memoryReclaim(refusedPatch))
            XCTFail("Enforce mode without a repair must be refused")
        } catch FleetControlError.backend(let status, let message) {
            try host.record(["status": status, "message": message], named: "refusal.json")
            XCTAssertEqual(status, 400)
            XCTAssertTrue(message.contains("must name at least one repair when mode is 'enforce'"), message)
        }
        XCTAssertEqual(try Data(contentsOf: host.registry), beforeRefusal)

        var invalid = MemoryPolicyDraft(state: savedState)
        invalid.numbers[.targetFreeMB] = "512"
        let invalidPatch = try XCTUnwrap(MemoryReclaimPatch(draft: invalid, current: savedState))
        do {
            _ = try await client.updatePolicy(at: address, target: host.name, patch: .memoryReclaim(invalidPatch))
            XCTFail("The target watermark must exceed the low watermark")
        } catch FleetControlError.backend(let status, let message) {
            try host.record(["status": status, "message": message], named: "watermark-refusal.json")
            XCTAssertEqual(status, 400)
            XCTAssertTrue(message.contains("must be greater than low_free_mb"), message)
        }
        XCTAssertEqual(try Data(contentsOf: host.registry), beforeRefusal)

        var removal = MemoryPolicyDraft(state: savedState)
        removal.mode = .report
        removal.repairsText = "{}"
        let removalPatch = try XCTUnwrap(MemoryReclaimPatch(draft: removal, current: savedState))
        _ = try await client.updatePolicy(at: address, target: host.name, patch: .memoryReclaim(removalPatch))
        let final = try host.policy()
        try host.record(final, named: "removed.json")
        XCTAssertEqual(final["mode"] as? String, "report")
        XCTAssertEqual((final["repairs"] as? [String: Any])?.count, 0)
        let finalState = try await state(client, address, host.name)
        XCTAssertEqual(finalState.declaredRepairNames, [])
    }

    private func state(_ client: FleetControlClient, _ address: OperationsDashboardAddress,
                       _ target: String) async throws -> MemoryPolicyState {
        let policy = try await client.policy(at: address)
        let host = try XCTUnwrap(policy.targets.first { $0.name == target })
        return MemoryPolicyState(target: target, declared: host.memory, report: nil)
    }
}
