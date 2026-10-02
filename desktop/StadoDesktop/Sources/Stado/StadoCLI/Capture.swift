import Foundation

extension StadoCLI {
    /// Use the selected service's existing operator API. A local executable is
    /// neither searched for nor started, including when the service refuses.
    func capture(
        arguments: [String],
        confirmsMutation: Bool,
        input: String? = nil,
        standardInput: String? = nil,
        destination: Destination = .selected
    ) async throws -> Completion {
        let source = try await Self.source(for: destination)
        let result: OperatorCommandResult
        do {
            result = try await client.run(
                arguments: arguments, confirmsMutation: confirmsMutation,
                at: source.address, authorizationToken: source.authorizationToken,
                input: input, standardInput: standardInput
            )
        } catch {
            throw StadoCLIError.response(
                exitCode: nil, stdout: Data(), stderr: Data(),
                message: "\(source.address.displayString): \(Self.commandLine(arguments)) — \(error.localizedDescription)"
            )
        }
        guard await Self.isCurrent(source) else { throw CancellationError() }
        let output = Data(result.standardOutput.utf8)
        let errors = Data(result.standardError.utf8)
        let incomplete = result.standardOutputTruncated || result.standardErrorTruncated
        let refused = !result.ok || result.exitCode != 0 || result.standardInputError != nil || incomplete
        let refusal: StadoCLIError? = refused ? .response(
            exitCode: result.exitCode, stdout: output, stderr: errors,
            message: "\(source.address.displayString): \(result.message)"
        ) : nil
        if incomplete || result.exitCode == nil, let refusal { throw refusal }
        return Completion(output: output, errors: errors, refusal: refusal, exitCode: result.exitCode)
    }
}
