import SwiftUI
import WisentDesignSystem

/// The strip and the counter row: the healthy facts, in the order the screen
/// reads them.
extension FleetPosture {
    /// A healthy fact is one line. It never grows into a panel just because
    /// there was room for one.
    var signals: [WisentSignal] {
        var values: [WisentSignal] = [
            WisentSignal(
                "Queued",
                value: snapshot.counts.queue.formatted(.number),
                tone: snapshot.counts.queue > 0 ? .warning : .neutral
            ),
            WisentSignal(
                "Running",
                value: snapshot.counts.running.formatted(.number),
                tone: snapshot.counts.running > 0 ? .success : .neutral
            ),
            WisentSignal(
                "Live hosts",
                value: "\(liveHosts.count) of \(snapshot.workers.count)",
                tone: liveHosts.isEmpty ? .warning : .success
            ),
            WisentSignal(
                "Accepting jobs",
                value: snapshot.workers.count { $0.acceptingJobs == true }.formatted(.number),
                tone: snapshot.workers.contains { $0.acceptingJobs == true } ? .success : .warning
            ),
        ]
        if let report {
            values.append(
                WisentSignal(
                    "Free disk",
                    value: DisplayFormat.bytes(report.freeBytesAfter),
                    tone: report.pressureActive == true ? .warning : .success
                )
            )
        }
        return values
    }

    var counters: [WisentCounterRow.Counter] {
        [
            WisentCounterRow.Counter(
                "Queued",
                value: snapshot.counts.queue.formatted(.number),
                detail: "Waiting for capacity",
                tone: queueBlocked ? .danger : .neutral
            ),
            WisentCounterRow.Counter(
                "Running",
                value: snapshot.counts.running.formatted(.number),
                detail: "Executing on live hosts"
            ),
            WisentCounterRow.Counter(
                "Recent failures",
                value: snapshot.recentFailed.count.formatted(.number),
                detail: "Published in this snapshot",
                tone: snapshot.recentFailed.isEmpty ? .neutral : .danger
            ),
            WisentCounterRow.Counter(
                "Average completion",
                value: StadoFormat.duration(snapshot.throughput.averageWallSecondsPerCompletedJob),
                detail: "\(snapshot.throughput.samples.formatted(.number)) samples"
            ),
        ]
    }
}
