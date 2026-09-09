import Foundation

/// Why a `stado` invocation produced no answer.
///
/// The failure carries the CLI's own sentence rather than a category of it.
/// An operator reading "the host refused the state rewrite" can act; an
/// operator reading "command failed" opens a terminal and runs the command
/// this console just ran, which is the outcome this screen exists to avoid.
enum StadoCLIError: LocalizedError, Sendable {
    case executableMissing
    case failed(exitCode: Int32, message: String)
    case response(exitCode: Int32, stdout: Data, stderr: Data, message: String)

    var errorDescription: String? {
        switch self {
        case .executableMissing:
            "The stado command line is not installed where this app can reach it. Looked on PATH and in ~/.local/bin, ~/.stado/bin, /opt/homebrew/bin and /usr/local/bin."
        case let .failed(exitCode, message):
            message.isEmpty ? "stado exited \(exitCode)." : message
        case let .response(exitCode, _, _, message):
            message.isEmpty ? "stado exited \(exitCode)." : message
        }
    }
}

/// The product CLI, run as a subprocess, in `--json` mode.
///
/// Every read on the operator screens goes through here rather than through
/// the dashboard's HTTP bridge, because these commands reach the hosts
/// themselves: a rollout diagnosis, a log tail off a target, a host's claiming
/// gates. The bridge projects what was published; these answer what is true on
/// the machine right now, which is the difference the release incidents turned
/// on.
///
/// The executable is resolved once and remembered: an app launched from Finder
/// inherits a four-entry PATH, so the search has to include the places the
/// installers actually write to.
/// A decoded JSON answer together with the complete process evidence that
/// produced it. Some commands intentionally print a complete document before
/// exiting non-zero, so callers must retain stdout, stderr, the refusal and the
/// process verdict rather than treating decoding as the whole invocation.
struct StadoCLIJSONResult<Value: Sendable>: Sendable {
    let value: Value
    let exitCode: Int32
    let stdout: Data
    let stderr: Data
    let refusal: String?
}

actor StadoCLI {
    private let configuredExecutable: String?
    private var resolvedExecutable: URL?

    init(executable: String? = nil) {
        configuredExecutable = executable
    }

    /// The command as an operator would type it, for a confirmation dialog to
    /// show before it runs and for a failure to quote afterwards.
    static func commandLine(_ arguments: [String]) -> String {
        (["stado"] + arguments.map(quoted)).joined(separator: " ")
    }

    func executableURL() throws -> URL {
        if let resolvedExecutable { return resolvedExecutable }
        let manager = FileManager.default
        if let configuredExecutable {
            let url = URL(fileURLWithPath: configuredExecutable)
            guard manager.isExecutableFile(atPath: url.path) else {
                throw StadoCLIError.executableMissing
            }
            resolvedExecutable = url
            return url
        }
        let home = manager.homeDirectoryForCurrentUser
        let onPath = (ProcessInfo.processInfo.environment["PATH"] ?? "")
            .split(separator: ":")
            .map { URL(fileURLWithPath: String($0)).appendingPathComponent("stado") }
        let installed = [
            home.appendingPathComponent(".local/bin/stado"),
            home.appendingPathComponent(".stado/bin/stado"),
            URL(fileURLWithPath: "/opt/homebrew/bin/stado"),
            URL(fileURLWithPath: "/usr/local/bin/stado"),
        ]
        guard let executable = (onPath + installed).first(where: {
            manager.isExecutableFile(atPath: $0.path)
        }) else {
            throw StadoCLIError.executableMissing
        }
        resolvedExecutable = executable
        return executable
    }

    /// Shell quoting for display only. Nothing here is ever handed to a shell:
    /// the process is executed directly with an argument vector.
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
