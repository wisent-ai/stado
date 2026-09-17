import AppKit
import SwiftUI
import XCTest
@testable import Stado

/// The Fleets screen's "What the fleet lacks" inspector, drawn with real
/// data. The real `stado` operator API serves an isolated deployment, the
/// screen's own store reads `stado fleet needs --json` through it, the real
/// `FleetsView` is hosted in a window that is never ordered on screen, and
/// its display is cached to a PNG beside the source revision. The assertion
/// is the report the screen holds: the CLI's own empty sentence.
@MainActor
final class FleetNeedsScreenTests: XCTestCase {
    private static let width: CGFloat = 1280
    private static let height: CGFloat = 800
    /// The CLI's own default window, the one the screen asks for.
    private static let windowDays = 7
    private static let repository = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()

    func testTheFleetsScreenDrawsWhatTheFleetLacks() async throws {
        let fleet = try RealFleet()
        defer { fleet.stop() }
        let groupStore = try await fleet.store()
        await groupStore.refresh()
        await groupStore.refreshNeeds(days: Self.windowDays)
        let report = try XCTUnwrap(groupStore.needs)
        XCTAssertEqual(report.emptySentence, "the fleet reports no unmet need in the last \(Self.windowDays) days")

        let screen = FleetsView(groupStore: groupStore, fleetStore: FleetControlStore(), scope: "isolated")
        let png = try render(screen, as: "fleets-needs")
        let attributes = try FileManager.default.attributesOfItem(atPath: png.path)
        XCTAssertGreaterThan(attributes[.size] as? Int ?? 0, 0, "the screen drew nothing at \(png.path)")
    }

    private func render(_ screen: some View, as name: String) throws -> URL {
        let hosting = NSHostingView(rootView: screen)
        hosting.frame = CGRect(origin: .zero, size: CGSize(width: Self.width, height: Self.height))
        let window = NSWindow(contentRect: hosting.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = hosting
        hosting.layoutSubtreeIfNeeded()
        let bitmap = try XCTUnwrap(hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds), "the screen must draw")
        hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
        let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
        let directory = Self.repository.appendingPathComponent(".wisent-output/screens", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let revision = try? String(contentsOf: Self.repository.appendingPathComponent("../../.git/HEAD"), encoding: .utf8)
        try revision?.write(to: directory.appendingPathComponent("\(name).revision.txt"), atomically: true, encoding: .utf8)
        let file = directory.appendingPathComponent("\(name).png")
        try png.write(to: file)
        return file
    }
}
