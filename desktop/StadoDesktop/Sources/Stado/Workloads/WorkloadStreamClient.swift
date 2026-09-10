import Foundation

struct WorkloadAttachmentRequest: Encodable, Sendable {
    let kind: String
    let target: String
    let workspace: String
    let resume: String?
    let confirmation: String
}

enum WorkloadStreamEvent: Sendable {
    case attached
    case stdout(Data)
    case stderr(Data)
    case exited(code: Int?, ok: Bool)
    case failure(String)
}

actor WorkloadStreamClient {
    private let session: URLSession
    private var socket: URLSessionWebSocketTask?

    init(session: URLSession? = nil) {
        if let session {
            self.session = session
        } else {
            let configuration = URLSessionConfiguration.ephemeral
            configuration.httpCookieStorage = nil
            configuration.httpShouldSetCookies = false
            configuration.urlCredentialStorage = nil
            self.session = URLSession(configuration: configuration)
        }
    }

    deinit { socket?.cancel(with: .goingAway, reason: nil) }

    func connect(_ attachment: WorkloadAttachmentRequest, at address: OperationsDashboardAddress,
                 authorizationToken: String?) async throws {
        guard socket == nil else { throw WorkloadStreamError.alreadyConnected }
        let endpoint = address.endpoint("api/operator/workload/attach")
        guard var components = URLComponents(url: endpoint, resolvingAgainstBaseURL: false) else {
            throw OperationsClientError.invalidDashboardURL
        }
        components.scheme = endpoint.scheme == "https" ? "wss" : "ws"
        guard let url = components.url else { throw OperationsClientError.invalidDashboardURL }
        var request = URLRequest(url: url)
        request.setValue("workload-attach", forHTTPHeaderField: "X-Stado-Action")
        if let authorizationToken, !authorizationToken.isEmpty {
            request.setValue("Bearer \(authorizationToken)", forHTTPHeaderField: "Authorization")
        }
        let task = session.webSocketTask(with: request)
        socket = task
        task.resume()
        do {
            let body = try JSONEncoder().encode(attachment)
            try await task.send(.string(String(decoding: body, as: UTF8.self)))
            guard socket === task else { throw CancellationError() }
        } catch {
            task.cancel(with: .goingAway, reason: nil)
            if socket === task { socket = nil }
            throw error
        }
    }

    func receive() async throws -> WorkloadStreamEvent {
        guard let socket else { throw WorkloadStreamError.notConnected }
        let message = try await socket.receive()
        switch message {
        case .data(let data):
            switch data.first {
            case 1: return .stdout(Data(data.dropFirst()))
            case 2: return .stderr(Data(data.dropFirst()))
            default: throw WorkloadStreamError.invalidFrame
            }
        case .string(let text):
            let control = try JSONDecoder().decode(Control.self, from: Data(text.utf8))
            switch control.type {
            case "attached":
                guard control.protocolName == "stado.workload.v1" else { throw WorkloadStreamError.invalidFrame }
                return .attached
            case "exit": return .exited(code: control.code, ok: control.ok == true)
            case "error": return .failure(control.message ?? "The workload stream failed without an error message.")
            default: throw WorkloadStreamError.invalidFrame
            }
        @unknown default: throw WorkloadStreamError.invalidFrame
        }
    }

    func send(_ text: String) async throws {
        guard let socket else { throw WorkloadStreamError.notConnected }
        try await socket.send(.string(text))
    }

    func finishInput() async throws {
        guard let socket else { throw WorkloadStreamError.notConnected }
        try await socket.send(.data(Data()))
    }

    func disconnect() {
        socket?.cancel(with: .goingAway, reason: nil)
        socket = nil
    }

    private struct Control: Decodable {
        let type: String
        let protocolName: String?
        let code: Int?
        let ok: Bool?
        let message: String?
        private enum CodingKeys: String, CodingKey {
            case type, code, ok, message
            case protocolName = "protocol"
        }
    }
}

private enum WorkloadStreamError: LocalizedError {
    case alreadyConnected, notConnected, invalidFrame
    var errorDescription: String? {
        switch self {
        case .alreadyConnected: "Disconnect the current workload before attaching another."
        case .notConnected: "No workload stream is connected."
        case .invalidFrame: "The Stado endpoint returned an invalid workload stream frame."
        }
    }
}
