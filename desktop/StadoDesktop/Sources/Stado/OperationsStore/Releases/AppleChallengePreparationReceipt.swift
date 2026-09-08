import Foundation

struct AppleChallengePreparationReceipt: Decodable, Sendable {
    let target: String
    let sshTarget: String
    let items: [[String]]
    let error: String?

    enum CodingKeys: String, CodingKey {
        case target, items, error
        case sshTarget = "ssh_target"
    }
}
