import SwiftUI
import WisentDesignSystem

extension HostReclaimSheet {
    // MARK: Step one

    @ViewBuilder
    var previewSection: some View {
        WisentSectionBox(
            title: "What reclamation would free",
            detail: "The dry run drives the janitor's own planning phase and writes nothing. Nothing can be applied until it has answered for this host.",
            trailing: preview.map { stageCount($0) }
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                if let preview {
                    if preview.stages.isEmpty {
                        Text("The dry run found nothing to reclaim on \(host). Applying would delete nothing, so whatever is holding this host below its watermark is not something the declared cleanup policy covers.")
                            .font(WisentTypeScale.body())
                            .foregroundStyle(WisentDesign.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    } else {
                        stageTable(preview)
                    }
                    Text(verbatim: spanLine(preview))
                        .font(WisentTypeScale.identifier())
                        .foregroundStyle(WisentDesign.ink)
                        .textSelection(.enabled)
                } else if store.isPreviewing {
                    WisentLoadingPanel(
                        title: "Running the dry run on \(host)",
                        detail: previewCommand
                    )
                } else {
                    WisentEmptyPanel(
                        title: "Nothing has been previewed",
                        detail: "The dry run has not answered for \(host), so there is nothing to apply and no apply is offered.",
                        symbol: "eye.slash",
                        action: WisentAction("Run the dry run", symbol: "play", kind: .primary) {
                            Task { await store.loadPreview(host: host) }
                        }
                    )
                }
                Text(verbatim: previewCommand)
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
                    .textSelection(.enabled)
            }
        }
    }

    private func stageCount(_ pass: HostReclaimPass) -> String {
        let stages = pass.stages.count == 1 ? "1 stage" : "\(pass.stages.count.formatted(.number)) stages"
        return "\(pass.mode) · \(stages) · \(pass.itemCount.formatted(.number)) items"
    }

    private func spanLine(_ pass: HostReclaimPass) -> String {
        guard let before = pass.freeGBBefore, let after = pass.freeGBAfter else {
            return "mode \(pass.mode) · the command reported no free-space figures for this pass."
        }
        let verb = pass.isDryRun ? "would leave" : "left"
        return "mode \(pass.mode) · \(StadoFormat.decimal(before)) GB free before · "
            + "\(verb) \(StadoFormat.decimal(after)) GB"
    }

    /// The stages, in the command's own order. A reclamation is a sequence, and
    /// which stage frees the space is the difference between a cache that will
    /// refill by tomorrow and a directory somebody wanted.
    private func stageTable(_ pass: HostReclaimPass) -> some View {
        VStack(spacing: 0) {
            ConsoleTableHead(cells: [
                ConsoleHeaderCell("Stage"),
                ConsoleHeaderCell("Items", width: 72, trailing: true),
                ConsoleHeaderCell("Free before", width: 108, trailing: true),
                ConsoleHeaderCell("Free after", width: 108, trailing: true),
            ])
            ForEach(pass.stages) { stage in
                ConsoleTableRow {
                    ConsoleCell(text: stage.stage, identifier: true, strong: true)
                    ConsoleCell(
                        text: stage.items.formatted(.number),
                        width: 72,
                        trailing: true,
                        digits: true
                    )
                    ConsoleCell(
                        text: ConsoleFormat.gigabytes(stage.freeGBBefore),
                        width: 108,
                        trailing: true,
                        digits: true
                    )
                    ConsoleCell(
                        text: ConsoleFormat.gigabytes(stage.freeGBAfter),
                        width: 108,
                        trailing: true,
                        digits: true
                    )
                }
            }
        }
        .background(WisentDesign.surface)
        .clipShape(RoundedRectangle(cornerRadius: WisentDesign.Radius.small))
        .overlay {
            RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
                .stroke(WisentDesign.border, lineWidth: WisentDesign.hairline)
        }
    }

    // MARK: Step two

    var reasonSection: some View {
        WisentSectionBox(
            title: "Why this host needs the space",
            detail: "Recorded with the pass. --reason is mandatory in the command and mandatory here: it is all somebody reading the audit record months from now will have."
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                TextField("the 0.7.6 candidate needs 12 GB and this host is at 2", text: $reason)
                    .textFieldStyle(.roundedBorder)
                    .font(WisentTypeScale.body())
                    .disabled(store.mutation.isWorking)
                Text(verbatim: applyCommand)
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(WisentDesign.ink)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                if !store.hasPreview(for: host) {
                    Text("The apply stays unavailable until the dry run above has answered for \(host).")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentTone.warning.color)
                } else if trimmedReason.isEmpty {
                    Text("Type a reason to enable the apply.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.muted)
                }
            }
        }
    }

    // MARK: What it did

    func appliedSection(_ applied: HostReclaimPass) -> some View {
        WisentSectionBox(
            title: "What reclamation freed",
            detail: "The pass as the command reported it. Applying again needs a new dry run: this host is no longer in the state the last one described.",
            trailing: stageCount(applied)
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                if applied.stages.isEmpty {
                    Text("The pass ran and reported no stages, so nothing was deleted on \(host).")
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                } else {
                    stageTable(applied)
                }
                Text(verbatim: spanLine(applied))
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(WisentDesign.ink)
                    .textSelection(.enabled)
                Text(verbatim: StadoCLI.commandLine(
                    HostGatesStore.applyArguments(host: host, reason: trimmedReason)
                ))
                .font(WisentTypeScale.identifierSmall())
                .foregroundStyle(WisentDesign.muted)
                .textSelection(.enabled)
            }
        }
    }

    var footer: some View {
        HStack(spacing: WisentDesign.Space.x2) {
            Spacer(minLength: 0)
            if store.applied == nil {
                WisentActionButton(
                    action: WisentAction("Cancel", kind: .plain) {
                        store.clearReclamation()
                        dismiss()
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Run the dry run again",
                        symbol: "arrow.clockwise",
                        isEnabled: !store.isPreviewing && !store.mutation.isWorking
                    ) {
                        Task { await store.loadPreview(host: host) }
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Reclaim now",
                        symbol: "externaldrive.badge.minus",
                        kind: .destructive,
                        isEnabled: canApply
                    ) {
                        Task {
                            await store.apply(host: host, reason: trimmedReason)
                            await refreshGates()
                        }
                    }
                )
            } else {
                WisentActionButton(
                    action: WisentAction("Done", kind: .primary) {
                        store.clearReclamation()
                        dismiss()
                    }
                )
            }
        }
    }
}
