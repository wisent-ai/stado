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

    /// A healthy verdict can still carry sentences, and they must not be
    /// dropped. Copied from the live `stado host link operator-host --json`
    /// answer on 2026-08-19, which exits 0 and still names one blocker: an old
    /// beacon format that predates the link block is not the host's ill health,
    /// so the command reports it without failing the verdict over it.
    ///
    /// It is also the only sentence explaining why the path, sleep, wake and
    /// interface-change fields below it read "Not reported", which is exactly
    /// why the inspector keeps it — neutral, beside the one healthy line.
    func testAHealthyVerdictKeepsTheBlockersItCameWith() throws {
        let link: HostLink = try XCTUnwrap(
            HostLinkStore.decode(
                from: """
                {"host": "operator-host", "beacon_age_seconds": 286, "ssh_reachable": true,
                 "path_kind": "unknown", "endpoint": null, "last_sleep_at": null,
                 "last_wake_at": null, "interface_changes": [], "silences": [],
                 "reader_refusals": {"window_seconds": 3600, "count": 0, "reasons": {}},
                 "verdict": "healthy",
                 "blockers": ["this host's beacon carries no link block, so its path, its sleep and wake times and its interface changes are unknown here"]}
                """
            )
        )
        XCTAssertEqual(link.verdict, .healthy)
        XCTAssertFalse(link.verdict.needsAttention, "a healthy link earns one line, not a panel")
        XCTAssertEqual(link.verdict.tone, .neutral, "a sentence on a healthy verdict is never red")
        XCTAssertEqual(
            link.blockers,
            [
                "this host's beacon carries no link block, so its path, its sleep and wake times and its interface changes are unknown here",
            ],
            "carried verbatim; dropping it loses the only explanation of the Not reported fields"
        )
        XCTAssertFalse(link.linkReported)
        XCTAssertNil(link.openSilence)
    }

    /// A host that has never published a beacon at all. `beacon_age_seconds`
    /// null must not read as "reported 0 s ago".
    func testANullBeaconAgeStaysNull() throws {
        let link: HostLink = try XCTUnwrap(
            HostLinkStore.decode(
                from: """
                {"host": "control-host", "beacon_age_seconds": null, "ssh_reachable": false,
                 "verdict": "silent", "blockers": ["no beacon has ever been published for this host"]}
                """
            )
        )
        XCTAssertNil(link.beaconAgeSeconds)
        XCTAssertFalse(link.sshReachable)
        XCTAssertEqual(link.verdict, .silent)
    }

    /// Severity is the layout, and absence by choice is never red. A healthy
    /// link earns one neutral line; the two verdicts the command exits 1 for
    /// earn a danger panel; a word this console does not know earns a warning
    /// rather than being folded into healthy.
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
            Self.decode(from: #"{"ready": true, "workers": []}"#)
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
            Self.decode(from: #"{"ready": true, "workers": []}"#)
        )
        let link: HostLink = try XCTUnwrap(HostLinkStore.decode(from: Self.fullDocument))
        let posture = FleetPosture(snapshot: snapshot, report: nil, links: [link])

        XCTAssertTrue(posture.openSilences.isEmpty)
        XCTAssertTrue(posture.decisions.isEmpty)
        XCTAssertNil(posture.silenceCommand)
    }

    /// Longest quiet first: when two hosts are both down, the one that has been
    /// down longer is the one the operator reads first.
    func testOpenSilencesAreOrderedLongestQuietFirst() throws {
        let snapshot: DashboardSnapshot = try XCTUnwrap(
            Self.decode(from: #"{"ready": true, "workers": []}"#)
        )
        let brief = try XCTUnwrap(Self.openSilenceLink(host: "gpu-host", quietFor: 320))
        let long = try XCTUnwrap(Self.openSilenceLink(host: "control-host", quietFor: 900))
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

    // MARK: Injected payloads

    private static func decode<T: Decodable>(from output: String) -> T? {
        guard let data = output.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }

    private static func openSilenceLink(host: String, quietFor seconds: TimeInterval) -> HostLink? {
        let started = Date().addingTimeInterval(-seconds)
        return HostLinkStore.decode(
            from: """
            {"host": "\(host)", "beacon_age_seconds": \(Int(seconds)), "ssh_reachable": false,
             "verdict": "silent", "blockers": ["ssh connect Operation timed out"],
             "silences": [{"host": "\(host)", "started_at": "\(started.formatted(.iso8601))",
                           "ended_at": null, "duration_seconds": null,
                           "first_reader_error": null, "observed_by": ["resolver"]}]}
            """
        )
    }

    private static let fullDocument = """
    {
      "host": "control-host",
      "beacon_age_seconds": 41,
      "ssh_reachable": true,
      "path_kind": "direct",
      "endpoint": "10.0.0.253:41641",
      "last_sleep_at": "2026-08-19T18:28:51Z",
      "last_wake_at": "2026-08-19T18:35:02Z",
      "interface_changes": [
        {"at": "2026-08-19T18:35:03Z", "detail": "en0 link up, 10.0.0.253 assigned"},
        {"at": "2026-08-19T18:29:04Z", "detail": "en0 link down"}
      ],
      "silences": [
        {
          "host": "control-host",
          "started_at": "2026-08-19T18:29:12Z",
          "ended_at": "2026-08-19T18:35:18Z",
          "duration_seconds": 366,
          "first_reader_error": "service directory cache is stale",
          "observed_by": ["resolver", "cli"]
        },
        {
          "host": "control-host",
          "started_at": "2026-08-14T02:11:40Z",
          "ended_at": "2026-08-14T02:19:02Z",
          "duration_seconds": 442,
          "first_reader_error": "registry authority exited: ssh connect Operation timed out",
          "observed_by": ["resolver"]
        }
      ],
      "reader_refusals": {
        "window_seconds": 3600,
        "count": 9,
        "reasons": {
          "directory_cache_stale": 5,
          "authority_unreachable": 3,
          "beacon_stale": 1
        }
      },
      "verdict": "degraded",
      "blockers": [
        "registry authority exited: ssh connect Operation timed out",
        "the newest beacon is 41 s old and one silence closed 6 min ago"
      ]
    }
    """

    /// Copied byte for byte from `stado host link gpu-host
    /// --json` on 2026-08-19, including the object-API refusal it was carrying
    /// that day. This is what a beacon with no link block actually looks like.
    private static let linkAbsentDocument = """
    {
      "beacon_age_seconds": 67531,
      "blockers": [
        "this host's newest beacon is 67531s old, past the 300s silence threshold",
        "this host's beacon carries no link block, so its path, its sleep and wake times and its interface changes are unknown here",
        "Stado object API error HTTP 401: {\\"error\\":\\"unauthorized or non-immutable release write\\"}"
      ],
      "endpoint": null,
      "host": "gpu-host",
      "interface_changes": [],
      "last_sleep_at": null,
      "last_wake_at": null,
      "path_kind": "unknown",
      "reader_refusals": {
        "count": 0,
        "reasons": {},
        "window_seconds": 3600
      },
      "silences": [],
      "ssh_reachable": true,
      "verdict": "degraded"
    }
    """
}
