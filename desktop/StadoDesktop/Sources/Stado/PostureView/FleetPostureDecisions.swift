import SwiftUI
import WisentDesignSystem

/// The one list on the screen that asks for a human, derived from the published
/// snapshot and from what `stado host link` reported.
extension FleetPosture {
    var decisions: [Decision] {
        var items: [Decision] = []
        // First: a host nobody can reach is why the rows under it look the way
        // they do, and it is the one state that left no trace at all before
        // silence records existed.
        for entry in openSilences {
            items.append(
                Decision(
                    id: "silent-\(entry.link.host)-\(entry.silence.startedAt)",
                    symbol: "wifi.slash",
                    tone: .danger,
                    title: "\(entry.link.host) has been silent for \(StadoFormat.duration(entry.silence.elapsedSeconds))",
                    detail: entry.silence.firstReaderError
                        ?? (entry.link.blockers.first ?? "No beacon has arrived since \(entry.silence.startedAt)."),
                    meta: entry.link.sshReachable ? "ssh answers" : "ssh silent too",
                    destination: .hosts,
                    host: entry.link.host
                )
            )
        }
        for host in unavailableHosts {
            items.append(
                Decision(
                    id: "unavailable-\(host.id)",
                    symbol: "xmark.octagon.fill",
                    tone: .danger,
                    title: host.displayName,
                    detail: host.availabilityReason,
                    meta: ConsoleFormat.age(host.ageSeconds),
                    destination: .hosts
                )
            )
        }
        for host in staleHosts {
            items.append(
                Decision(
                    id: "stale-\(host.id)",
                    symbol: "clock.badge.exclamationmark.fill",
                    tone: .warning,
                    title: host.displayName,
                    detail: host.availabilityReason,
                    meta: ConsoleFormat.age(host.ageSeconds),
                    destination: .hosts
                )
            )
        }
        for host in undeclaredHosts {
            items.append(
                Decision(
                    id: "undeclared-\(host.id)",
                    symbol: "questionmark.square.dashed",
                    tone: .warning,
                    title: host.displayName,
                    detail: "Publishes capacity but is not declared in the canonical registry.",
                    meta: "Undeclared",
                    destination: .registry
                )
            )
        }
        for job in snapshot.recentFailed {
            items.append(
                Decision(
                    id: "failed-\(job.jobID)",
                    symbol: "exclamationmark.triangle.fill",
                    tone: .danger,
                    title: "Job \(job.jobID)",
                    detail: job.error ?? "The dashboard published this failure without a sanitized reason.",
                    meta: job.model ?? "No model",
                    destination: .queue
                )
            )
        }
        if let report, report.pressureActive == true {
            items.append(
                Decision(
                    id: "disk-pressure",
                    symbol: "externaldrive.fill.badge.exclamationmark",
                    tone: report.outcomePresentation.severity == .critical ? .danger : .warning,
                    title: "Disk pressure on the dashboard host",
                    detail: report.errors.first ?? report.outcomePresentation.detail,
                    meta: DisplayFormat.bytes(report.freeBytesAfter),
                    destination: .disk
                )
            )
        }
        return items
    }

    var decisionCount: Int { decisions.count }
}
