import Foundation

/// One `stado service restart <name> --host <host> --json` result: the host's
/// operation marker and the postcondition it declared and observed.
struct ServiceRestartReport: Decodable, Sendable {
    let host: String
    let unit: String
    let status: String
    let detail: String
    let postcondition: Postcondition?

    struct Postcondition: Decodable, Sendable {
        let intent: String
        let state: String
        let detail: String

        enum CodingKeys: String, CodingKey {
            case intent, state, detail
        }

        init(from decoder: Decoder) throws {
            let values = try decoder.container(keyedBy: CodingKeys.self)
            intent = try values.decode(String.self, forKey: .intent)
            state = try values.decode(String.self, forKey: .state)
            detail = try values.decode(String.self, forKey: .detail)
        }
    }

    enum CodingKeys: String, CodingKey {
        case host, unit, status, detail, postcondition
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decode(String.self, forKey: .host)
        unit = try values.decode(String.self, forKey: .unit)
        status = try values.decode(String.self, forKey: .status)
        detail = try values.decode(String.self, forKey: .detail)
        postcondition = try values.decodeIfPresent(Postcondition.self, forKey: .postcondition)
    }

    /// Backend success accepts an operation without a declared postcondition;
    /// when it declares one, only the host's observed `met` proves success.
    var succeeded: Bool {
        guard status == "restarted" else { return false }
        guard let postcondition else { return true }
        return postcondition.state == "met"
    }

    var failureText: String {
        let reported = detail.isEmpty ? status : "\(status): \(detail)"
        guard let postcondition, postcondition.state != "met" else { return reported }
        return "\(reported); postcondition \(postcondition.state): \(postcondition.intent) (\(postcondition.detail))"
    }
}
