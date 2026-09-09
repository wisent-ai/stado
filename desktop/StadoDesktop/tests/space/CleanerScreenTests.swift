import Foundation
import XCTest
@testable import Stado

/// The Space screens' cleaner half: what the host declares, what reaches the
/// bytes outside the reclamation stage roots, and the command lines the two
/// buttons send.
///
/// The payloads in `CleanerFixtures` are what the built binary printed for
/// `charless-mac-mini` on 2026-09-09. That host declares all seven cleaners
/// this product implements, and the report still called
/// `~/.stado/local-storage` and `~/.stado/local-backup` places nothing looks
/// at — a sentence the screen repeated. A decoder that drops the mechanism, or
/// a label that prints `uncovered` beside a swept path, fails here.
@MainActor
final class CleanerScreenTests: XCTestCase {
    func testTheListingDecodesWhatTheHostDeclaresAndWhatItCouldArm() throws {
        let listing = try JSONDecoder().decode(
            HostCleanerListing.self,
            from: Data(CleanerFixtures.listing.utf8)
        )

        XCTAssertEqual(listing.target, "charless-mac-mini")
        XCTAssertEqual(listing.installedStado, "0.16.38")
        XCTAssertTrue(listing.declaresPolicy)

        let declared = try XCTUnwrap(listing.cleaners.first { $0.cleaner == "release_store" })
        XCTAssertTrue(declared.declared)
        XCTAssertEqual(declared.defaultRoot, ".stado/local-storage/ecosystem/releases")
        XCTAssertEqual(declared.since, "0.15.26")
        XCTAssertTrue(declared.supported)

        // The row an operator acts on: implemented here, undeclared there, and
        // the binary on that host is new enough to take it.
        let idle = try XCTUnwrap(listing.cleaners.first { $0.cleaner == "backup_twins" })
        XCTAssertFalse(idle.declared)
        XCTAssertTrue(idle.supported)
        XCTAssertEqual(
            idle.detail,
            "this product implements it and this host does not declare it; arm it with `stado space cleaners declare <target> --cleaner backup_twins`"
        )

        // And the row no button may offer, because writing it would make that
        // host reject its whole policy.
        let unsupported = try XCTUnwrap(listing.cleaners.first { $0.cleaner == "queue_workdirs" })
        XCTAssertFalse(unsupported.declared)
        XCTAssertFalse(unsupported.supported)
    }

    func testAnUnsweptPathIsLabelledWithTheMechanismThatReachesIt() throws {
        let coverage = try JSONDecoder().decode(
            HostSpaceReport.Coverage.self,
            from: Data(CleanerFixtures.coverage.utf8)
        )

        XCTAssertEqual(coverage.verdict, "uncovered")
        XCTAssertEqual(coverage.cleanerBytes, 67_600_000_000)
        XCTAssertEqual(
            coverage.unsweptBytes,
            coverage.uncoveredBytes - 67_600_000_000,
            "what nothing reaches is the remainder after the declared cleaners"
        )

        let swept = try XCTUnwrap(
            coverage.uncovered.first { $0.path == "/Users/charles/.stado/local-storage" }
        )
        XCTAssertEqual(swept.mechanism, "release_store")
        XCTAssertEqual(swept.mechanismDeclared, true)
        XCTAssertEqual(
            swept.label,
            "release_store",
            "a path a declared cleaner sweeps is never labelled uncovered again"
        )

        let unarmedRow = try XCTUnwrap(
            coverage.uncovered.first { $0.path == "/Users/charles/.stado/local-backup" }
        )
        XCTAssertEqual(unarmedRow.label, "unarmed:backup_twins")

        let stranded = try XCTUnwrap(coverage.uncovered.first { $0.path == "/private/var" })
        XCTAssertNil(stranded.mechanism)
        XCTAssertEqual(stranded.label, "uncovered")

        let unarmed = try XCTUnwrap(coverage.unarmed?.first)
        XCTAssertEqual(unarmed.cleaner, "backup_twins")
        XCTAssertTrue(unarmed.supported)
        XCTAssertTrue(unarmed.detail.contains("does not declare it"))
    }

    /// A Desktop meeting an installed `stado` that predates the mechanism
    /// fields still renders: the rest of the report is readable and every row
    /// reads as the word that older binary meant.
    func testAnOlderBinarysCoverageStillDecodes() throws {
        let coverage = try JSONDecoder().decode(
            HostSpaceReport.Coverage.self,
            from: Data(CleanerFixtures.coverageWithoutMechanisms.utf8)
        )

        XCTAssertNil(coverage.cleanerBytes)
        XCTAssertNil(coverage.unsweptBytes)
        XCTAssertNil(coverage.unarmed)
        XCTAssertEqual(coverage.uncovered.first?.label, "uncovered")
    }

    func testTheButtonsSendTheDocumentedCommandLines() {
        XCTAssertEqual(
            HostCleanersStore.listArguments(host: "charless-mac-mini"),
            ["space", "cleaners", "list", "charless-mac-mini", "--json"]
        )
        XCTAssertEqual(
            HostCleanersStore.declareArguments(
                host: "charless-mac-mini",
                cleaner: "release_store"
            ),
            [
                "space", "cleaners", "declare", "charless-mac-mini",
                "--cleaner", "release_store", "--json",
            ]
        )
        XCTAssertEqual(
            HostCleanersStore.withdrawArguments(
                host: "charless-mac-mini",
                cleaner: "release_store"
            ),
            [
                "space", "cleaners", "remove", "charless-mac-mini",
                "--cleaner", "release_store", "--json",
            ]
        )
    }
}
