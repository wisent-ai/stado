import Foundation

extension StadoCLI {
    /// Decode an operator API receipt. Mutations require the caller's explicit
    /// review confirmation; read-only operations do not acquire it implicitly.
    nonisolated func json<T: Decodable & Sendable>(
        _ type: T.Type,
        arguments: [String],
        standardInput: String? = nil,
        input: String? = nil,
        confirmsMutation: Bool = false,
        destination: Destination = .selected
    ) async throws -> T {
        let result = try await jsonResult(type, arguments: arguments, standardInput: standardInput,
                                          input: input, confirmsMutation: confirmsMutation, destination: destination)
        if confirmsMutation, let refusal = result.refusal {
            throw StadoCLIError.response(exitCode: result.exitCode, stdout: result.stdout,
                                        stderr: result.stderr, message: refusal)
        }
        return result.value
    }

    /// Retain complete API output even when the operation returned non-zero.
    /// File content travels in `input` and is referenced by the API's `$INPUT`
    /// placeholder; `standardInput` carries secret input separately.
    nonisolated func jsonResult<T: Decodable & Sendable>(
        _ type: T.Type,
        arguments: [String],
        standardInput: String? = nil,
        input: String? = nil,
        confirmsMutation: Bool = false,
        destination: Destination = .selected
    ) async throws -> StadoCLIJSONResult<T> {
        let completion = try await capture(
            arguments: arguments, confirmsMutation: confirmsMutation,
            input: input, standardInput: standardInput, destination: destination
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
        confirmsMutation: Bool = false
    ) async throws -> String {
        let completion = try await capture(arguments: arguments, confirmsMutation: confirmsMutation)
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
