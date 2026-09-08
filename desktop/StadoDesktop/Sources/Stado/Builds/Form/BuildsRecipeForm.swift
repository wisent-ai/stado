import SwiftUI
import WisentDesignSystem

/// What the form hands back: a recipe to author, or the fields to change on
/// one that exists.
enum BuildRecipeSubmission {
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
///
/// The form's own parts live beside this file: the fields in
/// `BuildsRecipeFields.swift`, the change review in
/// `BuildsRecipeReview.swift`, and the small controls they share in
/// `BuildsRecipeControls.swift`. The state and the derived values below are
/// internal rather than private only because those files read them: Swift
/// scopes `private` to one file.
struct BuildRecipeFormView: View {
    /// The recipe being changed; nil authors a new one.
    let original: BuildRecipe?
    /// The names the registry already carries. Only a new recipe can collide
    /// with one: a change never renames a recipe.
    let taken: Set<String>
    let submit: (BuildRecipeSubmission) -> Void
    let cancel: () -> Void

    @State var draft: BuildRecipeDraft
    /// The change the operator asked to look at before it is written. The
    /// dialog takes the place of the fields inside this one sheet: a second
    /// presentation over the first gives AppKit two sheets to arbitrate.
    @State var reviewing: BuildRecipeEdit?

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

    var isNew: Bool { original == nil }

    var problems: [String] {
        draft.problems(taken: isNew ? taken : [])
    }

    /// What this form would change, or nil while it is authoring a new recipe.
    var change: BuildRecipeEdit? {
        original.map { draft.change(from: $0) }
    }

    /// The invocation this form runs, exactly as it will run it.
    var commandLine: String {
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
}
