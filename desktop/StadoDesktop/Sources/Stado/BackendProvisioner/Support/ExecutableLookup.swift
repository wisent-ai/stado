import Foundation

// MARK: - Executable lookup

extension BackendProvisioner {
    func locateExecutable(named name: String, fixed: [String]) throws -> URL {
        let environment = ProcessInfo.processInfo.environment
        let candidates = (environment["PATH"] ?? "")
            .split(separator: ":")
            .map(String.init)
            .map { URL(fileURLWithPath: $0).appendingPathComponent(name) }
            + fixed.map { URL(fileURLWithPath: $0) }
        guard let executable = candidates.first(where: {
            fileManager.isExecutableFile(atPath: $0.path)
        }) else {
            throw BackendProvisioningError.commandFailed("\(name) is not installed or is not executable.")
        }
        return executable
    }

    func locateStadoCLI() throws -> URL {
        let environment = ProcessInfo.processInfo.environment
        let pathCandidates = (environment["PATH"] ?? "")
            .split(separator: ":")
            .map(String.init)
            .map { URL(fileURLWithPath: $0).appendingPathComponent("stado") }
        let fixedCandidates = [
            fileManager.homeDirectoryForCurrentUser.appendingPathComponent(".local/bin/stado"),
            URL(fileURLWithPath: "/opt/homebrew/bin/stado"),
            URL(fileURLWithPath: "/usr/local/bin/stado"),
            URL(fileURLWithPath: "/opt/homebrew/Caskroom/miniforge/base/bin/stado"),
        ]
        guard let executable = (pathCandidates + fixedCandidates).first(where: {
            fileManager.isExecutableFile(atPath: $0.path)
        }) else {
            throw BackendProvisioningError.cliUnavailable
        }
        return executable
    }
}
