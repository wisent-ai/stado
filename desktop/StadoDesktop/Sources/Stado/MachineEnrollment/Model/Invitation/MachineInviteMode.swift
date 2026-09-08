import Foundation

// MARK: - Invitation

/// Which of the two invitations was minted, and therefore what the operator
/// has to send.
///
/// They differ in one fact about the machine being added: whether it can reach
/// this fleet's control point at all. The online invitation is one line that
/// fetches the join script from that control point, so it is worthless to a
/// machine that cannot resolve or reach it. The offline invitation carries the
/// fleet's public key inside its own text and asks nothing of the network, so
/// the only thing left to require is that the operator can send that person a
/// message and read one back.
enum MachineInviteMode: String, Codable, Sendable {
    case online
    case offline

    var title: String {
        switch self {
        case .online: "One line the machine runs"
        case .offline: "A fragment you send to whoever has the machine"
        }
    }

    var summary: String {
        switch self {
        case .online:
            "They paste one line. It fetches the join script from this fleet's control point, installs the fleet's public key, and reports the machine back here for you to approve."
        case .offline:
            "They paste a short fragment that already carries the fleet's public key. Nothing is fetched and nothing reports back: they send you the address it prints, and you finish the enrollment with that address."
        }
    }

    /// What the method costs, said as the one condition that decides it.
    var requires: String {
        switch self {
        case .online:
            "The machine being added has to reach this fleet's control point. If it is not on the fleet's network, this is the wrong one."
        case .offline:
            "Nothing but a way to send that person a message and get one back. No route to the control point, from either side."
        }
    }
}
