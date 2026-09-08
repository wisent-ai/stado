import Foundation

// MARK: - Ways in

/// Which way into the fleet the operator is currently walking.
///
/// `methods` is not a step before the work; it is the screen that names the
/// four ways and what each one costs, because picking the wrong one is what
/// turns adding a machine into a phone call.
enum MachineEnrollmentFlow: String, Codable, CaseIterable, Sendable {
    case methods
    case invite
    case adopt
    case handKey
    case join
    case declare

    /// The control plane's method names, mapped to the screens this build can
    /// actually drive. Nothing else in the app decides which methods exist.
    init?(methodName: String) {
        switch methodName {
        case "invite": self = .invite
        case "adopt": self = .adopt
        case "join": self = .join
        case "declare": self = .declare
        default: return nil
        }
    }

    var title: String {
        switch self {
        case .methods: "Ways to add a machine"
        case .invite: "Invite"
        case .adopt: "Adopt"
        case .handKey: "Key installed by hand"
        case .join: "Join"
        case .declare: "Declare"
        }
    }

    var eyebrow: String {
        switch self {
        case .methods: "ADD A MACHINE"
        case .invite: "METHOD — INVITE"
        case .adopt: "METHOD — ADOPT"
        case .handKey: "METHOD — KEY BY HAND"
        case .join: "METHOD — JOIN"
        case .declare: "METHOD — DECLARE"
        }
    }
}
