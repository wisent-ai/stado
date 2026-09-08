import Foundation

/// The one filter the screen holds, over cleanup mode and queue eligibility.
///
/// Internal rather than private because the stored facet lives on
/// `RegistryView` in `RegistryView.swift` and the rail that selects one sits in
/// `Registry/RegistryFacets.swift`: Swift scopes `private` to one file.
enum RegistryFacet: String, Hashable {
    case all
    case enforce
    case report
    case off
    case undeclared
    case pinned
    case open
}

/// One pending policy write, held until the operator confirms it.
///
/// Internal rather than private because the buttons that build one sit in
/// `Registry/RegistryInspector.swift` and the sheet that reads one in
/// `Registry/RegistryDialogs.swift`.
enum PolicyDecision: Identifiable {
    case mode(target: String, mode: FleetCleanupMode, current: String)
    case pinned(target: String, value: Bool)
    case number(target: String, field: FleetCleanupNumericField, value: Int, current: Int?)
    case clearNumber(target: String, field: FleetCleanupNumericField)

    var id: String {
        switch self {
        case let .mode(target, mode, _): "mode-\(target)-\(mode.rawValue)"
        case let .pinned(target, value): "pinned-\(target)-\(value)"
        case let .number(target, field, value, _): "number-\(target)-\(field.rawValue)-\(value)"
        case let .clearNumber(target, field): "clear-\(target)-\(field.rawValue)"
        }
    }

    var target: String {
        switch self {
        case let .mode(target, _, _): target
        case let .pinned(target, _): target
        case let .number(target, _, _, _): target
        case let .clearNumber(target, _): target
        }
    }
}
