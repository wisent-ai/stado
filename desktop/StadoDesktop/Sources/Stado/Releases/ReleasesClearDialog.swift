import SwiftUI
import WisentDesignSystem

/// Reviewed release mutations, using the same commands shown in each dialog.
///
/// `clearDialog` is internal rather than private only because the sheet it is
/// presented from sits on `body` in `ReleasesView.swift`: Swift scopes
/// `private` to one file.
extension ReleasesView {
    // MARK: Reviewed mutations

    /// The reason is not optional and not defaulted. The command records it
    /// beside the state file, and a cleared digest with no recorded reason is
    /// an audit trail that answers nothing.
    func clearDialog(_ pending: PendingClearance) -> some View {
        let typed = reason.trimmingCharacters(in: .whitespacesAndNewlines)
        let command = StadoCLI.commandLine(
            ReleaseEvidenceStore.clearArguments(
                pair: pending.pair,
                digest: pending.entry.digest,
                reason: typed.isEmpty ? "<reason>" : typed
            )
        )
        return VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            HStack(alignment: .top, spacing: WisentDesign.Space.x3) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size:
                        17, weight: .semibold))
                    .foregroundStyle(WisentTone.warning.color)
                    .frame(width:
                        34, height:
                        34)
                    .background(WisentTone.warning.softColor, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small))
                    .accessibilityHidden(true)
                Text("Clear \(pending.entry.shortDigest) for \(pending.pair.product) on \(pending.pair.target)?")
                    .font(WisentTypography.heading(17))
                    .foregroundStyle(WisentDesign.ink)
                    .fixedSize(horizontal: false, vertical: true)
            }

            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text(pending.entry.isDesiredDigest
                    ? "This is the digest the registry currently desires, so every pass of the release agent skips it and the rollout never finishes. Clearing it is what lets the next pass try again."
                    : "The registry does not currently desire this digest. Clearing it retires the refusal; nothing rolls out until this digest is desired again.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Text("This console starts, stops and restarts nothing. The command rewrites the host's rollout state after backing it up, and the release agent picks the digest up on its next tick.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Text("quarantined at \(pending.entry.quarantinedAt ?? "an unreported time") because: \(pending.entry.reason.isEmpty ? "the host recorded no reason" : pending.entry.reason)")
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(WisentTone.warning.color)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }

            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text("REASON — REQUIRED, RECORDED IN THE AUDIT TRAIL")
                    .font(WisentTypeScale.eyebrow())
                    .tracking(0.6)
                    .foregroundStyle(WisentDesign.muted)
                TextField("rebuilt 0.9.14 after the signing key rotation", text: $reason, axis: .vertical)
                    .textFieldStyle(.roundedBorder)
                    .font(WisentTypeScale.body())
                    .lineLimit(2...4)
                    .disabled(store.mutation.isWorking)
                if typed.isEmpty {
                    Text("Without a reason this command does not run. It is what an audit reads months from now, when nobody remembers why the digest was given another chance.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text(command)
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.ink)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(WisentDesign.Space.x3)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(WisentDesign.canvasMuted, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small))

            HStack(spacing: WisentDesign.Space.x2) {
                Image(systemName: "doc.badge.clock")
                    .font(.system(size:
                        11))
                    .foregroundStyle(WisentDesign.muted)
                    .accessibilityHidden(true)
                Text("The host writes a backup of its rollout state document before the rewrite, and the clearance is appended to the audit trail beside it.")
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }

            HStack(spacing: WisentDesign.Space.x2) {
                Spacer(minLength:
                    0
                )
                WisentActionButton(
                    action: WisentAction("Leave it quarantined", kind: .primary) {
                        clearance = nil
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Clear digest",
                        kind: .destructive,
                        isEnabled: !typed.isEmpty && !store.mutation.isWorking
                    ) {
                        let pair = pending.pair
                        let digest = pending.entry.digest
                        clearance = nil
                        Task {
                            await store.clearQuarantine(pair: pair, digest: digest, reason: typed)
                        }
                    }
                )
            }
            .padding(.top, WisentDesign.Space.x2)
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: WisentAppLayout.dialogWidth, alignment: .leading)
        .background(WisentDesign.surface)
    }

    func resumeDialog(_ run: ReleasePipelineRunRecord) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            Text("Resume \(run.product) \(run.version)?")
                .font(WisentTypography.heading(17))
            Text("Uses the stored source and manifest. Running jobs and published builds are kept; failed jobs receive a new attempt.")
            Text("Source: \(run.sourceCommit.isEmpty ? "not recorded" : run.sourceCommit)")
                .font(WisentTypeScale.identifierSmall())
                .textSelection(.enabled)
            Text(StadoCLI.commandLine(ReleaseEvidenceStore.resumeArguments(runID: run.runID)))
                .font(WisentTypeScale.identifierSmall())
                .textSelection(.enabled)
            HStack {
                Spacer()
                WisentActionButton(action: WisentAction("Cancel") {
                    resumption = nil
                })
                WisentActionButton(action: WisentAction("Resume", kind: .primary) {
                    resumption = nil
                    Task { await store.resume(run) }
                })
            }
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: WisentAppLayout.dialogWidth, alignment: .leading)
    }
}
