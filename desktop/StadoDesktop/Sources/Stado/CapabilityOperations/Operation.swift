import Foundation

struct NativeCapabilityField: Identifiable, Sendable {
    let id: String
    let label: String
    var option: String?
    var required = false
    var flag = false
    var multiple = false
    var choices: [String] = []
    var initial = ""
}

struct NativeCapabilityRequest: Sendable {
    let arguments: [String]
    let input: String?
    let standardInput: String?
    let mutates: Bool
}

struct NativeCapabilityOperation: Identifiable, Sendable {
    enum HostPlacement: Sendable {
        case positional
        case option(String)
        case none
    }
    enum Payload: Sendable {
        case none
        case file(option: String?, label: String, initial: String)
        case standardInput(label: String, initial: String)

        var label: String? {
            switch self {
            case .none: nil
            case let .file(_, label, _), let .standardInput(label, _): label
            }
        }
        var initial: String {
            switch self {
            case .none: ""
            case let .file(_, _, initial), let .standardInput(_, initial): initial
            }
        }
    }

    let id: String
    let title: String
    let path: [String]
    var hostPlacement: HostPlacement = .option("--host")
    var fields: [NativeCapabilityField] = []
    var fixedArguments: [String] = []
    var payload: Payload = .none
    var mutates = true
    var jsonOutput = true

    func request(host: String, values: [String: String], content: String) throws -> NativeCapabilityRequest {
        var arguments = path
        switch hostPlacement {
        case .positional: arguments.append(host)
        case let .option(option): arguments += [option, host]
        case .none: break
        }
        for field in fields {
            let value = values[field.id] ?? field.initial
            if field.flag {
                if value == "true", let option = field.option { arguments.append(option) }
                continue
            }
            if value.isEmpty {
                if field.required { throw NativeCapabilityInputError.missing(field.label) }
                continue
            }
            if !field.choices.isEmpty && !field.choices.contains(value) {
                throw NativeCapabilityInputError.invalidChoice(field.label)
            }
            if field.multiple {
                let entries = value.split(whereSeparator: \.isNewline)
                if field.required && entries.isEmpty { throw NativeCapabilityInputError.missing(field.label) }
                for entry in entries {
                    if let option = field.option { arguments.append(option) }
                    arguments.append(String(entry))
                }
            } else {
                if let option = field.option { arguments.append(option) }
                arguments.append(value)
            }
        }
        arguments += fixedArguments
        var input: String?
        var standardInput: String?
        switch payload {
        case .none: break
        case let .file(option, label, _):
            guard !content.isEmpty else { throw NativeCapabilityInputError.missing(label) }
            if let option { arguments.append(option) }
            arguments.append("$INPUT")
            input = content
        case let .standardInput(label, _):
            guard !content.isEmpty else { throw NativeCapabilityInputError.missing(label) }
            standardInput = content
        }
        if jsonOutput { arguments.append("--json") }
        return NativeCapabilityRequest(arguments: arguments, input: input,
            standardInput: standardInput, mutates: mutates)
    }
}

private enum NativeCapabilityInputError: LocalizedError {
    case missing(String)
    case invalidChoice(String)

    var errorDescription: String? {
        switch self {
        case let .missing(label): "Enter \(label)."
        case let .invalidChoice(label): "Choose a listed value for \(label)."
        }
    }
}
