import Foundation

/// Any JSON value returned by the product API. The reconciliation schema grows
/// as new proofs are recorded, so Desktop retains every field rather than
/// decoding a lossy UI-shaped subset.
indirect enum StorageReconciliationJSON: Codable, Sendable {
    case object([String: StorageReconciliationJSON])
    case array([StorageReconciliationJSON])
    case string(String)
    case integer(Int64)
    case unsigned(UInt64)
    case number(Double)
    case boolean(Bool)
    case null

    init(from decoder: Decoder) throws {
        if let container = try? decoder.container(keyedBy: JSONKey.self) {
            var result: [String: StorageReconciliationJSON] = [:]
            for key in container.allKeys {
                result[key.stringValue] = try container.decode(StorageReconciliationJSON.self, forKey: key)
            }
            self = .object(result)
            return
        }
        if var container = try? decoder.unkeyedContainer() {
            var result: [StorageReconciliationJSON] = []
            while !container.isAtEnd {
                result.append(try container.decode(StorageReconciliationJSON.self))
            }
            self = .array(result)
            return
        }
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .boolean(value)
        } else if let value = try? container.decode(Int64.self) {
            self = .integer(value)
        } else if let value = try? container.decode(UInt64.self) {
            self = .unsigned(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else {
            self = .string(try container.decode(String.self))
        }
    }

    func encode(to encoder: Encoder) throws {
        switch self {
        case let .object(value):
            var container = encoder.container(keyedBy: JSONKey.self)
            for (key, item) in value {
                try container.encode(item, forKey: JSONKey(stringValue: key))
            }
        case let .array(value):
            var container = encoder.unkeyedContainer()
            for item in value { try container.encode(item) }
        case let .string(value):
            var container = encoder.singleValueContainer()
            try container.encode(value)
        case let .integer(value):
            var container = encoder.singleValueContainer()
            try container.encode(value)
        case let .unsigned(value):
            var container = encoder.singleValueContainer()
            try container.encode(value)
        case let .number(value):
            var container = encoder.singleValueContainer()
            try container.encode(value)
        case let .boolean(value):
            var container = encoder.singleValueContainer()
            try container.encode(value)
        case .null:
            var container = encoder.singleValueContainer()
            try container.encodeNil()
        }
    }

    subscript(key: String) -> StorageReconciliationJSON? {
        guard case let .object(value) = self else { return nil }
        return value[key]
    }

    var objectValue: [String: StorageReconciliationJSON]? {
        guard case let .object(value) = self else { return nil }
        return value
    }

    var stringValue: String? {
        guard case let .string(value) = self else { return nil }
        return value
    }

    var displayValue: String {
        switch self {
        case let .string(value): value
        case let .integer(value): String(value)
        case let .unsigned(value): String(value)
        case let .number(value): String(value)
        case let .boolean(value): value ? "true" : "false"
        case .null: "null"
        case .object, .array: compactJSON
        }
    }

    /// Foundation's JSON writer consumes the same value without reparsing text.
    var foundationValue: Any {
        switch self {
        case let .object(value): value.mapValues(\.foundationValue)
        case let .array(value): value.map(\.foundationValue)
        case let .string(value): value
        case let .integer(value): value
        case let .unsigned(value): value
        case let .number(value): value
        case let .boolean(value): value
        case .null: NSNull()
        }
    }

    var prettyJSON: String {
        encoded(options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
    }

    private var compactJSON: String {
        encoded(options: [.sortedKeys, .withoutEscapingSlashes])
    }

    private func encoded(options: JSONEncoder.OutputFormatting) -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = options
        guard let data = try? encoder.encode(self) else { return "<unrenderable JSON>" }
        return String(decoding: data, as: UTF8.self)
    }
}

private struct JSONKey: CodingKey {
    let stringValue: String
    let intValue: Int?

    init(stringValue: String) {
        self.stringValue = stringValue
        intValue = nil
    }

    init(intValue: Int) {
        stringValue = String(intValue)
        self.intValue = intValue
    }
}
