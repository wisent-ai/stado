import Foundation
import WisentDesignSystem
import XCTest
@testable import Stado

/// What the console makes of a host that went quiet: how long an open
/// silence has lasted, the posture decision it raises, the order several of
/// them are shown in, and the command the panel quotes for all of it.
@MainActor
final class FleetLinkSilenceTests: XCTestCase {
    /// An open silence has no recorded duration until it closes, so its length
    /// is measured from `started_at`. Posture's decision row title reads off
    /// this, and a nil there would have printed "silent for Unavailable".
    func testAnOpenSilenceMeasuresItsOwnElapsedTime() throws {
        let started = Date().addingTimeInterval(-450)
        let link: HostLink = try XCTUnwrap(
            HostLinkStore.decode(
                from: """
                {"host": "control-host", "beacon_age_seconds": 450, "ssh_reachable": false,
                 "verdict": "silent",
                 "blockers": ["ssh connect Operation timed out", "100% ping loss over 2 packets"],
                 "silences": [{"host": "control-host",
                               "started_at": "\(started.formatted(.iso8601))",
                               "ended_at": null, "duration_seconds": null,
                               "first_reader_error": "registry authority exited: ssh connect Operation timed out",
                               "observed_by": ["resolver"]}]}
                """
            )
        )
        let silence = try XCTUnwrap(link.openSilence)
        XCTAssertTrue(silence.isOpen)
        XCTAssertNil(silence.durationSeconds)
        let elapsed = try XCTUnwrap(silence.elapsedSeconds)
        XCTAssertEqual(elapsed, 450, accuracy: 30, "measured from started_at while the gap is open")
        XCTAssertEqual(
            silence.firstReaderError,
            "registry authority exited: ssh connect Operation timed out"
        )
    }

    /// The Posture decision row: one open silence becomes one row naming the
    /// host and how long it has been quiet, routed at that host, with the
    /// command it reproduces from under the section.
    func testAnOpenSilenceBecomesAPostureDecisionRoutedAtThatHost() throws {
        let snapshot: DashboardSnapshot = try XCTUnwrap(
            LinkDocuments.decode(from: #"{"ready": true, "workers": []}"#)
        )
        let started = Date().addingTimeInterval(-366)
        let link: HostLink = try XCTUnwrap(
            HostLinkStore.decode(
                from: """
                {"host": "control-host", "beacon_age_seconds": 366, "ssh_reachable": false,
                 "verdict": "silent", "blockers": ["ssh connect Operation timed out"],
                 "silences": [{"host": "control-host",
                               "started_at": "\(started.formatted(.iso8601))",
                               "ended_at": null, "duration_seconds": null,
                               "first_reader_error": "service directory cache is stale",
                               "observed_by": ["resolver", "cli"]}]}
                """
            )
        )
        let posture = FleetPosture(snapshot: snapshot, report: nil, links: [link])

        XCTAssertEqual(posture.openSilences.count, 1)
        let decision = try XCTUnwrap(posture.decisions.first)
        XCTAssertEqual(decision.host, "control-host")
        XCTAssertEqual(decision.destination, .hosts)
        XCTAssertEqual(decision.tone, .danger)
        XCTAssertEqual(
            decision.title,
            "control-host has been silent for 6 min",
            "the row states the host and the length of the gap"
        )
        XCTAssertEqual(
            decision.detail,
            "service directory cache is stale",
            "the reader's own refusal, not a paraphrase"
        )
        XCTAssertEqual(decision.meta, "ssh silent too")
        XCTAssertEqual(
            posture.silenceCommand,
            "stado host link control-host --json",
            "the exact command the operator would type"
        )
    }

    /// A closed silence raises nothing. A gap that ended is history, and a
    /// history entry rendered as an open decision is how red stops meaning
    /// anything.
    func testAClosedSilenceRaisesNoDecision() throws {
        let snapshot: DashboardSnapshot = try XCTUnwrap(
            LinkDocuments.decode(from: #"{"ready": true, "workers": []}"#)
        )
        let link: HostLink = try XCTUnwrap(HostLinkStore.decode(from: LinkDocuments.fullDocument))
        let posture = FleetPosture(snapshot: snapshot, report: nil, links: [link])

        XCTAssertTrue(posture.openSilences.isEmpty)
        XCTAssertTrue(posture.decisions.isEmpty)
        XCTAssertNil(posture.silenceCommand)
    }

    /// Longest quiet first: when two hosts are both down, the one that has been
    /// down longer is the one the operator reads first.
    func testOpenSilencesAreOrderedLongestQuietFirst() throws {
        let snapshot: DashboardSnapshot = try XCTUnwrap(
            LinkDocuments.decode(from: #"{"ready": true, "workers": []}"#)
        )
        let brief = try XCTUnwrap(LinkDocuments.openSilenceLink(host: "gpu-host", quietFor: 320))
        let long = try XCTUnwrap(LinkDocuments.openSilenceLink(host: "control-host", quietFor: 900))
        let posture = FleetPosture(snapshot: snapshot, report: nil, links: [brief, long])

        XCTAssertEqual(
            posture.openSilences.map(\.link.host),
            ["control-host", "gpu-host"]
        )
    }

    /// The command every alarm and panel quotes is the command that runs.
    func testTheQuotedCommandIsTheInvocation() {
        XCTAssertEqual(
            HostLinkStore.linkArguments(host: "control-host"),
            ["host", "link", "control-host", "--json"]
        )
        XCTAssertEqual(
            HostLinkStore.commandLine(host: "control-host"),
            "stado host link control-host --json"
        )
    }

    /// Output that is not a link document at all yields nothing rather than a
    /// half-populated host.
    func testMalformedOutputDecodesToNothing() {
        XCTAssertNil(HostLinkStore.decode(from: ""))
        XCTAssertNil(HostLinkStore.decode(from: "   \n "))
        XCTAssertNil(HostLinkStore.decode(from: "error: unrecognized subcommand 'link'"))
    }

}
