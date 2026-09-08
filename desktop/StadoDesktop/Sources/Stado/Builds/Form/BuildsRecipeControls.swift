import SwiftUI
import WisentDesignSystem

/// The small controls the fields are built out of: the label-over-control
/// wrapper, the two artifact-row helpers, and the platform switches.
///
/// All of these are internal rather than private only because the fields that
/// call them sit in `BuildsRecipeFields.swift`: Swift scopes `private` to one
/// file.
extension BuildRecipeFormView {
    // MARK: Controls

    /// Label over control, matching the inspector's label over value: the form
    /// reads like the row it was opened from.
    func labelled<Content: View>(
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
    func artifactBinding(_ index: Int) -> Binding<String> {
        Binding(
            get: { draft.artifacts.indices.contains(index) ? draft.artifacts[index] : "" },
            set: { value in
                guard draft.artifacts.indices.contains(index) else { return }
                draft.artifacts[index] = value
            }
        )
    }

    /// The list always keeps a row, so there is always somewhere to type.
    func removeArtifact(_ index: Int) {
        guard draft.artifacts.count > 1, draft.artifacts.indices.contains(index) else { return }
        draft.artifacts.remove(at: index)
    }

    /// The published platform table, plus any word this recipe already
    /// declares that the table does not carry: a registry written by hand can
    /// name one, and a switch the form refuses to draw is a value the operator
    /// cannot get rid of.
    var platformChoices: [String] {
        BuildPlatforms.all + draft.platforms.filter { !BuildPlatforms.all.contains($0) }
    }

    /// Selecting appends, so the flags come out in the order the operator
    /// named them; deselecting leaves the rest where they were.
    func platformBinding(_ platform: String) -> Binding<Bool> {
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
