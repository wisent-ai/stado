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

    /// The question an operator asked out loud — "where do I see whether
    /// anyone is logged in on that host" — answered from the command's own
    /// `session` block.
    ///
    /// `control-host` is an always-on box with nobody at its screen, and
    /// the console says exactly that in plain words. The console device and
    /// the launchd domain stay in the command's own detail sentence, which is
    /// carried unedited: it is the evidence for the headline, and an operator
    /// who doubts the headline has nowhere else to read it.
    func testAHeadlessHostSaysNobodyIsLoggedInWithoutInventingASeverity() throws {
        let link: HostLink = try XCTUnwrap(HostLinkStore.decode(from: Self.headlessDocument))

        let session = try XCTUnwrap(link.session)
        XCTAssertEqual(session.kind, .headless)
        XCTAssertEqual(session.consoleOwner, "root")
        XCTAssertEqual(
            session.detail,
            "/dev/console belongs to root, not charles: no graphical session, so gui/501 does not exist and a LaunchAgent has only the background domain user/501",
            "the resolver's own sentence, verbatim"
        )
        XCTAssertEqual(
            link.sessionLine,
            "Nobody logged in (headless)",
            "plain operator words; no domain and no console device in the headline"
        )
        XCTAssertEqual(session.headline, link.sessionLine)
    }

    /// The blocker the command adds for a headless host that declares a
    /// per-login unit arrives in the same `blockers` array as every other
    /// sentence, so it renders through the alert panel this console already
    /// has — verbatim, one entry per affected unit. Three units on the mini,
    /// three sentences, each carrying its own install command.
    func testTheHeadlessDeclarationBlockersArriveVerbatimAndInOrder() throws {
        let link: HostLink = try XCTUnwrap(HostLinkStore.decode(from: Self.headlessDocument))

        XCTAssertEqual(link.verdict, .degraded)
        XCTAssertTrue(link.verdict.needsAttention, "a degraded link earns the panel these render in")
        XCTAssertEqual(link.blockers.count, 6, "two beacon sentences, three declarations, one open silence")
        XCTAssertEqual(
            link.blockers[2],
            "nobody is logged in on the screen here, and com.wisent.compute.service.weles-keyword-planner-api is registered as a user service, so this machine cannot start it; install it as a machine service with one privileged command on the host: sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel /Users/charles/Library/LaunchAgents/com.wisent.compute.service.weles-keyword-planner-api.plist /Library/LaunchDaemons/com.wisent.compute.service.weles-keyword-planner-api.plist && /usr/bin/plutil -insert UserName -string charles /Library/LaunchDaemons/com.wisent.compute.service.weles-keyword-planner-api.plist'",
            "the command's sentence, character for character — the view never rewords it"
        )
        XCTAssertEqual(
            link.blockers[3],
            "nobody is logged in on the screen here, and com.wisent.weles-echo-api is registered as a user service, so this machine cannot start it; install it as a machine service with one privileged command on the host: sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel /Users/charles/Library/LaunchAgents/com.wisent.weles-echo-api.plist /Library/LaunchDaemons/com.wisent.weles-echo-api.plist && /usr/bin/plutil -insert UserName -string charles /Library/LaunchDaemons/com.wisent.weles-echo-api.plist'"
        )
        XCTAssertEqual(
            link.blockers[4],
            "nobody is logged in on the screen here, and com.wisent.compute.service.stado-agent-mini is registered as a user service, so this machine cannot start it; install it as a machine service with one privileged command on the host: sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel /Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist && /usr/bin/plutil -insert UserName -string charles /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist'",
            "the fleet's own agent, the unit whose absence starves the queue"
        )
        XCTAssertEqual(
            link.blockers.last,
            "a silence opened at 2026-08-18T21:15:16Z is still open",
            "the declaration sentences land among the command's other blockers, not instead of them"
        )
    }

    /// A Mac somebody is sitting at. The owner of the console is the name the
    /// line uses, because "logged in" without a name is the fact half-told:
    /// which account launchd will build a per-login domain for is the half
    /// that decides whether a LaunchAgent loads.
    func testAGraphicalSessionNamesWhoIsLoggedIn() throws {
        let link: HostLink = try XCTUnwrap(
            HostLinkStore.decode(
                from: """
                {"host": "operator-host", "beacon_age_seconds": 22, "ssh_reachable": true,
                 "path_kind": "direct", "endpoint": "100.71.4.19:41641", "verdict": "healthy",
                 "session": {"kind": "graphical", "console_owner": "lukaszbartoszcze",
                             "detail": "/dev/console belongs to lukaszbartoszcze: a graphical session exists, so launchd has gui/501"},
                 "blockers": []}
                """
            )
        )
        XCTAssertEqual(link.session?.kind, .graphical)
        XCTAssertEqual(link.sessionLine, "Logged in as lukaszbartoszcze")
        XCTAssertEqual(link.verdict, .healthy, "somebody being logged in is not a verdict")
    }

    /// The probe ran and could not tell. That is the command's own `unknown`,
    /// and it reads as nobody having said — never as nobody being there.
    func testAnUnknownSessionKindIsNotReported() throws {
        let link: HostLink = try XCTUnwrap(
            HostLinkStore.decode(
                from: """
                {"host": "gpu-host", "beacon_age_seconds": 31,
                 "ssh_reachable": true, "verdict": "healthy", "blockers": [],
                 "session": {"kind": "unknown", "console_owner": null,
                             "detail": "this host is not a mac: it has no /dev/console owner to read and no launchd domains"}}
                """
            )
        )
        XCTAssertEqual(link.session?.kind, .unknown)
        XCTAssertNil(link.session?.consoleOwner)
        XCTAssertEqual(link.sessionLine, "Not reported")
    }

    /// An older `stado` that carries no `session` object at all — the shape
    /// every host answered with before this reading existed.
    ///
    /// Absent is not headless. A console that read silence as "nobody is
    /// logged in" would be asserting the fact this reading was added to
    /// establish, on every host that never reported it.
    func testAnAbsentSessionObjectIsNotReportedRatherThanHeadless() throws {
        let link: HostLink = try XCTUnwrap(HostLinkStore.decode(from: Self.linkAbsentDocument))

        XCTAssertNil(link.session, "the command carried no session block")
        XCTAssertEqual(link.sessionLine, "Not reported")
    }

    /// A kind this console does not know is carried through rather than folded
    /// into one it does. Reading an unfamiliar word as `headless` would invent
    /// the answer.
    func testAnUnrecognisedSessionKindIsCarriedThrough() {
        XCTAssertEqual(HostLinkSessionKind("curtained"), .unrecognised("curtained"))
        XCTAssertEqual(HostLinkSessionKind("graphical"), .graphical)
        XCTAssertEqual(HostLinkSessionKind("headless"), .headless)
        XCTAssertEqual(HostLinkSessionKind(""), .unrecognised(""))
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
}
