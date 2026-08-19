import SwiftUI
import WisentDesignSystem

/// The pending flip, enqueue or removal, held until the operator confirms it.
/// A build command is arbitrary code on a fleet host and a removal is a
/// registry write, so neither the toggle, the run button nor the delete button
/// writes anything by itself.
private struct BuildDecision: Identifiable {
    enum Kind: String {
        case enable
        case disable
        case run
        case remove
    }

    let kind: Kind
    let recipe: BuildRecipe

    var id: String { "\(kind.rawValue)/\(recipe.name)" }
}

/// What the recipe form is authoring: a recipe that does not exist yet, or the
/// one it was opened from.
///
/// The identity of the add form is a string no recipe can be named — the CLI
/// takes kebab-case only — so opening the form on a recipe and opening it on
/// nothing are never the same sheet.
private struct BuildRecipeEditor: Identifiable {
    let original: BuildRecipe?

    var id: String { original?.name ?? "+new" }
}

/// Which repositories the control plane watches, what each platform's last
/// build produced, and which version came out of it.
///
/// The boundary, stated where the operator acts on it: a build ends at
/// artifacts under the job's results plus the version the built commit's tag
/// names. A recipe with auto-declare on writes that version into the
/// managed versions of the hosts on the run's platform — nothing more.
/// Promoting a signed release stays a separate, deliberate step
/// (`stado release promote`, which verifies the manifest and its signature),
/// and delivering it stays `converge --apply`.
struct BuildsView: View {
    @ObservedObject var store: BuildsStore
    let scope: String

    @State private var decision: BuildDecision?
    /// The recipe form, open on a new recipe or on an existing one. It is its
    /// own sheet rather than a case of `decision`, because the form confirms
    /// its own change: swapping one sheet's subject mid-flight gives AppKit two
    /// presentations to arbitrate over one binding.
    @State private var editor: BuildRecipeEditor?
    /// Which recipes have their per-platform runs open. Expansion is additive
    /// and remembered across refreshes: a refresh must not close the rows the
    /// operator opened to watch a build.
    @State private var expanded: Set<String> = []

    var body: some View {
        WisentScreen(
            title: "Builds",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(store.lastUpdated))",
            actions: [
                WisentAction("New recipe…", symbol: "plus", kind: .primary) {
                    editor = BuildRecipeEditor(original: nil)
                },
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task { await store.refresh() }
                },
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: 0) {
                if store.lastUpdated == nil, store.isRefreshing {
                    WisentLoadingPanel(
                        title: "Reading build recipes",
                        detail: "stado builds list --json against the canonical registry. Nothing is written."
                    )
                    .padding(WisentDesign.Space.x6)
                    Spacer(minLength: 0)
                } else {
                    notices
                    table
                }
            }
        }
        .task { await store.refresh() }
        .sheet(item: $decision) { pending in
            dialog(pending)
        }
        .sheet(item: $editor) { pending in
            BuildRecipeFormView(
                original: pending.original,
                taken: Set(store.recipes.map(\.name)),
                submit: { outcome in
                    editor = nil
                    switch outcome {
                    case let .add(draft):
                        Task { await store.add(draft) }
                    case let .change(change):
                        Task { await store.edit(change) }
                    }
                },
                cancel: { editor = nil }
            )
        }
    }

    // MARK: What went wrong, at the top

    @ViewBuilder
    private var notices: some View {
        VStack(spacing: WisentDesign.Space.x3) {
            WisentMutationBar(outcome: store.mutation) { store.clearMutation() }
            if let problem = store.problem {
                WisentAlertPanel(
                    tone: .warning,
                    title: "Build recipes could not be read",
                    detail: problem,
                    command: "stado builds list --json",
                    actions: [
                        WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                            Task { await store.refresh() }
                        },
                    ]
                )
            }
        }
        .padding(.horizontal, WisentDesign.Space.x4)
        .padding(.top, store.problem == nil && store.mutation == .idle ? 0 : WisentDesign.Space.x4)
    }

    // MARK: Rows

    /// The recipe row's columns. `recipe` is also the width of the gutter the
    /// platform rows indent past, which is what makes an expanded recipe read
    /// as one block instead of two tables.
    private enum Column {
        static let recipe: CGFloat = 148
        static let enabled: CGFloat = 56
        static let seen: CGFloat = 76
        static let platforms: CGFloat = 200
        static let latest: CGFloat = 92
        static let declare: CGFloat = 68
        static let action: CGFloat = 88
    }

    @ViewBuilder
    private var table: some View {
        if store.recipes.isEmpty {
            empty
        } else {
            ConsoleTable(head: [
                ConsoleHeaderCell("Recipe", width: Column.recipe),
                ConsoleHeaderCell("Repository"),
                ConsoleHeaderCell("On", width: Column.enabled),
                ConsoleHeaderCell("Last seen", width: Column.seen),
                ConsoleHeaderCell("Platforms", width: Column.platforms),
                ConsoleHeaderCell("Latest", width: Column.latest),
                ConsoleHeaderCell("Declare", width: Column.declare),
                ConsoleHeaderCell("", width: Column.action, trailing: true),
            ]) {
                ForEach(store.recipes) { recipe in
                    row(recipe)
                    if expanded.contains(recipe.id) {
                        inspector(recipe)
                        ForEach(recipe.platformRuns) { entry in
                            platformRow(entry, of: recipe)
                        }
                    }
                }
            }
        }
    }

    /// One recipe. The disclosure is its own button rather than the whole row:
    /// a row-wide button folds the enablement switch and the Run button into a
    /// single accessibility element, and both of those are writes an operator
    /// must be able to reach without a mouse. Every recipe discloses something
    /// — what it builds and how, plus the two verbs that change or remove it —
    /// so the arrow is always there and always opens onto an answer.
    private func row(_ recipe: BuildRecipe) -> some View {
        let isOpen = expanded.contains(recipe.id)
        return ConsoleTableRow(isSelected: isOpen) {
            HStack(spacing: WisentDesign.Space.x1) {
                Button { toggleExpansion(recipe) } label: {
                    Image(systemName: isOpen ? "chevron.down" : "chevron.right")
                        .font(.system(size: 8, weight: .semibold))
                        .foregroundStyle(WisentDesign.muted)
                        .frame(width: 14, height: WisentAppLayout.tableRowHeight)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(
                    isOpen
                        ? "Hide what \(recipe.name) builds"
                        : "Show what \(recipe.name) builds"
                )
                ConsoleCell(text: recipe.name, identifier: true, strong: true)
            }
            .frame(width: Column.recipe, alignment: .leading)
            ConsoleCell(text: "\(recipe.repo)@\(recipe.ref)", identifier: true)
            Toggle(recipe.enabled ? "Disable \(recipe.name)" : "Enable \(recipe.name)", isOn: enablementBinding(recipe))
                .labelsHidden()
                .toggleStyle(.switch)
                .controlSize(.mini)
                .disabled(store.mutation.isWorking)
                .frame(width: Column.enabled, alignment: .leading)
            ConsoleCell(
                text: recipe.lastSeenRef.map { String($0.prefix(8)) } ?? "Never",
                width: Column.seen,
                identifier: true
            )
            platformStrip(recipe)
            ConsoleCell(
                text: latestText(recipe),
                width: Column.latest,
                tone: recipe.hasFailedRun ? .danger : .neutral
            )
            ConsoleCell(
                text: recipe.autoDeclare ? "auto" : "manual",
                width: Column.declare,
                identifier: true,
                tone: recipe.autoDeclare ? .warning : .neutral
            )
            HStack {
                Spacer(minLength: 0)
                WisentActionButton(
                    action: WisentAction("Run now…", kind: .plain, isEnabled: !store.mutation.isWorking) {
                        decision = BuildDecision(kind: .run, recipe: recipe)
                    }
                )
            }
            .frame(width: Column.action)
        }
    }

    /// The recipe itself, under its own row: what each job runs, what it
    /// uploads, how often the poller looks, and the two verbs that change or
    /// remove the recipe.
    ///
    /// Change and Delete live here rather than in the row because the row's
    /// width is spent on what the fleet is doing right now. They also read in
    /// the right order: an operator opens a recipe to see what it builds, and
    /// the verb that rewrites it is one line under the answer.
    private func inspector(_ recipe: BuildRecipe) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            HStack(alignment: .top, spacing: WisentDesign.Space.x5) {
                WisentField(label: "build command", value: recipe.command)
                WisentField(
                    label: "artifacts",
                    value: recipe.artifacts.isEmpty
                        ? "none — a build uploads nothing"
                        : recipe.artifacts.joined(separator: "\n"),
                    tone: recipe.artifacts.isEmpty ? .warning : .neutral
                )
                WisentField(label: "poll", value: "every \(recipe.intervalSeconds.formatted(.number))s")
            }
            HStack(spacing: WisentDesign.Space.x2) {
                WisentActionButton(
                    action: WisentAction(
                        "Change…",
                        symbol: "slider.horizontal.3",
                        isEnabled: !store.mutation.isWorking
                    ) {
                        editor = BuildRecipeEditor(original: recipe)
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Delete…",
                        symbol: "trash",
                        kind: .destructive,
                        isEnabled: !store.mutation.isWorking
                    ) {
                        decision = BuildDecision(kind: .remove, recipe: recipe)
                    }
                )
                Spacer(minLength: 0)
                Text("enable, disable and run stay on the row above")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.muted)
            }
        }
        .padding(.vertical, WisentDesign.Space.x3)
        .padding(.trailing, WisentDesign.Space.x4)
        .padding(.leading, Column.recipe + WisentDesign.Space.x4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(WisentDesign.canvasMuted.opacity(0.5))
        .overlay(alignment: .bottom) {
            Rectangle()
                .fill(WisentDesign.border.opacity(0.6))
                .frame(height: WisentDesign.hairline)
        }
    }

    /// The platform list, each name carrying its own run's tone: a red
    /// `linux-amd64` next to a green `darwin-arm64` is the whole answer to
    /// "which half is broken" without opening anything.
    private func platformStrip(_ recipe: BuildRecipe) -> some View {
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
            Spacer(minLength: 0)
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
    private func platformRow(_ entry: BuildPlatformRun, of recipe: BuildRecipe) -> some View {
        let run = entry.run
        let tone = tone(for: run)
        return ConsoleTableRow {
            HStack {
                Spacer(minLength: 0)
                Image(systemName: "arrow.turn.down.right")
                    .font(.system(size: 9, weight: .regular))
                    .foregroundStyle(WisentDesign.border)
                    .accessibilityHidden(true)
            }
            .frame(width: Column.recipe)
            ConsoleCell(text: entry.platform, width: 168, identifier: true, tone: tone)
            ConsoleCell(text: run?.status ?? "never ran", width: 88, tone: tone)
            ConsoleCell(text: versionText(run), width: 100, identifier: true)
            declaredBadge(run, autoDeclare: recipe.autoDeclare)
                .frame(width: 136, alignment: .leading)
            ConsoleCell(
                text: run.map { "job \($0.jobID.prefix(8))" } ?? "—",
                width: 112,
                identifier: true
            )
            ConsoleCell(text: atText(run), width: 120)
            Spacer(minLength: 0)
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

    /// The switch shows what the registry records; flipping it asks first.
    /// The value on screen only changes when the CLI's answer comes back.
    private func enablementBinding(_ recipe: BuildRecipe) -> Binding<Bool> {
        Binding(
            get: { recipe.enabled },
            set: { enabled in
                decision = BuildDecision(kind: enabled ? .enable : .disable, recipe: recipe)
            }
        )
    }

    private func toggleExpansion(_ recipe: BuildRecipe) {
        if expanded.contains(recipe.id) {
            expanded.remove(recipe.id)
        } else {
            expanded.insert(recipe.id)
        }
    }

    private func tone(for run: BuildRun?) -> WisentTone {
        switch run?.status {
        case "failed": .danger
        case "succeeded": .success
        case "running": .warning
        default: .neutral
        }
    }

    /// A platform with no run yet is muted, not toned: never having built is
    /// not a state worth a colour.
    private func color(for run: BuildRun?) -> Color {
        run == nil ? WisentDesign.muted : tone(for: run).color
    }

    /// The age of the newest run across every platform, so the collapsed row
    /// still answers "did anything happen lately". "failed" with no age would
    /// leave the operator asking "failed when?", which is the whole question on
    /// this screen, and the per-platform rows below carry the rest.
    private func latestText(_ recipe: BuildRecipe) -> String {
        guard let run = recipe.newestRun else { return "Never ran" }
        return atText(run)
    }

    /// The stamp the registry wrote, as an age. An unparseable stamp is shown
    /// verbatim: a console that silently drops a value it cannot read is worse
    /// than one that shows the registry's own string.
    private func atText(_ run: BuildRun?) -> String {
        guard let run else { return "—" }
        guard let date = DisplayFormat.date(run.at) else {
            return run.at.isEmpty ? "unrecorded" : run.at
        }
        return ConsoleFormat.relative(date)
    }

    /// The tag the built commit carried, which is the only thing a build can
    /// declare. "untagged" is a conclusion; a run still in flight has not
    /// reached one.
    private func versionText(_ run: BuildRun?) -> String {
        guard let run else { return "—" }
        if let version = run.version { return version }
        return run.status == "running" ? "—" : "untagged"
    }

    /// Empty because the registry declares nothing, or empty because it could
    /// not be read. The first is answered by authoring a recipe, the second by
    /// reading again — two states, two remedies, so the one button is the one
    /// that applies.
    private var empty: some View {
        VStack {
            WisentEmptyPanel(
                title: store.problem == nil ? "No build recipes" : "Build recipes are unknown",
                detail: store.problem
                    ?? "The registry declares nothing to build. A recipe names a repository and branch to watch, the command each job runs in the checkout, the paths it uploads, and at least one platform — a build job can only be claimed by a worker that is that platform. A new recipe starts disabled; enabling it here is what makes the control plane poll it.",
                symbol: "hammer",
                action: store.problem == nil
                    ? WisentAction("New recipe…", symbol: "plus", kind: .primary) {
                        editor = BuildRecipeEditor(original: nil)
                    }
                    : WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary, isEnabled: !store.isRefreshing) {
                        Task { await store.refresh() }
                    }
            )
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(WisentDesign.surface)
    }

    // MARK: The decision, before the write

    private func dialog(_ pending: BuildDecision) -> WisentDecisionDialog {
        let recipe = pending.recipe
        switch pending.kind {
        case .enable:
            return WisentDecisionDialog(
                tone: .warning,
                title: "Enable builds for \(recipe.name)?",
                lines: [
                    "The control plane starts polling \(recipe.repo) at \(recipe.ref) every \(recipe.intervalSeconds.formatted(.number)) seconds. Every new commit it sees is built once per platform — \(BuildsStore.platformList(recipe)) — and each job can only be claimed by a worker of that platform.",
                    "Each job clones the repository on a fleet host, runs the recipe's build command there, and uploads the declared artifacts under the job's results. It also records the version: the semver tag on the built commit, or none when the commit carries no tag.",
                    declarationLine(recipe),
                ],
                listing: ["command: \(recipe.command)"]
                    + recipe.platforms.map { "platform: \($0)" }
                    + recipe.artifacts.map { "artifact: \($0)" },
                footnote: "Runs \(StadoCLI.commandLine(BuildsStore.enablementArguments(name: recipe.name, enabled: true))).",
                actions: [
                    WisentAction("Keep it disabled", kind: .secondary) { decision = nil },
                    WisentAction("Enable", symbol: "play.circle", kind: .primary) {
                        decision = nil
                        Task { await store.setEnabled(recipe, to: true) }
                    },
                ]
            )
        case .disable:
            return WisentDecisionDialog(
                tone: .warning,
                title: "Disable builds for \(recipe.name)?",
                lines: [
                    "The control plane stops polling \(recipe.repo) at \(recipe.ref). Commits made while it is disabled are not built, and nothing catches up on them when it is enabled again — only the next new commit is.",
                    "A job already enqueued keeps running; this stops new ones.",
                ],
                footnote: "Runs \(StadoCLI.commandLine(BuildsStore.enablementArguments(name: recipe.name, enabled: false))).",
                actions: [
                    WisentAction("Keep it enabled", kind: .secondary) { decision = nil },
                    WisentAction("Disable", symbol: "pause.circle", kind: .primary) {
                        decision = nil
                        Task { await store.setEnabled(recipe, to: false) }
                    },
                ]
            )
        case .run:
            return WisentDecisionDialog(
                tone: .warning,
                title: recipe.platforms.count == 1
                    ? "Enqueue a build of \(recipe.name) now?"
                    : "Enqueue \(recipe.platforms.count) builds of \(recipe.name) now?",
                lines: [
                    "One job per platform is enqueued immediately — \(BuildsStore.platformList(recipe)) — without waiting for the poller to see a new commit. Each claims a worker slot on its own platform, clones \(recipe.repo) at \(recipe.ref), runs the build command, and uploads the declared artifacts under the job's results.",
                    "Each job also records the version: the semver tag on the built commit, or none when the commit carries no tag.",
                    declarationLine(recipe),
                ],
                listing: ["command: \(recipe.command)"]
                    + recipe.platforms.map { "platform: \($0)" }
                    + recipe.artifacts.map { "artifact: \($0)" },
                footnote: "Runs \(StadoCLI.commandLine(BuildsStore.runArguments(name: recipe.name))).",
                actions: [
                    WisentAction("Not now", kind: .secondary) { decision = nil },
                    WisentAction(
                        recipe.platforms.count == 1 ? "Enqueue the build" : "Enqueue the builds",
                        symbol: "hammer",
                        kind: .primary
                    ) {
                        decision = nil
                        Task { await store.run(recipe) }
                    },
                ]
            )
        case .remove:
            return WisentDecisionDialog(
                tone: .danger,
                title: "Delete the build recipe \(recipe.name)?",
                lines: [
                    "The registry stops declaring \(recipe.name). What it built \(recipe.repo) at \(recipe.ref) with — the command, the artifact paths, its platforms, the commit it last saw and every run it recorded — is gone from the registry with it, and nothing here brings it back.",
                    "What already happened stays: a job it enqueued keeps running and keeps its results under the queue, and a version it declared stays the managed version of the hosts that took it. Deleting a recipe never un-declares anything.",
                    "To stop building it without losing it, disable it instead — the switch on its row.",
                ],
                listing: ["source: \(recipe.repo)@\(recipe.ref)", "command: \(recipe.command)"]
                    + recipe.platforms.map { "platform: \($0)" }
                    + recipe.artifacts.map { "artifact: \($0)" }
                    + ["last seen: \(recipe.lastSeenRef ?? "never polled")"]
                    + ["recorded runs: \(recipe.runs.count.formatted(.number))"],
                footnote: "Runs \(StadoCLI.commandLine(BuildsStore.removeArguments(name: recipe.name))).",
                actions: [
                    WisentAction("Keep the recipe", kind: .primary) { decision = nil },
                    WisentAction("Delete it", symbol: "trash", kind: .destructive) {
                        decision = nil
                        Task { await store.remove(recipe) }
                    },
                ]
            )
        }
    }

    /// What a succeeded build does to the fleet, which is the one sentence in
    /// these dialogs that is about a write and not about a build. Auto-declare
    /// is the only path from this screen to a host's managed versions, and even
    /// then a signed release is a separate, deliberate step.
    private func declarationLine(_ recipe: BuildRecipe) -> String {
        if recipe.autoDeclare {
            return "This recipe declares automatically: a succeeded build whose commit carried a semver tag writes that version into the managed versions of every registry host on the run's platform. An untagged commit declares nothing. Promoting a signed release is still separate — stado release promote verifies the manifest and its signature — and delivery is still converge --apply."
        }
        return "Nothing is declared to the fleet and no host converges onto it. The version a build records is a fact on the recipe until stado host declare-version writes it, and a signed release is a separate step: stado release promote verifies the manifest and its signature, then converge --apply delivers it."
    }
}

/// What the form hands back: a recipe to author, or the fields to change on
/// one that exists.
private enum BuildRecipeSubmission {
    case add(BuildRecipeDraft)
    case change(BuildRecipeEdit)
}

/// Author a build recipe, or change one.
///
/// One form for both, because a recipe is the same object either way and an
/// operator who has filled this in once should not have to learn a second
/// layout to correct a typo in it. The difference is what it submits: adding
/// gives `stado builds add` every field, since add requires them all, while
/// changing gives `stado builds edit` only the fields that actually moved —
/// a flag that is not passed is a value the registry keeps.
///
/// Every rule the CLI would refuse the recipe by is checked here, in the
/// CLI's own terms, and the exact command line sits under the fields. An
/// operator should never learn about a kebab-case name from a non-zero exit.
private struct BuildRecipeFormView: View {
    /// The recipe being changed; nil authors a new one.
    let original: BuildRecipe?
    /// The names the registry already carries. Only a new recipe can collide
    /// with one: a change never renames a recipe.
    let taken: Set<String>
    let submit: (BuildRecipeSubmission) -> Void
    let cancel: () -> Void

    @State private var draft: BuildRecipeDraft
    /// The change the operator asked to look at before it is written. The
    /// dialog takes the place of the fields inside this one sheet: a second
    /// presentation over the first gives AppKit two sheets to arbitrate.
    @State private var reviewing: BuildRecipeEdit?

    init(
        original: BuildRecipe?,
        taken: Set<String>,
        submit: @escaping (BuildRecipeSubmission) -> Void,
        cancel: @escaping () -> Void
    ) {
        self.original = original
        self.taken = taken
        self.submit = submit
        self.cancel = cancel
        _draft = State(
            initialValue: original.map { BuildRecipeDraft($0) } ?? BuildRecipeDraft()
        )
    }

    private var isNew: Bool { original == nil }

    private var problems: [String] {
        draft.problems(taken: isNew ? taken : [])
    }

    /// What this form would change, or nil while it is authoring a new recipe.
    private var change: BuildRecipeEdit? {
        original.map { draft.change(from: $0) }
    }

    /// The invocation this form runs, exactly as it will run it.
    private var commandLine: String {
        if let change {
            return StadoCLI.commandLine(BuildsStore.editArguments(change))
        }
        return StadoCLI.commandLine(BuildsStore.addArguments(draft))
    }

    var body: some View {
        if let reviewing {
            confirmation(reviewing)
        } else {
            form
        }
    }

    // MARK: The fields

    private var form: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            header
            ScrollView {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
                    source
                    build
                    artifacts
                    platforms
                    cadence
                }
                .padding(.trailing, WisentDesign.Space.x2)
            }
            .frame(maxHeight: 420)
            if !problems.isEmpty {
                refusals
            }
            footer
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 720)
        .background(WisentDesign.surface)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text(isNew ? "New build recipe" : "Change \(draft.name)")
                .font(WisentTypeScale.screenTitle())
                .foregroundStyle(WisentDesign.ink)
            Text(
                isNew
                    ? "What to watch, what to run in the checkout, what to keep, and which platforms to build for. The recipe starts disabled: nothing is polled and nothing is built until it is enabled."
                    : "A field left alone keeps the value the registry records. Pointing the recipe at another repository or branch clears the commit it last saw and the runs it recorded; changing how it builds keeps both."
            )
            .font(WisentTypeScale.caption())
            .foregroundStyle(WisentDesign.secondary)
            .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var source: some View {
        WisentSectionBox(
            title: "Name and source",
            detail: isNew
                ? "A kebab-case name, an https:// clone URL, and the branch the poller watches."
                : "The name is how every command addresses this recipe and is not changed here. The source is."
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                if isNew {
                    labelled("name") {
                        TextField("stado-nightly", text: $draft.name)
                            .textFieldStyle(.roundedBorder)
                            .frame(maxWidth: 280)
                    }
                } else {
                    WisentField(label: "name", value: draft.name)
                }
                labelled("repository") {
                    TextField("https://github.com/wisent-ai/example.git", text: $draft.repo)
                        .textFieldStyle(.roundedBorder)
                }
                labelled("branch") {
                    TextField("main", text: $draft.branch)
                        .textFieldStyle(.roundedBorder)
                        .frame(maxWidth: 280)
                }
            }
        }
    }

    private var build: some View {
        WisentSectionBox(
            title: "Build command",
            detail: "One POSIX sh command, run in the checkout on a fleet host. It is arbitrary code on that host, which is why enabling and running are separate, confirmed steps."
        ) {
            TextField("make release", text: $draft.command)
                .textFieldStyle(.roundedBorder)
                .font(WisentTypeScale.identifier())
        }
    }

    private var artifacts: some View {
        WisentSectionBox(
            title: "Artifacts",
            detail: isNew
                ? "Paths in the checkout each job uploads under its results. Relative to the checkout, never climbing out of it."
                : "Paths in the checkout each job uploads. Changing any row replaces the whole recorded list — --artifact never appends to it.",
            trailing: draft.artifactPaths.count == 1 ? "1 path" : "\(draft.artifactPaths.count) paths"
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                ForEach(draft.artifacts.indices, id: \.self) { index in
                    HStack(spacing: WisentDesign.Space.x2) {
                        TextField("dist/example-darwin-arm64.tar.gz", text: artifactBinding(index))
                            .textFieldStyle(.roundedBorder)
                            .font(WisentTypeScale.identifier())
                        Button { removeArtifact(index) } label: {
                            Image(systemName: "minus.circle")
                                .font(.system(size: 12))
                                .foregroundStyle(WisentDesign.muted)
                        }
                        .buttonStyle(.plain)
                        .disabled(draft.artifacts.count == 1)
                        .accessibilityLabel("Remove artifact path \(index + 1)")
                    }
                }
                WisentActionButton(
                    action: WisentAction("Add a path", symbol: "plus", kind: .plain) {
                        draft.artifacts.append("")
                    }
                )
            }
        }
    }

    private var platforms: some View {
        WisentSectionBox(
            title: "Platforms",
            detail: isNew
                ? "One build job per platform, and a job can only be claimed by a worker that is that platform."
                : "Naming a different set replaces the recorded list. A platform named for the first time simply has no run yet, and dropping one keeps the run it already recorded.",
            trailing: draft.platforms.isEmpty ? "none" : draft.platforms.joined(separator: " · ")
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                ForEach(platformChoices, id: \.self) { platform in
                    Toggle(isOn: platformBinding(platform)) {
                        HStack(spacing: WisentDesign.Space.x2) {
                            Text(platform)
                                .font(WisentTypeScale.identifier())
                                .foregroundStyle(WisentDesign.ink)
                            if !BuildPlatforms.all.contains(platform) {
                                WisentBadge("not a release platform", tone: .danger)
                            }
                        }
                    }
                    .toggleStyle(.checkbox)
                }
            }
        }
    }

    private var cadence: some View {
        WisentSectionBox(
            title: "Cadence and declaration",
            detail: "How often the poller asks the repository for the branch head, and whether a succeeded build writes its version to the fleet."
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                labelled("poll interval, seconds") {
                    TextField(String(BuildRecipeDraft.defaultIntervalSeconds), text: $draft.interval)
                        .textFieldStyle(.roundedBorder)
                        .frame(maxWidth: 140)
                }
                Toggle(isOn: $draft.autoDeclare) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Declare the version a succeeded build records")
                            .font(WisentTypeScale.bodyStrong())
                            .foregroundStyle(WisentDesign.ink)
                        Text("A succeeded build whose commit carried a semver tag becomes the managed version of every registry host on that run's platform. An untagged commit declares nothing, and promoting a signed release stays stado release promote.")
                            .font(WisentTypeScale.caption())
                            .foregroundStyle(WisentDesign.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .toggleStyle(.checkbox)
            }
        }
    }

    /// Every rule the CLI would refuse this by, before it is asked to.
    private var refusals: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("stado builds \(isNew ? "add" : "edit") would refuse this as it stands:")
                .font(WisentTypeScale.bodyStrong())
                .foregroundStyle(WisentDesign.ink)
            ForEach(problems, id: \.self) { problem in
                HStack(alignment: .top, spacing: WisentDesign.Space.x2) {
                    Image(systemName: "exclamationmark.circle")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(WisentTone.warning.color)
                        .accessibilityHidden(true)
                    Text(problem)
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .padding(WisentDesign.Space.x3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            WisentTone.warning.softColor,
            in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
        )
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            HStack(alignment: .top, spacing: WisentDesign.Space.x2) {
                Image(systemName: "terminal")
                    .font(.system(size: 11))
                    .foregroundStyle(WisentDesign.muted)
                    .accessibilityHidden(true)
                Text(commandLine)
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(spacing: WisentDesign.Space.x3) {
                WisentActionButton(action: WisentAction("Cancel", perform: cancel))
                Spacer(minLength: 0)
                if let change, change.isEmpty {
                    Text("Nothing has changed yet.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.muted)
                }
                WisentActionButton(action: primaryAction)
            }
        }
    }

    /// Adding writes once the fields hold up; changing asks the operator to
    /// read what the change does to the recipe's recorded state first.
    private var primaryAction: WisentAction {
        if let change {
            return WisentAction(
                "Review the change…",
                symbol: "arrow.right.circle",
                kind: .primary,
                isEnabled: problems.isEmpty && !change.isEmpty
            ) {
                reviewing = change
            }
        }
        return WisentAction(
            "Add the recipe",
            symbol: "plus",
            kind: .primary,
            isEnabled: problems.isEmpty
        ) {
            submit(.add(draft))
        }
    }

    // MARK: The decision, before the write

    /// What the change does to the recipe, field by field and in words, with
    /// the invocation it runs quoted underneath.
    ///
    /// A change that moves the source discards recorded state that does not
    /// come back, so it wears the red button; a change to how the recipe
    /// builds keeps everything and does not.
    private func confirmation(_ change: BuildRecipeEdit) -> some View {
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

    // MARK: Controls

    /// Label over control, matching the inspector's label over value: the form
    /// reads like the row it was opened from.
    private func labelled<Content: View>(
        _ label: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text(label.uppercased())
                .font(WisentTypeScale.eyebrow())
                .tracking(0.6)
                .foregroundStyle(WisentDesign.muted)
            content()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// A bounds-checked binding onto one artifact row: a row removed while its
    /// field is still on screen must not index past the end of the list.
    private func artifactBinding(_ index: Int) -> Binding<String> {
        Binding(
            get: { draft.artifacts.indices.contains(index) ? draft.artifacts[index] : "" },
            set: { value in
                guard draft.artifacts.indices.contains(index) else { return }
                draft.artifacts[index] = value
            }
        )
    }

    /// The list always keeps a row, so there is always somewhere to type.
    private func removeArtifact(_ index: Int) {
        guard draft.artifacts.count > 1, draft.artifacts.indices.contains(index) else { return }
        draft.artifacts.remove(at: index)
    }

    /// The published platform table, plus any word this recipe already
    /// declares that the table does not carry: a registry written by hand can
    /// name one, and a switch the form refuses to draw is a value the operator
    /// cannot get rid of.
    private var platformChoices: [String] {
        BuildPlatforms.all + draft.platforms.filter { !BuildPlatforms.all.contains($0) }
    }

    /// Selecting appends, so the flags come out in the order the operator
    /// named them; deselecting leaves the rest where they were.
    private func platformBinding(_ platform: String) -> Binding<Bool> {
        Binding(
            get: { draft.platforms.contains(platform) },
            set: { selected in
                if selected {
                    if !draft.platforms.contains(platform) {
                        draft.platforms.append(platform)
                    }
                } else {
                    draft.platforms.removeAll { $0 == platform }
                }
            }
        )
    }
}
