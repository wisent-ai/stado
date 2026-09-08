import SwiftUI
import WisentDesignSystem

/// The per-platform half of the table: the strip on the recipe row and the
/// indented line each platform gets when the recipe is opened.
///
/// `platformStrip` and `platformRow` are internal rather than private only
/// because `row` and `table` sit in `BuildsTable.swift`: Swift scopes
/// `private` to one file. The badge and its reason keep their `private`,
/// because `platformRow` is the only caller and shares this file.
extension BuildsView {
    /// The platform list, each name carrying its own run's tone: a red
    /// `linux-amd64` next to a green `darwin-arm64` is the whole answer to
    /// "which half is broken" without opening anything.
    func platformStrip(_ recipe: BuildRecipe) -> some View {
        let entries = recipe.platformRuns
        return HStack(spacing: WisentDesign.Space.x1) {
            if entries.isEmpty {
                Text("none declared")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.muted)
            } else {
                ForEach(Array(entries.enumerated()), id: \.element.id) { index, entry in
                    if index > 0 {
                        Text("·")
                            .font(WisentTypeScale.identifier())
                            .foregroundStyle(WisentDesign.border)
                    }
                    Text(entry.platform)
                        .font(WisentTypeScale.identifier())
                        .foregroundStyle(color(for: entry.run))
                        .lineLimit(1)
                }
            }
            Spacer(minLength:
                0)
        }
        .frame(width: Column.platforms, alignment: .leading)
    }

    /// One platform beneath its recipe: what the run did, which version came
    /// out of it, whether that version was declared, the job to look up on the
    /// Queue screen, and when. A failed run tints the whole line.
    ///
    /// The line starts where the recipe row's repository column starts and packs
    /// left from there. It deliberately does not line up with the recipe row's
    /// right-hand columns: a platform's version has no counterpart up there, and
    /// a value parked under a header that does not describe it reads as a lie.
    func platformRow(_ entry: BuildPlatformRun, of recipe: BuildRecipe) -> some View {
        let run = entry.run
        let tone = tone(for: run)
        return ConsoleTableRow {
            HStack {
                Spacer(minLength:
                    0)
                Image(systemName: "arrow.turn.down.right")
                    .font(.system(size:
                        9, weight: .regular))
                    .foregroundStyle(WisentDesign.border)
                    .accessibilityHidden(true)
            }
            .frame(width: Column.recipe)
            ConsoleCell(text: entry.platform, width:
                168, identifier: true, tone: tone)
            ConsoleCell(text: run?.status ?? "never ran", width:
                88, tone: tone)
            ConsoleCell(text: versionText(run), width:
                100, identifier: true)
            declaredBadge(run, autoDeclare: recipe.autoDeclare)
                .frame(width:
                    136, alignment: .leading)
            ConsoleCell(
                text: run.map { "job \($0.jobID.prefix(8))" } ?? "—",
                width:
                    112,
                identifier: true
            )
            ConsoleCell(text: atText(run), width:
                120)
            Spacer(minLength:
                0)
        }
        .background(tone == .danger ? WisentTone.danger.softColor : WisentDesign.canvasMuted.opacity(0.5))
    }

    /// Declared is a fleet write, so it gets a badge; everything else says why
    /// there was nothing to declare rather than leaving the cell blank.
    @ViewBuilder
    private func declaredBadge(_ run: BuildRun?, autoDeclare: Bool) -> some View {
        if let run, run.declared {
            WisentBadge("declared", symbol: "checkmark.seal", tone: .success)
        } else if let run, run.version != nil, run.status == "succeeded", autoDeclare {
            WisentBadge("not declared", tone: .warning)
        } else {
            Text(declarationReason(run, autoDeclare: autoDeclare))
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.muted)
                .lineLimit(1)
                .truncationMode(.middle)
        }
    }

    /// Why nothing was declared, without repeating the columns beside it. A run
    /// that failed has no declaration question to answer — the status cell one
    /// column left already says what happened.
    private func declarationReason(_ run: BuildRun?, autoDeclare: Bool) -> String {
        guard let run, run.status != "failed" else { return "—" }
        if run.status == "running" { return "still building" }
        if run.version == nil { return "nothing to declare" }
        return autoDeclare ? "—" : "declare by hand"
    }
}
