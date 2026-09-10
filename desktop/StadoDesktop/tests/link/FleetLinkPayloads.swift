import Foundation
import WisentDesignSystem
import XCTest

@testable import Stado

/// The exact `stado host link --json` payloads these cases decode, and the
/// one decoder they share. Split out of `FleetLinkStoreTests.swift`, which
/// had grown past the 300-line file limit.
extension FleetLinkStoreTests {
    static func decode<T: Decodable>(from output: String) -> T? {
        guard let data = output.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }

    static func openSilenceLink(host: String, quietFor seconds: TimeInterval) -> HostLink? {
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

    static let fullDocument = """
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
    static let linkAbsentDocument = """
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

    /// `control-host` as `stado host link control-host --json` answers
    /// for it, copied from the live payload on 2026-08-19: the beacon age, the
    /// open silence, the two sentences the command already printed, the
    /// `session` block, and one declaration sentence per affected unit — in the
    /// order the document prints them, which is the order the panel shows them
    /// in.
    ///
    /// The install commands inside the blockers are the same strings the same
    /// host's `service list --json` rows carry, because the command composes
    /// both from one place. Two surfaces quoting one fact must quote it
    /// identically or the operator has to decide which one to trust.
    static let headlessDocument = """
    {
      "beacon_age_seconds": 88038,
      "blockers": [
        "this host's newest beacon is 88038s old, past the 300s silence threshold",
        "this host's beacon carries no link block, so its path, its sleep and wake times and its interface changes are unknown here",
        "nobody is logged in on the screen here, and com.wisent.compute.service.weles-keyword-planner-api is registered as a user service, so this machine cannot start it; install it as a machine service with one privileged command on the host: sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel /Users/charles/Library/LaunchAgents/com.wisent.compute.service.weles-keyword-planner-api.plist /Library/LaunchDaemons/com.wisent.compute.service.weles-keyword-planner-api.plist && /usr/bin/plutil -insert UserName -string charles /Library/LaunchDaemons/com.wisent.compute.service.weles-keyword-planner-api.plist'",
        "nobody is logged in on the screen here, and com.wisent.weles-echo-api is registered as a user service, so this machine cannot start it; install it as a machine service with one privileged command on the host: sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel /Users/charles/Library/LaunchAgents/com.wisent.weles-echo-api.plist /Library/LaunchDaemons/com.wisent.weles-echo-api.plist && /usr/bin/plutil -insert UserName -string charles /Library/LaunchDaemons/com.wisent.weles-echo-api.plist'",
        "nobody is logged in on the screen here, and com.wisent.compute.service.stado-agent-mini is registered as a user service, so this machine cannot start it; install it as a machine service with one privileged command on the host: sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel /Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist && /usr/bin/plutil -insert UserName -string charles /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist'",
        "a silence opened at 2026-08-18T21:15:16Z is still open"
      ],
      "endpoint": null,
      "host": "control-host",
      "interface_changes": [],
      "last_sleep_at": null,
      "last_wake_at": null,
      "path_kind": "unknown",
      "reader_refusals": {
        "count": 0,
        "reasons": {},
        "window_seconds": 3600
      },
      "session": {
        "kind": "headless",
        "console_owner": "root",
        "detail": "/dev/console belongs to root, not charles: no graphical session, so gui/501 does not exist and a LaunchAgent has only the background domain user/501"
      },
      "silences": [
        {
          "host": "control-host",
          "started_at": "2026-08-18T21:15:16Z",
          "ended_at": null,
          "duration_seconds": null,
          "first_reader_error": null,
          "observed_by": ["resolver"]
        }
      ],
      "ssh_reachable": true,
      "verdict": "degraded"
    }
    """
}
