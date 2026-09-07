import Foundation
import XCTest
@testable import Stado

/// The Memory screen's two reads and its one write, driven through the real
/// decoders and the real patch construction: the dashboard's cleanup envelope
/// carries the pass report under `memory_reclaim`, the registry projection
/// carries the declaration under the same key, and the editor composes the
/// exact body `POST /api/registry/policy` receives.
@MainActor
final class MemoryDeclarationTests: XCTestCase {
    // MARK: Reads

    func testAnUndeclaredHostIsMeasuredAgainstTheReportingDefaultAndArmsNoRepair() throws {
        let report = try XCTUnwrap(Self.memoryReport(in: MemoryReportFixtures.defaulted))
        let state = MemoryPolicyState(target: "mac-mini", declared: nil, report: report)

        XCTAssertTrue(state.isDefaulted, "policy_defaulted names the host nothing was declared for")
        XCTAssertEqual(state.mode, .report)
        XCTAssertEqual(state.lowFreeMB, 900, "the watermark is the report's low_bytes in MiB")
        XCTAssertEqual(state.targetFreeMB, 1500)
        XCTAssertEqual(state.highSwapUsedPct, 80)
        XCTAssertFalse(state.repairsArmed, "report mode with no declared repair arms nothing")
        XCTAssertEqual(state.declaredRepairNames, [])
        XCTAssertFalse(report.examinedRepairs, "a null repair table is not a table of zeros")
        XCTAssertEqual(
            MemoryRepairsSection(state: state).rows.count,
            Int.zero,
            "a host with no declared and no reported repair offers no repair row to control"
        )
        XCTAssertFalse(state.isRefusingPlacement, "this host does not declare refuse_placement")
        XCTAssertEqual(report.currentReading?.availableMB, 1300)
        XCTAssertEqual(report.currentReading?.compressorPages, 797_000)
        XCTAssertEqual(report.currentReading?.swapouts, 12_200_000)
        XCTAssertEqual(report.currentReading?.swapUsedPct, 86)
    }

    func testAHostThatHasNeverRunAPassDecodesWithEveryNumberAbsent() throws {
        let report = try XCTUnwrap(Self.memoryReport(in: MemoryReportFixtures.neverRun))

        XCTAssertEqual(report.outcome, MemoryReclaimReport.neverRun)
        XCTAssertFalse(report.hasEverRun)
        XCTAssertNil(report.currentReading?.availableBytes)
        XCTAssertNil(report.lowBytes)
        XCTAssertNil(report.durationMs)
        XCTAssertNil(report.activeJobCount)
        XCTAssertFalse(report.examinedRepairs)

        let state = MemoryPolicyState(target: "mac-mini", declared: nil, report: report)
        XCTAssertNil(state.lowFreeMB, "an absent watermark is absent, never zero")
        XCTAssertTrue(state.isDefaulted)
        XCTAssertEqual(MemoryRepairsSection(state: state).rows.count, Int.zero)
    }

    func testARefusingHostReportsThePressureAdmissionReasonAndItsRepairCounts() throws {
        let report = try XCTUnwrap(Self.memoryReport(in: MemoryReportFixtures.refusing))
        let state = MemoryPolicyState(target: "mac-mini", declared: nil, report: report)

        XCTAssertTrue(report.isRefusingPlacement, "refuse_placement with live pressure is a refusal")
        XCTAssertTrue(state.isRefusingPlacement)
        XCTAssertEqual(MemoryReclaimReport.admissionReason, "memory_pressure_active")

        let rows = MemoryRepairsSection(state: state).rows
        XCTAssertEqual(rows.map(\.name), ["restart_unit"])
        let repair = try XCTUnwrap(rows.first?.report)
        XCTAssertEqual(repair.examined, 3)
        XCTAssertEqual(repair.eligible, 1)
        XCTAssertEqual(repair.repaired, 1)
        XCTAssertEqual(repair.subjects, ["ai.wisent.precheck-runner"])
        XCTAssertEqual(repair.sortedSkipped.map(\.0), ["not_declared", "younger_than_min_age"])
        XCTAssertEqual(report.caps?.activeLabels, ["repair budget"])
        XCTAssertEqual(report.writer, "queue-agent")
    }

    func testAHostUnderItsWatermarkIsNotRefusingEvenWhenItDeclaresRefusal() throws {
        let report = try XCTUnwrap(Self.memoryReport(in: MemoryReportFixtures.declaredRefusalClear))

        XCTAssertTrue(report.refusePlacement)
        XCTAssertEqual(report.pressureActive, false)
        XCTAssertFalse(
            report.isRefusingPlacement,
            "a declared refusal only bites while the host is over a watermark"
        )
        XCTAssertTrue(report.examinedRepairs, "an empty repair table is a pass that looked")
    }

    // MARK: Write

    func testTheEditorComposesTheWhitelistedMemoryReclaimBody() throws {
        let declared = try XCTUnwrap(Self.declaration(in: MemoryReportFixtures.registryProjection))
        let state = MemoryPolicyState(target: "mac-mini", declared: declared, report: nil)

        var draft = MemoryPolicyDraft(state: state)
        XCTAssertNil(
            MemoryReclaimPatch(draft: draft, current: state),
            "an unedited draft is not a patch, so the review action stays disabled"
        )

        draft.mode = .enforce
        draft.numbers[.lowFreeMB] = "1200"
        draft.numbers[.highSwapUsedPct] = "70"
        draft.refusePlacement = true
        let patch = try XCTUnwrap(MemoryReclaimPatch(draft: draft, current: state))

        XCTAssertEqual(
            patch.canonicalJSON(target: "mac-mini"),
            MemoryReportFixtures.expectedPatchBody,
            "the reviewed body is the posted body"
        )

        let posted = try JSONSerialization.data(
            withJSONObject: FleetPolicyPatch.memoryReclaim(patch).requestBody(target: "mac-mini")
        )
        let parsed = try XCTUnwrap(JSONSerialization.jsonObject(with: posted) as? [String: Any])
        let expected = try XCTUnwrap(
            JSONSerialization.jsonObject(
                with: Data(MemoryReportFixtures.expectedPatchBody.utf8)
            ) as? [String: Any]
        )
        XCTAssertEqual(parsed as NSDictionary, expected as NSDictionary)
    }

    func testThePatchNeverCarriesARepairAndRefusesAnImpossibleWatermark() throws {
        let declared = try XCTUnwrap(Self.declaration(in: MemoryReportFixtures.registryProjection))
        let state = MemoryPolicyState(target: "mac-mini", declared: declared, report: nil)
        XCTAssertEqual(
            state.declaredRepairNames,
            ["restart_unit"],
            "the declaration's repair is read, never rewritten from here"
        )
        XCTAssertEqual(
            MemoryRepairsSection(state: state).rows.first?.declared?.units,
            ["ai.wisent.precheck-runner"]
        )

        var draft = MemoryPolicyDraft(state: state)
        draft.numbers[.highSwapUsedPct] = "140"
        XCTAssertNil(
            MemoryReclaimPatch(draft: draft, current: state),
            "swap cannot be more than wholly used, so the typed value is not a watermark"
        )

        draft.numbers[.highSwapUsedPct] = "70"
        draft.numbers[.maxRepairsPerPass] = "4"
        let patch = try XCTUnwrap(MemoryReclaimPatch(draft: draft, current: state))
        XCTAssertEqual(Set(patch.fields.keys), ["high_swap_used_pct", "max_repairs_per_pass"])
        XCTAssertNil(patch.fields["repairs"], "arming a repair is a registry declaration, never a Desktop write")
        XCTAssertFalse(patch.authorizesRepairs, "an unchanged mode authorizes nothing new")
    }

    // MARK: Decode helpers

    /// Decoded through `CleanupResponse`, which is the type the screen reads:
    /// a memory block that only decodes in isolation is a block the app
    /// cannot use.
    private static func memoryReport(in envelope: String) -> MemoryReclaimReport? {
        let response = try? JSONDecoder().decode(CleanupResponse.self, from: Data(envelope.utf8))
        return response?.report.memoryReclaim
    }

    private static func declaration(in projection: String) -> FleetMemoryPolicy? {
        let policy = try? JSONDecoder().decode(FleetPolicy.self, from: Data(projection.utf8))
        return policy?.targets.first { $0.name == "mac-mini" }?.memory
    }
}
