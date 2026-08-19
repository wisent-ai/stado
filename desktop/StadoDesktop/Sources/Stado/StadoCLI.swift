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
    case malformedJSON(String)

    var errorDescription: String? {
        switch self {
        case .executableMissing:
            "The stado command line is not installed where this app can reach it. Looked on PATH and in ~/.local/bin, ~/.stado/bin, /opt/homebrew/bin and /usr/local/bin."
        case let .failed(exitCode, message):
            message.isEmpty ? "stado exited \(exitCode)." : message
        case let .malformedJSON(detail):
            "stado answered with something this console could not read: \(detail)"
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

    /// Run `stado <arguments>` and decode its stdout.
    ///
    /// `arguments` excludes the program name and includes `--json`: the caller
    /// names the exact command so the string this console shows in a
    /// confirmation is the string it runs.
    nonisolated func json<T: Decodable & Sendable>(
        _ type: T.Type,
        arguments: [String],
        timeoutSeconds: Int = 120
    ) async throws -> T {
        let executable = try await executableURL()
        let output = try await Self.capture(
            executable: executable,
            arguments: arguments,
            timeoutSeconds: timeoutSeconds
        )
        do {
            return try JSONDecoder().decode(T.self, from: output)
        } catch {
            throw StadoCLIError.malformedJSON(
                "\(Self.commandLine(arguments)) — \(error.localizedDescription)"
            )
        }
    }

    private func executableURL() throws -> URL {
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

    /// The process, and the one thing another thread is allowed to do to it.
    ///
    /// A `stado release doctor` reaches a host over its channel; a host that
    /// has stopped answering makes that read hang, and a hung read with no
    /// deadline is a screen that never finishes loading and never says why.
    private final class Invocation: @unchecked Sendable {
        let process = Process()
        private let lock = NSLock()
        private var expired = false

        var didTimeOut: Bool {
            lock.withLock { expired }
        }

        func terminateForTimeout() {
            lock.withLock {
                guard process.isRunning else { return }
                expired = true
                process.terminate()
            }
        }
    }

    /// Everything the reader thread and the timer thread share, behind a lock.
    private final class ErrorOutput: @unchecked Sendable {
        private let lock = NSLock()
        private var data = Data()

        func store(_ value: Data) {
            lock.withLock { data = value }
        }

        var text: String {
            let value = lock.withLock { data }
            return String(data: value, encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        }
    }

    private static func capture(
        executable: URL,
        arguments: [String],
        timeoutSeconds: Int
    ) async throws -> Data {
        let invocation = Invocation()
        let watchdog = Task.detached(priority: .utility) {
            try? await Task.sleep(for: .seconds(timeoutSeconds))
            invocation.terminateForTimeout()
        }
        defer { watchdog.cancel() }

        return try await Task.detached(priority: .userInitiated) {
            try runToCompletion(
                invocation: invocation,
                executable: executable,
                arguments: arguments,
                timeoutSeconds: timeoutSeconds
            )
        }.value
    }

    /// The blocking half, deliberately synchronous.
    ///
    /// It waits on a pipe and on a process, and a blocking wait is unavailable
    /// from an async context for a reason: it would hold a cooperative thread
    /// that every other concurrent read is sharing. One detached task calls
    /// this plain function, which states where the blocking happens instead of
    /// hiding it behind a timed wait.
    private static func runToCompletion(
        invocation: Invocation,
        executable: URL,
        arguments: [String],
        timeoutSeconds: Int
    ) throws -> Data {
        let process = invocation.process
        let output = Pipe()
        let errors = Pipe()
        process.executableURL = executable
        process.arguments = arguments
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = output
        process.standardError = errors

        // stderr is drained on a queue of its own. A refusal longer than the
        // pipe buffer would otherwise block the CLI mid-write while this
        // thread waits on stdout, and neither side would move again.
        let collected = ErrorOutput()
        let drained = DispatchSemaphore(value: 0)
        DispatchQueue.global(qos: .userInitiated).async {
            collected.store(errors.fileHandleForReading.readDataToEndOfFile())
            drained.signal()
        }

        do {
            try process.run()
        } catch {
            drained.signal()
            throw StadoCLIError.failed(
                exitCode: -1,
                message: "\(commandLine(arguments)) could not be started: \(error.localizedDescription)"
            )
        }
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        drained.wait()

        guard process.terminationStatus == 0 else {
            if invocation.didTimeOut {
                throw StadoCLIError.failed(
                    exitCode: process.terminationStatus,
                    message: "\(commandLine(arguments)) gave no answer within \(timeoutSeconds) s and was stopped. Nothing was written."
                )
            }
            let stderrText = collected.text
            let stdoutText = String(data: data, encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            let message = stderrText.isEmpty ? stdoutText : stderrText
            throw StadoCLIError.failed(
                exitCode: process.terminationStatus,
                message: message.isEmpty
                    ? "\(commandLine(arguments)) exited \(process.terminationStatus) and said nothing."
                    : message
            )
        }
        return data
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
