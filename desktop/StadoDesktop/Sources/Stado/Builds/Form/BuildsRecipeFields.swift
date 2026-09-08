import SwiftUI
import WisentDesignSystem

/// The recipe form's fields, the refusal list under them, and the footer that
/// quotes the invocation and carries the primary button.
///
/// `form` is internal rather than private only because `body` sits in
/// `BuildsRecipeForm.swift`: Swift scopes `private` to one file. Everything
/// else here keeps its `private`, because `form` is the only caller and it
/// shares this file.
extension BuildRecipeFormView {
    // MARK: The fields

    var form: some View {
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
            .frame(maxHeight:
                420)
            if !problems.isEmpty {
                refusals
            }
            footer
        }
        .padding(WisentDesign.Space.x6)
        .frame(width:
            720)
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
                            .frame(maxWidth:
                                280)
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
                        .frame(maxWidth:
                            280)
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
                                .font(.system(size:
                                    12))
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
                        .frame(maxWidth:
                            140)
                }
                Toggle(isOn: $draft.autoDeclare) {
                    VStack(alignment: .leading, spacing:
                        2) {
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
                        .font(.system(size:
                            11, weight: .semibold))
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
                    .font(.system(size:
                        11))
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
                Spacer(minLength:
                    0)
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
}
