import SwiftUI
import WisentDesignSystem

/// What the change does to the recipe, read back before the registry is
/// written.
///
/// `confirmation` is internal rather than private only because `body` sits in
/// `BuildsRecipeForm.swift`: Swift scopes `private` to one file. The two
/// paragraph builders and the word joiner keep their `private`, because
/// `confirmation` is their only caller and it shares this file.
extension BuildRecipeFormView {
    // MARK: The decision, before the write

    /// What the change does to the recipe, field by field and in words, with
    /// the invocation it runs quoted underneath.
    ///
    /// A change that moves the source discards recorded state that does not
    /// come back, so it wears the red button; a change to how the recipe
    /// builds keeps everything and does not.
    func confirmation(_ change: BuildRecipeEdit) -> some View {
        WisentDecisionDialog(
            tone: change.movesSource ? .danger : .warning,
            title: change.movesSource
                ? "Point \(change.name) at another source?"
                : "Change \(change.name)?",
            lines: consequences(change),
            listing: listing(change),
            footnote: "Runs \(StadoCLI.commandLine(BuildsStore.editArguments(change))).",
            actions: change.movesSource
                ? [
                    WisentAction("Back to the form", kind: .primary) { reviewing = nil },
                    WisentAction("Change the source", symbol: "arrow.triangle.branch", kind: .destructive) {
                        submit(.change(change))
                    },
                ]
                : [
                    WisentAction("Back to the form", kind: .secondary) { reviewing = nil },
                    WisentAction("Apply the change", symbol: "checkmark.circle", kind: .primary) {
                        submit(.change(change))
                    },
                ]
        )
    }

    /// The state consequence, stated because it decides whether the recipe
    /// re-fires: a different source has to be built from its current head, so
    /// the last seen commit and the recorded runs go; a different command,
    /// artifact list, platform set or cadence keeps both.
    private func consequences(_ change: BuildRecipeEdit) -> [String] {
        guard let original else { return [] }
        let seen = original.lastSeenRef.map { String($0.prefix(8)) } ?? "none yet"
        let runs = original.runs.count == 1 ? "1 recorded run" : "\(original.runs.count.formatted(.number)) recorded runs"
        let interval = (change.intervalSeconds ?? original.intervalSeconds).formatted(.number)
        var lines = [
            "This rewrites \(change.changedFields.joined(separator: ", ")) on \(change.name) in the canonical registry, and nothing else: every field left alone keeps its value, and enablement is not one of them — \(original.enabled ? "the recipe stays enabled" : "the recipe stays disabled") until enable or disable says otherwise.",
        ]
        if change.movesSource {
            lines.append(
                "It moves the source from \(original.repo) at \(original.ref) to \(change.repo ?? original.repo) at \(change.branch ?? original.ref). The commit it last saw (\(seen)) and its \(runs) are cleared with it: they describe a source this recipe no longer builds. Nothing catches up on the old branch, and the runs do not come back."
            )
            lines.append(
                original.enabled
                    ? "The recipe is enabled, so the next poll — within \(interval)s — builds the current head of the new source, whatever commit that is."
                    : "The recipe is disabled, so nothing is built until it is enabled or Run now… asks for a build."
            )
        } else {
            lines.append(
                "It leaves the source alone, so the commit it last saw (\(seen)) and its \(runs) stay exactly as they are: how it builds moved, not what it builds from. A platform named here for the first time simply has no run yet, and a platform dropped keeps the run it already recorded."
            )
        }
        return lines
    }

    /// Old value on the left, new on the right, in the registry's own field
    /// names — and, for the two list fields, the word that says the flag
    /// replaces rather than appends.
    private func listing(_ change: BuildRecipeEdit) -> [String] {
        guard let original else { return [] }
        var rows: [String] = []
        if let repo = change.repo {
            rows.append("repo: \(original.repo) → \(repo)")
        }
        if let branch = change.branch {
            rows.append("ref: \(original.ref) → \(branch)")
        }
        if let command = change.command {
            rows.append("command: \(original.command) → \(command)")
        }
        if let artifacts = change.artifacts {
            rows.append("artifacts: \(Self.words(original.artifacts)) → \(Self.words(artifacts)) (replaces the list)")
        }
        if let platforms = change.platforms {
            rows.append("platforms: \(Self.words(original.platforms)) → \(Self.words(platforms)) (replaces the list)")
        }
        if let seconds = change.intervalSeconds {
            rows.append("interval_seconds: \(original.intervalSeconds) → \(seconds)")
        }
        if let autoDeclare = change.autoDeclare {
            rows.append("auto_declare: \(original.autoDeclare) → \(autoDeclare)")
        }
        return rows
    }

    private static func words(_ values: [String]) -> String {
        values.isEmpty ? "none" : values.joined(separator: ", ")
    }
}
