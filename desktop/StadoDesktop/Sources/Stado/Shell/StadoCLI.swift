import Foundation

/// A native operator API refusal, retaining the operation's own evidence.
enum StadoCLIError: LocalizedError, Sendable {
    case unconfigured
    case failed(exitCode: Int?, message: String)
    case response(exitCode: Int?, stdout: Data, stderr: Data, message: String)

    var errorDescription: String? {
        switch self {
        case .unconfigured:
            "No Stado API endpoint is selected. Choose a source before running this operation."
        case let .failed(exitCode, message), let .response(exitCode, _, _, message):
            Self.sentence(exitCode: exitCode, message: message)
        }
    }

    /// The refusal's own message when it has one; otherwise the exit code,
    /// or that the process ended by a signal, which is the one way it ends
    /// without a code.
    private static func sentence(exitCode: Int?, message: String) -> String {
        if !message.isEmpty { return message }
        switch exitCode {
        case .some(let code): return "stado exited \(code)."
        case .none: return "stado ended by a signal, without an exit code."
        }
    }
}

/// A decoded native operator receipt and its complete output evidence.
struct StadoCLIJSONResult<Value: Sendable>: Sendable {
    let value: Value
    let exitCode: Int?
    let stdout: Data
    let stderr: Data
    let refusal: String?
}

actor StadoCLI {
    let client: FleetControlClient

    init(client: FleetControlClient = FleetControlClient()) {
        self.client = client
    }

    /// The command as an operator would type it, for a confirmation dialog to
    /// show before it runs and for a failure to quote afterwards.
    static func commandLine(_ arguments: [String]) -> String {
        (["stado"] + arguments.map(quoted)).joined(separator: " ")
    }

    /// Command notation is for review and diagnostics, never a local shell.
    private static func quoted(_ argument: String) -> String {
        let bare = CharacterSet(
            charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-./:=@+,"
        )
        if !argument.isEmpty, argument.unicodeScalars.allSatisfy({ bare.contains($0) }) {
            return argument
        }
        let escaped = argument
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
        return "\"\(escaped)\""
    }
}
