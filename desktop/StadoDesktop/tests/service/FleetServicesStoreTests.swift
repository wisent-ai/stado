import Foundation
import WisentDesignSystem
import XCTest
@testable import Stado

/// What `stado service list --json` says about a declaration the host cannot
/// honour, driven by injecting the command's own payload rather than by running
/// anything.
///
/// The rows below are copied from the live answer on 2026-08-19. Three of the
/// twenty-two rows the fleet reports carry a `misdeclared_domain` object, all
/// three on `charless-mac-mini`, and the first of them is the fleet's own
/// agent: a LaunchAgent on a machine nobody logs in to, which launchd there
/// has no per-login domain to load. That is why the host publishes no
/// capacity, which is why a job pinned to it waited 122 hours — and no screen
/// in the product said any of it.
///
/// Every sentence and every command is asserted character for character. A
/// console that rewords the finding becomes a second opinion about why a unit
/// cannot load, and an install command this console composed itself is a
/// command nobody has ever run.
@MainActor
final class FleetServicesStoreTests: XCTestCase {
    func testTheDeclarationFindingDecodesWithItsSentenceAndInstallCommand() throws {
        let rows = try XCTUnwrap(FleetServicesStore.decode(from: Self.listDocument))
        let agent = try XCTUnwrap(rows.first { $0.unitID == Self.agentUnit })

        XCTAssertEqual(agent.host, "charless-mac-mini")
        XCTAssertEqual(agent.state, "missing")
        XCTAssertEqual(agent.domain, .user, "a home LaunchAgent loads only for a logged-in account")

        let finding = try XCTUnwrap(agent.misdeclaredDomain)
        XCTAssertEqual(finding.host, "charless-mac-mini")
        XCTAssertEqual(finding.unit, Self.agentUnit)
        XCTAssertEqual(
            finding.path,
            "/Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist"
        )
        XCTAssertEqual(finding.declaredDomain, "user")
        XCTAssertEqual(finding.loadableDomain, "system")
        XCTAssertEqual(
            finding.daemonPath,
            "/Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist"
        )
        XCTAssertEqual(
            finding.installCommand,
            "sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel /Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist && /usr/bin/plutil -insert UserName -string charles /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist'",
            "the command the CLI printed, verbatim: the inspector offers it to be copied and run"
        )
        XCTAssertEqual(
            finding.detail,
            "com.wisent.compute.service.stado-agent-mini is declared in launchd's user domain (/Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist), and charless-mac-mini is declared always-on, so no account is logged in graphically there, launchd builds no gui/<uid>, and system is the only domain that host can load a unit into; install it there with one privileged command on the host: sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel /Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist && /usr/bin/plutil -insert UserName -string charles /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist'",
            "the finding sentence the CLI and registry doctor both print, unedited"
        )
    }

    /// The object is absent — not null — on every row where the declaration
    /// and the host agree, which is how a finding stays a minority state. A
    /// row that never carried one must not decode into an empty finding.
    func testARowWithoutTheFindingCarriesNone() throws {
        let rows = try XCTUnwrap(FleetServicesStore.decode(from: Self.listDocument))

        let daemon = try XCTUnwrap(rows.first { $0.unitID == "com.wisent.always-on.weles" })
        XCTAssertNil(
            daemon.misdeclaredDomain,
            "a system LaunchDaemon on an always-on host is where it belongs"
        )
        XCTAssertEqual(daemon.domain, .system)

        let interactive = try XCTUnwrap(rows.first { $0.host == "lukasz-macbook" })
        XCTAssertEqual(interactive.domain, .user, "the same kind of home LaunchAgent as the mini's")
        XCTAssertNil(
            interactive.misdeclaredDomain,
            "a user agent on a machine somebody logs in to is not a finding"
        )
    }

    /// The count the facet rail shows, and the rows the facet lists. Three of
    /// five rows here, so the finding stays a minority state on this screen
    /// exactly as it is on the fleet: three of twenty-two.
    func testTheStoreNarrowsToTheRowsCarryingTheFinding() throws {
        let rows = try XCTUnwrap(FleetServicesStore.decode(from: Self.listDocument))
        XCTAssertEqual(rows.count, 5)

        let flagged = rows.filter { $0.misdeclaredDomain != nil }
        XCTAssertEqual(
            flagged.map(\.unitID).sorted(),
            [
                "com.wisent.compute.service.stado-agent-mini",
                "com.wisent.compute.service.weles-keyword-planner-api",
                "com.wisent.weles-echo-api",
            ],
            "the three units charless-mac-mini declares where it cannot load them"
        )
        XCTAssertTrue(
            flagged.allSatisfy { $0.host == "charless-mac-mini" },
            "the host with no graphical session is the only host with the finding"
        )
    }

    /// Output that is not a service list at all yields nothing rather than an
    /// empty fleet: a screen that reads "no managed services" from a parse
    /// failure is a screen reporting a fleet nobody declared.
    func testMalformedOutputDecodesToNothing() {
        XCTAssertNil(FleetServicesStore.decode(from: ""))
        XCTAssertNil(FleetServicesStore.decode(from: "  \n "))
        XCTAssertNil(FleetServicesStore.decode(from: "error: unrecognized subcommand 'list'"))
    }

    /// The command every panel on this screen quotes is the command that runs.
    func testTheQuotedCommandIsTheInvocation() {
        XCTAssertEqual(FleetServicesStore.listArguments(), ["service", "list", "--json"])
    }

    // MARK: Injected payload

    private static let agentUnit = "com.wisent.compute.service.stado-agent-mini"

    /// Five rows out of the live twenty-two: the mini's three findings, one
    /// system LaunchDaemon on the same host that is where it belongs, and the
    /// same kind of home LaunchAgent on the Mac somebody logs in to — which
    /// carries no finding, because there the domain exists. Field for field as
    /// `stado service list --json` printed them on 2026-08-19.
    private static let listDocument = """
    [
      {
        "host": "charless-mac-mini",
        "host_heuristic": null,
        "name": "com.wisent.compute.service.stado-agent-mini",
        "unit": "",
        "label": "com.wisent.compute.service.stado-agent-mini",
        "unit_id": "com.wisent.compute.service.stado-agent-mini",
        "path": "/Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist",
        "kind": "launchd",
        "source": "registry",
        "managed_since": "2026-08-19T00:46:51.797832+00:00",
        "program": "",
        "args": [],
        "state": "missing",
        "reported_at": "2026-08-18T21:15:16Z",
        "detail": "declared here; the latest beacon does not report it",
        "misdeclared_domain": {
          "host": "charless-mac-mini",
          "unit": "com.wisent.compute.service.stado-agent-mini",
          "path": "/Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist",
          "declared_domain": "user",
          "loadable_domain": "system",
          "daemon_path": "/Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist",
          "install_command": "sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel /Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist && /usr/bin/plutil -insert UserName -string charles /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist'",
          "detail": "com.wisent.compute.service.stado-agent-mini is declared in launchd's user domain (/Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist), and charless-mac-mini is declared always-on, so no account is logged in graphically there, launchd builds no gui/<uid>, and system is the only domain that host can load a unit into; install it there with one privileged command on the host: sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel /Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist && /usr/bin/plutil -insert UserName -string charles /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist'"
        }
      },
      {
        "host": "charless-mac-mini",
        "name": "weles-keyword-planner-api",
        "unit": "",
        "label": "com.wisent.compute.service.weles-keyword-planner-api",
        "unit_id": "com.wisent.compute.service.weles-keyword-planner-api",
        "path": "/Users/charles/Library/LaunchAgents/com.wisent.compute.service.weles-keyword-planner-api.plist",
        "kind": "launchd",
        "source": "registry",
        "state": "missing",
        "reported_at": "2026-08-18T21:15:16Z",
        "detail": "declared here; the latest beacon does not report it",
        "misdeclared_domain": {
          "host": "charless-mac-mini",
          "unit": "com.wisent.compute.service.weles-keyword-planner-api",
          "path": "/Users/charles/Library/LaunchAgents/com.wisent.compute.service.weles-keyword-planner-api.plist",
          "declared_domain": "user",
          "loadable_domain": "system",
          "daemon_path": "/Library/LaunchDaemons/com.wisent.compute.service.weles-keyword-planner-api.plist",
          "install_command": "sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel /Users/charles/Library/LaunchAgents/com.wisent.compute.service.weles-keyword-planner-api.plist /Library/LaunchDaemons/com.wisent.compute.service.weles-keyword-planner-api.plist && /usr/bin/plutil -insert UserName -string charles /Library/LaunchDaemons/com.wisent.compute.service.weles-keyword-planner-api.plist'",
          "detail": "com.wisent.compute.service.weles-keyword-planner-api is declared in launchd's user domain (/Users/charles/Library/LaunchAgents/com.wisent.compute.service.weles-keyword-planner-api.plist), and charless-mac-mini is declared always-on, so no account is logged in graphically there, launchd builds no gui/<uid>, and system is the only domain that host can load a unit into; install it there with one privileged command on the host: sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel /Users/charles/Library/LaunchAgents/com.wisent.compute.service.weles-keyword-planner-api.plist /Library/LaunchDaemons/com.wisent.compute.service.weles-keyword-planner-api.plist && /usr/bin/plutil -insert UserName -string charles /Library/LaunchDaemons/com.wisent.compute.service.weles-keyword-planner-api.plist'"
        }
      },
      {
        "host": "charless-mac-mini",
        "name": "com.wisent.weles-echo-api",
        "unit": "",
        "label": "com.wisent.weles-echo-api",
        "unit_id": "com.wisent.weles-echo-api",
        "path": "/Users/charles/Library/LaunchAgents/com.wisent.weles-echo-api.plist",
        "kind": "launchd",
        "source": "registry",
        "state": "missing",
        "reported_at": "2026-08-18T21:15:16Z",
        "detail": "declared here; the latest beacon does not report it",
        "misdeclared_domain": {
          "host": "charless-mac-mini",
          "unit": "com.wisent.weles-echo-api",
          "path": "/Users/charles/Library/LaunchAgents/com.wisent.weles-echo-api.plist",
          "declared_domain": "user",
          "loadable_domain": "system",
          "daemon_path": "/Library/LaunchDaemons/com.wisent.weles-echo-api.plist",
          "install_command": "sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel /Users/charles/Library/LaunchAgents/com.wisent.weles-echo-api.plist /Library/LaunchDaemons/com.wisent.weles-echo-api.plist && /usr/bin/plutil -insert UserName -string charles /Library/LaunchDaemons/com.wisent.weles-echo-api.plist'",
          "detail": "com.wisent.weles-echo-api is declared in launchd's user domain (/Users/charles/Library/LaunchAgents/com.wisent.weles-echo-api.plist), and charless-mac-mini is declared always-on, so no account is logged in graphically there, launchd builds no gui/<uid>, and system is the only domain that host can load a unit into; install it there with one privileged command on the host: sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel /Users/charles/Library/LaunchAgents/com.wisent.weles-echo-api.plist /Library/LaunchDaemons/com.wisent.weles-echo-api.plist && /usr/bin/plutil -insert UserName -string charles /Library/LaunchDaemons/com.wisent.weles-echo-api.plist'"
        }
      },
      {
        "host": "charless-mac-mini",
        "name": "com.wisent.always-on.weles",
        "unit": "",
        "label": "com.wisent.always-on.weles",
        "unit_id": "com.wisent.always-on.weles",
        "path": "/Library/LaunchDaemons/com.wisent.always-on.weles.plist",
        "kind": "launchd",
        "source": "registry",
        "managed_since": "2026-08-04T00:36:01.183071+00:00",
        "state": "active",
        "reported_at": "2026-08-18T21:15:16Z",
        "detail": ""
      },
      {
        "host": "lukasz-macbook",
        "name": "oko-autonomy",
        "unit": "",
        "label": "com.wisent.compute.service.oko-autonomy",
        "unit_id": "com.wisent.compute.service.oko-autonomy",
        "path": "/Users/lukaszbartoszcze/Library/LaunchAgents/com.wisent.compute.service.oko-autonomy.plist",
        "kind": "launchd",
        "source": "registry",
        "managed_since": "2026-08-14T04:37:55.120343+00:00",
        "state": "missing",
        "reported_at": "2026-08-19T21:20:56Z",
        "detail": "declared here; the latest beacon does not report it"
      }
    ]
    """
}
