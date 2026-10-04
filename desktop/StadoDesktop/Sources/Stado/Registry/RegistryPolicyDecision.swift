import Foundation

/// The one filter the screen holds, over queue eligibility.
///
/// Internal rather than private because the stored facet lives on
/// `RegistryView` in `RegistryView.swift` and the rail that selects one sits in
/// `Registry/RegistryFacets.swift`: Swift scopes `private` to one file.
enum RegistryFacet: String, Hashable {
    case all
    case pinned
    case open
}

/// One pending policy write, held until the operator confirms it.
///
/// Internal rather than private because the buttons that build one sit in
/// `Registry/RegistryInspector.swift` and the sheet that reads one in
/// `Registry/RegistryDialogs.swift`.
enum PolicyDecision: Identifiable {
    case pinned(target: String, value: Bool)

    var id: String {
        switch self {
        case let .pinned(target, value): "pinned-\(target)-\(value)"
        }
    }

    var target: String {
        switch self {
        case let .pinned(target, _): target
        }
    }
}
