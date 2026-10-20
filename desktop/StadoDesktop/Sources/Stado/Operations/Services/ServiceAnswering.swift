import Foundation

/// The independent API question a host asked of a managed unit, as published
/// in its beacon. A running unit can still hold or refuse this operation.
struct ServiceAnswering: Decodable, Sendable {
    let state: String
    let cause: String?
    let since: String?
    let waitedSeconds: UInt64?
    let boundSeconds: UInt64?
    let last: Outcome?
    let result: Outcome?
    let recovery: Recovery?

    struct Outcome: Decodable, Sendable {
        let state: String
        let cause: String?
        let status: Int?
        let successful: Bool?
        let elapsedMs: UInt64?
        let at: String?
        let responseDetail: String?

        enum CodingKeys: String, CodingKey {
            case state, cause, status, successful, at
            case elapsedMs = "elapsed_ms"
            case responseDetail = "detail"
        }

        var succeeded: Bool {
            state == "answered" && status != nil && successful == true
        }

        var detail: String {
            var parts = [state]
            if let status { parts.append("HTTP \(status)") }
            if state == "answered", status == nil { parts.append("HTTP status absent from beacon") }
            if state == "answered", successful == nil { parts.append("HTTP success verdict absent from beacon") }
            if successful == false { parts.append("HTTP answer failed") }
            if let elapsedMs { parts.append("in \(elapsedMs) ms") }
            if let at { parts.append("at \(at)") }
            if let cause { parts.append("cause: \(cause)") }
            if state == "refused", cause == nil { parts.append("refusal cause absent from beacon") }
            if let responseDetail { parts.append("response: \(responseDetail)") }
            return parts.joined(separator: "; ")
        }
    }

    enum Recovery: Decodable, Sendable {
        case alreadyAttempting
        case alreadyAttempted(Record)
        case refused(String)
        case completed(Record)

        enum CodingKeys: String, CodingKey {
            case state, refusal
            case lastRecovery = "last_recovery"
        }

        init(from decoder: Decoder) throws {
            let values = try decoder.container(keyedBy: CodingKeys.self)
            if let state = try values.decodeIfPresent(String.self, forKey: .state) {
                switch state {
                case "already_attempting": self = .alreadyAttempting
                case "already_attempted":
                    self = .alreadyAttempted(try values.decode(Record.self, forKey: .lastRecovery))
                default:
                    throw DecodingError.dataCorruptedError(
                        forKey: .state, in: values,
                        debugDescription: "Unknown recovery episode state \(state)"
                    )
                }
                return
            }
            if let refusal = try values.decodeIfPresent(String.self, forKey: .refusal) {
                self = .refused(refusal)
                return
            }
            self = .completed(try Record(from: decoder))
        }

        var detail: String {
            switch self {
            case .alreadyAttempting: return "recovery already attempting"
            case .alreadyAttempted(let record): return "already attempted; \(record.detail)"
            case .refused(let cause): return "recovery refused: \(cause)"
            case .completed(let record): return record.detail
            }
        }

        struct Record: Decodable, Sendable {
            let at: String
            let restarted: Bool
            let repair: [Step]
            let restart: Restart
            let recordingRefusal: String?
            let alert: [Alert]
            let heldRequests: [HeldRequest]?

            enum CodingKeys: String, CodingKey {
                case at, restarted, repair, restart, alert
                case recordingRefusal = "recording_refusal"
                case heldRequests = "held_requests"
            }

            var detail: String {
                var parts = [restarted ? "restarted at \(at)" : "restart refused at \(at)"]
                if let heldRequests {
                    let unreadable = heldRequests.compactMap(\.unreadable)
                    if unreadable.isEmpty {
                        parts.append("held requests: \(heldRequests.count)")
                    } else {
                        for cause in unreadable { parts.append("resolver waiting list: \(cause)") }
                    }
                } else {
                    parts.append("held request evidence absent from beacon")
                }
                if case .refused(let cause) = restart { parts.append("restart: \(cause)") }
                for step in repair {
                    parts.append("\(step.name): \(step.status)")
                    if let refusal = step.refusal { parts.append("repair: \(refusal)") }
                }
                if let recordingRefusal { parts.append("recovery register: \(recordingRefusal)") }
                for delivery in alert where !delivery.delivered {
                    parts.append("alert \(delivery.channel): \(delivery.detail)")
                }
                return parts.joined(separator: "; ")
            }
        }

        struct HeldRequest: Decodable, Sendable {
            let unreadable: String?
        }

        struct Step: Decodable, Sendable {
            let name: String
            let status: String
            let refusal: String?
        }

        struct Alert: Decodable, Sendable {
            let channel: String
            let delivered: Bool
            let detail: String
        }

        enum Restart: Decodable, Sendable {
            case reported
            case refused(String)

            enum CodingKeys: String, CodingKey { case refusal }

            init(from decoder: Decoder) throws {
                do {
                    _ = try decoder.unkeyedContainer()
                    self = .reported
                } catch DecodingError.typeMismatch(_, _) {
                    let values = try decoder.container(keyedBy: CodingKeys.self)
                    self = .refused(try values.decode(String.self, forKey: .refusal))
                } catch {
                    throw error
                }
            }
        }
    }

    enum CodingKeys: String, CodingKey {
        case state, cause, since, last, result, recovery
        case waitedSeconds = "waited_seconds"
        case boundSeconds = "bound_seconds"
    }

    var needsAttention: Bool {
        switch state {
        case "asked": return false
        case "waiting":
            guard let last else { return false }
            return !last.succeeded
        case "stood": return true
        case "finished":
            guard let result else { return true }
            return !result.succeeded
        default: return true
        }
    }

    var detail: String {
        var parts = ["state: \(state)"]
        if let cause { parts.append("cause: \(cause)") }
        if state == "unreadable", cause == nil { parts.append("unreadable cause absent from beacon") }
        if let since { parts.append("since \(since)") }
        if let waitedSeconds { parts.append("waited \(waitedSeconds) s") }
        if let boundSeconds { parts.append("bound \(boundSeconds) s") }
        if let last { parts.append("last: \(last.detail)") }
        if let result {
            parts.append("result: \(result.detail)")
        } else if state == "finished" {
            parts.append("finished without a result in the beacon")
        }
        if let recovery {
            parts.append("recovery: \(recovery.detail)")
        } else if state == "stood" {
            parts.append("recovery absent from beacon")
        }
        return parts.joined(separator: "; ")
    }
}
