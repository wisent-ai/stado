import SwiftUI
import WisentDesignSystem

extension HostReclaimSheet {
    /// The command's observed skips and refusals, not an inference from item counts.
    @ViewBuilder
    func reclaimDiagnostics(_ pass: HostReclaimPass) -> some View {
        if !pass.skipped.isEmpty
            || pass.stages.contains(where: { $0.detail != nil || !$0.refused.isEmpty })
            || pass.janitor != nil {
            WisentSectionBox(
                title: "Pass diagnostics",
                detail: "Skipped stages, stage refusals and the janitor's own cleaner evidence from the command receipt."
            ) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    ForEach(pass.skipped) { skipped in
                        Text(verbatim: "Skipped \(skipped.stage): \(skipped.reason)")
                            .font(WisentTypeScale.identifier())
                            .foregroundStyle(WisentTone.warning.color)
                            .textSelection(.enabled)
                    }
                    ForEach(pass.stages) { stage in
                        if let detail = stage.detail {
                            Text(verbatim: "\(stage.stage): \(detail)")
                                .font(WisentTypeScale.identifier())
                                .foregroundStyle(WisentTone.warning.color)
                                .textSelection(.enabled)
                        }
                        ForEach(stage.refused.indices, id: \.self) { index in
                            Text(verbatim: "\(stage.stage) refused: \(stage.refused[index])")
                                .font(WisentTypeScale.identifierSmall())
                                .foregroundStyle(WisentTone.warning.color)
                                .textSelection(.enabled)
                        }
                    }
                    if let janitor = pass.janitor {
                        Text(verbatim: "Janitor: \(janitor.outcome)")
                            .font(WisentTypeScale.identifier())
                            .textSelection(.enabled)
                        ForEach(janitor.errors.indices, id: \.self) { index in
                            Text(verbatim: "Janitor error: \(janitor.errors[index])")
                                .font(WisentTypeScale.identifierSmall())
                                .foregroundStyle(WisentTone.warning.color)
                                .textSelection(.enabled)
                        }
                        if let cleaners = janitor.cleaners {
                            ForEach(cleaners.keys.sorted(), id: \.self) { name in
                                if let cleaner = cleaners[name] {
                                    Text(verbatim: "\(name): scanned \(cleaner.scannedItems), eligible \(cleaner.eligibleItems), deleted \(cleaner.deletedItems)")
                                        .font(WisentTypeScale.identifierSmall())
                                        .textSelection(.enabled)
                                    ForEach(cleaner.skipped.keys.sorted(), id: \.self) { reason in
                                        if let count = cleaner.skipped[reason] {
                                            if let bytes = cleaner.skippedBytes[reason] {
                                                Text(verbatim: "\(name) kept \(count) for \(reason) (\(bytes) bytes)")
                                                    .font(WisentTypeScale.identifierSmall())
                                                    .foregroundStyle(WisentTone.warning.color)
                                                    .textSelection(.enabled)
                                            } else {
                                                Text(verbatim: "\(name) kept \(count) for \(reason)")
                                                    .font(WisentTypeScale.identifierSmall())
                                                    .foregroundStyle(WisentTone.warning.color)
                                                    .textSelection(.enabled)
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            Text("The janitor did not reach a cleaner.")
                                .font(WisentTypeScale.body())
                                .foregroundStyle(WisentTone.warning.color)
                        }
                    }
                }
            }
        }
    }
}
