import Foundation

// MARK: - Process runners

extension BackendProvisioner {
    func run(_ executable: String, _ arguments: [String]) async throws {
        try await Task.detached(priority: .utility) {
            let process = Process()
            let errors = Pipe()
            process.executableURL = URL(fileURLWithPath: executable)
            process.arguments = arguments
            process.standardOutput = FileHandle.nullDevice
            process.standardError = errors
            try process.run()
            let errorData = errors.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            guard process.terminationStatus == 0 else {
                let detail = String(data: errorData, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines)
                throw BackendProvisioningError.commandFailed(detail?.isEmpty == false ? detail! : "exit \(process.terminationStatus)")
            }
        }.value
    }

    func runWithInput(
        _ executable: String,
        _ arguments: [String],
        input: String
    ) async throws {
        try await Task.detached(priority: .utility) {
            let process = Process()
            let standardInput = Pipe()
            let errors = Pipe()
            process.executableURL = URL(fileURLWithPath: executable)
            process.arguments = arguments
            process.standardInput = standardInput
            process.standardOutput = FileHandle.nullDevice
            process.standardError = errors
            try process.run()
            standardInput.fileHandleForWriting.write(Data(input.utf8))
            try standardInput.fileHandleForWriting.close()
            let errorData = errors.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            guard process.terminationStatus == 0 else {
                let detail = String(data: errorData, encoding: .utf8)?
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                throw BackendProvisioningError.commandFailed(
                    detail?.isEmpty == false ? detail! : "exit \(process.terminationStatus)"
                )
            }
        }.value
    }

    func runCapture(_ executable: String, _ arguments: [String]) async throws -> String {
        try await Task.detached(priority: .utility) {
            let process = Process()
            let output = Pipe()
            process.executableURL = URL(fileURLWithPath: executable)
            process.arguments = arguments
            process.standardOutput = output
            process.standardError = output
            try process.run()
            let data = output.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            let text = String(data: data, encoding: .utf8) ?? ""
            guard process.terminationStatus == 0 else {
                let detail = text.trimmingCharacters(in: .whitespacesAndNewlines)
                throw BackendProvisioningError.commandFailed(
                    detail.isEmpty ? "exit \(process.terminationStatus)" : detail
                )
            }
            return text
        }.value
    }
}
