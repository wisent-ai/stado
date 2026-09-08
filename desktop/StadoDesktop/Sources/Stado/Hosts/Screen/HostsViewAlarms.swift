import SwiftUI
import WisentDesignSystem

extension HostsView {
    /// The alarms, above the table rather than inside it.
    ///
    /// Two questions, both invisible everywhere else on this console. A host
    /// that claims nothing looks exactly like a host with nothing to do, and
    /// the last time the two were confused every release build waited hours on
    /// a machine sitting at 2 GB free against a 55 GB policy. A host that has
    /// stopped publishing beacons looks exactly like a host nobody asked, and
    /// the last time that happened the only evidence of a six-minute gap on
    /// control-host was an operator's two ping packets.
    @ViewBuilder
    var alarms: some View {
        // A pinned host with nothing pinned to it is policy, not an alarm; it
        // still costs a pinned job when the queue says so.
        let notClaiming = gatesStore.notClaiming.filter { $0.refusingUnpinned || !$0.waitingJobs.isEmpty }
        let unreadableGates = gatesStore.failures
        let quiet = linkStore.needingAttention
        let unreadableLinks = linkStore.failures
        VStack(spacing: WisentDesign.Space.x3) {
            if !notClaiming.isEmpty {
                WisentAlertPanel(
                    tone: .danger,
                    title: notClaiming.count == 1
                        ? "\(notClaiming[0].host) is claiming no work"
                        : "\(notClaiming.count.formatted(.number)) hosts are claiming no work",
                    detail: silentDetail(notClaiming),
                    actions: [
                        WisentAction("Show them", symbol: "arrow.down.right") {
                            facet = .notClaiming
                            selection = nil
                        },
                    ]
                )
            }
            if let worst = quiet.first {
                WisentAlertPanel(
                    tone: worst.verdict.tone,
                    title: quiet.count == 1
                        ? linkAlarmTitle(worst)
                        : "\(quiet.count.formatted(.number)) hosts have a link the fleet cannot vouch for",
                    detail: quietDetail(quiet),
                    actions: [
                        WisentAction("Show them", symbol: "arrow.down.right") {
                            facet = worst.verdict == .degraded ? .degradedLink : .silentLink
                            selection = nil
                        },
                    ]
                )
            }
            if !unreadableGates.isEmpty {
                WisentAlertPanel(
                    tone: .warning,
                    title: unreadableGates.count == 1
                        ? "One host did not answer whether it is claiming work"
                        : "\(unreadableGates.count.formatted(.number)) hosts did not answer whether they are claiming work",
                    detail: unreadableGates
                        .sorted { $0.key < $1.key }
                        .map { "\($0.key): \($0.value)" }
                        .joined(separator: "\n"),
                    actions: [
                        WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !gatesStore.isRefreshing) {
                            Task { await gatesStore.refresh(hosts: gateHostNames) }
                        },
                    ]
                )
            }
            if !unreadableLinks.isEmpty {
                WisentAlertPanel(
                    tone: .warning,
                    title: unreadableLinks.count == 1
                        ? "One host's link could not be read"
                        : "\(unreadableLinks.count.formatted(.number)) hosts' links could not be read",
                    detail: unreadableLinks
                        .sorted { $0.key < $1.key }
                        .map { "\($0.key): \($0.value)" }
                        .joined(separator: "\n"),
                    actions: [
                        WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !linkStore.isRefreshing) {
                            Task { await linkStore.refresh(hosts: gateHostNames) }
                        },
                    ]
                )
            }
        }
        .padding(.horizontal, WisentDesign.Space.x4)
        .padding(
            .top,
            notClaiming.isEmpty && quiet.isEmpty && unreadableGates.isEmpty && unreadableLinks.isEmpty
                ? 0
                : WisentDesign.Space.x4
        )
    }

    /// The headline for one host, in the shape the operator asks the question:
    /// how long has it been quiet.
    func linkAlarmTitle(_ link: HostLink) -> String {
        if let silence = link.openSilence {
            return "\(link.host) has been silent for \(StadoFormat.duration(silence.elapsedSeconds))"
        }
        if link.verdict == .silent {
            return "\(link.host) is silent"
        }
        return "\(link.host)'s link is \(link.verdict.word)"
    }

    /// Every quiet host's own blockers, verbatim, and the first refusal a reader
    /// hit while it was quiet — the sentence that used to reach nothing but
    /// ~/.stado/logs/stado-resolver.err.
    private func quietDetail(_ links: [HostLink]) -> String {
        var lines = links.prefix(3).map { link -> String in
            var line = "\(link.host) — \(link.verdict.word)"
            if let silence = link.openSilence {
                line += ", quiet for \(StadoFormat.duration(silence.elapsedSeconds))"
            } else if let age = link.beaconAgeSeconds {
                line += ", newest beacon \(ConsoleFormat.age(Double(age)))"
            } else {
                line += ", no beacon has ever been published for it"
            }
            line += ": "
            line += link.blockers.isEmpty
                ? "the command named no blocker, which is itself the thing to chase"
                : link.blockers.joined(separator: "; ")
            if let reader = link.openSilence?.firstReaderError, !reader.isEmpty {
                line += "\nFirst reader refusal: \(reader)"
            }
            return line
        }
        if links.count > 3 {
            lines.append("and \((links.count - 3).formatted(.number)) more in the Link facets")
        }
        return lines.joined(separator: "\n")
    }

    private func silentDetail(_ hosts: [HostGates]) -> String {
        var lines = hosts.prefix(3).map { gates -> String in
            let blockers = gates.blockers.isEmpty
                ? "the host named no blocker, which is itself the thing to chase"
                : gates.blockers.joined(separator: "; ")
            var line = "\(gates.host): \(blockers)"
            if let disk = gates.disk, let free = disk.freeGB, let low = disk.lowWatermarkGB {
                line += " (\(StadoFormat.decimal(free)) GB free against a \(StadoFormat.decimal(low)) GB watermark)"
            }
            if !gates.waitingJobs.isEmpty {
                // The refusal's cost, in the refusal's own sentence: work is
                // sitting in the queue for this exact host right now.
                let oldest = gates.waitingJobs.compactMap(\.ageSeconds).max()
                line += " — starving \(gates.waitingJobs.count) pinned job(s)"
                if let oldest {
                    line += ", oldest \(ConsoleFormat.age(Double(oldest)))"
                }
            }
            return line
        }
        if hosts.count > 3 {
            lines.append("and \((hosts.count - 3).formatted(.number)) more in the Not claiming filter")
        }
        return lines.joined(separator: "\n")
    }
}
