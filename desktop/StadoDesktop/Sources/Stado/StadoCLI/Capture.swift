import Foundation

extension StadoCLI {
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

        var value: Data {
            lock.withLock { data }
        }
    }

    static func capture(
        executable: URL,
        arguments: [String],
        timeoutSeconds: Int?
    ) async throws -> Completion {
        let invocation = Invocation()
        let watchdog = timeoutSeconds.map { seconds in
            Task.detached(priority: .utility) {
                try? await Task.sleep(for: .seconds(seconds))
                invocation.terminateForTimeout()
            }
        }
        defer { watchdog?.cancel() }

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
        timeoutSeconds: Int?
    ) throws -> Completion {
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
                        .trimmingCharacters(in: .whitespacesAndNewlines) ?? "",
                    timedOut: invocation.didTimeOut,
                    timeoutSeconds: timeoutSeconds
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
        stderr: String,
        timedOut: Bool,
        timeoutSeconds: Int?
    ) -> StadoCLIError {
        if timedOut, let timeoutSeconds {
            return .failed(
                exitCode: exitCode,
                message: "\(commandLine(arguments)) gave no answer within \(timeoutSeconds) s and was stopped. This does not show whether the command changed anything."
            )
        }
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
