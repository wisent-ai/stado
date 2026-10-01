import CryptoKit
import Foundation

// MARK: - Endpoint

extension BackendProvisioner {
    /// The line `stado local-control-plane` writes to its error log once its
    /// API listener is bound and its daemons have started
    /// (`remote::control_plane::READY_MARKER`).
    static let localReadyLine = "[local-control-plane] listening="

    func stablePort(for deploymentID: String) -> Int {
        let digest = SHA256.hash(data: Data(deploymentID.utf8))
        let value = digest.prefix(2).reduce(0) { ($0 << 8) | Int($1) }
        let port = 8800 + value % 1000
        return port
    }

    /// One request to a deployment whose platform already reported it running
    /// (`gcloud run deploy`, `aws apprunner wait service-running`, the Azure
    /// revision restart). Any answer other than 200 is the failure, reported
    /// with what the endpoint said.
    func confirmHealthy(endpoint: String) async throws {
        guard let url = URL(string: endpoint + "/healthz") else {
            throw BackendProvisioningError.healthCheckFailed(endpoint, "the endpoint is not a URL")
        }
        let response: URLResponse
        do {
            (_, response) = try await session.data(from: url)
        } catch {
            throw BackendProvisioningError.healthCheckFailed(endpoint, error.localizedDescription)
        }
        guard let http = response as? HTTPURLResponse else {
            throw BackendProvisioningError.healthCheckFailed(endpoint, "the answer was not HTTP")
        }
        guard http.statusCode == 200 else {
            throw BackendProvisioningError.healthCheckFailed(endpoint, "/healthz answered HTTP \(http.statusCode)")
        }
    }

    /// Size of the service error log before launchd starts the service, so
    /// readiness is read only from what this start writes.
    func logOffset(_ log: URL) -> UInt64 {
        let size = (try? fileManager.attributesOfItem(atPath: log.path)[.size] as? NSNumber)?.uint64Value
        return size ?? 0
    }

    /// Returns when the service launchd holds as `target` writes its ready
    /// line to `log` after `offset`. Throws, with the service's own error
    /// output, when that process exits first or launchd holds no process.
    /// Driven by file and process events; nothing here polls.
    func awaitLocalServiceReady(target: String, log: URL, from offset: UInt64) async throws {
        let state = try await runCapture("/bin/launchctl", ["print", target])
        guard let pid = Self.launchdPID(state) else {
            throw BackendProvisioningError.serviceExited(
                target, Self.written(log, from: offset), Self.launchdLastExit(state)
            )
        }
        let descriptor = open(log.path, O_EVTONLY)
        guard descriptor >= 0 else {
            throw BackendProvisioningError.commandFailed(
                "the service log \(log.path) could not be opened: \(String(cString: strerror(errno)))"
            )
        }
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            let queue = DispatchQueue(label: "ai.wisent.stado.service-readiness")
            let file = DispatchSource.makeFileSystemObjectSource(
                fileDescriptor: descriptor, eventMask: [.extend, .write], queue: queue
            )
            let exit = DispatchSource.makeProcessSource(identifier: pid, eventMask: .exit, queue: queue)
            let outcome = ReadinessOutcome(file: file, exit: exit, continuation: continuation)
            let ready: @Sendable () -> Bool = { Self.written(log, from: offset).contains(Self.localReadyLine) }
            let exited: @Sendable () -> BackendProvisioningError = {
                BackendProvisioningError.serviceExited(target, Self.written(log, from: offset), "process \(pid) exited")
            }
            file.setEventHandler { if ready() { outcome.finish(.success(())) } }
            file.setCancelHandler { close(descriptor) }
            exit.setEventHandler { outcome.finish(ready() ? .success(()) : .failure(exited())) }
            file.resume()
            exit.resume()
            // The line may have been written, or the process may have gone,
            // before the sources were registered.
            queue.async {
                if ready() {
                    outcome.finish(.success(()))
                } else if kill(pid, 0) != 0 {
                    outcome.finish(.failure(exited()))
                }
            }
        }
    }

    static func written(_ log: URL, from offset: UInt64) -> String {
        guard let handle = try? FileHandle(forReadingFrom: log) else { return "" }
        defer { try? handle.close() }
        guard (try? handle.seek(toOffset: offset)) != nil,
              let data = try? handle.readToEnd() else { return "" }
        return String(decoding: data, as: UTF8.self)
    }

    static func launchdPID(_ state: String) -> pid_t? {
        launchdField("pid", in: state).flatMap { pid_t($0) }
    }

    static func launchdLastExit(_ state: String) -> String {
        launchdField("last exit code", in: state).map { "launchd holds no process; last exit code \($0)" }
            ?? "launchd holds no process and reported no exit code"
    }

    private static func launchdField(_ name: String, in state: String) -> String? {
        for line in state.split(separator: "\n") {
            let parts = line.split(separator: "=", maxSplits: 1)
            if parts.count == 2, parts[0].trimmingCharacters(in: .whitespaces) == name {
                return parts[1].trimmingCharacters(in: .whitespaces)
            }
        }
        return nil
    }
}

/// Resumes the readiness continuation exactly once and releases both sources.
private final class ReadinessOutcome: @unchecked Sendable {
    private let file: DispatchSourceFileSystemObject
    private let exit: DispatchSourceProcess
    private var continuation: CheckedContinuation<Void, Error>?

    init(file: DispatchSourceFileSystemObject, exit: DispatchSourceProcess,
         continuation: CheckedContinuation<Void, Error>) {
        self.file = file
        self.exit = exit
        self.continuation = continuation
    }

    /// Called only on the readiness queue.
    func finish(_ result: Result<Void, Error>) {
        guard let continuation else { return }
        self.continuation = nil
        file.cancel()
        exit.cancel()
        continuation.resume(with: result)
    }
}
