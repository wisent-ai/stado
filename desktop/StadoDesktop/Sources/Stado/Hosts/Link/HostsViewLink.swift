import SwiftUI
import WisentDesignSystem

extension HostsView {
    func hostLink(_ host: WorkerNode) -> HostLink? {
        linkStore.link(for: host.displayName)
    }

    private func linkFailure(_ host: WorkerNode) -> String? {
        linkStore.failure(for: host.displayName)
    }

    private func beaconAgeDescription(_ link: HostLink) -> String {
        guard let age = link.beaconAgeSeconds else {
            return "No beacon has ever been published for this host"
        }
        return ConsoleFormat.age(Double(age))
    }

    /// Why this host went quiet, under the gates that decide whether it works.
    ///
    /// The gates answer "is it taking jobs"; this answers "is it there at all",
    /// which is the question that has no surface anywhere else when a host
    /// drops off the network for minutes. A healthy link
    /// is one line and no card: absence of an incident is not an incident.
    @ViewBuilder
    func linkSection(for host: WorkerNode) -> some View {
        if let link = hostLink(host) {
            if link.verdict.needsAttention {
                WisentAlertPanel(
                    tone: link.verdict.tone,
                    title: linkAlarmTitle(link),
                    detail: link.blockers.isEmpty
                        ? "The command called this link \(link.verdict.word) and named no blocker. Nothing downstream reports it either, so the next reader to refuse will be the only trace."
                        : link.blockers.joined(separator: "\n"),
                    actions: linkRepairActions(link)
                )
            } else {
                WisentField(
                    label: "Link",
                    value: healthyLinkLine(link),
                    tone: .neutral
                )
                if !link.blockers.isEmpty {
                    // A healthy verdict can still carry sentences: an old
                    // beacon format that predates the link block is not the
                    // host's ill health, and the command says so rather than
                    // failing the verdict over it. Neutral, because absence by
                    // choice is never red — but never dropped either, because
                    // the alternative is a console that quietly loses the one
                    // sentence explaining why the fields below read "Not
                    // reported".
                    WisentField(
                        label: "Blockers",
                        value: link.blockers.joined(separator: "\n"),
                        tone: .neutral
                    )
                }
            }
            WisentMutationBar(outcome: linkStore.repairOutcome(for: link.host)) {
                linkStore.clearRepair()
            }
            WisentField(
                label: "Newest beacon",
                value: beaconAgeDescription(link),
                tone: link.verdict.needsAttention ? link.verdict.tone : .neutral
            )
            if let publisher = link.beaconPublisher {
                WisentField(
                    label: "Beacon publisher",
                    value: "\(publisher.unit)\n\(publisher.detail)",
                    tone: publisher.repairable ? .danger : .warning
                )
            }
            WisentField(
                label: "SSH reachable",
                value: link.sshReachable ? "Yes" : "No",
                tone: link.sshReachable ? .neutral : .danger
            )
            WisentField(
                label: "Host-control routes",
                value: connectionPathsDescription(link),
                tone: connectionPathsTone(link)
            )
            WisentActionButton(
                action: WisentAction(
                    "Manage host-control routes…",
                    symbol: "network",
                    kind: .secondary,
                    isEnabled: !connectionPathStore.mutation.isWorking
                ) {
                    connectionPathsTarget = HostConnectionPathsTarget(host: link.host)
                }
            )
            WisentActionButton(
                action: WisentAction(
                    "Edit or remove registry declaration…",
                    symbol: "square.and.pencil",
                    kind: .secondary,
                    isEnabled: !declarationStore.mutation.isWorking
                ) {
                    declarationTarget = HostDeclarationTarget(host: link.host)
                }
            )
            // What this host DIALS, beside the routes that reach it. The two
            // are different questions and only the first had a surface: a
            // marker naming a port the fleet never declared is how a product
            // on that machine ends up talking to the wrong service, or to
            // nothing, with the file as the only statement of the address.
            WisentField(
                label: "Service addresses this host dials",
                value: forwardDescription(link),
                tone: forwardTone(link)
            )
            // Which store every credential write and authoritative read on
            // that host goes through. Counts alone said how much a machine
            // held and never which vault answered, and two vaults claiming
            // one owner is a refusal the operator only met as somebody
            // else's failed command.
            WisentField(
                label: "Credential vault this host resolves",
                value: vaultDescription(link),
                tone: vaultTone(link)
            )
            // Whether anybody is logged in on the screen there, which had no
            // surface anywhere in the product: `ssh_reachable` above answers
            // "can this machine be reached", and this answers "is there a
            // login session on it" — the fact that decides whether launchd on
            // that host has a domain to load a per-login unit into at all.
            //
            // Neutral in every kind. An always-on box with nobody at its
            // screen is the normal state for an always-on box, and the reason
            // that matters to this host arrives as one of the command's own
            // blockers, rendered verbatim above.
            WisentField(label: "Screen session", value: sessionDescription(link))
            WisentField(label: "Beacon network path", value: pathDescription(link))
            WisentField(label: "Last sleep", value: stampDescription(link.lastSleepAt))
            WisentField(label: "Last wake", value: stampDescription(link.lastWakeAt))
            WisentField(label: "Interface changes", value: interfaceDescription(link))
            // What the host could not read of its own link, each with its
            // cause, so a null above says whether a probe failed or the host
            // has no such tool.
            WisentField(
                label: "Unread link data",
                value: link.unreadLine,
                tone: link.linkUnread?.isEmpty == false ? .warning : .neutral
            )
            // What the host's own disk scans last decided. A scan standing
            // skipped is the host above its load ceiling, so the readings
            // that scan produces (unit events, build caches, inventory) are
            // the last admitted pass's, not this one's.
            WisentField(
                label: "Disk scans",
                value: HostScans.description(of: link.scans),
                tone: HostScans.tone(of: link.scans)
            )
            WisentField(
                label: "Recorded silences",
                value: silenceDescription(link),
                tone: link.openSilence == nil ? .neutral : .danger
            )
            // Silences are newest first and only the newest can be open, so
            // its refusal is the current gap's when one is open.
            if let reader = link.silences.first?.firstReaderError, !reader.isEmpty {
                // The refusal in the reader's own words.
                WisentField(label: "First reader refusal", value: reader, tone: .danger)
            }
            WisentField(
                label: "Reader refusals",
                value: refusalDescription(link.readerRefusals),
                tone: link.readerRefusals?.reasons.isEmpty == false ? .warning : .neutral
            )
            // What the host ran, beside what it is now.
            HostActivitySection(host: link.host)
        } else if let failure = linkFailure(host) {
            WisentAlertPanel(
                tone: .warning,
                title: "This host's link could not be read",
                detail: failure,
                actions: [
                    WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !linkStore.isRefreshing) {
                        Task { await linkStore.refresh(hosts: gateHostNames) }
                    },
                ]
            )
        } else if linkStore.isRefreshing {
            WisentField(label: "Link", value: "Reading…")
        } else {
            WisentField(
                label: "Link",
                value: host.declared
                    ? "Not read for this host"
                    : "Not asked: this host is not a declared registry target",
                tone: .warning
            )
        }
    }

    private func linkRepairActions(_ link: HostLink) -> [WisentAction] {
        guard link.beaconPublisher?.repairable == true else { return [] }
        return [
            WisentAction(
                "Repair beacon publication",
                symbol: "wrench.and.screwdriver",
                kind: .primary,
                isEnabled: !linkStore.isRefreshing && !linkStore.isRepairing(link.host)
            ) {
                Task { await linkStore.repair(host: link.host) }
            },
        ]
    }
}
