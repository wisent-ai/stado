import Foundation
import WisentDesignSystem
import XCTest

@testable import Stado

/// What a verdict keeps and what a login session reports. Split out of
/// `FleetLinkStoreTests.swift` for the 300-line file limit.
@MainActor
final class FleetLinkSessionTests: XCTestCase {
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
        let link: HostLink = try XCTUnwrap(HostLinkStore.decode(from: FleetLinkStoreTests.headlessDocument))

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
        let link: HostLink = try XCTUnwrap(HostLinkStore.decode(from: FleetLinkStoreTests.headlessDocument))

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
        let link: HostLink = try XCTUnwrap(HostLinkStore.decode(from: FleetLinkStoreTests.linkAbsentDocument))

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
}
