import SwiftUI
import WisentDesignSystem

/// Aggregation happens before rendering: the screen asks one value object what
/// needs a human, instead of every view re-deriving the same counts.
@MainActor
struct FleetPosture {
    struct Decision: Identifiable {
        let id: String
        let symbol: String
        let tone: WisentTone
        let title: String
        /// The backend's own sentence, never a paraphrase.
        let detail: String
        let meta: String
        let destination: ConsoleDestination
        /// The host this decision is about, when it is about one. The route
        /// carries it so the Hosts table opens on that row instead of on twelve.
        var host: String?
    }

    let snapshot: DashboardSnapshot
    let report: CleanupReport?
    /// What `stado host link` said about each registry host. Read here rather
    /// than re-derived: the verdict and the blockers are the command's.
    let links: [HostLink]

    var liveHosts: [WorkerNode] { snapshot.workers.filter { $0.status == .live } }
    var staleHosts: [WorkerNode] { snapshot.workers.filter { $0.status == .stale } }
    var unavailableHosts: [WorkerNode] { snapshot.workers.filter { $0.status == .unavailable } }
    var undeclaredHosts: [WorkerNode] { snapshot.workers.filter { $0.status == .live && !$0.declared } }

    var queueBlocked: Bool {
        snapshot.counts.queue > 0 && liveHosts.isEmpty
    }

    var newestFailure: FailedJob? { snapshot.recentFailed.first }

    /// Hosts with a silence record that has not closed, longest quiet first.
    ///
    /// A silence opens when the newest beacon for a host is older than the
    /// declared threshold and closes on the first fresher beacon, so an open one
    /// is a machine that is quiet right now.
    var openSilences: [(link: HostLink, silence: HostSilenceRecord)] {
        links
            .compactMap { link in link.openSilence.map { (link: link, silence: $0) } }
            .sorted { lhs, rhs in
                let left = lhs.silence.elapsedSeconds ?? 0
                let right = rhs.silence.elapsedSeconds ?? 0
                return left == right ? lhs.link.host < rhs.link.host : left > right
            }
    }

    /// The command every silence row reproduces, named once under the section
    /// rather than repeated on each row.
    var silenceCommand: String? {
        openSilences.first.map { HostLinkStore.commandLine(host: $0.link.host) }
    }
}
