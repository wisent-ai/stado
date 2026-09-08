import Foundation

/// One recipe as the form holds it while the operator types: every field a
/// string or a list, before any of the CLI's rules are applied to it.
///
/// This is the only place the console decides anything about a recipe.
/// `problems(taken:)` is `stado builds add`'s own set of refusals, checked
/// here so the operator reads the problem beside the field instead of getting
/// it back as a non-zero exit; `change(from:)` is the diff that becomes flags.
struct BuildRecipeDraft {
    /// The CLI's own `--interval-seconds` default, so a new recipe starts at
    /// the cadence `stado builds add` would have chosen anyway.
    static let defaultIntervalSeconds: UInt64 = 300

    var name = ""
    var repo = ""
    var branch = "main"
    var command = ""
    /// One row per `--artifact`, in the order the registry records them. A
    /// blank row is a row not filled in yet, not an artifact: the form always
    /// carries one so there is somewhere to type.
    var artifacts = [""]
    /// The platforms named, in the order they were named — the order the CLI
    /// canonicalizes and the registry stores.
    var platforms: [String] = []
    /// Seconds, as typed. Held as text so a half-typed number stays a
    /// half-typed number instead of collapsing to zero.
    var interval = String(BuildRecipeDraft.defaultIntervalSeconds)
    var autoDeclare = false

    init() {}

    /// The recipe as the registry records it, ready to be changed. `ref` is
    /// the registry's name for the branch; the flag that writes it is
    /// `--branch`.
    init(_ recipe: BuildRecipe) {
        name = recipe.name
        repo = recipe.repo
        branch = recipe.ref
        command = recipe.command
        artifacts = recipe.artifacts.isEmpty ? [""] : recipe.artifacts
        platforms = recipe.platforms
        interval = String(recipe.intervalSeconds)
        autoDeclare = recipe.autoDeclare
    }

    var recipeName: String { name.trimmingCharacters(in: .whitespaces) }
    var repoURL: String { repo.trimmingCharacters(in: .whitespaces) }
    var branchName: String { branch.trimmingCharacters(in: .whitespaces) }
    var buildCommand: String { command.trimmingCharacters(in: .whitespaces) }

    /// The artifact rows that carry a path, trimmed, in order.
    var artifactPaths: [String] {
        artifacts
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }

    /// The cadence as a number, or nil while the text is not one.
    var intervalSeconds: UInt64? {
        UInt64(interval.trimmingCharacters(in: .whitespaces))
    }

    /// Every rule the CLI would refuse this draft by, in the CLI's own terms.
    ///
    /// `taken` are the recipe names the registry already carries — empty when
    /// changing a recipe, since a change never renames one.
    func problems(taken: Set<String>) -> [String] {
        var problems: [String] = []
        if !Self.isRecipeName(recipeName) {
            problems.append(
                "The name must be kebab-case: lowercase letters, digits and '-', starting and ending with a letter or a digit."
            )
        } else if taken.contains(recipeName) {
            problems.append(
                "A build recipe named \(recipeName) already exists. Change that one instead, or pick another name."
            )
        }
        if !repoURL.hasPrefix("https://") {
            problems.append("The repository must be an https:// clone URL.")
        }
        if branchName.isEmpty {
            problems.append("Name the branch the poller watches.")
        }
        if buildCommand.isEmpty {
            problems.append("Name the build command each job runs in the checkout.")
        }
        let paths = artifactPaths
        if paths.isEmpty {
            problems.append("Name at least one artifact path to upload from the checkout.")
        }
        for path in paths where !Self.isArtifactPath(path) {
            problems.append("Artifact paths are relative to the checkout and never climb out of it: \(path)")
        }
        if platforms.isEmpty {
            problems.append(
                "Name at least one platform: a build job can only be claimed by a worker that is that platform."
            )
        }
        for platform in platforms where !BuildPlatforms.all.contains(platform) {
            problems.append(
                "\(platform) is not a release platform. The published table carries \(BuildPlatforms.all.joined(separator: " and "))."
            )
        }
        switch intervalSeconds {
        case .none:
            problems.append("The poll interval must be a whole number of seconds.")
        case .some(0):
            problems.append("The poll interval must be positive.")
        case .some:
            break
        }
        return problems
    }

    /// The fields this draft changes on `recipe`, and nothing else.
    ///
    /// Ordered lists are compared in order, platforms excepted: the form names
    /// platforms with a set of switches, which cannot express an order, so a
    /// selection naming the same platforms is not a change and the registry
    /// keeps the order it recorded.
    func change(from recipe: BuildRecipe) -> BuildRecipeEdit {
        BuildRecipeEdit(
            name: recipe.name,
            repo: repoURL == recipe.repo ? nil : repoURL,
            branch: branchName == recipe.ref ? nil : branchName,
            command: buildCommand == recipe.command ? nil : buildCommand,
            artifacts: artifactPaths == recipe.artifacts ? nil : artifactPaths,
            platforms: Set(platforms) == Set(recipe.platforms) ? nil : platforms,
            intervalSeconds: intervalSeconds == recipe.intervalSeconds ? nil : intervalSeconds,
            autoDeclare: autoDeclare == recipe.autoDeclare ? nil : autoDeclare
        )
    }

    /// `^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$`, the CLI's own rule: a name that is
    /// safe verbatim in a shell word, a JSON key and a table column.
    static func isRecipeName(_ value: String) -> Bool {
        let bare = { (character: Character) in
            character.isASCII && (character.isLowercase || character.isNumber)
        }
        guard let first = value.first, let last = value.last, bare(first), bare(last) else {
            return false
        }
        return value.allSatisfy { bare($0) || $0 == "-" }
    }

    /// A path inside the checkout: relative, and never climbing out of it.
    static func isArtifactPath(_ path: String) -> Bool {
        !path.hasPrefix("/")
            && !path.split(separator: "/", omittingEmptySubsequences: false).contains("..")
    }
}
