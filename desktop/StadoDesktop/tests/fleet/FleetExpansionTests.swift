import AppKit
import SwiftUI
import XCTest
@testable import Stado

/// Real HTTP operator API and binary; unknowns and conflicts survive the GUI.
@MainActor
final class FleetExpansionTests: XCTestCase {
    func testCatalogAndPlanPersistThroughTheGraphicalSurface() async throws {
        let fleet = try RealFleet()
        defer { fleet.stop() }
        let groups = try await fleet.store()
        let store = FleetExpansionStore()
        try fleet.recordMissingMacDemand()
        store.configure(address: groups.address, token: groups.authorizationToken)
        await store.load(days: "7")
        XCTAssertNil(store.failure)
        XCTAssertTrue(store.loaded)
        let formatter = ISO8601DateFormatter()
        store.options = [FleetExpansionOption(id: "mac", label: "Declared Mac scenario", kind: "buy",
            needKeys: ["host:darwin-arm64"], benefitGroup: "gui-work", upfrontUsd: nil,
            monthlyCostUsd: 20, monthlySavingsUsd: 100, monthlyMarginUsd: 0, leadTimeDays: 0,
            evidence: "Explicit journey assumptions, not a vendor quote or measured earnings",
            observedAt: formatter.string(from: Date().addingTimeInterval(-60)),
            validUntil: formatter.string(from: Date().addingTimeInterval(3600)))]
        await store.save()
        XCTAssertNil(store.failure)
        let version = try XCTUnwrap(store.catalogVersion)
        let reader = FleetExpansionStore()
        reader.configure(address: groups.address, token: groups.authorizationToken)
        await reader.load(days: "7")
        XCTAssertNil(reader.options.first?.upfrontUsd)
        XCTAssertEqual(reader.options.first?.id, "mac")
        XCTAssertEqual(reader.catalogVersion, version)
        await store.plan(budget: "10000", months: "24", days: "7")
        let report = try XCTUnwrap(store.report, store.failure ?? "no report")
        XCTAssertEqual(report.status, "insufficient_evidence")
        XCTAssertTrue(report.portfolio.selectedIds.isEmpty)
        await reader.show(id: report.id)
        let persisted = try fleet.expansionPlan(id: report.id)
        XCTAssertEqual(persisted["plan_id"] as? String, report.id)
        XCTAssertEqual(persisted["status"] as? String, report.status)
        XCTAssertEqual(reader.report?.id, report.id)
        XCTAssertNil(reader.report?.candidates.first?.monthlyNetUsd)
        store.options[0].upfrontUsd = 1000
        await store.save()
        XCTAssertNil(store.failure)
        await store.plan(budget: "10000", months: "24", days: "7")
        let ready = try XCTUnwrap(store.report)
        XCTAssertEqual(ready.status, "ready")
        XCTAssertEqual(ready.portfolio.selectedIds, ["mac"])
        XCTAssertEqual(ready.portfolio.paybackMonths, 12.5)
        XCTAssertEqual(ready.portfolio.horizonNetUsd, 920)
        XCTAssertEqual(try fleet.expansionPlan(id: ready.id)["status"] as? String, "ready")
        store.options = []
        await store.save()
        XCTAssertNil(store.failure)
        await reader.save()
        XCTAssertTrue(reader.failure?.contains("version conflict") == true)
        XCTAssertEqual(reader.options.first?.id, "mac", "refusal preserves unsaved edits")
        await reader.load(days: "7")
        XCTAssertTrue(reader.options.isEmpty)
        XCTAssertTrue(reader.plans.contains { $0.id == report.id })
        await reader.show(id: ready.id)
        try render(FleetExpansionView(groupStore: groups, store: reader), name: ready.id)
        reader.configure(address: nil, token: nil)
        XCTAssertNil(reader.report)
        XCTAssertTrue(reader.plans.isEmpty)
        XCTAssertTrue(reader.options.isEmpty)
    }

    private func render(_ screen: some View, name: String) throws {
        let hosting = NSHostingView(rootView: screen)
        hosting.frame = CGRect(origin: .zero, size: CGSize(width: FleetExpansionDefaults.width, height: FleetExpansionDefaults.height))
        let window = NSWindow(contentRect: hosting.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = hosting
        hosting.layoutSubtreeIfNeeded()
        let bitmap = try XCTUnwrap(hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds))
        hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
        let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
        let package = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let directory = package.appendingPathComponent(".wisent-output/expansion-screens")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try data.write(to: directory.appendingPathComponent("\(name).png"))
    }
}
