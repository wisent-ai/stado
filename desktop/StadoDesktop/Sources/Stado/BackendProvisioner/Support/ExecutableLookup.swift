import Foundation

// MARK: - Executable lookup

extension BackendProvisioner {
    /// The directories `PATH` names, in order; a process started without a
    /// `PATH` searches no directory, and the fixed places are searched after.
    private var searchPathDirectories: [URL] {
        switch ProcessInfo.processInfo.environment["PATH"] {
        case .some(let path):
            return path.split(separator: ":").map { URL(fileURLWithPath: String($0)) }
        case .none:
            return []
        }
    }

    /// No executable was found; every place that was looked in.
    private struct ExecutableMissing: Error {
        let searched: [URL]
    }

    /// The first executable file named `name` in the search path, then in
    /// `fixed`; a refusal names every place that was looked in.
    private func executable(named name: String, after fixed: [URL]) -> Result<URL, ExecutableMissing> {
        let candidates = searchPathDirectories.map { $0.appendingPathComponent(name) } + fixed
        switch candidates.first(where: { fileManager.isExecutableFile(atPath: $0.path) }) {
        case .some(let found): return .success(found)
        case .none: return .failure(ExecutableMissing(searched: candidates))
        }
    }

    func locateExecutable(named name: String, fixed: [String]) throws -> URL {
        switch executable(named: name, after: fixed.map { URL(fileURLWithPath: $0) }) {
        case .success(let found):
            return found
        case .failure(let missing):
            throw BackendProvisioningError.commandFailed(
                "\(name) is not installed or is not executable; looked in \(missing.searched.map(\.path).joined(separator: ", "))."
            )
        }
    }

    func locateStadoCLI() throws -> URL {
        let fixedCandidates = [
            fileManager.homeDirectoryForCurrentUser.appendingPathComponent(".stado/bin/stado"),
            fileManager.homeDirectoryForCurrentUser.appendingPathComponent(".local/bin/stado"),
            URL(fileURLWithPath: "/opt/homebrew/bin/stado"),
            URL(fileURLWithPath: "/usr/local/bin/stado"),
            URL(fileURLWithPath: "/opt/homebrew/Caskroom/miniforge/base/bin/stado"),
        ]
        switch executable(named: "stado", after: fixedCandidates) {
        case .success(let found): return found
        case .failure: throw BackendProvisioningError.cliUnavailable
        }
    }
}
