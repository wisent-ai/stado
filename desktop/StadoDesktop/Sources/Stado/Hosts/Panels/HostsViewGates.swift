import SwiftUI
import WisentDesignSystem

extension HostsView {
    /// The gates come first in the inspector, before hardware and before
    /// policy: this is the field that decides whether the host does any work at
    /// all.
    @ViewBuilder
    func gateSection(for host: WorkerNode) -> some View {
        if let gates = hostGates(host) {
            if gates.complete == true, gates.claiming == false, !(gates.pinnedByDesign && gates.waitingJobs.isEmpty) {
                WisentAlertPanel(
                    tone: .danger,
                    title: "This host is claiming no work",
                    detail: gates.blockers.isEmpty
                        ? "The host reports that it is not claiming and named no blocker. Nothing downstream will report this either."
                        : gates.blockers.joined(separator: "\n")
                )
            }
            WisentField(
                label: "Claiming work",
                value: claimingLabel(host),
                tone: claimingTone(host)
            )
            if gates.pinnedByDesign {
                // The pin is the explanation, not a failure: say in one place
                // what claiming looks like on this host, why the row is calm,
                // and where the policy is changed. Without this sentence a
                // "No" reads as a broken agent, and it has been misread twice.
                Text("Registry policy pins this host (pinned_only): it takes only jobs explicitly routed to it, and unpinned queue work goes to open hosts by design. Jobs pinned to this host still run here, so nothing is lost — the alarm above appears only when pinned work is actually waiting. Change the pin under Registry → this host → Change policy.")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if !gates.waitingJobs.isEmpty {
                    Text("The pin is currently costing work: the queue holds jobs addressed to this host.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.danger)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            WisentField(
                label: "Blockers",
                value: gates.blockers.isEmpty
                    ? "None reported"
                    : (gates.pinnedByDesign
                        ? "Pinned by the registry (agent word: pinned_only)"
                        : gates.blockers.joined(separator: "\n")),
                tone: gates.blockers.isEmpty || (gates.pinnedByDesign && gates.waitingJobs.isEmpty) ? .neutral : .danger
            )
            WisentField(
                label: "Waiting pinned jobs",
                value: gates.observations.first(where: { $0.operation == "queue" })?.complete != true
                    ? "Not observed"
                    : gates.waitingJobs.isEmpty
                    ? "None"
                    : gates.waitingJobs
                        .map { job in
                            let age = job.ageSeconds.map { ConsoleFormat.age(Double($0)) } ?? "age unknown"
                            return "\(String(job.jobID.prefix(8))) — in queue \(age)"
                        }
                        .joined(separator: "\n"),
                tone: gates.waitingJobs.isEmpty || gates.claiming == true ? .neutral : .danger
            )
            WisentField(
                label: "Free space",
                value: diskDescription(gates.disk),
                tone: gates.disk?.isBelowWatermark == true ? .danger : .neutral
            )
            WisentField(label: "Disk reading time", value: gates.disk?.observedAt ?? "Not observed")
            WisentField(label: "Disk pressure evidence", value: gates.disk?.pressureSource ?? "Not observed")
            if let bytes = gates.disk?.freeBytes {
                WisentField(label: "Measured available bytes", value: bytes.formatted(.number))
            }
            ForEach(gates.observations) { read in
                WisentSectionBox(title: read.operation, detail: read.source) {
                    WisentField(label: "Read result", value: read.state, tone: read.complete ? .neutral : .warning)
                    WisentField(label: "Elapsed", value: "\(read.elapsedMs.formatted(.number)) ms")
                    WisentField(label: "Read budget", value: "\(read.budgetMs.formatted(.number)) ms")
                    WisentField(label: "Finished", value: read.finishedAt)
                    if let detail = read.detail {
                        Text(detail).textSelection(.enabled).font(WisentTypeScale.body())
                    }
                }
            }
            if let diagnostics = gates.capacity?.diagnostics {
                DisclosureGroup("Published agent diagnostics — separate from measured disk space") {
                    Text(diagnostics.prettyJSON).font(WisentTypeScale.identifierSmall()).textSelection(.enabled)
                }
            }
            WisentField(label: "Cleanup policy mode", value: gates.disk?.policyMode ?? "Not reported")
            WisentField(label: "Capacity published", value: gates.capacity?.publishedAt ?? "Not observed")
            WisentField(
                label: "Capacity report age",
                value: ConsoleFormat.age(gates.capacity?.ageSeconds),
                tone: (gates.capacity?.ageSeconds ?? 0) > 900 ? .warning : .neutral
            )
            WisentField(label: "Capacity", value: capacityDescription(gates.capacity))
            WisentActionButton(
                action: WisentAction(
                    "Reclaim disk…",
                    symbol: "externaldrive.badge.minus",
                    kind: gates.disk?.isBelowWatermark == true ? .primary : .secondary,
                    isEnabled: !gatesStore.mutation.isWorking
                ) {
                    reclaimTarget = HostReclaimTarget(host: gates.host)
                }
            )
        } else if let failure = gateFailure(host) {
            WisentAlertPanel(
                tone: .warning,
                title: "Claiming gates could not be read",
                detail: failure,
                actions: [
                    WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !gatesStore.isRefreshing) {
                        Task { await gatesStore.refresh(hosts: gateHostNames) }
                    },
                ]
            )
        } else if gatesStore.isRefreshing {
            WisentField(label: "Claiming work", value: "Reading…")
        } else {
            WisentField(
                label: "Claiming work",
                value: host.declared
                    ? "Not read for this host"
                    : "Not asked: this host is not a declared registry target",
                tone: .warning
            )
        }
    }
}
