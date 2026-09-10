import Foundation
import XCTest
@testable import Stado

/// Arming a host from the declared catalog through the graphical surface's own
/// client, against a real dashboard and a real registry.
///
/// The screen offers the policies this route publishes and posts the document
/// the declaration carries, so this case proves the two surfaces agree: what
/// Desktop writes is what `stado space watermark --policy` writes, and the
/// backend then reports the host as carrying that reviewed policy.
@MainActor
final class MemoryDeclaredPolicyTests: XCTestCase {
    func testApplyingADeclaredPolicyArmsTheHostAndIsReportedAsReviewed() async throws {
        let host = try NativeMemoryHost()
        defer { host.stop() }
        try await host.waitUntilListening()
        let client = FleetControlClient()
        let address = try OperationsDashboardAddress(host.endpoint)

        let catalog = try await client.memoryPolicies(at: address)
        try host.record(
            ["declared": catalog.map(\.name), "fitting": try await fit(client, address, host.name).fitting],
            named: "declared-policies.json"
        )
        XCTAssertFalse(catalog.isEmpty, "the dashboard must publish the declared catalog")

        let before = try await fit(client, address, host.name)
        XCTAssertFalse(before.automatic.armed, "an undeclared host repairs nothing: \(before.automatic.detail)")
        XCTAssertNil(before.automatic.reviewedPolicy)
        let fitting = Set(before.fitting)
        XCTAssertFalse(fitting.isEmpty, "the fixture's platform and role must fit a declared policy")

        let chosen = try XCTUnwrap(
            catalog.first { fitting.contains($0.name) && $0.endsGraphicalSession },
            "a policy that ends session processes is the one whose authorization matters"
        )
        let state = try await self.state(client, address, host.name)
        let draft = MemoryPolicyDraft(declared: chosen, state: state)
        let patch = try XCTUnwrap(
            MemoryReclaimPatch(draft: draft, current: state),
            "seeding the editor from a declared policy must produce a patch"
        )
        let generation = try await client.updatePolicy(
            at: address,
            target: host.name,
            patch: .memoryReclaim(patch)
        )

        let persisted = try host.policy()
        try host.record(
            ["generation": generation, "applied": chosen.name, "policy": persisted],
            named: "declared-policy-applied.json"
        )
        XCTAssertEqual(persisted["mode"] as? String, chosen.policy.mode)
        XCTAssertEqual(persisted["low_free_mb"] as? Int, chosen.policy.lowFreeMB)
        XCTAssertEqual(persisted["target_free_mb"] as? Int, chosen.policy.targetFreeMB)
        XCTAssertEqual(persisted["high_swap_used_pct"] as? Int, chosen.policy.highSwapUsedPct)
        XCTAssertEqual(persisted["max_repairs_per_pass"] as? Int, chosen.policy.maxRepairsPerPass)
        XCTAssertEqual(persisted["refuse_placement"] as? Bool, chosen.policy.refusePlacement)
        let repairs = try XCTUnwrap(persisted["repairs"] as? [String: Any])
        XCTAssertEqual(Set(repairs.keys), Set(chosen.repairNames))
        let session = try XCTUnwrap(repairs["graphical_session"] as? [String: Any])
        XCTAssertEqual(session["processes"] as? [String], chosen.sessionProcesses)
        XCTAssertEqual(
            session["allow_graphical_session"] as? Bool,
            true,
            "the authorization the catalog declares must reach the registry"
        )

        let after = try await fit(client, address, host.name)
        try host.record(
            ["armed": after.automatic.armed,
             "reviewed_policy": after.automatic.reviewedPolicy ?? NSNull(),
             "detail": after.automatic.detail],
            named: "declared-policy-verdict.json"
        )
        XCTAssertTrue(after.automatic.armed, "an enforcing policy with repairs is armed: \(after.automatic.detail)")
        XCTAssertEqual(after.automatic.reviewedPolicy, chosen.name)
        XCTAssertEqual(Set(after.automatic.repairs), Set(chosen.repairNames))
    }

    private func fit(
        _ client: FleetControlClient,
        _ address: OperationsDashboardAddress,
        _ target: String
    ) async throws -> FleetMemoryPolicyFit {
        let policy = try await client.policy(at: address)
        let host = try XCTUnwrap(policy.targets.first { $0.name == target })
        return try XCTUnwrap(host.memoryPolicies, "the projection must carry the memory policy verdict")
    }

    private func state(
        _ client: FleetControlClient,
        _ address: OperationsDashboardAddress,
        _ target: String
    ) async throws -> MemoryPolicyState {
        let policy = try await client.policy(at: address)
        let host = try XCTUnwrap(policy.targets.first { $0.name == target })
        return MemoryPolicyState(target: target, declared: host.memory, report: nil)
    }
}
