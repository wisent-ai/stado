import Foundation

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
