import SwiftUI
import WisentDesignSystem

/// The digests this host refuses to roll out again, and the last clearance
/// recorded against them.
///
/// `quarantine` is internal rather than private only because `detail` sits in
/// `Detail/ReleasesDetail.swift`: Swift scopes `private` to one file. The row,
/// its age line and the clearance record are read only from here and stay
/// private; the "Clear…" button hands `PendingClearance` back to the sheet in
/// `ReleasesView.swift`.
extension ReleasesView {
    // MARK: Quarantine

    @ViewBuilder
    func quarantine(_ row: ReleaseRow) -> some View {
        WisentSectionBox(
            title: "Quarantine",
            detail: "Digests this host refuses to roll out again. The agent skips a quarantined digest on every pass, so the one the registry desires is a rollout that never finishes on its own.",
            trailing: store.quarantinePair == row.pair
                ? store.quarantine.map { "\($0.entries.count) held" }
                : nil
        ) {
            Text(StadoCLI.commandLine(ReleaseEvidenceStore.quarantineArguments(pair: row.pair)))
                .font(WisentTypeScale.identifierSmall())
                .foregroundStyle(WisentDesign.muted)
                .textSelection(.enabled)

            if let problem = store.quarantineProblem {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The quarantine map could not be read",
                    detail: problem,
                    actions: [
                        WisentAction("Read again", symbol: "arrow.clockwise") {
                            Task { await store.loadQuarantine(for: row.pair) }
                        }
                    ]
                )
                // The diagnosis already carried the host's quarantine map, so
                // a failed second read degrades to a list without the desired
                // flag rather than to nothing at all.
                if let held = row.report?.quarantined, !held.isEmpty {
                    Text("The diagnosis above read \(held.count) quarantined \(held.count == 1 ? "digest" : "digests") on this host. Clearing is unavailable until the map itself can be read.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    ForEach(held.desiredFirst) { entry in
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                            Text(entry.digest)
                                .font(WisentTypeScale.identifier())
                                .foregroundStyle(WisentDesign.ink)
                                .textSelection(.enabled)
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Text("\(entry.quarantinedAt ?? "no timestamp recorded") — \(entry.reason.isEmpty ? "the host recorded no reason" : entry.reason)")
                                .font(WisentTypeScale.caption())
                                .foregroundStyle(WisentDesign.secondary)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        .padding(WisentDesign.Space.x3)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(WisentDesign.canvasMuted, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small))
                    }
                }
            } else if store.isLoadingQuarantine, store.quarantine == nil {
                WisentSkeletonList(
                    rows:
                        2,
                    lines:
                        2,
                    media: false,
                    label: "Reading the host's quarantine map"
                )
            } else if let report = store.quarantine, store.quarantinePair == row.pair {
                if report.entries.isEmpty {
                    Text("Nothing is quarantined for \(report.product) on \(report.target). No digest is being skipped here.")
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                } else {
                    VStack(spacing: WisentDesign.Space.x2) {
                        ForEach(report.entries.desiredFirst) { entry in
                            quarantineRow(entry, pair: row.pair)
                        }
                    }
                }
            }

            if let record = store.clearance, store.quarantinePair == row.pair {
                clearanceRecord(record)
            }
        }
    }

    private func quarantineRow(_ entry: ReleaseQuarantineEntry, pair: ReleaseInventoryPair) -> some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x4) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                HStack(spacing: WisentDesign.Space.x2) {
                    Text(entry.shortDigest)
                        .font(WisentTypeScale.identifier())
                        .foregroundStyle(WisentDesign.ink)
                        .textSelection(.enabled)
                    if entry.isDesiredDigest {
                        WisentStatusChip(text: "Desired — blocks the rollout", tone: .danger)
                    }
                    Text(quarantinedAge(entry))
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.muted)
                }
                Text(entry.digest)
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(entry.reason.isEmpty ? "The host recorded no reason." : entry.reason)
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: WisentDesign.Space.x2)
            WisentActionButton(
                action: WisentAction(
                    "Clear…",
                    symbol: "arrow.uturn.forward",
                    isEnabled: !store.mutation.isWorking
                ) {
                    reason = ""
                    clearance = PendingClearance(pair: pair, entry: entry)
                }
            )
        }
        .padding(WisentDesign.Space.x3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            entry.isDesiredDigest ? WisentTone.danger.softColor : WisentDesign.canvasMuted,
            in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
        )
    }

    private func quarantinedAge(_ entry: ReleaseQuarantineEntry) -> String {
        guard let at = entry.quarantinedAt, !at.isEmpty else { return "no timestamp recorded" }
        guard let age = entry.quarantinedAge else { return at }
        return "\(at) · \(ConsoleFormat.age(age))"
    }

    private func clearanceRecord(_ record: ReleaseQuarantineClearance) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text("LAST CLEARANCE")
                .font(WisentTypeScale.eyebrow())
                .tracking(0.6)
                .foregroundStyle(WisentDesign.muted)
            Text("\(record.digest) on \(record.target), recorded at \(record.auditedAt ?? "an unreported time") because: \(record.reason)")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            Text("Previous state backed up on the host at \(record.stateBackup ?? "a path the command did not report").")
                .font(WisentTypeScale.identifierSmall())
                .foregroundStyle(WisentDesign.muted)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(WisentDesign.Space.x3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(WisentTone.success.softColor, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small))
    }
}
