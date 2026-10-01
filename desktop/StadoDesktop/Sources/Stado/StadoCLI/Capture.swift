import Foundation

extension StadoCLI {
    /// Everything the stderr reader thread hands back, behind a lock.
    private final class ErrorOutput: @unchecked Sendable {
        private let lock = NSLock()
        private var data = Data()

        func store(_ value: Data) {
            lock.withLock { data = value }
        }

        var value: Data {
            lock.withLock { data }
        }
    }

    /// Run `stado` until it exits. Its exit code, stdout and stderr are the
    /// answer; no timer stops it.
    static func capture(
        executable: URL,
        arguments: [String]
    ) async throws -> Completion {
        try await Task.detached(priority: .userInitiated) {
            try runToCompletion(executable: executable, arguments: arguments)
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
        executable: URL,
        arguments: [String]
    ) throws -> Completion {
        let process = Process()
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
        let drained = DispatchSemaphore(
            value:
                0
        )
        DispatchQueue.global(qos: .userInitiated).async {
            collected.store(errors.fileHandleForReading.readDataToEndOfFile())
            drained.signal()
        }

        do {
            try process.run()
        } catch {
            drained.signal()
            throw StadoCLIError.failed(
                exitCode:
                    -1,
                message: "\(commandLine(arguments)) could not be started: \(error.localizedDescription)"
            )
        }
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        drained.wait()

        let exitCode = process.terminationStatus
        let errorData = collected.value
        guard exitCode == 0 else {
            return Completion(
                output: data,
                errors: errorData,
                refusal: refusal(
                    arguments: arguments,
                    exitCode: exitCode,
                    stdout: data,
                    stderr: String(data: errorData, encoding: .utf8)?
                        .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                ),
                exitCode: exitCode
            )
        }
        return Completion(
            output: data,
            errors: errorData,
            refusal: nil,
            exitCode: exitCode
        )
    }

    /// The sentence a non-zero exit refused with, in the CLI's own words.
    private static func refusal(
        arguments: [String],
        exitCode: Int32,
        stdout: Data,
        stderr: String
    ) -> StadoCLIError {
        let stdoutText = String(data: stdout, encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let message = stderr.isEmpty ? stdoutText : stderr
        return .failed(
            exitCode: exitCode,
            message: message.isEmpty
                ? "\(commandLine(arguments)) exited \(exitCode) and said nothing."
                : message
        )
    }
}
