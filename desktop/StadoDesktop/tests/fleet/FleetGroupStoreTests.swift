import Foundation
import XCTest
@testable import Stado

/// `FleetGroupStore` against the real product, with no stand-in anywhere. The
/// real `stado` binary serves its own operator API on loopback, the store's
/// reads and writes travel over it through the real `FleetControlClient`, the
/// API runs the real `stado fleet …` commands, and every assertion is the
/// registry document those commands left on disk or the sentence the command
/// itself printed. Isolation is data only: a temp storage root and a temp
/// `HOME`, reached by the API's children through the config file `stado config
/// init` writes in that `HOME`.
@MainActor
final class FleetGroupStoreTests: XCTestCase {
    /// A create through the store is a write on the canonical document.
    func testTheFleetCreatedThroughTheStoreIsTheFleetTheRegistryCarries() async throws {
        let fleet = try RealFleet()
        defer { fleet.stop() }
        let store = try await fleet.store()

        await store.create(name: "build", notes: "ci builders")

        guard case let .succeeded(created) = store.mutation else {
            return XCTFail("the create did not succeed: \(store.mutation)")
        }
        XCTAssertTrue(
            created.hasPrefix("fleet 'build' created (generation "),
            "the receipt is the CLI's own sentence: \(created)"
        )
        // The document on disk is the contract, not the sentence.
        XCTAssertEqual(try fleet.declaredFleets(), [["name": "build", "notes": "ci builders"]])
        XCTAssertEqual(store.fleets, [FleetGroup(name: "build", notes: "ci builders", members: [])])

        // A fleet declared with no notes reads as empty notes and no members,
        // because that is what `fleet list --json` prints for it.
        try fleet.cli(["fleet", "create", "edge"])
        await store.refresh()
        XCTAssertEqual(store.fleets.last, FleetGroup(name: "edge", notes: "", members: []))

        // Membership comes from the registry, never from this store's own
        // bookkeeping: the assignment below is made outside it.
        try fleet.cli(["fleet", "assign", "w1", "build"])
        await store.refresh()
        XCTAssertEqual(store.fleets.first?.members, ["w1"])
        XCTAssertEqual(try fleet.fleetOfRegisteredTarget(), "build")

        // The bridge rejects an empty argument, so the store's own create with
        // blank notes never reaches the registry — and says so in its words.
        await store.create(name: "spare", notes: "")
        guard case let .failed(blank) = store.mutation else {
            return XCTFail("blank notes must be refused: \(store.mutation)")
        }
        XCTAssertEqual(blank, "arguments must be non-empty, bounded strings without NUL bytes")
        // Declaring a fleet twice is refused, and the declared notes stay.
        await store.create(name: "build", notes: "a second opinion")
        guard case let .failed(duplicate) = store.mutation else {
            return XCTFail("a duplicate create must fail: \(store.mutation)")
        }
        XCTAssertEqual(RealFleet.sentence(of: duplicate), "Error: fleet 'build' already exists")
        XCTAssertEqual(try fleet.declaredFleets().compactMap { $0["name"] }, ["build", "edge"])
        XCTAssertEqual(try fleet.declaredFleets().first?["notes"], "ci builders")
    }

    /// A fleet a target still points at cannot be retired, and an empty one
    /// can. Both answers are the CLI's, and the document proves each.
    func testARefusedDeleteArrivesInTheCLIsOwnSentence() async throws {
        let fleet = try RealFleet()
        defer { fleet.stop() }
        let store = try await fleet.store()
        try fleet.cli(["fleet", "create", "build", "--notes", "ci builders"])
        try fleet.cli(["fleet", "create", "edge", "--notes", "spare"])
        try fleet.cli(["fleet", "assign", "w1", "build"])

        await store.delete(name: "build")

        guard case let .failed(refusal) = store.mutation else {
            return XCTFail("a refused delete must fail the mutation: \(store.mutation)")
        }
        XCTAssertEqual(
            RealFleet.sentence(of: refusal),
            "Error: fleet 'build' still has 1 member(s): w1; reassign them first"
        )
        // The refused delete retired nothing, on disk or on screen.
        XCTAssertEqual(try fleet.declaredFleets().compactMap { $0["name"] }, ["build", "edge"])
        XCTAssertEqual(try fleet.fleetOfRegisteredTarget(), "build")
        XCTAssertEqual(store.fleets.map(\.name), ["build", "edge"])

        await store.delete(name: "edge")

        guard case let .succeeded(deleted) = store.mutation else {
            return XCTFail("the unmanned fleet was not retired: \(store.mutation)")
        }
        XCTAssertTrue(
            deleted.hasPrefix("fleet 'edge' deleted (generation "),
            "the receipt is the CLI's own sentence: \(deleted)"
        )
        // The document lost exactly the fleet that was retired.
        XCTAssertEqual(try fleet.declaredFleets(), [["name": "build", "notes": "ci builders"]])
        XCTAssertEqual(store.fleets.map(\.name), ["build"])
    }

    /// With no registry object at all the screen carries the backend's
    /// sentence and no fleets — never an empty fleet list read from a failure.
    func testAReadFailureNamesTheBackendSentence() async throws {
        let fleet = try RealFleet()
        defer { fleet.stop() }
        let store = try await fleet.store()
        try fleet.removeRegistryDocument()

        await store.refresh()

        XCTAssertEqual(store.fleets, [])
        XCTAssertEqual(
            RealFleet.sentence(of: try XCTUnwrap(store.failure)),
            "Error: no registry document at local:registry.json"
        )
        XCTAssertNil(store.lastReadAt)
    }

    /// `stado fleet needs` through the same bridge: an idle deployment
    /// answers the CLI's own empty sentence with an empty list, and a
    /// deployment whose store cannot be read answers with the failure.
    func testWhatTheFleetLacksIsTheCLIsOwnAnswer() async throws {
        let fleet = try RealFleet()
        defer { fleet.stop() }
        let store = try await fleet.store()

        await store.refreshNeeds(days: 7)

        let report = try XCTUnwrap(store.needs)
        XCTAssertEqual(report.needs, [])
        XCTAssertEqual(report.windowDays, 7)
        XCTAssertEqual(report.emptySentence, "the fleet reports no unmet need in the last 7 days")
        XCTAssertNil(store.needsFailure)

        try fleet.removeRegistryDocument()
        await store.refreshNeeds(days: 7)
        XCTAssertEqual(
            RealFleet.sentence(of: try XCTUnwrap(store.needsFailure)),
            "Error: no registry document at local:registry.json"
        )
    }
}

