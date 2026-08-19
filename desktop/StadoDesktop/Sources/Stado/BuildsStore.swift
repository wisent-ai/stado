import Foundation
import WisentDesignSystem

/// One build recipe, exactly as `stado builds list --json` prints it.
///
/// Field names are the registry's own (snake_case, the branch serialized as
/// `ref`); nothing here is renamed or derived, so a row on the Builds screen
/// can be checked against the CLI's output word for word.
///
/// The four fields the registry writes with `#[serde(default)]` are decoded
/// with `decodeIfPresent`: a registry written before builds became
/// per-platform declares no platforms and records no runs, and that recipe
/// still has to list.
struct BuildRecipe: Decodable, Identifiable, Hashable, Sendable {
    let name: String
    let repo: String
    let ref: String
    let command: String
    let artifacts: [String]
    let enabled: Bool
    let intervalSeconds: UInt64
    /// The commit the poller last saw on `repo@ref`; `nil` until the first
    /// poll, which is a different fact from "the last build failed".
    let lastSeenRef: String?
    /// The platforms this recipe builds for, from the registry's own platform
    /// keys (`darwin-arm64`, `linux-amd64`). One build job is enqueued per
    /// platform, and only a worker on that platform can claim it.
    let platforms: [String]
    /// Whether a succeeded run whose commit carried a semver tag declares that
    /// version on every registry host of the run's platform. This is the one
    /// field on the row that writes to the fleet.
    let autoDeclare: Bool
    /// The run recorded for each platform, keyed by the platform string.
    let runs: [String: BuildRun]

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name
        case repo
        case ref
        case command
        case artifacts
        case enabled
        case intervalSeconds = "interval_seconds"
        case lastSeenRef = "last_seen_ref"
        case platforms
        case autoDeclare = "auto_declare"
        case runs
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        name = try values.decode(String.self, forKey: .name)
        repo = try values.decode(String.self, forKey: .repo)
        ref = try values.decode(String.self, forKey: .ref)
        command = try values.decode(String.self, forKey: .command)
        artifacts = try values.decodeIfPresent([String].self, forKey: .artifacts) ?? []
        enabled = try values.decodeIfPresent(Bool.self, forKey: .enabled) ?? false
        intervalSeconds = try values.decode(UInt64.self, forKey: .intervalSeconds)
        lastSeenRef = try values.decodeIfPresent(String.self, forKey: .lastSeenRef)
        platforms = try values.decodeIfPresent([String].self, forKey: .platforms) ?? []
        autoDeclare = try values.decodeIfPresent(Bool.self, forKey: .autoDeclare) ?? false
        runs = try values.decodeIfPresent([String: BuildRun].self, forKey: .runs) ?? [:]
    }

    /// Every platform this recipe has something to say about, each paired with
    /// the run the registry recorded for it.
    ///
    /// Declared platforms are first class: one that has never built is a row
    /// that says so, not a row that is missing. A run recorded for a platform
    /// the recipe no longer declares is kept too — dropping it would hide the
    /// last thing that actually happened.
    var platformRuns: [BuildPlatformRun] {
        var keys = Set(platforms)
        keys.formUnion(runs.keys)
        return keys.sorted().map { BuildPlatformRun(platform: $0, run: runs[$0]) }
    }

    /// The newest recorded run across every platform, by the RFC3339 stamp the
    /// registry wrote. Nil while no platform has run.
    var newestRun: BuildRun? {
        runs.values.max { $0.at < $1.at }
    }

    var hasFailedRun: Bool {
        runs.values.contains { $0.status == "failed" }
    }
}

/// One platform of a recipe: what it is, and what the registry recorded for
/// it. `run` is nil for a declared platform that has not built yet.
struct BuildPlatformRun: Identifiable, Hashable, Sendable {
    let platform: String
    let run: BuildRun?

    var id: String { platform }
}

/// What the last enqueued build of a recipe on one platform did, as the
/// registry recorded it.
struct BuildRun: Decodable, Hashable, Sendable {
    /// "succeeded" | "failed" | "running", in the scheduler's own words.
    let status: String
    /// RFC3339 stamp of when the job was enqueued or concluded.
    let at: String
    let jobID: String
    let artifactURIs: [String]
    /// The semver tag on the built commit, without its `v`. Nil when the
    /// commit carried no exact-semver tag, which is what keeps an untagged
    /// build from being declared anywhere.
    let version: String?
    /// Whether this run's version was declared on the hosts of its platform.
    let declared: Bool

    enum CodingKeys: String, CodingKey {
        case status
        case at
        case jobID = "job_id"
        case artifactURIs = "artifact_uris"
        case version
        case declared
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        status = try values.decode(String.self, forKey: .status)
        at = try values.decode(String.self, forKey: .at)
        jobID = try values.decode(String.self, forKey: .jobID)
        artifactURIs = try values.decodeIfPresent([String].self, forKey: .artifactURIs) ?? []
        version = try values.decodeIfPresent(String.self, forKey: .version)
        declared = try values.decodeIfPresent(Bool.self, forKey: .declared) ?? false
    }
}

/// What `stado builds run <name> --json` answers with: the job it enqueued for
/// each platform, and the recipe as the registry now records it.
struct BuildRunReceipt: Decodable, Sendable {
    let name: String
    /// Platform to queue job id. One entry per platform the recipe declares.
    let jobs: [String: String]
    let recipe: BuildRecipe

    enum CodingKeys: String, CodingKey {
        case name
        case jobs
        case recipe
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        name = try values.decode(String.self, forKey: .name)
        jobs = try values.decodeIfPresent([String: String].self, forKey: .jobs) ?? [:]
        recipe = try values.decode(BuildRecipe.self, forKey: .recipe)
    }

    /// "darwin-arm64 6f2c1ab0, linux-amd64 9d40e21c" — every job the one
    /// command enqueued, so the operator can find each on the Queue screen.
    var enqueued: String {
        jobs
            .sorted { $0.key < $1.key }
            .map { "\($0.key) \($0.value)" }
            .joined(separator: ", ")
    }
}

/// What `stado builds remove <name> --json` answers with: the recipe it was
/// asked about, and whether the registry stopped carrying it.
struct BuildRecipeRemoval: Decodable, Sendable {
    let name: String
    let removed: Bool
}

/// The release platforms a recipe may name, in the order the published
/// platform table declares them.
///
/// The form offers exactly these words. A word the table does not carry is a
/// usage error from the CLI, and a console that lets an operator type one is a
/// console that hands back a refusal it could have prevented.
enum BuildPlatforms {
    static let all: [String] = ["darwin-arm64", "linux-amd64"]
}

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

/// The write `stado builds edit` makes, as the set of fields that actually
/// change. A field the operator left alone is absent here and gets no flag,
/// and a flag that is not passed leaves the registry's value untouched.
struct BuildRecipeEdit {
    let name: String
    let repo: String?
    let branch: String?
    let command: String?
    /// The whole artifact list, when it changed. `--artifact` replaces the
    /// recorded list rather than appending to it, so a list that changed at
    /// all is sent whole.
    let artifacts: [String]?
    /// The whole platform list, when it changed, on the same replace terms.
    let platforms: [String]?
    let intervalSeconds: UInt64?
    let autoDeclare: Bool?

    /// Nothing changed, so there is no write to make.
    var isEmpty: Bool {
        repo == nil && branch == nil && command == nil && artifacts == nil
            && platforms == nil && intervalSeconds == nil && autoDeclare == nil
    }

    /// Whether this change points the recipe at a different source, which is
    /// the one thing on this form that discards recorded state: the last seen
    /// commit and every recorded run describe the repository and branch that
    /// were there before, so they go with them.
    var movesSource: Bool { repo != nil || branch != nil }

    /// The registry's own field names for what changed, so a sentence can say
    /// which fields moved without quoting their values a second time.
    var changedFields: [String] {
        var fields: [String] = []
        if repo != nil { fields.append("repo") }
        if branch != nil { fields.append("ref") }
        if command != nil { fields.append("command") }
        if artifacts != nil { fields.append("artifacts") }
        if platforms != nil { fields.append("platforms") }
        if intervalSeconds != nil { fields.append("interval_seconds") }
        if autoDeclare != nil { fields.append("auto_declare") }
        return fields
    }
}

/// The build recipes in the canonical registry, read and written through the
/// product CLI.
///
/// Reads run `stado builds list --json`. Every write — authoring a recipe,
/// changing one, removing one, flipping its enablement, enqueuing a build of
/// every platform now — runs the same `stado builds` command an operator would
/// type, so the confirmation dialog can quote the exact invocation. A refresh
/// that fails keeps the rows from the last successful read on screen: a
/// registry that stopped answering does not erase what it last said.
@MainActor
final class BuildsStore: ObservableObject {
    @Published private(set) var recipes: [BuildRecipe] = []
    /// The list command's own sentence when the last read produced no answer.
    @Published private(set) var problem: String?
    @Published private(set) var isRefreshing = false
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var mutation: WisentMutationOutcome = .idle

    private let cli: StadoCLI
    private var refreshGeneration = 0

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func listArguments() -> [String] {
        ["builds", "list", "--json"]
    }

    nonisolated static func enablementArguments(name: String, enabled: Bool) -> [String] {
        ["builds", enabled ? "enable" : "disable", name, "--json"]
    }

    nonisolated static func runArguments(name: String) -> [String] {
        ["builds", "run", name, "--json"]
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

    func refresh() async {
        guard !isRefreshing else { return }
        refreshGeneration += 1
        let generation = refreshGeneration
        isRefreshing = true
        defer {
            if generation == refreshGeneration {
                isRefreshing = false
            }
        }

        do {
            let listed = try await cli.json([BuildRecipe].self, arguments: Self.listArguments())
            guard generation == refreshGeneration else { return }
            recipes = listed.sorted { $0.name < $1.name }
            problem = nil
        } catch {
            guard generation == refreshGeneration else { return }
            problem = Self.message(for: error)
        }
        lastUpdated = Date()
    }

    /// `stado builds enable|disable <name> --json`. The CLI answers with the
    /// recipe as the registry now records it, which replaces the row before
    /// the follow-up read confirms it.
    func setEnabled(_ recipe: BuildRecipe, to enabled: Bool) async {
        mutation = .working(enabled ? "Enabling \(recipe.name)" : "Disabling \(recipe.name)")
        do {
            let updated = try await cli.json(
                BuildRecipe.self,
                arguments: Self.enablementArguments(name: recipe.name, enabled: enabled)
            )
            replace(updated)
            mutation = .succeeded(
                enabled
                    ? "\(updated.name) is enabled. Every new commit on \(updated.repo) at \(updated.ref) is built once per platform: \(Self.platformList(updated))."
                    : "\(updated.name) is disabled. The control plane stops polling it; nothing new is built until it is enabled again."
            )
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        await refresh()
    }

    /// `stado builds run <name> --json`: one build job per platform the recipe
    /// declares, enqueued now, without waiting for the poller to notice a
    /// commit.
    func run(_ recipe: BuildRecipe) async {
        mutation = .working("Enqueuing a build of \(recipe.name) on \(Self.platformList(recipe))")
        do {
            let receipt = try await cli.json(
                BuildRunReceipt.self,
                arguments: Self.runArguments(name: recipe.name)
            )
            replace(receipt.recipe)
            let enqueued = receipt.enqueued
            mutation = .succeeded(
                enqueued.isEmpty
                    ? "Enqueued the build of \(receipt.name). The Queue screen tracks its jobs from here."
                    : "Enqueued \(receipt.jobs.count == 1 ? "job" : "jobs") for \(receipt.name): \(enqueued). The Queue screen tracks them from here."
            )
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        await refresh()
    }

    /// `stado builds add … --json`. The CLI answers with the recipe it wrote,
    /// which lands in the table before the follow-up read confirms it.
    ///
    /// A new recipe is disabled: authoring one polls nothing and builds
    /// nothing until an operator says so, which is why this write asks for no
    /// confirmation of its own.
    func add(_ draft: BuildRecipeDraft) async {
        mutation = .working("Adding \(draft.recipeName)")
        do {
            let created = try await cli.json(
                BuildRecipe.self,
                arguments: Self.addArguments(draft)
            )
            replace(created)
            var sentence =
                "\(created.name) is recorded for \(Self.platformList(created)) and starts disabled: nothing is polled and nothing is built until it is enabled here, and Run now… builds it once without enabling it."
            if created.autoDeclare {
                sentence +=
                    " Auto-declare is on: a succeeded build whose commit carried a semver tag writes that version into the managed versions of every registry host on the run's platform."
            }
            mutation = .succeeded(sentence)
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        await refresh()
    }

    /// `stado builds edit <name> [changed flags] --json`: the fields the
    /// operator changed, and no others.
    ///
    /// Whether the recipe re-fires is decided by which fields moved, so the
    /// outcome says which state the write cleared. Moving the source clears the
    /// last seen commit and the recorded runs — they describe a repository and
    /// branch that are no longer the recipe's; changing how it builds keeps
    /// both, and a platform named for the first time simply has no run yet.
    func edit(_ change: BuildRecipeEdit) async {
        mutation = .working("Changing \(change.name)")
        do {
            let updated = try await cli.json(
                BuildRecipe.self,
                arguments: Self.editArguments(change)
            )
            replace(updated)
            let fields = change.changedFields.joined(separator: ", ")
            mutation = .succeeded(
                change.movesSource
                    ? "\(updated.name) now builds \(updated.repo) at \(updated.ref) (changed: \(fields)). The last seen commit and every recorded run were cleared: they described the source it no longer builds, so the next poll builds the current head of this one."
                    : "\(updated.name) is changed (changed: \(fields)). The last seen commit and the recorded runs are untouched — how it builds moved, not what it builds from, and a platform named for the first time simply has no run yet."
            )
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        await refresh()
    }

    /// `stado builds remove <name> --json`. The registry stops declaring the
    /// recipe; what the recipe already did to the fleet stays done.
    func remove(_ recipe: BuildRecipe) async {
        mutation = .working("Removing \(recipe.name)")
        do {
            let removal = try await cli.json(
                BuildRecipeRemoval.self,
                arguments: Self.removeArguments(name: recipe.name)
            )
            if removal.removed {
                recipes.removeAll { $0.name == removal.name }
                mutation = .succeeded(
                    "\(removal.name) is removed from the registry: nothing polls it and no new job is enqueued for it. A job it already enqueued keeps running and keeps its results, and a version it declared stays declared on the hosts that took it."
                )
            } else {
                mutation = .failed(
                    "The registry still declares \(removal.name). Nothing was removed."
                )
            }
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        await refresh()
    }

    func clearMutation() {
        mutation = .idle
    }

    private func replace(_ updated: BuildRecipe) {
        if let index = recipes.firstIndex(where: { $0.name == updated.name }) {
            recipes[index] = updated
        } else {
            recipes = (recipes + [updated]).sorted { $0.name < $1.name }
        }
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}
