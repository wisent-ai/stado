import Foundation

struct RepairService: Decodable, Identifiable, Sendable {
    let name: String
    let summary: String
    let repair: [RepairDeclaration]

    var id: String { name }
}

struct RepairDeclaration: Decodable, Identifiable, Sendable {
    let name: String
    let summary: String
    let mutating: Bool
    let proof: String

    var id: String { name }
}

struct RepairReport: Decodable, Sendable {
    let declaration: String
    let service: String
    let target: String?
    let applied: Bool
    let steps: [RepairStepReport]
}

struct RepairStepReport: Decodable, Identifiable, Sendable {
    let name: String
    let summary: String
    let mutating: Bool
    let proof: String
    let status: String
    let observation: RepairObservation

    var id: String { name }
}

indirect enum RepairObservation: Decodable, Sendable {
    case object([String: RepairObservation])
    case array([RepairObservation])
    case string(String)
    case integer(Int64)
    case number(Double)
    case boolean(Bool)
    case null

    init(from decoder: Decoder) throws {
        if let container = try? decoder.container(keyedBy: RepairJSONKey.self) {
            var fields: [String: RepairObservation] = [:]
            for key in container.allKeys {
                fields[key.stringValue] = try container.decode(RepairObservation.self, forKey: key)
            }
            self = .object(fields)
            return
        }
        if var container = try? decoder.unkeyedContainer() {
            var values: [RepairObservation] = []
            while !container.isAtEnd {
                values.append(try container.decode(RepairObservation.self))
            }
            self = .array(values)
            return
        }
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .boolean(value)
        } else if let value = try? container.decode(Int64.self) {
            self = .integer(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else {
            self = .string(try container.decode(String.self))
        }
    }

    var text: String {
        switch self {
        case let .object(fields):
            fields.keys.sorted().map { key in
                "\(key): \(fields[key]?.text ?? "null")"
            }.joined(separator: "\n")
        case let .array(values):
            values.map(\.text).joined(separator: "\n")
        case let .string(value): value
        case let .integer(value): String(value)
        case let .number(value): value.formatted(.number.precision(.fractionLength(0 ... 3)))
        case let .boolean(value): value ? "true" : "false"
        case .null: "null"
        }
    }
}

private struct RepairJSONKey: CodingKey {
    let stringValue: String
    let intValue: Int?

    init?(stringValue: String) {
        self.stringValue = stringValue
        intValue = nil
    }

    init?(intValue: Int) {
        stringValue = String(intValue)
        self.intValue = intValue
    }
}
