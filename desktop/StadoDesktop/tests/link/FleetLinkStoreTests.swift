import Foundation
import WisentDesignSystem
import XCTest
@testable import Stado

/// The Link reading's decode and severity paths, driven by injecting the
/// command's own `--json` payload rather than by running anything.
///
/// The document below is the shape `stado host link` prints for the
/// 2026-08-19 connectivity gap on `control-host`: a six-minute silence
/// bounded by 18:29 and 18:35 UTC, `direct 10.0.0.253:41641` afterwards, and
/// the two reader refusals that reached nothing but
/// `~/.stado/logs/stado-resolver.err` before the refusal records existed. The
/// refusal sentences are quoted, not paraphrased: a console that rewords them
/// becomes a second opinion about why a reader gave up.
@MainActor
final class FleetLinkStoreTests: XCTestCase {
    func testDecodesTheWholeLinkDocument() throws {
        let link: HostLink = try XCTUnwrap(HostLinkStore.decode(from: Self.fullDocument))

        XCTAssertEqual(link.host, "control-host")
        XCTAssertEqual(link.beaconAgeSeconds, 41)
        XCTAssertTrue(link.sshReachable)
        XCTAssertEqual(link.pathKind, .direct)
        XCTAssertEqual(link.endpoint, "10.0.0.253:41641")
        XCTAssertEqual(link.lastSleepAt, "2026-08-19T18:28:51Z")
        XCTAssertEqual(link.lastWakeAt, "2026-08-19T18:35:02Z")
        XCTAssertTrue(link.linkReported, "the beacon carried a link block")

        XCTAssertEqual(link.interfaceChanges.count, 2)
        XCTAssertEqual(link.interfaceChanges[0].at, "2026-08-19T18:35:03Z")
        XCTAssertEqual(link.interfaceChanges[0].detail, "en0 link up, 10.0.0.253 assigned")

        XCTAssertEqual(link.silences.count, 2, "newest first, as the command orders them")
        let newest = try XCTUnwrap(link.silences.first)
        XCTAssertEqual(newest.startedAt, "2026-08-19T18:29:12Z")
        XCTAssertEqual(newest.endedAt, "2026-08-19T18:35:18Z")
        XCTAssertEqual(newest.durationSeconds, 366)
        XCTAssertEqual(newest.elapsedSeconds, 366)
        XCTAssertFalse(newest.isOpen)
        XCTAssertEqual(newest.observedBy, ["resolver", "cli"])
        XCTAssertEqual(
            newest.firstReaderError,
            "service directory cache is stale",
            "the reader's own sentence, verbatim"
        )
        XCTAssertNil(link.openSilence, "both recorded silences closed")

        let refusals = try XCTUnwrap(link.readerRefusals)
        XCTAssertEqual(refusals.windowSeconds, 3_600)
        XCTAssertEqual(refusals.count, 9)
        XCTAssertEqual(refusals.reasons["directory_cache_stale"], 5)
        XCTAssertEqual(refusals.reasons["authority_unreachable"], 3)
        XCTAssertEqual(refusals.reasons["beacon_stale"], 1)
        XCTAssertEqual(
            refusals.rankedReasons.map(\.reason),
            ["directory_cache_stale", "authority_unreachable", "beacon_stale"],
            "commonest reason first, ties broken by token so two reads order alike"
        )

        XCTAssertEqual(link.verdict, .degraded)
        XCTAssertEqual(
            link.blockers,
            [
                "registry authority exited: ssh connect Operation timed out",
                "the newest beacon is 41 s old and one silence closed 6 min ago",
            ],
            "blockers arrive verbatim and in the command's order"
        )
    }

    /// A beacon with no `link` block, copied verbatim from what
    /// `stado host link gpu-host --json` answered on
    /// 2026-08-19: the command prints `path_kind: "unknown"` with every other
    /// link field empty and names the absence in its own blocker sentence.
    ///
    /// A bare `unknown` is therefore the absence of a report, not a report of
    /// an unknown path, and the console must not read it as one — the
    /// difference is whether an operator chases the network or the collector.
    func testALinkBlockThatWasNeverCollectedIsNotReportedRatherThanAPath() throws {
        let link: HostLink = try XCTUnwrap(HostLinkStore.decode(from: Self.linkAbsentDocument))

        XCTAssertEqual(link.host, "gpu-host")
        XCTAssertEqual(link.pathKind, .unknown, "the command's own word survives decode")
        XCTAssertFalse(link.linkReported, "unknown with nothing else is no link block at all")
        XCTAssertNil(link.endpoint)
        XCTAssertNil(link.lastSleepAt)
        XCTAssertNil(link.lastWakeAt)
        XCTAssertTrue(link.interfaceChanges.isEmpty)
        XCTAssertTrue(link.silences.isEmpty)
        XCTAssertNil(link.openSilence)
        XCTAssertEqual(link.beaconAgeSeconds, 67_531)
        XCTAssertTrue(link.sshReachable, "ssh answered; only the beacon stopped")
        XCTAssertEqual(link.verdict, .degraded)
        XCTAssertEqual(
            link.blockers,
            [
                "this host's newest beacon is 67531s old, past the 300s silence threshold",
                "this host's beacon carries no link block, so its path, its sleep and wake times and its interface changes are unknown here",
                "Stado object API error HTTP 401: {\"error\":\"unauthorized or non-immutable release write\"}",
            ],
            "the command's sentences, verbatim — the console never rewords them"
        )

        let refusals = try XCTUnwrap(link.readerRefusals)
        XCTAssertEqual(refusals.count, 0)
        XCTAssertEqual(refusals.windowSeconds, 3_600)
        XCTAssertTrue(refusals.rankedReasons.isEmpty)
    }

    /// The same absence spelled with a null. Either way the path is unreported
    /// and never a fabricated value.
    func testANullPathKindIsAlsoUnreported() throws {
        let link: HostLink = try XCTUnwrap(
            HostLinkStore.decode(
                from: """
                {"host": "operator-host", "beacon_age_seconds": 22, "ssh_reachable": true,
                 "path_kind": null, "endpoint": null, "verdict": "healthy", "blockers": []}
                """
            )
        )
        XCTAssertNil(link.pathKind)
        XCTAssertFalse(link.linkReported)
        XCTAssertNil(link.readerRefusals, "an absent aggregate is absent, not zero refusals")
        XCTAssertEqual(link.verdict, .healthy)
        XCTAssertTrue(link.blockers.isEmpty)
    }

    func testVerdictToneAndAttention() {
        XCTAssertEqual(HostLinkVerdict("healthy"), .healthy)
        XCTAssertEqual(HostLinkVerdict.healthy.tone, .neutral)
        XCTAssertFalse(HostLinkVerdict.healthy.needsAttention)

        XCTAssertEqual(HostLinkVerdict("silent"), .silent)
        XCTAssertEqual(HostLinkVerdict.silent.tone, .danger)
        XCTAssertTrue(HostLinkVerdict.silent.needsAttention)

        XCTAssertEqual(HostLinkVerdict("degraded"), .degraded)
        XCTAssertEqual(HostLinkVerdict.degraded.tone, .danger)
        XCTAssertTrue(HostLinkVerdict.degraded.needsAttention)

        let unknown = HostLinkVerdict("wedged")
        XCTAssertEqual(unknown, .unrecognised("wedged"))
        XCTAssertEqual(unknown.tone, .warning, "a verdict nobody recognised never reads as fine")
        XCTAssertTrue(unknown.needsAttention)
        XCTAssertEqual(unknown.word, "wedged", "the command's own word survives")
        XCTAssertEqual(HostLinkVerdict("").word, "unreported")
    }

    /// The route a path kind takes is a fact, not a severity: a relay is slower
    /// but working, and a collector that could not tell is not an outage.
    func testPathKindCarriesTheBeaconsOwnWordAndNoSeverity() {
        XCTAssertEqual(HostLinkPathKind("relay").word, "relay")
        XCTAssertEqual(HostLinkPathKind("relay").tone, .neutral)
        XCTAssertEqual(HostLinkPathKind("unknown"), .unknown)
        XCTAssertEqual(HostLinkPathKind("unknown").tone, .neutral)
        XCTAssertEqual(HostLinkPathKind("mesh-exit"), .unrecognised("mesh-exit"))
        XCTAssertEqual(HostLinkPathKind("mesh-exit").word, "mesh-exit")
    }
}
