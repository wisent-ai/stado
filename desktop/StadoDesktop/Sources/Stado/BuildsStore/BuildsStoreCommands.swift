import Foundation

/// Every `stado builds` invocation this screen runs, as the argument list an
/// operator would have typed.
///
/// They are `nonisolated static` so a confirmation dialog can quote the exact
/// command line before the write is made, without reaching into the store's
/// state to build it.
extension BuildsStore {
    nonisolated static func listArguments() -> [String] {
        ["builds", "list", "--json"]
    }

    nonisolated static func enablementArguments(name: String, enabled: Bool) -> [String] {
        ["builds", enabled ? "enable" : "disable", name, "--json"]
    }

    nonisolated static func runArguments(name: String, runID: String) -> [String] {
        ["builds", "run", name, "--run-id", runID, "--json"]
    }

    /// `stado builds add --name … --json`. Every field is given because add
    /// requires them: a recipe with no artifact and no platform builds nothing.
    nonisolated static func addArguments(_ draft: BuildRecipeDraft) -> [String] {
        var arguments = [
            "builds", "add",
            "--name", draft.recipeName,
            "--repo", draft.repoURL,
            "--branch", draft.branchName,
            "--command", draft.buildCommand,
        ]
        for path in draft.artifactPaths {
            arguments += ["--artifact", path]
        }
        for platform in draft.platforms {
            arguments += ["--platform", platform]
        }
        arguments += [
            "--interval-seconds",
            String(draft.intervalSeconds ?? BuildRecipeDraft.defaultIntervalSeconds),
        ]
        if draft.autoDeclare {
            arguments.append("--auto-declare")
        }
        arguments.append("--json")
        return arguments
    }

    /// `stado builds edit <name> [the changed flags only] --json`. A field the
    /// operator left alone contributes no flag, and the registry keeps it.
    nonisolated static func editArguments(_ change: BuildRecipeEdit) -> [String] {
        var arguments = ["builds", "edit", change.name]
        if let repo = change.repo {
            arguments += ["--repo", repo]
        }
        if let branch = change.branch {
            arguments += ["--branch", branch]
        }
        if let command = change.command {
            arguments += ["--command", command]
        }
        for path in change.artifacts ?? [] {
            arguments += ["--artifact", path]
        }
        for platform in change.platforms ?? [] {
            arguments += ["--platform", platform]
        }
        if let seconds = change.intervalSeconds {
            arguments += ["--interval-seconds", String(seconds)]
        }
        if let autoDeclare = change.autoDeclare {
            arguments.append(autoDeclare ? "--auto-declare" : "--no-auto-declare")
        }
        arguments.append("--json")
        return arguments
    }

    nonisolated static func removeArguments(name: String) -> [String] {
        ["builds", "remove", name, "--json"]
    }

    /// The recipe's platforms in one clause, for a sentence an operator reads
    /// once. A recipe with no platform declares nothing to build, and saying so
    /// is better than an empty gap in the sentence.
    nonisolated static func platformList(_ recipe: BuildRecipe) -> String {
        recipe.platforms.isEmpty ? "no platform" : recipe.platforms.joined(separator: " and ")
    }
}
