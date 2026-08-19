import SwiftUI
import WisentDesignSystem

/// The pending flip or enqueue, held until the operator confirms it. A build
/// command is arbitrary code on a fleet host, so neither the toggle nor the
/// run button writes anything by itself.
private struct BuildDecision: Identifiable {
    enum Kind: String {
        case enable
        case disable
        case run
    }

    let kind: Kind
    let recipe: BuildRecipe

    var id: String { "\(kind.rawValue)/\(recipe.name)" }
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
    /// must be able to reach without a mouse. A recipe that declares no platform
    /// and has recorded no run gets no disclosure at all — an arrow that opens
    /// nothing is worse than no arrow.
    private func row(_ recipe: BuildRecipe) -> some View {
        let isOpen = expanded.contains(recipe.id)
        let opens = !recipe.platformRuns.isEmpty
        return ConsoleTableRow(isSelected: isOpen && opens) {
            HStack(spacing: WisentDesign.Space.x1) {
                if opens {
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
                            ? "Hide the platforms of \(recipe.name)"
                            : "Show the platforms of \(recipe.name)"
                    )
                } else {
                    Color.clear.frame(width: 14, height: WisentAppLayout.tableRowHeight)
                }
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

    private var empty: some View {
        VStack {
            WisentEmptyPanel(
                title: store.problem == nil ? "No build recipes" : "Build recipes are unknown",
                detail: store.problem
                    ?? "The registry declares nothing to build. Add a recipe with stado builds add --name NAME --repo URL --branch BRANCH --command CMD --artifact PATH --platform darwin-arm64; every recipe needs at least one platform, since a build job can only be claimed by a worker of that platform. It starts disabled, and enabling it here is what makes the control plane poll it.",
                symbol: "hammer",
                action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary, isEnabled: !store.isRefreshing) {
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
