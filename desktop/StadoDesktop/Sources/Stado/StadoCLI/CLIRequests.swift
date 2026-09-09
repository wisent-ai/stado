import Foundation

extension StadoCLI {
    /// Run `stado <arguments>` and decode its stdout.
    ///
    /// `arguments` excludes the program name and includes `--json`: the caller
    /// names the exact command so the string this console shows in a
    /// confirmation is the string it runs.
    nonisolated func json<T: Decodable & Sendable>(
        _ type: T.Type,
        arguments: [String],
        timeoutSeconds: Int? = 120
    ) async throws -> T {
        try await jsonResult(type, arguments: arguments, timeoutSeconds: timeoutSeconds).value
    }

    /// Run one JSON command while retaining its complete process evidence.
    ///
    /// A valid payload is always returned, including for a non-zero exit, with
    /// exact stdout and stderr plus the CLI refusal. An absent or malformed
    /// payload throws the same evidence in `StadoCLIError.response`. A `nil`
    /// deadline leaves bounded host operations to the product command itself;
    /// readonly callers keep the default screen deadline.
    nonisolated func jsonResult<T: Decodable & Sendable>(
        _ type: T.Type,
        arguments: [String],
        timeoutSeconds: Int? = 120
    ) async throws -> StadoCLIJSONResult<T> {
        let executable = try await executableURL()
        let completion = try await Self.capture(
            executable: executable,
            arguments: arguments,
            timeoutSeconds: timeoutSeconds
        )
        do {
            return StadoCLIJSONResult(
                value: try JSONDecoder().decode(T.self, from: completion.output),
                exitCode: completion.exitCode,
                stdout: completion.output,
                stderr: completion.errors,
                refusal: completion.refusal?.errorDescription
            )
        } catch {
            // A missing or malformed payload still has real process evidence.
            // Carry it through the thrown value so a receipt-oriented caller
            // can render both streams instead of collapsing them into one
            // synthesized decoding sentence.
            let message = completion.refusal?.errorDescription
                ?? "stado answered with something this console could not read: \(Self.commandLine(arguments)) — \(error.localizedDescription)"
            throw StadoCLIError.response(
                exitCode: completion.exitCode,
                stdout: completion.output,
                stderr: completion.errors,
                message: message
            )
        }
    }

    /// Run a command whose successful stdout is intentionally plain text.
    ///
    /// This is reserved for explicit reveal/copy flows such as
    /// `credentials token mint --raw-token`. Unlike `json`, a non-zero exit can
    /// never be a decodable state, so the CLI's refusal is thrown immediately.
    nonisolated func text(
        arguments: [String],
        timeoutSeconds: Int = 120
    ) async throws -> String {
        let executable = try await executableURL()
        let completion = try await Self.capture(
            executable: executable,
            arguments: arguments,
            timeoutSeconds: timeoutSeconds
        )
        if let refusal = completion.refusal { throw refusal }
        let value = String(data: completion.output, encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !value.isEmpty else {
            throw StadoCLIError.failed(
                exitCode:
                    0,
                message: "\(Self.commandLine(arguments)) succeeded but returned no value."
            )
        }
        return value
    }
}
